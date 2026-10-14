use std::time::Instant;

use crossterm::event::KeyCode;

use crate::protocol::{
    BranchView, Change, ErrorCode, Event, LoadResult, MarkKind, MarkResult, MarkView, MomentView,
    Request, TreePlace, TreeResult, What,
};

use super::super::app::{App, Mode};
use super::super::worker::{Failure, Job, Reply};
use super::render::{render, text};
use super::{answer, connected, press, type_text};

fn calls(app: &mut App) -> Vec<Request> {
    app.take_jobs()
        .into_iter()
        .map(|job| match job {
            Job::Call(request) => request,
            other => panic!("unexpected job: {other:?}"),
        })
        .collect()
}

fn status(app: &App) -> String {
    app.status(Instant::now())
        .map(|status| status.text.clone())
        .unwrap_or_default()
}

fn place() -> TreePlace {
    TreePlace {
        commit: "a".repeat(40),
        branch: Some("main".into()),
        mark: None,
        change: Some(812),
    }
}

fn moment(n: u64) -> MomentView {
    MomentView {
        commit: format!("{n:040x}"),
        short: format!("{n:012x}"),
        change: Some(n),
        seat: "Tom".into(),
        at: "2026-10-13T10:00:00Z".into(),
        summary: format!("turned cutoff {n}"),
    }
}

fn listed() -> TreeResult {
    TreeResult {
        parents: std::collections::BTreeMap::new(),
        here: Some(place()),
        through: 812,
        revision: 814,
        branches: ["main", "bud"]
            .into_iter()
            .map(|name| BranchView {
                name: name.into(),
                tip: moment(812),
                grew_from: (name == "bud").then(|| "dawn".into()),
            })
            .collect(),
        branch_count: 7,
        marks: ["dawn", "dusk", "saved"]
            .into_iter()
            .map(|name| MarkView {
                name: name.into(),
                tag: format!("save/{name}"),
                kind: if name == "saved" {
                    MarkKind::Save
                } else {
                    MarkKind::Mark
                },
                moment: moment(810),
            })
            .collect(),
        mark_count: 3,
        recent: (809..813).map(moment).collect(),
    }
}

fn open_list() -> App {
    let mut app = connected();
    press(&mut app, KeyCode::Char('B'));
    assert_eq!(calls(&mut app), [Request::Tree { recent: None }]);
    answer(&mut app, Request::Tree { recent: None }, &listed());
    app
}

#[test]
fn b_asks_for_a_name_and_enter_marks_the_moment() {
    let mut app = connected();
    press(&mut app, KeyCode::Char('b'));
    assert!(text(&render(&mut app, 120, 40)).contains("mark this moment as:"));
    type_text(&mut app, "dawn");
    press(&mut app, KeyCode::Enter);
    assert_eq!(
        calls(&mut app),
        [Request::Mark {
            name: "dawn".into()
        }]
    );
    let marked = MarkResult {
        name: "dawn".into(),
        tag: "dawn".into(),
        at: place(),
    };
    answer(
        &mut app,
        Request::Mark {
            name: "dawn".into(),
        },
        &marked,
    );
    assert!(status(&app).contains("marked “dawn” (after change 812) · B shows the tree"));
    press(&mut app, KeyCode::Char('b'));
    type_text(&mut app, "bad/name");
    press(&mut app, KeyCode::Enter);
    assert!(calls(&mut app).is_empty());
    assert!(status(&app).contains("a mark's name is 1 to 64"));
    assert!(format!("{:?}", app.mode()).contains("bad/name"));
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Char('b'));
    type_text(&mut app, "before-dawn");
    press(&mut app, KeyCode::Enter);
    assert_eq!(
        calls(&mut app),
        [Request::Mark {
            name: "before-dawn".into()
        }]
    );
}

#[test]
fn a_mark_name_taken_reopens_the_prompt_with_it() {
    let mut app = connected();
    app.on_reply(
        Reply::Answer {
            request: Request::Mark {
                name: "dawn".into(),
            },
            outcome: Err(Failure {
                code: Some(ErrorCode::MarkExists),
                message: "a mark named dawn is already taken".into(),
            }),
        },
        Instant::now(),
    );
    assert!(format!("{:?}", app.mode()).contains("Mark { text: \"dawn\" }"));
    assert_eq!(status(&app), "a mark named dawn is already taken");
}

