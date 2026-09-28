//! Keys, and the exact requests they queue for the wall.

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use kazoo_wall::protocol::{
    ErrorCode, MonitorResult, RecordResult, Request, ShutdownResult, TempoResult,
};

use super::super::app::{
    App, Exit, Focus, Mode, Patching, TURN_LONGEST, TURN_SETTLE, Tone, jack_names,
};
use super::super::knob::{Step, Travel};
use super::super::link::LinkState;
use super::super::worker::{Failure, Job, Reply};
use super::render::render;
use super::{change, changed, connected, press, snapshot, type_text};

fn calls(app: &mut App) -> Vec<Request> {
    app.take_jobs()
        .into_iter()
        .map(|job| match job {
            Job::Call(request) => request,
            Job::Launch => panic!("a key started the wall unasked"),
        })
        .collect()
}

/// Select `module`'s knob `knob` with the arrow keys.
fn select(app: &mut App, module: &str, knob: &str) {
    let snapshot = snapshot();
    let target = snapshot
        .modules
        .iter()
        .position(|view| view.id == module)
        .expect("the module is on the wall");
    while app.selected_index() != Some(target) {
        let right = app.selected_index().is_none_or(|index| index < target);
        press(app, if right { KeyCode::Right } else { KeyCode::Left });
    }
    let knob_index = snapshot.modules[target]
        .knobs
        .iter()
        .position(|view| view.name == knob)
        .expect("the knob is on the module");
    while app.knob_index() > knob_index {
        press(app, KeyCode::Up);
    }
    while app.knob_index() < knob_index {
        press(app, KeyCode::Down);
    }
}

fn travel_of(module: &str, knob: &str) -> Travel {
    let snapshot = snapshot();
    let view = snapshot
        .modules
        .iter()
        .find(|view| view.id == module)
        .and_then(|view| view.knobs.iter().find(|view| view.name == knob))
        .expect("the knob exists");
    let catalogue = kazoo_wall::daemon::wall::catalogue();
    let kind = &snapshot
        .modules
        .iter()
        .find(|view| view.id == module)
        .expect("the module exists")
        .kind;
    let info = catalogue
        .kinds
        .iter()
        .find(|info| info.kind == *kind)
        .and_then(|info| info.knobs.iter().find(|info| info.name == knob));
    Travel::of(view, info)
}

#[test]
fn a_turn_goes_from_the_target_with_the_chosen_glide_once_the_key_rests() {
    let mut app = connected();
    select(&mut app, "vcf1", "cutoff");
    press(&mut app, KeyCode::Char(']'));
    assert!((app.glide_beats() - 4.0).abs() < f64::EPSILON);
    let start = Instant::now();
    app.handle_key(KeyEvent::new(KeyCode::Char('='), KeyModifiers::NONE), start);
    app.tick(start + TURN_SETTLE / 2);
    assert!(app.take_jobs().is_empty(), "the key has not rested yet");

    // The knob is gliding from 420 Hz to 800 Hz: the turn starts from
    // where it is going, not where it is.
    let expected = travel_of("vcf1", "cutoff").turned(800.0, true, Step::Normal);
    assert_eq!(app.ahead("vcf1", "cutoff"), Some(expected));
    app.tick(start + TURN_SETTLE);
    assert_eq!(
        calls(&mut app),
        vec![Request::Turn {
            module: "vcf1".to_string(),
            knob: "cutoff".to_string(),
            value: expected,
            glide_beats: Some(4.0),
        }]
    );
    // Shown ahead of the wall until the wall catches up.
    assert_eq!(app.ahead("vcf1", "cutoff"), Some(expected));
}

#[test]
fn a_held_key_is_one_turn_per_rest_or_per_long_hold() {
    let mut app = connected();
    select(&mut app, "vco1", "level");
    let travel = travel_of("vco1", "level");
    let start = Instant::now();
    let mut expected = snapshot().modules[0]
        .knobs
        .iter()
        .find(|knob| knob.name == "level")
        .expect("a level knob")
        .target;
    // Key repeat every 30 ms for 300 ms: nothing is sent while it is held.
    for repeat in 0..10_u32 {
        let at = start + Duration::from_millis(u64::from(repeat) * 30);
        app.handle_key(KeyEvent::new(KeyCode::Char('+'), KeyModifiers::SHIFT), at);
        expected = travel.turned(expected, true, Step::Fine);
        app.tick(at);
    }
    assert!(app.take_jobs().is_empty());
    // Held past the longest hold: one turn carrying every press.
    app.tick(start + TURN_LONGEST);
    let sent = calls(&mut app);
    assert_eq!(sent.len(), 1);
    let Request::Turn { value, .. } = &sent[0] else {
        panic!("expected a turn, got {sent:?}");
    };
    assert!((value - expected).abs() < 1e-9, "{value} vs {expected}");
    app.tick(start + TURN_LONGEST * 3);
    assert!(app.take_jobs().is_empty(), "nothing more to send");
}

#[test]
fn coarse_and_downward_turns_and_the_ends_of_travel() {
    let mut app = connected();
    select(&mut app, "vco1", "level");
    let travel = travel_of("vco1", "level");
    let start = Instant::now();
    app.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE), start);
    app.handle_key(KeyEvent::new(KeyCode::Char('-'), KeyModifiers::ALT), start);
    let from = snapshot().modules[0]
        .knobs
        .iter()
        .find(|knob| knob.name == "level")
        .expect("a level knob")
        .target;
    let expected = travel.turned(
        travel.turned(from, false, Step::Coarse),
        false,
        Step::Coarse,
    );
    app.tick(start + TURN_LONGEST);
    let sent = calls(&mut app);
    assert!(
        matches!(&sent[..], [Request::Turn { knob, value, .. }] if knob == "level" && (value - expected).abs() < 1e-9),
        "{sent:?}"
    );
}

