use kazoo_wall::protocol::{Change, TreePlace, What};

use super::*;

const WINDOW: Duration = Duration::from_secs(20);
const REST: Duration = Duration::from_secs(5);

fn go(news: &mut News, seq: u64, seat: &str, moment: u64, now: Instant) {
    let place = TreePlace {
        commit: "a1b2c3d4e5f6012345678901234567890123456789".to_string(),
        mark: None,
        branch: None,
        change: Some(moment),
    };
    let what = What::Go {
        from: place.clone(),
        to: place,
        backup: "before-go".to_string(),
        fingerprint: "hash".to_string(),
        modules: 140,
        cables: 189,
        tempo: 96.0,
        by_console: true,
    };
    news.take(
        Feed::Event(Event::Change {
            change: Box::new(Change {
                notes: Vec::new(),
                seq,
                at: "2026-10-15T00:00:00Z".to_string(),
                seat: seat.to_string(),
                summary: kazoo_wall::change::summarise(seat, &what, None),
                what,
                undoes: None,
                clock: None,
                comment: None,
            }),
        }),
        now,
    );
    news.take(
        Feed::Event(Event::Loaded {
            seq,
            seat: seat.to_string(),
            save: None,
            backup: "before-go".to_string(),
        }),
        now,
    );
}

fn tell(news: &mut News, now: Instant) -> Option<String> {
    news.take(Feed::Own(0), now);
    if news.due(WINDOW).is_some_and(|due| due <= now) {
        let content = news.params()["content"].as_str().unwrap().to_string();
        news.sent(now);
        Some(content)
    } else {
        None
    }
}

fn stale(seat: &str) -> String {
    format!(
        "News from the wall:\n- {seat} is moving the wall through the tree: \
         what this seat knew of it is stale\n{HINT}"
    )
}

fn single(seat: &str, moment: u64) -> String {
    format!(
        "News from the wall:\n- {seat} went to the moment after change {moment}: \
         140 modules, 189 cables, 96 BPM; the wall as it was is saved as \"before-go\"\n{HINT}"
    )
}

fn walk(seat: &str, count: u64, moment: u64) -> String {
    format!(
        "News from the wall:\n- {seat} walked the tree: {count} moments, now at \
         the moment after change {moment} (140 modules, 189 cables, 96 BPM)\n{HINT}"
    )
}

#[test]
fn twenty_two_goes_are_one_stale_line_and_one_walk_after_rest() {
    let t0 = Instant::now();
    let mut news = News::new("Waffles".to_string());
    let mut told = Vec::new();
    for index in 0..22 {
        let now = t0 + Duration::from_secs(index);
        go(&mut news, index + 41, "Tom", 6804 + index, now);
        if let Some(content) = tell(&mut news, now) {
            told.push(content);
        }
        if index == 0 {
            assert_eq!(told, vec![stale("Tom")]);
        } else {
            assert_eq!(told.len(), 1, "no notification during the walk");
        }
    }
    let last = t0 + Duration::from_secs(21);
    assert_eq!(news.due(WINDOW), Some(last + REST));
    assert_eq!(tell(&mut news, last + REST - Duration::from_nanos(1)), None);
    told.push(tell(&mut news, last + REST).unwrap());
    assert_eq!(told, vec![stale("Tom"), walk("Tom", 22, 6825)]);
    assert_eq!(news.params()["meta"]["seq"], "62");
    assert_eq!(tell(&mut news, last + REST + WINDOW), None);
}

#[test]
fn one_go_says_stale_at_once_and_went_to_after_rest() {
    let t0 = Instant::now();
    let mut news = News::new("Waffles".to_string());
    go(&mut news, 41, "Tom", 6825, t0);
    assert_eq!(tell(&mut news, t0), Some(stale("Tom")));
    assert_eq!(news.due(WINDOW), Some(t0 + REST));
    assert_eq!(tell(&mut news, t0 + REST - Duration::from_nanos(1)), None);
    assert_eq!(tell(&mut news, t0 + REST), Some(single("Tom", 6825)));
    assert_eq!(tell(&mut news, t0 + REST + WINDOW), None);
}