#[test]
fn capital_b_lists_the_tree_and_enter_goes_without_a_question() {
    let mut app = open_list();
    for _ in 0..3 {
        press(&mut app, KeyCode::Char('j'));
    }
    press(&mut app, KeyCode::Enter);
    let screen = text(&render(&mut app, 120, 40));
    assert!(!screen.contains("go to “dusk”"), "{screen}");
    assert_eq!(*app.mode(), Mode::Normal);
    assert_eq!(
        calls(&mut app),
        [Request::Go {
            moment: "dusk".into()
        }]
    );
}

#[test]
fn y_and_n_do_nothing_in_the_tree_list() {
    let mut app = open_list();
    for key in ['y', 'n'] {
        press(&mut app, KeyCode::Char(key));
        assert_eq!(*app.mode(), Mode::Tree { index: 0 });
        assert!(calls(&mut app).is_empty());
    }
}

#[test]
fn moving_the_tree_cursor_says_the_distance_from_the_wall() {
    let mut app = open_list();
    let mut result = listed();
    result.here.as_mut().unwrap().commit = moment(16).commit;
    result.branches[0].tip = moment(16);
    result.branches[1].tip = moment(24);
    result.marks[0].moment = moment(13);
    result.recent.clear();
    let mut value = serde_json::to_value(result).unwrap();
    let mut parents: serde_json::Map<String, serde_json::Value> = (1..=24)
        .map(|n| {
            let parent = if n == 17 { 12 } else { n - 1 };
            (moment(n).commit, serde_json::json!(moment(parent).commit))
        })
        .collect();
    parents.insert(moment(0).commit, serde_json::Value::Null);
    value["parents"] = serde_json::json!(parents);
    app.on_reply(
        Reply::Answer {
            request: Request::Tree { recent: None },
            outcome: Ok(value.clone()),
        },
        Instant::now(),
    );
    press(&mut app, KeyCode::Down);
    assert_eq!(
        status(&app),
        "12 changes from the wall: 4 back to where they part, 8 on · Enter puts the wall here"
    );
    press(&mut app, KeyCode::Char('j'));
    assert_eq!(
        status(&app),
        "3 changes back from the wall · Enter puts the wall here"
    );
    press(&mut app, KeyCode::Char('k'));
    press(&mut app, KeyCode::Up);
    assert_eq!(status(&app), "the wall is here");
    assert!(calls(&mut app).is_empty(), "moving only looks");
    // The tree sends the newest parents only: a line that parts from the
    // wall's further back than that is said to, never guessed.
    for n in 0..=13 {
        parents.remove(&moment(n).commit);
    }
    value["parents"] = serde_json::json!(parents);
    app.on_reply(
        Reply::Answer {
            request: Request::Tree { recent: None },
            outcome: Ok(value),
        },
        Instant::now(),
    );
    press(&mut app, KeyCode::Down);
    assert_eq!(
        status(&app),
        "further from the wall than the tree counted · Enter puts the wall here"
    );
}

#[test]
fn one_change_from_the_wall_is_said_in_the_singular() {
    let mut app = open_list();
    let mut result = listed();
    result.here.as_mut().unwrap().commit = moment(16).commit;
    result.branches[0].tip = moment(16);
    result.branches[1].tip = moment(15);
    result.marks.clear();
    result.recent.clear();
    result.parents = (1..=16)
        .map(|n| (moment(n).commit, Some(moment(n - 1).commit)))
        .chain(std::iter::once((moment(0).commit, None)))
        .collect();
    answer(&mut app, Request::Tree { recent: None }, &result);
    press(&mut app, KeyCode::Down);
    assert_eq!(
        status(&app),
        "1 change back from the wall · Enter puts the wall here"
    );
}

#[test]
fn the_tree_list_draws_here_branches_marks_and_recent() {
    let mut app = open_list();
    let screen = text(&render(&mut app, 120, 40));
    for word in [
        "here: on main at #812",
        "2 changes behind",
        "branches",
        "marks",
        "recent",
        "“dawn”",
        "save “saved”",
        "Tom: turned cutoff",
        "and 5 more",
        "grew from dawn",
    ] {
        assert!(screen.contains(word), "missing {word}: {screen}");
    }
    let mut empty = listed();
    empty.here = None;
    empty.through = empty.revision;
    empty.branches.clear();
    empty.marks.clear();
    empty.recent.clear();
    empty.branch_count = 0;
    empty.mark_count = 0;
    answer(&mut app, Request::Tree { recent: None }, &empty);
    assert!(
        text(&render(&mut app, 120, 40))
            .contains("the tree has no moments yet: the first change makes one")
    );
}