#[test]
fn enter_types_a_value_or_a_named_position() {
    let mut app = connected();
    select(&mut app, "vcf1", "cutoff");
    press(&mut app, KeyCode::Enter);
    type_text(&mut app, "1200");
    press(&mut app, KeyCode::Enter);
    assert_eq!(
        calls(&mut app),
        vec![Request::Turn {
            module: "vcf1".to_string(),
            knob: "cutoff".to_string(),
            value: 1200.0,
            glide_beats: Some(2.0),
        }]
    );
    // Out of range is held to the range; nonsense is refused.
    press(&mut app, KeyCode::Enter);
    type_text(&mut app, "99999");
    press(&mut app, KeyCode::Enter);
    assert!(
        matches!(&calls(&mut app)[..], [Request::Turn { value, .. }] if (*value - 18_000.0).abs() < 1e-9)
    );
    press(&mut app, KeyCode::Enter);
    type_text(&mut app, "loud");
    press(&mut app, KeyCode::Enter);
    assert!(app.take_jobs().is_empty());
    assert!(
        app.status(Instant::now())
            .is_some_and(|status| status.tone == Tone::Trouble && status.text.contains("'loud'"))
    );
}

#[test]
fn patching_picks_an_output_then_an_input_then_the_amount() {
    let mut app = connected();
    select(&mut app, "lfo1", "rate");
    press(&mut app, KeyCode::Char('p'));
    assert_eq!(
        app.mode(),
        &Mode::Patch(Patching::From {
            module: "lfo1".to_string(),
            port: 0
        })
    );
    press(&mut app, KeyCode::Enter);
    // To the filter's resonance jack.
    press(&mut app, KeyCode::Char('l'));
    let filter = &snapshot().modules[2];
    let resonance = jack_names(filter)
        .iter()
        .position(|name| *name == "resonance")
        .expect("the filter has a resonance jack");
    for _ in 0..resonance {
        press(&mut app, KeyCode::Char('j'));
    }
    assert_eq!(
        app.mode(),
        &Mode::Patch(Patching::To {
            from: "lfo1.out".to_string(),
            module: "vcf1".to_string(),
            jack: resonance
        })
    );
    // The wall follows the patch cursor onto the knob.
    assert_eq!(app.selected_index(), Some(2));
    press(&mut app, KeyCode::Enter);
    for _ in 0..12 {
        press(&mut app, KeyCode::Char('-'));
    }
    press(&mut app, KeyCode::Char('+'));
    assert!(app.take_jobs().is_empty(), "nothing is sent until the end");
    press(&mut app, KeyCode::Enter);
    let sent = calls(&mut app);
    let [Request::Patch { from, to, amount }] = &sent[..] else {
        panic!("expected one patch, got {sent:?}");
    };
    assert_eq!(from, "lfo1.out");
    assert_eq!(to, "vcf1.resonance");
    assert!((amount.expect("an amount") - 0.41).abs() < 1e-9);
    assert_eq!(app.mode(), &Mode::Normal);
}

#[test]
fn patching_into_a_used_input_says_what_it_replaces_and_esc_steps_back() {
    let mut app = connected();
    select(&mut app, "lfo1", "rate");
    press(&mut app, KeyCode::Char('p'));
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Char('l'));
    let cutoff = jack_names(&snapshot().modules[2])
        .iter()
        .position(|name| *name == "cutoff")
        .expect("a cutoff jack");
    for _ in 0..cutoff {
        press(&mut app, KeyCode::Down);
    }
    press(&mut app, KeyCode::Enter);
    assert_eq!(
        app.mode(),
        &Mode::Patch(Patching::Amount {
            from: "lfo1.out".to_string(),
            to: "vcf1.cutoff".to_string(),
            amount: 1.0,
            replaces: Some(2)
        })
    );
    press(&mut app, KeyCode::Char('i'));
    assert!(
        matches!(app.mode(), Mode::Patch(Patching::Amount { amount, .. }) if (*amount + 1.0).abs() < 1e-9)
    );
    press(&mut app, KeyCode::Esc);
    assert!(matches!(app.mode(), Mode::Patch(Patching::To { jack, .. }) if *jack == cutoff));
    press(&mut app, KeyCode::Esc);
    assert!(matches!(app.mode(), Mode::Patch(Patching::From { module, .. }) if module == "lfo1"));
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.mode(), &Mode::Normal);
    assert!(app.take_jobs().is_empty());
}

#[test]
fn a_patch_ends_when_its_module_leaves_the_wall() {
    let mut app = connected();
    select(&mut app, "lfo1", "rate");
    press(&mut app, KeyCode::Char('p'));
    let mut gone = snapshot();
    gone.modules.remove(1);
    app.on_reply(Reply::Snapshot(Box::new(gone)), Instant::now());
    assert_eq!(app.mode(), &Mode::Normal);
    assert!(
        app.status(Instant::now())
            .is_some_and(|status| status.text.contains("lfo1 left the wall"))
    );
}

#[test]
fn undo_takes_the_latest_change_not_yet_undone_or_the_chosen_one() {
    let mut app = connected();
    press(&mut app, KeyCode::Char('z'));
    assert_eq!(calls(&mut app), vec![Request::Undo { change: 58 }]);
    changed(
        &mut app,
        Request::Undo { change: 58 },
        change(59, "Tom", "Tom undid change 58", Some(58)),
        None,
    );
    // 59 is an undo and 58 is undone: the next one back is 57.
    press(&mut app, KeyCode::Char('z'));
    assert_eq!(calls(&mut app), vec![Request::Undo { change: 57 }]);

    // In the log, z undoes the chosen line.
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::Tab);
    assert_eq!(app.focus(), Focus::Log);
    let line_58 = app
        .log()
        .lines()
        .position(|line| line.seq == Some(58))
        .expect("58 is in the log");
    for _ in 0..line_58 {
        press(&mut app, KeyCode::Char('j'));
    }
    press(&mut app, KeyCode::Char('z'));
    assert_eq!(calls(&mut app), vec![Request::Undo { change: 58 }]);
}