#[test]
fn interleaved_seats_have_separate_walks_and_rest_deadlines() {
    let t0 = Instant::now();
    let mut news = News::new("Waffles".to_string());
    for (index, seat, moment) in [
        (0, "Tom", 6810),
        (1, "Cally", 6811),
        (2, "Tom", 6820),
        (3, "Cally", 6825),
    ] {
        let now = t0 + Duration::from_secs(index);
        go(&mut news, 41 + index, seat, moment, now);
        let expected = if index < 2 { Some(stale(seat)) } else { None };
        assert_eq!(tell(&mut news, now), expected);
    }
    assert_eq!(news.due(WINDOW), Some(t0 + Duration::from_secs(7)));
    assert_eq!(tell(&mut news, t0 + Duration::from_secs(6)), None);
    assert_eq!(
        tell(&mut news, t0 + Duration::from_secs(7)),
        Some(walk("Tom", 2, 6820))
    );
    assert_eq!(news.due(WINDOW), Some(t0 + Duration::from_secs(8)));
    assert_eq!(
        tell(&mut news, t0 + Duration::from_secs(8)),
        Some(walk("Cally", 2, 6825))
    );
    assert_eq!(tell(&mut news, t0 + WINDOW), None);
}

#[test]
fn a_save_load_is_urgent_and_splits_the_walk() {
    let t0 = Instant::now();
    let mut news = News::new("Waffles".to_string());
    go(&mut news, 41, "Tom", 6810, t0);
    assert_eq!(tell(&mut news, t0), Some(stale("Tom")));
    go(&mut news, 42, "Tom", 6820, t0 + Duration::from_secs(1));
    assert_eq!(tell(&mut news, t0 + Duration::from_secs(1)), None);
    let load = t0 + Duration::from_secs(2);
    news.take(
        Feed::Event(Event::Change {
            change: Box::new(Change {
                notes: Vec::new(),
                seq: 43,
                at: "2026-10-15T00:00:02Z".to_string(),
                seat: "Cally".to_string(),
                what: What::Load {
                    save: Some("jam".to_string()),
                    backup: "before-load".to_string(),
                    fingerprint: "hash".to_string(),
                    modules: 12,
                    cables: 30,
                    tempo: 96.0,
                    by_console: true,
                },
                summary: "Cally loaded the save \"jam\" (12 modules, 30 cables, 96 BPM)"
                    .to_string(),
                undoes: None,
                clock: None,
                comment: None,
            }),
        }),
        load,
    );
    news.take(
        Feed::Event(Event::Loaded {
            seq: 43,
            seat: "Cally".to_string(),
            save: Some("jam".to_string()),
            backup: "before-load".to_string(),
        }),
        load,
    );
    let told = tell(&mut news, load).unwrap();
    assert!(
        told.contains("Tom walked the tree: 2 moments, now at the moment after change 6820"),
        "{told}"
    );
    assert!(
        told.contains("Cally loaded the save \"jam\" (12 modules, 30 cables, 96 BPM)"),
        "{told}"
    );
    assert!(
        told.contains("the whole wall was replaced: what this seat knew of it is stale"),
        "{told}"
    );
    let again = t0 + Duration::from_secs(3);
    go(&mut news, 44, "Tom", 6825, again);
    assert_eq!(tell(&mut news, again), Some(stale("Tom")));
    assert_eq!(tell(&mut news, again + REST), Some(single("Tom", 6825)));
    assert_eq!(tell(&mut news, again + REST + WINDOW), None);
}

#[test]
fn own_goes_are_silent_and_do_not_change_another_seats_walk() {
    let t0 = Instant::now();
    let mut news = News::new("Waffles".to_string());
    go(&mut news, 41, "Waffles", 6810, t0);
    assert_eq!(news.due(WINDOW), None);
    assert_eq!(tell(&mut news, t0 + REST), None);
    let other = t0 + REST;
    go(&mut news, 42, "Tom", 6820, other);
    assert_eq!(tell(&mut news, other), Some(stale("Tom")));
    go(&mut news, 43, "Tom", 6825, other + Duration::from_secs(1));
    go(
        &mut news,
        44,
        "Waffles",
        6830,
        other + Duration::from_secs(2),
    );
    assert_eq!(tell(&mut news, other + Duration::from_secs(2)), None);
    assert_eq!(
        tell(&mut news, other + Duration::from_secs(6)),
        Some(walk("Tom", 2, 6825))
    );
    assert_eq!(news.params()["meta"]["seq"], "44");
    assert_eq!(tell(&mut news, other + WINDOW), None);
}

