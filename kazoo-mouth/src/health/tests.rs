use super::*;

fn losses(set: impl FnOnce(&mut Losses)) -> Losses {
    let mut losses = Losses::default();
    set(&mut losses);
    losses
}

fn text(health: &Health, now: Instant, why: Option<&str>) -> Option<String> {
    health.row(now, why).map(|row| row.text)
}

#[test]
fn nothing_lost_shows_nothing() {
    let mut health = Health::default();
    let now = Instant::now();
    health.observe(Losses::default(), now);
    assert_eq!(health.row(now, Some("ignored")), None);
    assert_eq!(health.totals(), None);
    assert_eq!(health.report(None), None);
}

#[test]
fn a_rise_is_said_for_ten_seconds_then_the_totals_stay() {
    let mut health = Health::default();
    let now = Instant::now();
    health.observe(losses(|l| l.mic_dropped = 28), now);
    assert_eq!(
        health.row(now, None),
        Some(Row {
            text: "28 mic samples dropped".to_owned(),
            fresh: true,
        })
    );
    assert_eq!(
        health.row(now + RISE_LIFETIME, None),
        Some(Row {
            text: "lost so far: mic 28".to_owned(),
            fresh: false,
        })
    );
}

/// A counter that keeps rising adds to the rise being said, instead of
/// starting it again every frame.
#[test]
fn rises_while_one_is_said_add_to_it() {
    let mut health = Health::default();
    let now = Instant::now();
    health.observe(losses(|l| l.mic_dropped = 1), now);
    assert_eq!(
        text(&health, now, None).as_deref(),
        Some("1 mic sample dropped")
    );
    let soon = now + Duration::from_secs(4);
    health.observe(losses(|l| l.mic_dropped = 40), soon);
    assert_eq!(
        text(&health, soon, None).as_deref(),
        Some("40 mic samples dropped")
    );
    // Ten seconds from the last rise, not the first.
    assert!(
        health
            .row(now + RISE_LIFETIME, None)
            .is_some_and(|row| row.fresh)
    );
    let after = soon + RISE_LIFETIME;
    health.observe(losses(|l| l.mic_dropped = 41), after);
    assert_eq!(
        text(&health, after, None).as_deref(),
        Some("1 mic sample dropped"),
        "a rise after the last has ended is said afresh"
    );
}

#[test]
fn the_worst_is_named_the_rest_counted_and_the_totals_worst_first() {
    let mut health = Health::default();
    let now = Instant::now();
    health.observe(
        losses(|l| {
            l.display_dropped = 3;
            l.mic_skipped = 64;
            l.judge.samples_dropped = 100;
        }),
        now,
    );
    assert_eq!(
        text(&health, now, None).as_deref(),
        Some(
            "64 mic samples skipped to catch up (+2 others) · lost so far: mic skip 64 · \
             pitch in 100 · screen 3"
        )
    );
    assert_eq!(
        health.totals().as_deref(),
        Some("mic skip 64 · pitch in 100 · screen 3")
    );
}

#[test]
fn a_failed_pitch_run_is_said_with_its_reason_whenever_it_rises() {
    let now = Instant::now();
    let why = Some("wrong length");
    let mut alone = Health::default();
    alone.observe(losses(|l| l.judge.pitch_runs_failed = 1), now);
    assert_eq!(
        text(&alone, now, why).as_deref(),
        Some("1 pYIN run failed, read as unvoiced (engine fault): wrong length")
    );
    let mut worst = Health::default();
    worst.observe(
        losses(|l| {
            l.judge.pitch_runs_failed = 2;
            l.fx_faults = 1;
        }),
        now,
    );
    assert_eq!(
        text(&worst, now, why).as_deref(),
        Some(
            "2 pYIN runs failed, read as unvoiced (engine fault): wrong length (+1 other) · \
             lost so far: pyin 2 · fx 1"
        )
    );
    let mut behind = Health::default();
    behind.observe(
        losses(|l| {
            l.judge.pitch_runs_failed = 1;
            l.mic_dropped = 5;
        }),
        now,
    );
    assert_eq!(
        text(&behind, now, why).as_deref(),
        Some(
            "5 mic samples dropped (+1 other); pYIN failed: wrong length · lost so far: \
             mic 5 · pyin 1"
        )
    );
    let mut none = Health::default();
    none.observe(losses(|l| l.mic_dropped = 5), now);
    assert_eq!(
        text(&none, now, why).as_deref(),
        Some("5 mic samples dropped"),
        "no reason without a failed run"
    );
}

#[test]
fn a_stopped_judge_is_said_first_and_stays_red() {
    let mut health = Health::default();
    let now = Instant::now();
    health.observe(losses(|l| l.judge_stopped = true), now);
    assert_eq!(
        health.row(now, None),
        Some(Row {
            text: "the voice judge has stopped".to_owned(),
            fresh: true,
        })
    );
    health.observe(
        losses(|l| {
            l.judge_stopped = true;
            l.judge.samples_dropped = 4800;
        }),
        now,
    );
    assert_eq!(
        text(&health, now, None).as_deref(),
        Some("the voice judge has stopped; 4800 samples the voice judge missed")
    );
    assert_eq!(
        health.row(now + RISE_LIFETIME, None),
        Some(Row {
            text: "the voice judge has stopped; lost so far: pitch in 4800".to_owned(),
            fresh: true,
        })
    );
}