#[test]
fn removing_a_module_asks_first() {
    let mut app = connected();
    select(&mut app, "vco1", "octave");
    press(&mut app, KeyCode::Char('x'));
    assert_eq!(
        app.mode(),
        &Mode::Remove {
            module: "vco1".to_string(),
            cables: 1
        }
    );
    assert!(app.take_jobs().is_empty());
    press(&mut app, KeyCode::Char('n'));
    assert_eq!(app.mode(), &Mode::Normal);
    assert!(app.take_jobs().is_empty());
    press(&mut app, KeyCode::Char('x'));
    press(&mut app, KeyCode::Char('y'));
    assert_eq!(
        calls(&mut app),
        vec![Request::Remove {
            module: "vco1".to_string()
        }]
    );
}

#[test]
fn stopping_the_wall_asks_first_and_quitting_leaves_it_playing() {
    let mut app = connected();
    press(&mut app, KeyCode::Char('Q'));
    assert_eq!(app.mode(), &Mode::Stop);
    press(&mut app, KeyCode::Esc);
    assert!(app.take_jobs().is_empty());
    assert_eq!(app.exit(), None);
    press(&mut app, KeyCode::Char('Q'));
    press(&mut app, KeyCode::Char('y'));
    assert_eq!(calls(&mut app), vec![Request::Shutdown]);
    super::answer(
        &mut app,
        Request::Shutdown,
        &ShutdownResult { stopping: true },
    );
    assert_eq!(app.exit(), Some(Exit::Stopped));

    let mut app = connected();
    press(&mut app, KeyCode::Char('q'));
    assert_eq!(app.exit(), Some(Exit::Left));
    assert!(app.take_jobs().is_empty());
    let mut app = connected();
    app.handle_key(
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        Instant::now(),
    );
    assert_eq!(app.exit(), Some(Exit::Left));
}

#[test]
fn tempo_is_typed_and_checked() {
    let mut app = connected();
    press(&mut app, KeyCode::Char('t'));
    type_text(&mut app, "96.5");
    press(&mut app, KeyCode::Enter);
    assert_eq!(calls(&mut app), vec![Request::Tempo { bpm: 96.5 }]);
    super::answer(
        &mut app,
        Request::Tempo { bpm: 96.5 },
        &TempoResult {
            bpm: 96.5,
            desk: false,
            change: change(59, "Tom", "Tom set the tempo 96 BPM → 96.5 BPM", None),
        },
    );
    assert_eq!(app.log().latest_seq(), Some(59));
    press(&mut app, KeyCode::Char('t'));
    type_text(&mut app, "9x");
    press(&mut app, KeyCode::Enter);
    assert!(app.take_jobs().is_empty());
    assert!(
        app.status(Instant::now())
            .is_some_and(|status| status.tone == Tone::Trouble)
    );
}

#[test]
fn adding_picks_a_kind_optionally_named_and_selects_it_when_it_arrives() {
    let mut app = connected();
    press(&mut app, KeyCode::Char('a'));
    assert_eq!(
        app.mode(),
        &Mode::Add {
            index: 0,
            filter: String::new(),
            place: None
        },
        "the list view leaves the place to the wall"
    );
    let picker: Vec<String> = app
        .picker()
        .iter()
        .map(|(_, info)| info.kind.clone())
        .collect();
    // Sound sources come first.
    assert_eq!(picker[0], "vco");
    let lfo = picker
        .iter()
        .position(|kind| kind == "lfo")
        .expect("the catalogue has an lfo");
    for _ in 0..lfo {
        press(&mut app, KeyCode::Down);
    }
    press(&mut app, KeyCode::Enter);
    assert_eq!(
        calls(&mut app),
        vec![Request::Add {
            kind: "lfo".to_string(),
            name: None,
            place: None,
        }]
    );

    press(&mut app, KeyCode::Char('a'));
    press(&mut app, KeyCode::Tab);
    type_text(&mut app, "wob!");
    press(&mut app, KeyCode::Enter);
    assert!(app.take_jobs().is_empty(), "'!' is not allowed in a name");
    press(&mut app, KeyCode::Backspace);
    press(&mut app, KeyCode::Enter);
    assert_eq!(
        calls(&mut app),
        vec![Request::Add {
            kind: "vco".to_string(),
            name: Some("wob".to_string()),
            place: None,
        }]
    );

    // The new module is selected once the wall shows it.
    changed(
        &mut app,
        Request::Add {
            kind: "vco".to_string(),
            name: Some("wob".to_string()),
            place: None,
        },
        change(59, "Tom", "Tom added vco2 (wob)", None),
        Some("vco2"),
    );
    let mut grown = snapshot();
    grown.revision = 59;
    grown
        .modules
        .push(super::module("vco2", "vco", Some("wob")));
    app.on_reply(Reply::Snapshot(Box::new(grown)), Instant::now());
    assert_eq!(app.selected_index(), Some(4));
}

#[test]
fn unplugging_takes_the_cable_in_the_knob_or_the_one_chosen() {
    let mut app = connected();
    select(&mut app, "vcf1", "cutoff");
    press(&mut app, KeyCode::Char('u'));
    assert_eq!(
        calls(&mut app),
        vec![Request::Unpatch {
            cable: Some(2),
            to: None
        }]
    );
    select(&mut app, "vcf1", "resonance");
    press(&mut app, KeyCode::Char('u'));
    assert!(
        app.take_jobs().is_empty(),
        "nothing is in the resonance jack"
    );
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Char('u'));
    assert_eq!(
        calls(&mut app),
        vec![Request::Unpatch {
            cable: Some(3),
            to: None
        }]
    );
}

#[test]
fn the_wall_s_refusals_reach_the_status_line() {
    let mut app = connected();
    app.on_reply(
        Reply::Answer {
            request: Request::Remove {
                module: "vco9".to_string(),
            },
            outcome: Err(Failure {
                code: Some(ErrorCode::UnknownModule),
                message: "there is no module vco9".to_string(),
            }),
        },
        Instant::now(),
    );
    let status = app.status(Instant::now()).expect("a status");
    assert_eq!(status.tone, Tone::Trouble);
    assert_eq!(
        status.text,
        "remove vco9 refused [unknown_module]: there is no module vco9"
    );
}

