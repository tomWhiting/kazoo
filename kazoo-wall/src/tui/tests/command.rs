//! The command line (`:`) and the picker's groups and detail pane.

use crossterm::event::KeyCode;

use crate::protocol::Request;

use super::super::app::{App, Mode, load_prefs, save_prefs};
use super::super::worker::Job;
use super::render::{render, text};
use super::{connected, press, type_text};

fn calls(app: &mut App) -> Vec<Request> {
    app.take_jobs()
        .into_iter()
        .filter_map(|job| match job {
            Job::Call(request) => Some(request),
            Job::Launch | Job::Wall(_) => None,
        })
        .collect()
}

/// The command line as typed so far.
fn line(app: &App) -> String {
    match app.mode() {
        Mode::Command(line) => line.text.clone(),
        other => panic!("the command line closed: {other:?}"),
    }
}

/// The error on the command line, if any.
fn error(app: &App) -> Option<String> {
    match app.mode() {
        Mode::Command(line) => line.error.clone(),
        other => panic!("the command line closed: {other:?}"),
    }
}

/// Open the command line, with the saves list asked for and taken.
fn open(app: &mut App) {
    press(app, KeyCode::Char(':'));
    assert_eq!(calls(app), vec![Request::Saves]);
}

#[test]
fn tab_completes_commands_modules_and_jacks_and_enter_patches() {
    let mut app = connected();
    open(&mut app);
    type_text(&mut app, "pa");
    press(&mut app, KeyCode::Tab);
    assert_eq!(line(&app), "patch ");
    type_text(&mut app, "lf");
    press(&mut app, KeyCode::Tab);
    assert_eq!(line(&app), "patch lfo1.", "a module, then its port");
    press(&mut app, KeyCode::Tab);
    assert_eq!(line(&app), "patch lfo1.out ");
    type_text(&mut app, "vcf1.");
    let jacks: Vec<String> = app
        .completions("patch lfo1.out vcf1.")
        .into_iter()
        .map(|completion| completion.word)
        .collect();
    assert!(jacks.contains(&"vcf1.in".to_string()), "{jacks:?}");
    assert!(jacks.contains(&"vcf1.cutoff".to_string()), "knobs too");
    // The arrows go through them, putting each on the line.
    press(&mut app, KeyCode::Down);
    assert_eq!(line(&app), format!("patch lfo1.out {}", jacks[0]));
    press(&mut app, KeyCode::Down);
    assert_eq!(line(&app), format!("patch lfo1.out {}", jacks[1]));
    press(&mut app, KeyCode::Up);
    press(&mut app, KeyCode::Up);
    assert_eq!(
        line(&app),
        format!("patch lfo1.out {}", jacks[jacks.len() - 1]),
        "round to the last"
    );
    let screen = text(&render(&mut app, 120, 40));
    assert!(screen.contains(" :patch lfo1.out"), "{screen}");
    assert!(
        screen.contains("patch <from.output> <to.input>"),
        "{screen}"
    );
    for _ in 0..jacks[jacks.len() - 1].len() {
        press(&mut app, KeyCode::Backspace);
    }
    type_text(&mut app, "vcf1.cutoff 0.5");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.mode(), &Mode::Normal);
    assert_eq!(
        calls(&mut app),
        vec![Request::Patch {
            from: "lfo1.out".to_string(),
            to: "vcf1.cutoff".to_string(),
            amount: Some(0.5),
        }]
    );
}