#[test]
fn a_rested_walk_joins_ordinary_news_without_shortening_its_window() {
    let t0 = Instant::now();
    let mut news = News::new("Waffles".to_string());
    go(&mut news, 41, "Tom", 6825, t0);
    assert_eq!(tell(&mut news, t0), Some(stale("Tom")));
    let changed = t0 + Duration::from_secs(1);
    news.take(
        Feed::Event(Event::Change {
            change: Box::new(Change {
                notes: Vec::new(),
                seq: 42,
                at: "2026-10-15T00:00:01Z".to_string(),
                seat: "Cally".to_string(),
                what: What::Tempo {
                    from: 92.0,
                    to: 96.0,
                    desk: false,
                },
                summary: "Cally set the tempo to 96 BPM".to_string(),
                undoes: None,
                clock: None,
                comment: None,
            }),
        }),
        changed,
    );
    assert_eq!(tell(&mut news, t0 + REST), None);
    assert_eq!(news.due(WINDOW), Some(changed + WINDOW));
    assert_eq!(
        tell(&mut news, changed + WINDOW - Duration::from_nanos(1)),
        None
    );
    let told = tell(&mut news, changed + WINDOW).unwrap();
    assert!(told.contains("Cally set the tempo to 96 BPM"), "{told}");
    assert!(
        told.contains("Tom went to the moment after change 6825"),
        "{told}"
    );
    assert_eq!(tell(&mut news, changed + WINDOW * 2), None);
}

#[test]
fn a_fault_is_urgent_and_a_failed_walk_notification_keeps_its_count() {
    let t0 = Instant::now();
    let mut news = News::new("Waffles".to_string());
    go(&mut news, 41, "Tom", 6810, t0);
    assert_eq!(tell(&mut news, t0), Some(stale("Tom")));
    go(&mut news, 42, "Tom", 6825, t0 + Duration::from_secs(1));
    let fault = t0 + Duration::from_secs(2);
    news.take(
        Feed::Event(Event::Fault {
            summary: "vco2 produced NaN; reset".to_string(),
            seq: Some(42),
        }),
        fault,
    );
    assert_eq!(
        tell(&mut news, fault),
        Some(format!(
            "News from the wall:\n- fault: vco2 produced NaN; reset\n{HINT}"
        ))
    );
    let rested = t0 + Duration::from_secs(6);
    news.take(Feed::Own(0), rested);
    assert_eq!(news.due(WINDOW), Some(rested));
    assert_eq!(news.params()["content"], walk("Tom", 2, 6825));
    news.failed(rested);
    assert_eq!(news.due(WINDOW), Some(rested + WINDOW));
    assert_eq!(
        tell(&mut news, rested + WINDOW - Duration::from_nanos(1)),
        None
    );
    assert_eq!(tell(&mut news, rested + WINDOW), Some(walk("Tom", 2, 6825)));
    assert_eq!(tell(&mut news, rested + WINDOW * 2), None);
}

fn go_change(seq: u64, seat: &str, what: What, undoes: Option<u64>) -> Feed {
    Feed::Event(Event::Change {
        change: Box::new(Change {
            notes: Vec::new(),
            seq,
            at: "2026-10-15T00:00:00Z".to_string(),
            seat: seat.to_string(),
            summary: kazoo_wall::change::summarise(seat, &what, undoes),
            what,
            undoes,
            clock: None,
            comment: None,
        }),
    })
}

fn replaced(seq: u64, seat: &str) -> Feed {
    Feed::Event(Event::Loaded {
        seq,
        seat: seat.to_string(),
        save: None,
        backup: "before-go".to_string(),
    })
}

fn tempo(seq: u64, seat: &str) -> Feed {
    Feed::Event(Event::Change {
        change: Box::new(Change {
            notes: Vec::new(),
            seq,
            at: "2026-10-15T00:00:00Z".to_string(),
            seat: seat.to_string(),
            what: What::Tempo {
                from: 92.0,
                to: 96.0,
                desk: false,
            },
            summary: format!("{seat} did {seq}"),
            undoes: None,
            clock: None,
            comment: None,
        }),
    })
}