#[test]
fn a_restarted_daemon_is_noticed_and_caught_up() {
    let mut app = connected();
    let now = Instant::now();
    app.on_reply(
        Reply::Link(LinkState::Down {
            reason: "the wall closed the connection".to_string(),
            attempts: 1,
            was_up: true,
        }),
        now,
    );
    assert!(!app.link().is_up());
    assert!(
        app.log()
            .lines()
            .next()
            .is_some_and(|line| line.text.contains("lost the wall"))
    );
    // Starting it from the console.
    press(&mut app, KeyCode::Char('s'));
    assert_eq!(app.take_jobs(), vec![Job::Launch]);
    press(&mut app, KeyCode::Char('s'));
    assert!(app.take_jobs().is_empty(), "one start at a time");
    app.on_reply(Reply::Launched(Ok(())), now);
    app.on_reply(
        Reply::Link(LinkState::Connected {
            daemon: "kazoo-wall 0.1.0".to_string(),
        }),
        now,
    );
    assert_eq!(
        calls(&mut app),
        vec![
            Request::Catalogue,
            Request::Log {
                before: None,
                limit: Some(100)
            }
        ]
    );
    // A snapshot ahead of the log fetches what was missed, once.
    super::answer(
        &mut app,
        Request::Log {
            before: None,
            limit: Some(100),
        },
        &kazoo_wall::protocol::LogPage {
            changes: Vec::new(),
            more: false,
        },
    );
    let mut ahead = snapshot();
    ahead.revision = 64;
    app.on_reply(Reply::Snapshot(Box::new(ahead.clone())), now);
    assert_eq!(
        calls(&mut app),
        vec![Request::Log {
            before: None,
            limit: Some(50)
        }]
    );
    app.on_reply(Reply::Snapshot(Box::new(ahead)), now);
    assert!(app.take_jobs().is_empty(), "already asked");
}

#[test]
fn other_seats_changes_arrive_as_events() {
    let mut app = connected();
    app.on_reply(
        Reply::Event(kazoo_wall::protocol::Event::Change {
            change: Box::new(change(60, "Vesper", "Vesper turned vcf1 drive", None)),
        }),
        Instant::now(),
    );
    app.on_reply(
        Reply::Event(kazoo_wall::protocol::Event::Seat {
            seat: "Vesper".to_string(),
            joined: true,
            seq: None,
        }),
        Instant::now(),
    );
    let lines: Vec<&str> = app.log().lines().map(|line| line.text.as_str()).collect();
    assert_eq!(lines[0], "Vesper joined the wall");
    assert_eq!(lines[1], "Vesper turned vcf1 drive");
    assert_eq!(app.log().latest_seq(), Some(60));
}

#[test]
fn movement_crosses_modules_and_rows() {
    let mut app = connected();
    app.columns = 2;
    assert_eq!(app.selected_index(), Some(0));
    press(&mut app, KeyCode::Char('l'));
    assert_eq!(app.selected_index(), Some(1));
    // Past the last knob, down goes to the module below.
    let knobs = snapshot().modules[1].knobs.len();
    for _ in 0..knobs {
        press(&mut app, KeyCode::Char('j'));
    }
    assert_eq!(app.selected_index(), Some(3));
    assert_eq!(app.knob_index(), 0);
    // Up from the first knob goes to the last knob of the module above.
    press(&mut app, KeyCode::Char('k'));
    assert_eq!(app.selected_index(), Some(1));
    assert_eq!(app.knob_index(), knobs - 1);
    press(&mut app, KeyCode::Char('h'));
    press(&mut app, KeyCode::Char('h'));
    assert_eq!(app.selected_index(), Some(0));
    // A removed selection falls to its neighbour.
    let mut fewer = snapshot();
    fewer.modules.remove(0);
    app.on_reply(Reply::Snapshot(Box::new(fewer)), Instant::now());
    assert_eq!(app.selected_index(), Some(0));
    assert_eq!(app.knob_index(), 0);
}

#[test]
fn fingerprints_arrive_at_once_and_empty_shares_clear_them() {
    use std::collections::BTreeMap;
    let mut app = connected();
    let event = |modules: BTreeMap<String, BTreeMap<String, f64>>| {
        Reply::Event(kazoo_wall::protocol::Event::Fingerprints {
            seq: 59,
            modules,
            cables: BTreeMap::new(),
        })
    };
    app.on_reply(
        event(BTreeMap::from([(
            "lfo1".to_string(),
            BTreeMap::from([("Vesper".to_string(), 1.0)]),
        )])),
        Instant::now(),
    );
    app.on_reply(
        event(BTreeMap::from([("vcf1".to_string(), BTreeMap::new())])),
        Instant::now(),
    );
    let held = &app.snapshot().expect("a snapshot").fingerprints.modules;
    assert_eq!(held.keys().collect::<Vec<_>>(), vec!["lfo1"]);
}

#[test]
fn m_makes_the_wall_heard_or_silent_again() {
    // The test wall is heard: m silences it.
    let mut app = connected();
    press(&mut app, KeyCode::Char('m'));
    assert_eq!(calls(&mut app), vec![Request::Monitor { on: false }]);
    super::answer(
        &mut app,
        Request::Monitor { on: false },
        &MonitorResult { on: false },
    );
    assert!(!app.snapshot().unwrap().heard);
    // Now silent: m makes it heard.
    press(&mut app, KeyCode::Char('m'));
    assert_eq!(calls(&mut app), vec![Request::Monitor { on: true }]);
    super::answer(
        &mut app,
        Request::Monitor { on: true },
        &MonitorResult { on: true },
    );
    assert!(app.snapshot().unwrap().heard);
}