#[test]
fn a_go_answer_forgets_the_wall_and_says_where_it_went() {
    let mut app = connected();
    let mut to = place();
    to.mark = Some("dawn".into());
    let result = LoadResult {
        change: Change {
            seq: 815,
            at: "2026-10-13T10:00:00Z".into(),
            seat: "Tom".into(),
            what: What::Go {
                to,
                from: place(),
                backup: "before-go".into(),
                fingerprint: "abc".into(),
                modules: 4,
                cables: 3,
                tempo: 96.0,
                by_console: true,
            },
            summary: "went to dawn".into(),
            undoes: None,
            clock: None,
        },
        backup: "before-go".into(),
        notes: vec!["kept the old wall".into()],
    };
    answer(
        &mut app,
        Request::Go {
            moment: "dawn".into(),
        },
        &result,
    );
    assert!(app.snapshot().is_some());
    assert!(status(&app).contains("went to mark \"dawn\""));
    assert!(status(&app).contains("z undoes it"));
    assert!(text(&render(&mut app, 120, 40)).contains("kept the old wall"));
}

fn command(app: &mut App, line: &str) {
    press(app, KeyCode::Char(':'));
    calls(app);
    type_text(app, line);
    press(app, KeyCode::Enter);
}

#[test]
fn the_commands_mark_go_and_tree() {
    let mut app = connected();
    command(&mut app, "mark dawn");
    assert_eq!(
        calls(&mut app),
        [Request::Mark {
            name: "dawn".into()
        }]
    );
    command(&mut app, "go #812");
    assert!(calls(&mut app).is_empty());
    press(&mut app, KeyCode::Char('y'));
    assert_eq!(
        calls(&mut app),
        [Request::Go {
            moment: "#812".into()
        }]
    );
    command(&mut app, "tree");
    assert!(
        calls(&mut app).is_empty(),
        "the command line already asked for the tree"
    );
    answer(&mut app, Request::Tree { recent: None }, &listed());
    assert!(
        app.completions("go da")
            .iter()
            .any(|completion| completion.word == "dawn")
    );
    assert!(
        app.completions("go ma")
            .iter()
            .any(|completion| completion.word == "main")
    );
}

#[test]
fn help_lists_b_and_capital_b() {
    let mut app = connected();
    press(&mut app, KeyCode::Char('?'));
    let mut screens = String::new();
    for _ in 0..12 {
        screens.push_str(&text(&render(&mut app, 120, 40)));
        press(&mut app, KeyCode::PageDown);
    }
    assert!(screens.contains("mark the moment the wall is at under a name"));
    assert!(screens.contains("the tree: j k say how far from the wall"));
    assert!(matches!(app.mode(), Mode::Help { .. }));
}

#[test]
fn help_says_enter_in_the_tree_list_goes_at_once() {
    let mut app = connected();
    press(&mut app, KeyCode::Char('?'));
    let mut screens = String::new();
    for _ in 0..12 {
        screens.push_str(&text(&render(&mut app, 120, 40)));
        press(&mut app, KeyCode::PageDown);
    }
    // The whole line shows: the help cuts a longer one off.
    assert!(
        screens.contains("the tree: j k say how far from the wall, ⏎ goes there, z undoes"),
        "{screens}"
    );
    assert!(!screens.contains("then y puts the wall"), "{screens}");
}

#[test]
fn a_go_question_keeps_the_keys_visible_for_long_names() {
    let mut app = connected();
    command(&mut app, &format!("go {}", "a".repeat(64)));
    assert!(calls(&mut app).is_empty());
    let screen = text(&render(&mut app, 120, 40));
    assert!(screen.contains("y goes · n stays"), "{screen}");
}

#[test]
fn a_typed_go_asks_before_sending_and_n_stays() {
    let mut app = connected();
    command(&mut app, "go #812");
    assert!(calls(&mut app).is_empty());
    assert!(matches!(app.mode(), Mode::Go { moment, .. } if moment == "#812"));
    press(&mut app, KeyCode::Char('n'));
    assert_eq!(*app.mode(), Mode::Normal);
    command(&mut app, "go #812");
    press(&mut app, KeyCode::Char('y'));
    assert_eq!(
        calls(&mut app),
        [Request::Go {
            moment: "#812".into()
        }]
    );
}

