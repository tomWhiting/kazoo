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
fn capital_b_lists_the_tree_and_enter_then_y_goes() {
    let mut app = open_list();
    for _ in 0..3 {
        press(&mut app, KeyCode::Char('j'));
    }
    press(&mut app, KeyCode::Enter);
    let screen = text(&render(&mut app, 120, 40));
    assert!(screen.contains("go to “dusk”"), "{screen}");
    press(&mut app, KeyCode::Enter);
    assert!(calls(&mut app).is_empty());
    assert!(format!("{:?}", app.mode()).starts_with("Go"));
    press(&mut app, KeyCode::Char('n'));
    assert_eq!(format!("{:?}", app.mode()), "Tree { index: 3 }");
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Char('y'));
    assert_eq!(
        calls(&mut app),
        [Request::Go {
            moment: "dusk".into()
        }]
    );
    press(&mut app, KeyCode::Char('B'));
    calls(&mut app);
    press(&mut app, KeyCode::Char('r'));
    assert_eq!(calls(&mut app), [Request::Tree { recent: None }]);
    app.on_reply(
        Reply::Event(Event::Marked {
            seat: "Tom".into(),
            name: "dawn".into(),
            tag: "dawn".into(),
            at: place(),
        }),
        Instant::now(),
    );
    assert_eq!(calls(&mut app), [Request::Tree { recent: None }]);
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
    assert!(app.snapshot().is_none());
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
    assert_eq!(
        calls(&mut app),
        [Request::Go {
            moment: "#812".into()
        }]
    );
    command(&mut app, "tree");
    assert_eq!(calls(&mut app), [Request::Tree { recent: None }]);
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
    assert!(screens.contains("the tree: where the wall is, branches, marks"));
    assert!(matches!(app.mode(), Mode::Help { .. }));
}

#[test]
fn a_go_question_keeps_the_keys_visible_for_long_names() {
    let mut app = open_list();
    let mut result = listed();
    result.marks[0].name = "a".repeat(64);
    answer(&mut app, Request::Tree { recent: None }, &result);
    press(&mut app, KeyCode::Char('j'));
    press(&mut app, KeyCode::Char('j'));
    press(&mut app, KeyCode::Enter);
    let screen = text(&render(&mut app, 120, 40));
    assert!(screen.contains("y goes · n stays"), "{screen}");
}