#[test]
fn r_starts_a_recording_and_stops_it_again() {
    let mut app = connected();
    press(&mut app, KeyCode::Char('r'));
    assert_eq!(calls(&mut app), vec![Request::Record { on: true }]);
    let path = "/Users/tom/Music/kazoo-wall/wall-2026-09-27-203001.wav";
    let started = change(
        70,
        "Tom",
        "Tom started recording wall-2026-09-27-203001.wav",
        None,
    );
    super::answer(
        &mut app,
        Request::Record { on: true },
        &RecordResult {
            on: true,
            path: Some(path.to_string()),
            seconds: 0.0,
            dropped: 0,
            change: Some(started),
        },
    );
    let shown = app.snapshot().unwrap().recording.clone().unwrap();
    assert_eq!(shown.path, path);
    assert!(
        app.status(Instant::now()).is_some_and(|status| status
            .text
            .contains("recording wall-2026-09-27-203001.wav (r to stop)")),
        "{:?}",
        app.status(Instant::now())
    );
    // Recording now: r stops it.
    press(&mut app, KeyCode::Char('r'));
    assert_eq!(calls(&mut app), vec![Request::Record { on: false }]);
    let stopped = change(
        71,
        "Tom",
        "Tom stopped recording wall-2026-09-27-203001.wav after 3:20",
        None,
    );
    super::answer(
        &mut app,
        Request::Record { on: false },
        &RecordResult {
            on: false,
            path: Some(path.to_string()),
            seconds: 200.5,
            dropped: 0,
            change: Some(stopped),
        },
    );
    assert!(app.snapshot().unwrap().recording.is_none());
    assert!(
        app.status(Instant::now()).is_some_and(|status| status
            .text
            .contains("recorded wall-2026-09-27-203001.wav: 3:20")),
        "{:?}",
        app.status(Instant::now())
    );
}

#[test]
fn r_with_no_wall_says_there_is_nothing_to_record() {
    let mut app = App::new("Tom", "/tmp/kw/kazoo-wall.sock".to_string());
    press(&mut app, KeyCode::Char('r'));
    assert!(calls(&mut app).is_empty());
    assert!(app.status(Instant::now()).is_some_and(
        |status| status.tone == Tone::Trouble && status.text.contains("nothing to record")
    ));
}

/// A wall of `count` modules of every sort.
fn many(count: usize) -> kazoo_wall::protocol::Snapshot {
    const KINDS: [&str; 6] = ["vco", "lfo", "vcf", "seq", "env", "out"];
    let mut wall = snapshot();
    wall.modules = (0..count)
        .map(|n| {
            let kind = KINDS[n % KINDS.len()];
            super::module(&format!("{kind}{n}"), kind, None)
        })
        .collect();
    wall.cables.clear();
    wall
}

#[test]
fn in_the_rack_view_up_and_down_go_on_to_the_faceplate_above_or_below() {
    let mut app = connected();
    press(&mut app, KeyCode::Char('v'));
    app.on_reply(Reply::Snapshot(Box::new(many(12))), Instant::now());
    render(&mut app, 160, 80);
    let rows: std::collections::BTreeSet<u32> = app
        .rack
        .layout
        .plates
        .iter()
        .map(|plate| plate.at.y)
        .collect();
    assert_eq!(rows.len(), 2, "two rows fit");
    // The list view's panels across mean nothing here.
    app.columns = 1;
    let start = 1;
    while app.selected_index() != Some(start) {
        press(&mut app, KeyCode::Char('l'));
    }
    let top = app.rack.layout.plate(start).expect("a faceplate").at.y;
    let knobs = app.snapshot().expect("a wall").modules[start].knobs.len();
    for _ in 0..knobs {
        press(&mut app, KeyCode::Char('j'));
    }
    let below = app.selected_index().expect("a selection");
    assert_eq!(Some(below), app.rack.layout.beside(start, true));
    let plate = app.rack.layout.plate(below).expect("a faceplate");
    assert!(plate.at.y > top, "down went to the row below");
    assert_eq!(app.knob_index(), 0);
    // Straight below: the nearest faceplate to the middle of the one above.
    let above = app.rack.layout.plate(start).expect("a faceplate");
    let middle = |at: u32, width: u16| at * 2 + u32::from(width);
    let nearest = app
        .rack
        .layout
        .plates
        .iter()
        .filter(|candidate| candidate.at.y == plate.at.y)
        .map(|candidate| {
            middle(candidate.at.x, candidate.width).abs_diff(middle(above.at.x, above.width))
        })
        .min();
    assert_eq!(
        Some(middle(plate.at.x, plate.width).abs_diff(middle(above.at.x, above.width))),
        nearest
    );
    // Up from the first knob goes back up, to the last knob there.
    press(&mut app, KeyCode::Char('k'));
    let back = app.selected_index().expect("a selection");
    assert_eq!(app.rack.layout.plate(back).expect("a faceplate").at.y, top);
    assert_eq!(Some(back), app.rack.layout.beside(below, false));
    let last = app.snapshot().expect("a wall").modules[back].knobs.len() - 1;
    assert_eq!(app.knob_index(), last);
    // Nothing above the top row, nothing below the bottom one.
    for _ in 0..=last {
        press(&mut app, KeyCode::Char('k'));
    }
    assert_eq!(app.selected_index(), Some(back));
    assert_eq!(app.knob_index(), 0);
}