#[test]
fn a_late_name_taken_keeps_the_active_mode() {
    let mut app = open_list();
    app.on_reply(
        Reply::Answer {
            request: Request::Mark {
                name: "dawn".into(),
            },
            outcome: Err(Failure {
                code: Some(ErrorCode::MarkExists),
                message: "dawn is taken".into(),
            }),
        },
        Instant::now(),
    );
    assert_eq!(*app.mode(), Mode::Tree { index: 0 });
    assert_eq!(status(&app), "dawn is taken · b to mark again");
}

#[test]
fn tree_events_are_throttled_and_do_not_overlap() {
    use std::time::Duration;
    let mut app = open_list();
    let now = Instant::now();
    for n in 1..30 {
        app.on_reply(
            Reply::Event(Event::Marked {
                seat: "Tom".into(),
                name: format!("dawn{n}"),
                tag: format!("mark/dawn{n}"),
                at: place(),
            }),
            now,
        );
    }
    assert!(calls(&mut app).is_empty());
    app.tick(now);
    assert!(calls(&mut app).is_empty());
    let later = now + Duration::from_secs(1);
    app.tick(later);
    assert_eq!(calls(&mut app), [Request::Tree { recent: None }]);
    app.on_reply(
        Reply::Event(Event::Loaded {
            seq: 815,
            seat: "Tom".into(),
            save: None,
            backup: "before-go".into(),
        }),
        later,
    );
    press(&mut app, KeyCode::Char('r'));
    app.tick(later + Duration::from_secs(1));
    assert!(calls(&mut app).is_empty());
    answer(&mut app, Request::Tree { recent: None }, &listed());
    app.tick(later + Duration::from_secs(1));
    assert_eq!(calls(&mut app), [Request::Tree { recent: None }]);
    answer(&mut app, Request::Tree { recent: None }, &listed());
    press(&mut app, KeyCode::Esc);
    app.on_reply(
        Reply::Event(Event::Marked {
            seat: "Tom".into(),
            name: "dusk".into(),
            tag: "mark/dusk".into(),
            at: place(),
        }),
        later,
    );
    app.tick(later + Duration::from_secs(4));
    assert!(calls(&mut app).is_empty());
}

#[test]
fn any_change_event_refreshes_the_tree_on_the_tick() {
    let mut app = open_list();
    let now = Instant::now();
    app.on_reply(
        Reply::Event(Event::Change {
            change: Box::new(super::change(815, "Tom", "turned cutoff", None)),
        }),
        now,
    );
    assert!(calls(&mut app).is_empty());
    app.tick(now + std::time::Duration::from_secs(1));
    assert_eq!(calls(&mut app), [Request::Tree { recent: None }]);
}

#[test]
fn tree_refusals_stay_in_the_list() {
    let mut app = open_list();
    let before = status(&app);
    app.on_reply(
        Reply::Answer {
            request: Request::Tree { recent: None },
            outcome: Err(Failure {
                code: Some(ErrorCode::SlowDown),
                message: "tree asked too often".into(),
            }),
        },
        Instant::now(),
    );
    assert_eq!(status(&app), before);
    assert!(text(&render(&mut app, 120, 40)).contains("tree asked too often"));
}

#[test]
fn the_tree_cursor_follows_the_commit_and_clamps_when_gone() {
    let mut app = open_list();
    for _ in 0..6 {
        press(&mut app, KeyCode::Char('j'));
    }
    let mut result = listed();
    result.branches.insert(
        0,
        BranchView {
            name: "new".into(),
            tip: moment(900),
            grew_from: None,
        },
    );
    result.branch_count += 1;
    answer(&mut app, Request::Tree { recent: None }, &result);
    assert_eq!(*app.mode(), Mode::Tree { index: 7 });
    result.branches.clear();
    result.marks.clear();
    result.recent = vec![moment(900)];
    result.branch_count = 0;
    result.mark_count = 0;
    answer(&mut app, Request::Tree { recent: None }, &result);
    press(&mut app, KeyCode::Char('n'));
    assert_eq!(*app.mode(), Mode::Tree { index: 0 });
    press(&mut app, KeyCode::Enter);
    assert_eq!(
        calls(&mut app),
        [Request::Go {
            moment: "#900".into()
        }]
    );
}