#[test]
fn a_walk_ending_on_an_undone_go_to_a_mark_names_the_mark_whole() {
    let t0 = Instant::now();
    let mut news = News::new("Waffles".to_string());
    go(&mut news, 41, "Tom", 6810, t0);
    assert_eq!(tell(&mut news, t0), Some(stale("Tom")));
    let place = TreePlace {
        commit: "a1b2c3d4e5f6012345678901234567890123456789".to_string(),
        mark: Some("we went to sea".to_string()),
        branch: None,
        change: Some(7),
    };
    let what = What::Go {
        from: place.clone(),
        to: place,
        backup: "before-go".to_string(),
        fingerprint: "hash".to_string(),
        modules: 140,
        cables: 189,
        tempo: 96.0,
        by_console: true,
    };
    let undone = t0 + Duration::from_secs(1);
    news.take(go_change(42, "Tom", what, Some(41)), undone);
    news.take(replaced(42, "Tom"), undone);
    assert_eq!(
        tell(&mut news, undone + REST),
        Some(format!(
            "News from the wall:\n- Tom walked the tree: 2 moments, now at mark \
             \"we went to sea\", after change 7 (140 modules, 189 cables, 96 BPM)\n{HINT}"
        ))
    );
}

#[test]
fn a_stale_line_and_a_walk_line_are_never_counted_away() {
    let t0 = Instant::now();
    let mut news = News::new("Waffles".to_string());
    news.sent(t0);
    for seq in 1..=12 {
        news.take(tempo(seq, "Cally"), t0);
    }
    go(&mut news, 41, "Tom", 6810, t0 + Duration::from_secs(1));
    let told = tell(&mut news, t0 + Duration::from_secs(1)).unwrap();
    assert!(
        told.contains(
            "- Tom is moving the wall through the tree: what this seat knew of it is stale\n"
        ),
        "{told}"
    );
    assert_eq!(told.matches("\n- ").count(), MAX_LINES + 1, "{told}");
    assert!(told.contains("- and 3 more (see wall_log)\n"), "{told}");

    let busy = t0 + Duration::from_secs(2);
    for seq in 50..=61 {
        news.take(tempo(seq, "Cally"), busy);
    }
    go(&mut news, 62, "Tom", 6825, busy);
    let rested = busy + REST;
    assert_eq!(tell(&mut news, rested), None);
    let told = tell(&mut news, busy + WINDOW).unwrap();
    assert!(
        told.contains("- Tom walked the tree: 2 moments, now at the moment after change 6825"),
        "{told}"
    );
    assert!(told.contains("- and 3 more (see wall_log)\n"), "{told}");
}

#[test]
fn the_walker_leaving_tells_its_walk_at_once() {
    let t0 = Instant::now();
    let mut news = News::new("Waffles".to_string());
    go(&mut news, 41, "Tom", 6810, t0);
    assert_eq!(tell(&mut news, t0), Some(stale("Tom")));
    go(&mut news, 42, "Tom", 6825, t0 + Duration::from_secs(1));
    let left = t0 + Duration::from_secs(2);
    news.take(
        Feed::Event(Event::Seat {
            seat: "Tom".to_string(),
            joined: false,
            seq: Some(42),
        }),
        left,
    );
    assert_eq!(
        tell(&mut news, left),
        Some(format!(
            "News from the wall:\n- Tom walked the tree: 2 moments, now at the moment after \
             change 6825 (140 modules, 189 cables, 96 BPM)\n- Tom left the wall\n{HINT}"
        ))
    );
    assert_eq!(news.due(WINDOW), None);
}

#[test]
fn a_go_cut_off_by_the_line_dropping_is_still_told() {
    let t0 = Instant::now();
    let mut news = News::new("Waffles".to_string());
    let place = TreePlace {
        commit: "a1b2c3d4e5f6012345678901234567890123456789".to_string(),
        mark: None,
        branch: None,
        change: Some(6810),
    };
    let what = What::Go {
        from: place.clone(),
        to: place,
        backup: "before-go".to_string(),
        fingerprint: "hash".to_string(),
        modules: 140,
        cables: 189,
        tempo: 96.0,
        by_console: true,
    };
    news.take(go_change(41, "Tom", what, None), t0);
    news.take(Feed::Down, t0);
    let told = tell(&mut news, t0).unwrap();
    assert!(
        told.contains("- Tom went to the moment after change 6810: 140 modules"),
        "{told}"
    );
    assert!(told.contains("- the line to the wall dropped"), "{told}");
}