#[test]
fn a_module_added_in_the_rack_view_is_brought_into_sight() {
    let mut app = connected();
    press(&mut app, KeyCode::Char('v'));
    app.on_reply(Reply::Snapshot(Box::new(many(14))), Instant::now());
    render(&mut app, 120, 40);
    assert_eq!(app.rack.pan.x, 0);
    let add = Request::Add {
        kind: "lfo".to_string(),
        name: None,
        place: None,
    };
    changed(
        &mut app,
        add,
        change(59, "Tom", "Tom added lfo99", None),
        Some("lfo99"),
    );
    // Frames go by before the wall's next look shows it.
    render(&mut app, 120, 40);
    render(&mut app, 120, 40);
    let mut grown = many(14);
    grown.revision = 59;
    grown.modules.push(super::module("lfo99", "lfo", None));
    app.on_reply(Reply::Snapshot(Box::new(grown)), Instant::now());
    render(&mut app, 120, 40);
    assert_eq!(app.selected_index(), Some(14));
    let plate = app.rack.layout.plate(14).expect("its faceplate");
    let area = app.rack.area;
    assert!(
        plate.at.x >= app.rack.pan.x,
        "{:?} at {:?}",
        app.rack.pan,
        plate.at
    );
    assert!(
        plate.at.x + u32::from(plate.width) <= app.rack.pan.x + u32::from(area.width),
        "{:?} at {:?}",
        app.rack.pan,
        plate.at
    );
}

#[test]
fn f_puts_the_rack_view_s_log_away_and_tab_passes_it_by() {
    let mut app = connected();
    press(&mut app, KeyCode::Char('f'));
    assert!(app.log_shown(), "the list view keeps its log");
    assert!(
        app.status(Instant::now())
            .is_some_and(|status| status.text.contains("rack view"))
    );
    press(&mut app, KeyCode::Char('v'));
    press(&mut app, KeyCode::Tab);
    assert_eq!(app.focus(), Focus::Log);
    press(&mut app, KeyCode::Char('f'));
    assert!(!app.log_shown());
    assert_eq!(
        app.focus(),
        Focus::Wall,
        "the log's focus goes back to the wall"
    );
    press(&mut app, KeyCode::Tab);
    assert_eq!(app.focus(), Focus::Wall, "nothing else to go to");
    let screen = super::render::text(&render(&mut app, 160, 50));
    assert!(!screen.contains("LOG"), "{screen}");
    let tall = app.rack.area.height;
    // The list view shows it still; back in the rack it stays away.
    press(&mut app, KeyCode::Char('v'));
    assert!(app.log_shown());
    let screen = super::render::text(&render(&mut app, 160, 50));
    assert!(screen.contains("LOG"), "{screen}");
    press(&mut app, KeyCode::Char('v'));
    assert!(!app.log_shown());
    press(&mut app, KeyCode::Char('f'));
    assert!(app.log_shown());
    let screen = super::render::text(&render(&mut app, 160, 50));
    assert!(screen.contains("LOG"), "{screen}");
    assert!(app.rack.area.height < tall, "the log takes rows back");
    assert!(app.take_jobs().is_empty());
}

#[test]
fn shift_arrows_pan_a_quarter_and_dot_centres_the_selection() {
    let mut app = connected();
    press(&mut app, KeyCode::Char('v'));
    app.on_reply(Reply::Snapshot(Box::new(many(24))), Instant::now());
    render(&mut app, 120, 40);
    let area = app.rack.area;
    assert_eq!(app.rack.pan.x, 0);
    let shift = |app: &mut App, code| {
        app.handle_key(KeyEvent::new(code, KeyModifiers::SHIFT), Instant::now());
        render(app, 120, 40);
    };
    shift(&mut app, KeyCode::Right);
    assert_eq!(app.rack.pan.x, u32::from(area.width / 4));
    assert_eq!(app.selected_index(), Some(0), "the selection stays");
    shift(&mut app, KeyCode::Right);
    shift(&mut app, KeyCode::Left);
    assert_eq!(app.rack.pan.x, u32::from(area.width / 4));
    shift(&mut app, KeyCode::Left);
    shift(&mut app, KeyCode::Left);
    assert_eq!(app.rack.pan.x, 0, "never past the start");
    // Shift-down pans down, as far as the rack goes.
    shift(&mut app, KeyCode::Down);
    let most = app
        .rack
        .layout
        .height
        .saturating_sub(u32::from(area.height));
    assert_eq!(app.rack.pan.y, u32::from(area.height / 4).min(most));

    // . puts the selected faceplate in the middle.
    for _ in 0..12 {
        press(&mut app, KeyCode::Char('l'));
    }
    let chosen = app.selected_index().expect("a selection");
    app.rack.pan.x = 0;
    press(&mut app, KeyCode::Char('.'));
    render(&mut app, 120, 40);
    let plate = app.rack.layout.plate(chosen).expect("its faceplate");
    let middle = plate.at.x + u32::from(plate.width / 2);
    assert_eq!(middle - app.rack.pan.x, u32::from(area.width / 2));
    assert!(app.take_jobs().is_empty());
    // In the list view . says where it works.
    press(&mut app, KeyCode::Char('v'));
    press(&mut app, KeyCode::Char('.'));
    assert!(
        app.status(Instant::now())
            .is_some_and(|status| status.text.contains("rack view"))
    );
}

#[test]
fn g_jumps_to_a_module_by_id_name_or_kind() {
    let mut app = connected();
    press(&mut app, KeyCode::Char('g'));
    assert_eq!(
        app.mode(),
        &Mode::Jump {
            text: String::new()
        }
    );
    type_text(&mut app, "vcf1");
    let screen = super::render::text(&render(&mut app, 140, 40));
    assert!(screen.contains("jump to: vcf1█ → vcf1"), "{screen}");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.mode(), &Mode::Normal);
    assert_eq!(app.selected_index(), Some(2));
    // By name, ignoring case.
    press(&mut app, KeyCode::Char('g'));
    type_text(&mut app, "SLOW WOBBLE");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.selected_index(), Some(1));
    // By kind, and the start of a name.
    press(&mut app, KeyCode::Char('g'));
    type_text(&mut app, "out");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.selected_index(), Some(3));
    press(&mut app, KeyCode::Char('g'));
    type_text(&mut app, "slo");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.selected_index(), Some(1));
    // Nothing by that name says so and stays put.
    press(&mut app, KeyCode::Char('g'));
    type_text(&mut app, "zither");
    let screen = super::render::text(&render(&mut app, 140, 40));
    assert!(screen.contains("nothing by that name"), "{screen}");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.selected_index(), Some(1));
    assert!(
        app.status(Instant::now())
            .is_some_and(|status| status.tone == Tone::Trouble)
    );
    // Esc cancels; Backspace takes a letter back.
    press(&mut app, KeyCode::Char('g'));
    type_text(&mut app, "vcx");
    press(&mut app, KeyCode::Backspace);
    type_text(&mut app, "o");
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.selected_index(), Some(1));
    assert!(app.take_jobs().is_empty());
}