#[test]
fn an_empty_tree_reports_lag_in_one_line_with_singular_changes() {
    let mut app = open_list();
    let mut result = listed();
    result.here = None;
    result.branches.clear();
    result.marks.clear();
    result.recent.clear();
    result.branch_count = 0;
    result.mark_count = 0;
    for (revision, expected) in [
        (813, "the tree is 1 change behind and holds no moments yet"),
        (814, "the tree is 2 changes behind and holds no moments yet"),
    ] {
        result.revision = revision;
        answer(&mut app, Request::Tree { recent: None }, &result);
        let screen = text(&render(&mut app, 120, 40));
        assert!(screen.contains(expected), "{screen}");
        assert!(!screen.contains("the first change makes one"));
    }
    result.here = Some(place());
    result.revision = 813;
    answer(&mut app, Request::Tree { recent: None }, &result);
    assert!(text(&render(&mut app, 120, 40)).contains("the tree is 1 change behind the wall"));
}

#[test]
fn unreadable_tree_times_keep_the_parse_error() {
    let mut app = open_list();
    let mut result = listed();
    result.branches[0].tip.at = "bad".into();
    answer(&mut app, Request::Tree { recent: None }, &result);
    let problem = chrono::DateTime::parse_from_rfc3339("bad")
        .unwrap_err()
        .to_string();
    assert!(
        text(&render(&mut app, 120, 40)).contains(&format!("unreadable time: bad ({problem})"))
    );
}

#[test]
fn undoing_go_and_load_keeps_the_backup_and_notes() {
    for what in [
        What::Go {
            to: place(),
            from: place(),
            backup: "before-undo".into(),
            fingerprint: "abc".into(),
            modules: 4,
            cables: 3,
            tempo: 96.0,
            by_console: true,
        },
        What::Load {
            save: Some("dawn".into()),
            backup: "before-undo".into(),
            fingerprint: "abc".into(),
            modules: 4,
            cables: 3,
            tempo: 96.0,
            by_console: true,
        },
    ] {
        let mut app = connected();
        let result = LoadResult {
            change: Change {
                what,
                ..super::change(816, "Tom", "undid the replacement", Some(815))
            },
            backup: "before-undo".into(),
            notes: vec!["undo kept the tempo".into()],
        };
        answer(&mut app, Request::Undo { change: 815 }, &result);
        assert!(status(&app).contains("before-undo"));
        assert!(text(&render(&mut app, 120, 40)).contains("undo kept the tempo"));
        assert!(app.snapshot().is_some());
    }
}

#[test]
fn the_tree_help_lists_every_available_key() {
    let mut app = open_list();
    let screen = text(&render(&mut app, 120, 40));
    assert!(
        screen.contains("j k move · ⏎ go there · z undo · Esc closes"),
        "{screen}"
    );
}

#[test]
fn undoing_a_replacement_says_what_the_wall_did() {
    for (what, summary) in [
        (
            What::Go {
                to: place(),
                from: place(),
                backup: "before-undo".into(),
                fingerprint: "abc".into(),
                modules: 4,
                cables: 3,
                tempo: 96.0,
                by_console: true,
            },
            "went back to main, after change 812 · the wall as it was is saved as “before-undo” · \
             z undoes it",
        ),
        (
            What::Load {
                save: None,
                backup: "before-undo".into(),
                fingerprint: "abc".into(),
                modules: 4,
                cables: 3,
                tempo: 96.0,
                by_console: true,
            },
            "brought back the wall it replaced · the wall as it was is saved as “before-undo” · z \
             undoes it",
        ),
    ] {
        let mut app = connected();
        let result = LoadResult {
            change: Change {
                what,
                ..super::change(816, "Tom", "undid change 815", Some(815))
            },
            backup: "before-undo".into(),
            notes: Vec::new(),
        };
        answer(&mut app, Request::Undo { change: 815 }, &result);
        let said = status(&app);
        assert!(!said.contains("cleared the wall"), "{said}");
        assert_eq!(said, summary);
    }
}

#[test]
fn the_tree_list_names_only_its_own_keys() {
    let mut app = open_list();
    let screen = text(&render(&mut app, 120, 40));
    assert!(screen.contains("⏎ go there"), "{screen}");
    assert!(!screen.contains("y goes"), "{screen}");
    assert!(!screen.contains("n stays"), "{screen}");
}