#[test]
fn a_wrong_line_says_why_and_stays_open() {
    let mut app = connected();
    open(&mut app);
    type_text(&mut app, "patch vcf1.cutoff lfo1.out");
    press(&mut app, KeyCode::Enter);
    assert_eq!(
        error(&app).as_deref(),
        Some("vcf1 has no output called 'cutoff'")
    );
    assert!(calls(&mut app).is_empty());
    let screen = text(&render(&mut app, 100, 30));
    assert!(
        screen.contains("✗ vcf1 has no output called 'cutoff'"),
        "{screen}"
    );
    // Typing clears it; Esc lets the line go.
    press(&mut app, KeyCode::Backspace);
    assert_eq!(error(&app), None);
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.mode(), &Mode::Normal);
    for (typed, why) in [
        ("frob", "'frob' is not a command"),
        ("u", "'u' is not a command"),
        ("add nothing", "there is no kind of module called 'nothing'"),
        ("remove vco9", "nothing on the wall is called 'vco9'"),
        (
            "turn vcf1.cutoff loud",
            "'loud' is not a value for vcf1 cutoff",
        ),
        ("patch lfo1.out vcf1.cutoff 2", "'2' is not an amount"),
        ("unplug vco1.pitch", "nothing is plugged into vco1.pitch"),
        ("save before-x", "the wall's own saves"),
        ("turn", "type turn <module.knob> <value>"),
    ] {
        press(&mut app, KeyCode::Char(':'));
        type_text(&mut app, typed);
        press(&mut app, KeyCode::Enter);
        let said = error(&app).unwrap_or_default();
        assert!(said.contains(why), "{typed}: {said}");
        press(&mut app, KeyCode::Esc);
    }
    assert!(
        calls(&mut app)
            .iter()
            .all(|request| *request == Request::Saves)
    );
}

#[test]
fn every_command_sends_its_request() {
    let mut app = connected();
    open(&mut app);
    press(&mut app, KeyCode::Esc);
    for (typed, wanted) in [
        (
            "add lfo slow one",
            Request::Add {
                kind: "lfo".to_string(),
                name: Some("slow one".to_string()),
                place: None,
            },
        ),
        (
            "remove vco1",
            Request::Remove {
                module: "vco1".to_string(),
            },
        ),
        (
            "unplug vcf1.cutoff",
            Request::Unpatch {
                cable: Some(2),
                to: None,
            },
        ),
        (
            "unplug 2",
            Request::Unpatch {
                cable: Some(2),
                to: None,
            },
        ),
        (
            "save tom fiddle",
            Request::Save {
                name: "tom fiddle".to_string(),
                overwrite: false,
            },
        ),
        (
            "load tom fiddle",
            Request::Load {
                name: "tom fiddle".to_string(),
            },
        ),
        ("undo", Request::Undo { change: 58 }),
    ] {
        press(&mut app, KeyCode::Char(':'));
        type_text(&mut app, typed);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.mode(), &Mode::Normal, "{typed}");
        assert_eq!(calls(&mut app), vec![wanted], "{typed}");
    }
    // A turn goes at once, by name or by named position.
    press(&mut app, KeyCode::Char(':'));
    type_text(&mut app, "turn vcf1.mode ");
    press(&mut app, KeyCode::Tab);
    assert_eq!(line(&app), "turn vcf1.mode low-pass", "the first position");
    press(&mut app, KeyCode::Tab);
    assert_eq!(line(&app), "turn vcf1.mode band-pass");
    press(&mut app, KeyCode::Enter);
    assert!(matches!(
        &calls(&mut app)[..],
        [Request::Turn { module, knob, value, .. }]
            if module == "vcf1" && knob == "mode" && (*value - 1.0).abs() < 1e-9
    ));
}

#[test]
fn tab_moves_between_the_pickers_groups_and_a_pane_tells_of_the_kind() {
    let mut app = connected();
    press(&mut app, KeyCode::Char('a'));
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::Tab);
    assert!(matches!(
        app.mode(),
        Mode::Add {
            tab: 2,
            index: 0,
            ..
        }
    ));
    let shown: Vec<String> = app
        .picker_shown(2, "")
        .iter()
        .map(|(_, info)| info.kind.clone())
        .collect();
    assert_eq!(
        shown,
        [
            "compare", "logic", "branch", "switch", "select", "divide", "math"
        ]
    );
    for (width, height) in [(80, 24), (120, 40), (200, 60)] {
        let screen = text(&render(&mut app, width, height));
        assert!(
            screen.contains("control flow"),
            "{width}×{height}\n{screen}"
        );
        assert!(screen.contains("compare"), "{width}×{height}\n{screen}");
        assert!(screen.contains("INPUTS"), "{width}×{height}\n{screen}");
        assert!(screen.contains("threshold"), "{width}×{height}\n{screen}");
    }
    // Typing finds within the category while it has a match, else goes to
    // the first that does; Tab still steps category by category.
    type_text(&mut app, "lfo");
    let Mode::Add { tab, index, .. } = app.mode().clone() else {
        panic!("the picker closed");
    };
    assert_eq!(index, 0);
    assert_eq!(app.picker_shown(tab, "lfo")[0].1.kind, "lfo");
    press(&mut app, KeyCode::Tab);
    let Mode::Add { tab: next, .. } = app.mode().clone() else {
        panic!("the picker closed");
    };
    assert_eq!(next, (tab + 1) % crate::tui::app::GROUPS.len());
}