#[test]
fn jumping_again_goes_on_to_the_next_of_a_kind() {
    let mut app = connected();
    app.on_reply(Reply::Snapshot(Box::new(many(12))), Instant::now());
    let lfos: Vec<usize> = app
        .snapshot()
        .expect("a wall")
        .modules
        .iter()
        .enumerate()
        .filter(|(_, module)| module.kind == "lfo")
        .map(|(index, _)| index)
        .collect();
    assert_eq!(lfos, vec![1, 7]);
    for expected in [1, 7, 1] {
        press(&mut app, KeyCode::Char('g'));
        type_text(&mut app, "lfo");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.selected_index(), Some(expected));
    }
    // An exact id beats a kind.
    press(&mut app, KeyCode::Char('g'));
    type_text(&mut app, "lfo7");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.selected_index(), Some(7));
}

/// A console in the rack view of the small wall, hung in rows: vco1 and
/// lfo1 on the top row, vcf1 and out1 below.
fn in_rows() -> App {
    let mut app = connected();
    press(&mut app, KeyCode::Char('v'));
    let mut wall = snapshot();
    wall.rack = Some(rows(&[&["vco1", "lfo1"], &["vcf1", "out1"]]));
    app.on_reply(Reply::Snapshot(Box::new(wall)), Instant::now());
    render(&mut app, 160, 60);
    app
}

fn rows(rows: &[&[&str]]) -> Vec<Vec<String>> {
    rows.iter()
        .map(|row| row.iter().map(|id| (*id).to_string()).collect())
        .collect()
}

fn shove(app: &mut App, key: char, at: Instant) {
    app.handle_key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::SHIFT), at);
}

fn shown(app: &App) -> Option<Vec<Vec<String>>> {
    app.rack
        .rows(app.snapshot().and_then(|snapshot| snapshot.rack.as_deref()))
        .map(<[Vec<String>]>::to_vec)
}

#[test]
fn shift_hjkl_shove_a_module_and_the_shoves_go_as_one_move() {
    let mut app = in_rows();
    let t0 = Instant::now();
    assert_eq!(app.selected_index(), Some(0), "vco1");
    // Right along its row, then down to the row below, then down again
    // onto a row of its own: shown at once, sent once when the keys rest.
    shove(&mut app, 'L', t0);
    assert_eq!(
        shown(&app),
        Some(rows(&[&["lfo1", "vco1"], &["vcf1", "out1"]]))
    );
    shove(&mut app, 'J', t0 + Duration::from_millis(50));
    let down = shown(&app).expect("rows");
    assert_eq!(down[0], vec!["lfo1"]);
    assert!(down[1].contains(&"vco1".to_string()));
    shove(&mut app, 'J', t0 + Duration::from_millis(100));
    assert_eq!(
        shown(&app),
        Some(rows(&[&["lfo1"], &["vcf1", "out1"], &["vco1"]]))
    );
    app.tick(t0 + Duration::from_millis(100) + TURN_SETTLE / 2);
    assert!(app.take_jobs().is_empty(), "the keys have not rested");
    app.tick(t0 + Duration::from_millis(100) + TURN_SETTLE);
    assert_eq!(
        calls(&mut app),
        vec![Request::Arrange {
            module: "vco1".to_string(),
            row: 2,
            before: None,
            own: true,
        }]
    );
    // Drawn where it is going.
    render(&mut app, 160, 60);
    let lfo = app.rack.layout.plate(1).expect("lfo1").at.y;
    let vco = app.rack.layout.plate(0).expect("vco1").at.y;
    assert!(vco > lfo);
    // Up from the top row, alone there, it stays; off the end of a row too.
    let mut app = in_rows();
    shove(&mut app, 'K', t0);
    assert_eq!(
        shown(&app),
        Some(rows(&[&["vco1"], &["lfo1"], &["vcf1", "out1"]]))
    );
    shove(&mut app, 'K', t0);
    assert!(
        app.status(t0)
            .is_some_and(|status| status.text.contains("top row"))
    );
    shove(&mut app, 'H', t0);
    assert!(
        app.status(t0)
            .is_some_and(|status| status.text.contains("start of its row"))
    );
}

#[test]
fn held_shoves_stay_inside_the_flood_guard() {
    let mut app = in_rows();
    let t0 = Instant::now();
    let mut sent = 0;
    // Key repeat for three seconds: right and left, over and over.
    for step in 0..100_u32 {
        let at = t0 + Duration::from_millis(u64::from(step) * 30);
        shove(&mut app, if step % 2 == 0 { 'L' } else { 'H' }, at);
        app.tick(at);
        sent += calls(&mut app).len();
    }
    assert!(sent <= 8, "{sent} moves");
}