/// Every counter at once, in its order: what is heard before what is seen.
#[test]
fn every_counter_is_covered_in_its_order() {
    let all = Losses {
        mic_dropped: 1,
        mic_skipped: 2,
        display_dropped: 3,
        voice_refused: 4,
        audio_refused: 5,
        desk_lost: 6,
        fx_faults: 7,
        fx_refused: 8,
        desk_blocks_dropped: 9,
        desk_messages_dropped: 10,
        judge: JudgeStatsSnapshot {
            samples_dropped: 11,
            results_dropped: 12,
            pitch_runs_failed: 13,
        },
        judge_stopped: false,
    };
    let mut read: Vec<u64> = COUNTERS.iter().map(|c| (c.read)(&all)).collect();
    read.sort_unstable();
    assert_eq!(read, (1..=13).collect::<Vec<u64>>());
    let mut health = Health::default();
    health.observe(all, Instant::now());
    assert_eq!(
        health.totals().as_deref(),
        Some(
            "mic 1 · mic skip 2 · desk audio 9 · pitch in 11 · pyin 13 · pitch out 12 · fx 7 · \
             desk msgs 10 · desk voice 4 · desk gone 5 · desk sync 6 · fx refused 8 · screen 3"
        )
    );
    assert_eq!(COUNTERS.iter().filter(|c| c.says_pitch_failure).count(), 1);
}

#[test]
fn the_report_says_every_loss_of_the_session() {
    let mut health = Health::default();
    health.observe(
        losses(|l| {
            l.mic_dropped = 28;
            l.judge.pitch_runs_failed = 1;
        }),
        Instant::now(),
    );
    assert_eq!(
        health.report(Some("wrong length")).as_deref(),
        Some(
            "kazoo-mouth: lost 28 mic samples dropped, 1 pYIN run failed, read as unvoiced \
             (engine fault); pYIN failed: wrong length"
        )
    );
}

/// Read from the mouth's own counters, the desk link's and the judge's.
#[test]
fn losses_are_read_from_the_stats_the_desk_and_the_judge() {
    let stats = Stats::default();
    stats.mic_dropped.store(7, Ordering::Relaxed);
    stats.desk_lost.store(2, Ordering::Relaxed);
    let config = kazoo_core::analysis::PitchDetectorConfig::for_rate(48_000);
    let (mut feed, _results, judge) = kazoo_core::voice::judge::spawn(
        config,
        128,
        kazoo_core::voice::judge::OnsetConfig::default(),
    )
    .expect("a judge at 48 kHz");
    // Its ring holds a tenth of a second: 4800 of 4900 fit.
    feed.push(&vec![0.0; 4_900]);
    let counters = judge.counters();
    let desk = LinkStatus {
        voice: kazoo_core::ipc::link::VoiceLink::default(),
        normal_priority: false,
        connected: true,
        strip: Some(0),
        blocks_sent: 10,
        blocks_dropped: 3,
        messages_dropped: 1,
        connections: 1,
        last_refusal: None,
    };
    let read = Losses::read(&stats, &counters, false, Some(&desk));
    assert_eq!(read.mic_dropped, 7);
    assert_eq!(read.desk_lost, 2);
    assert_eq!(read.desk_blocks_dropped, 3);
    assert_eq!(read.desk_messages_dropped, 1);
    assert_eq!(read.judge.samples_dropped, 100);
    assert!(!read.judge_stopped);
    let gone = Losses::read(&stats, &counters, true, None);
    assert_eq!(gone.desk_blocks_dropped, 0);
    assert!(gone.judge_stopped);
    drop(feed);
    judge.join().expect("the judge exits");
}

/// Losses that keep rising never hide the others: the totals ride along.
#[test]
fn rising_for_half_a_minute_keeps_the_others_in_sight() {
    let mut health = Health::default();
    let start = Instant::now();
    for step in 0..7_u64 {
        let now = start + Duration::from_secs(5 * step);
        health.observe(
            losses(|l| {
                l.desk_blocks_dropped = 100 * (step + 1);
                l.mic_skipped = 64;
                l.display_dropped = 3;
            }),
            now,
        );
        let row = health.row(now, None).expect("a loss is shown");
        assert!(row.fresh);
        assert!(row.text.contains("lost so far:"), "{}", row.text);
        assert!(row.text.contains("mic skip 64"), "{}", row.text);
        assert!(row.text.contains("screen 3"), "{}", row.text);
    }
}

/// The report says a judge that stopped, though the last read, made after
/// it is joined, cannot.
#[test]
fn a_judge_seen_stopped_is_in_the_report() {
    let mut health = Health::default();
    let now = Instant::now();
    health.observe(losses(|l| l.judge_stopped = true), now);
    health.observe(Losses::default(), now);
    assert_eq!(
        health.report(None).as_deref(),
        Some("kazoo-mouth: the voice judge stopped")
    );
}