#[test]
fn a_bare_add_opens_the_picker() {
    let mut app = connected();
    open(&mut app);
    type_text(&mut app, "a");
    press(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode(), Mode::Add { .. }), "{:?}", app.mode());
    assert!(calls(&mut app).is_empty());
}

#[test]
fn keys_opens_the_help() {
    let mut app = connected();
    open(&mut app);
    type_text(&mut app, "keys");
    press(&mut app, KeyCode::Enter);
    assert!(
        matches!(app.mode(), Mode::Help { scroll: 0 }),
        "{:?}",
        app.mode()
    );
    assert!(calls(&mut app).is_empty());
}

#[test]
fn the_picker_resizes_within_its_bounds_and_is_remembered() {
    let mut app = connected();
    press(&mut app, KeyCode::Char('a'));
    assert_eq!(app.picker_percent(), 100);
    press(&mut app, KeyCode::Char('>'));
    assert_eq!(app.picker_percent(), 100);
    for _ in 0..10 {
        press(&mut app, KeyCode::Char('<'));
    }
    assert_eq!(app.picker_percent(), 40);
    assert!(matches!(app.mode(), Mode::Add { filter, .. } if filter.is_empty()));
    press(&mut app, KeyCode::Char('>'));
    assert_eq!(app.prefs().picker_percent, 50);
}

#[test]
fn settings_survive_a_round_trip_and_a_missing_file_is_the_defaults() {
    let dir = tempfile::tempdir().expect("a folder");
    let file = dir.path().join("console.json");
    assert_eq!(
        load_prefs(&file),
        Ok(App::new("Tom", String::new()).prefs())
    );
    let mut app = connected();
    press(&mut app, KeyCode::Char('v'));
    press(&mut app, KeyCode::Char('c'));
    let prefs = app.prefs();
    assert!(prefs.rack);
    save_prefs(&file, &prefs).expect("saved");
    let mut fresh = App::new("Tom", String::new());
    fresh.apply_prefs(load_prefs(&file).expect("loaded"));
    assert_eq!(fresh.prefs(), prefs);
    std::fs::write(&file, "not json").expect("written");
    assert!(load_prefs(&file).is_err());
}

#[test]
fn group_shows_only_its_modules_and_cables_until_group_alone() {
    let mut app = connected();
    let groups = std::collections::BTreeMap::from([(
        "voice".to_string(),
        vec!["vco1".to_string(), "vcf1".to_string()],
    )]);
    app.on_reply(
        super::super::worker::Reply::Event(crate::protocol::Event::Groups { groups }),
        std::time::Instant::now(),
    );
    open(&mut app);
    type_text(&mut app, "group voice");
    press(&mut app, KeyCode::Enter);
    let shown = app.snapshot().unwrap();
    let ids: Vec<&str> = shown
        .modules
        .iter()
        .map(|module| module.id.as_str())
        .collect();
    assert_eq!(ids, ["vco1", "vcf1"]);
    assert_eq!(shown.cables.len(), 1);
    assert_eq!(app.group(), Some("voice"));
    press(&mut app, KeyCode::Char(':'));
    type_text(&mut app, "group");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.snapshot().unwrap().modules.len(), 4);
    assert_eq!(app.group(), None);
}

#[test]
fn tag_sends_a_group_request_for_the_named_modules() {
    let mut app = connected();
    open(&mut app);
    type_text(&mut app, "tag kick vco1 vcf1");
    press(&mut app, KeyCode::Enter);
    assert_eq!(
        calls(&mut app),
        vec![Request::Group {
            group: "kick".to_string(),
            modules: vec!["vco1".to_string(), "vcf1".to_string()],
            join: true,
        }]
    );
}

include!("chain_command_tests.rs");