#[test]
fn z_puts_the_latest_move_back_then_undoes_from_the_log() {
    let mut app = in_rows();
    let t0 = Instant::now();
    shove(&mut app, 'L', t0);
    app.tick(t0 + TURN_SETTLE);
    let moved = rows(&[&["lfo1", "vco1"], &["vcf1", "out1"]]);
    assert_eq!(calls(&mut app).len(), 1);
    app.on_reply(
        Reply::Event(kazoo_wall::protocol::Event::Rack { rows: moved }),
        t0,
    );
    press(&mut app, KeyCode::Char('z'));
    assert_eq!(
        calls(&mut app),
        vec![Request::Arrange {
            module: "vco1".to_string(),
            row: 0,
            before: Some("lfo1".to_string()),
            own: false,
        }]
    );
    // The next z is the log's.
    press(&mut app, KeyCode::Char('z'));
    assert_eq!(calls(&mut app), vec![Request::Undo { change: 58 }]);

    // Put back before the wall has even shown the move: measured from the
    // rows the wall will have, it still goes back.
    let mut app = in_rows();
    shove(&mut app, 'L', t0);
    app.tick(t0 + TURN_SETTLE);
    assert_eq!(calls(&mut app).len(), 1);
    press(&mut app, KeyCode::Char('z'));
    assert_eq!(
        calls(&mut app),
        vec![Request::Arrange {
            module: "vco1".to_string(),
            row: 0,
            before: Some("lfo1".to_string()),
            own: false,
        }]
    );
    assert_eq!(
        shown(&app),
        Some(rows(&[&["vco1", "lfo1"], &["vcf1", "out1"]]))
    );

    // Anything else done after a move: z is the log's.
    let mut app = in_rows();
    shove(&mut app, 'L', t0);
    app.tick(t0 + TURN_SETTLE);
    assert_eq!(calls(&mut app).len(), 1);
    press(&mut app, KeyCode::Char('t'));
    type_text(&mut app, "100");
    press(&mut app, KeyCode::Enter);
    assert_eq!(calls(&mut app), vec![Request::Tempo { bpm: 100.0 }]);
    press(&mut app, KeyCode::Char('z'));
    assert_eq!(calls(&mut app), vec![Request::Undo { change: 58 }]);
}

#[test]
fn an_older_wall_says_its_modules_cannot_move() {
    let mut app = connected();
    press(&mut app, KeyCode::Char('v'));
    render(&mut app, 160, 60);
    shove(&mut app, 'L', Instant::now());
    assert!(app.take_jobs().is_empty());
    assert!(
        app.status(Instant::now())
            .is_some_and(|status| status.tone == Tone::Trouble && status.text.contains("older"))
    );
    // And in the list view, the keys say where they work.
    let mut app = in_rows();
    press(&mut app, KeyCode::Char('v'));
    shove(&mut app, 'L', Instant::now());
    assert!(app.take_jobs().is_empty());
    assert!(
        app.status(Instant::now())
            .is_some_and(|status| status.text.contains("rack view"))
    );
}

#[test]
fn typing_in_the_picker_finds_a_kind() {
    let mut app = connected();
    press(&mut app, KeyCode::Char('a'));
    type_text(&mut app, "LF");
    let Mode::Add { filter, index, .. } = app.mode().clone() else {
        panic!("the picker closed");
    };
    assert_eq!((filter.as_str(), index), ("LF", 0));
    let found: Vec<String> = app
        .picker_for("lf")
        .iter()
        .map(|(_, info)| info.kind.clone())
        .collect();
    assert!(found.contains(&"lfo".to_string()), "{found:?}");
    assert!(found.len() < app.picker().len());
    let screen = super::render::text(&render(&mut app, 120, 50));
    assert!(screen.contains("find: LF█"), "{screen}");
    // j, k, n and q are letters to find by, not keys.
    press(&mut app, KeyCode::Backspace);
    press(&mut app, KeyCode::Backspace);
    type_text(&mut app, "quant");
    press(&mut app, KeyCode::Enter);
    assert_eq!(
        calls(&mut app),
        vec![Request::Add {
            kind: "quant".to_string(),
            name: None,
            place: None,
        }]
    );
    // Nothing matching says so, and waits.
    press(&mut app, KeyCode::Char('a'));
    type_text(&mut app, "zzz");
    let screen = super::render::text(&render(&mut app, 120, 50));
    assert!(
        screen.contains("no kind of module matches 'zzz'"),
        "{screen}"
    );
    press(&mut app, KeyCode::Enter);
    assert!(app.take_jobs().is_empty());
    assert!(matches!(app.mode(), Mode::Add { .. }));
    // Esc clears what was typed, then closes.
    press(&mut app, KeyCode::Esc);
    assert!(matches!(app.mode(), Mode::Add { filter, .. } if filter.is_empty()));
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.mode(), &Mode::Normal);
}

#[test]
fn in_the_rack_view_a_goes_after_the_selected_module_and_says_where() {
    let mut app = in_rows();
    // vco1 is selected: the new module goes after it, before lfo1.
    press(&mut app, KeyCode::Char('a'));
    let Mode::Add { place, .. } = app.mode().clone() else {
        panic!("no picker");
    };
    let place = place.expect("a place");
    assert_eq!(place.row, 0);
    assert_eq!(place.before.as_deref(), Some("lfo1"));
    assert_eq!(app.place_words(&place), "to row 1 after vco1");
    let screen = super::render::text(&render(&mut app, 160, 60));
    assert!(
        screen.contains("add a module to row 1 after vco1"),
        "{screen}"
    );
    type_text(&mut app, "noise");
    press(&mut app, KeyCode::Tab);
    type_text(&mut app, "hiss");
    let screen = super::render::text(&render(&mut app, 160, 60));
    assert!(
        screen.contains("name the new noise (to row 1 after vco1): hiss"),
        "{screen}"
    );
    press(&mut app, KeyCode::Enter);
    assert_eq!(
        calls(&mut app),
        vec![Request::Add {
            kind: "noise".to_string(),
            name: Some("hiss".to_string()),
            place: Some(place),
        }]
    );
    assert!(
        app.status(Instant::now())
            .is_some_and(|status| status.text == "adding noise to row 1 after vco1…")
    );
    // At the end of a row, and on an older wall, where the wall puts it.
    press(&mut app, KeyCode::Char('l'));
    press(&mut app, KeyCode::Char('a'));
    let Mode::Add { place, .. } = app.mode().clone() else {
        panic!("no picker");
    };
    assert_eq!(place, Some(kazoo_wall::protocol::Place::end_of(0)));
    press(&mut app, KeyCode::Esc);
    let mut old = connected();
    press(&mut old, KeyCode::Char('v'));
    press(&mut old, KeyCode::Char('a'));
    assert!(matches!(old.mode(), Mode::Add { place: None, .. }));
}
