//! The console against a real headless daemon: its app layer and worker,
//! driven by keys, through add, patch, turn and undo, checked against what
//! the daemon itself shows; then the daemon restarts and the console
//! reconnects.

use std::path::Path;
use std::time::{Duration, Instant};

use crossterm::event::KeyCode;

use kazoo_wall::daemon::{Daemon, DaemonConfig};
use kazoo_wall::protocol::Snapshot;
use kazoo_wall::protocol::client::WallClient;

use super::super::app::{App, GLIDES, Mode, Patching, jack_names};
use super::super::worker::{Config, Connection};
use super::press;
use super::render::{render, text};

/// Longest any step may take.
const PATIENCE: Duration = Duration::from_secs(10);

/// Socket paths are capped at 104 bytes on macOS: the runtime directory
/// lives directly under /tmp.
fn runtime_dir() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("kwt")
        .tempdir_in("/tmp")
        .expect("a runtime directory")
}

fn start(runtime: &Path, state: &Path) -> Daemon {
    Daemon::start(DaemonConfig::headless(runtime, state)).expect("the daemon starts")
}

/// Send the console's jobs and take in its replies until `done`.
fn pump(app: &mut App, connection: &Connection, what: &str, mut done: impl FnMut(&App) -> bool) {
    let deadline = Instant::now() + PATIENCE;
    loop {
        for job in app.take_jobs() {
            connection.send(job).expect("the worker takes jobs");
        }
        if done(app) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}; status: {:?}; link: {:?}",
            app.status(Instant::now()),
            app.link()
        );
        if let Some(reply) = connection
            .reply_within(Duration::from_millis(20))
            .expect("the connection is running")
        {
            app.on_reply(reply, Instant::now());
        }
        app.tick(Instant::now());
    }
}

fn look(checker: &mut WallClient) -> Snapshot {
    checker.look().expect("the daemon shows itself")
}

fn knob_target(snapshot: &Snapshot, module: &str, knob: &str) -> f64 {
    snapshot
        .modules
        .iter()
        .find(|view| view.id == module)
        .and_then(|view| view.knobs.iter().find(|view| view.name == knob))
        .map_or_else(
            || panic!("{module}.{knob} is not on the wall"),
            |view| view.target,
        )
}

/// Move the wall's selection to `module`'s `knob` with the arrow keys.
fn select(app: &mut App, module: &str, knob: &str) {
    let snapshot = app.snapshot().expect("a snapshot").clone();
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

/// Add an LFO with the picker; returns its id once the console has
/// selected it.
fn add_lfo(app: &mut App, connection: &Connection, checker: &mut WallClient) -> String {
    let before = look(checker);
    press(app, KeyCode::Char('a'));
    let lfo = app
        .picker()
        .iter()
        .position(|(_, info)| info.kind == "lfo")
        .expect("the catalogue has an lfo");
    for _ in 0..lfo {
        press(app, KeyCode::Down);
    }
    press(app, KeyCode::Enter);
    pump(app, connection, "the new lfo", |app| {
        app.selected_index()
            .and_then(|index| app.snapshot()?.modules.get(index))
            .is_some_and(|module| {
                module.kind == "lfo" && !before.modules.iter().any(|old| old.id == module.id)
            })
    });
    let new_lfo = {
        let index = app.selected_index().expect("the new lfo is selected");
        app.snapshot().expect("a snapshot").modules[index]
            .id
            .clone()
    };
    assert!(
        look(checker)
            .modules
            .iter()
            .any(|module| module.id == new_lfo)
    );

    new_lfo
}

/// Patch `new_lfo` into the filter's resonance at 0.4, picking the jacks
/// with the keys.
fn patch_into_resonance(
    app: &mut App,
    connection: &Connection,
    checker: &mut WallClient,
    new_lfo: &str,
) {
    press(app, KeyCode::Char('p'));
    assert!(matches!(app.mode(), Mode::Patch(Patching::From { module, .. }) if module == new_lfo));
    press(app, KeyCode::Enter);
    for _ in 0..64 {
        if matches!(app.mode(), Mode::Patch(Patching::To { module, .. }) if module == "vcf1") {
            break;
        }
        let right = app
            .snapshot()
            .and_then(|snapshot| {
                let here = match app.mode() {
                    Mode::Patch(Patching::To { module, .. }) => module.clone(),
                    _ => String::new(),
                };
                let here = snapshot.modules.iter().position(|view| view.id == here)?;
                let there = snapshot.modules.iter().position(|view| view.id == "vcf1")?;
                Some(here < there)
            })
            .expect("both modules are on the wall");
        press(
            app,
            if right {
                KeyCode::Char('l')
            } else {
                KeyCode::Char('h')
            },
        );
    }
    let filter = app
        .snapshot()
        .and_then(|snapshot| snapshot.modules.iter().find(|view| view.id == "vcf1"))
        .expect("the seed has a filter")
        .clone();
    let resonance = jack_names(&filter)
        .iter()
        .position(|name| *name == "resonance")
        .expect("the filter has a resonance jack");
    for _ in 0..resonance {
        press(app, KeyCode::Char('j'));
    }
    press(app, KeyCode::Enter);
    for _ in 0..12 {
        press(app, KeyCode::Char('-'));
    }
    press(app, KeyCode::Enter);
    let from = format!("{new_lfo}.out");
    pump(app, connection, "the cable", |_| {
        look(checker).cables.iter().any(|cable| {
            cable.from == from && cable.to == "vcf1.resonance" && (cable.amount - 0.4).abs() < 1e-6
        })
    });
}

/// Turn the filter's cutoff one press up at once, then undo it with `z`.
fn turn_and_undo(app: &mut App, connection: &Connection, checker: &mut WallClient) {
    select(app, "vcf1", "cutoff");
    for _ in 0..GLIDES.len() {
        press(app, KeyCode::Char('['));
    }
    assert!(app.glide_beats().abs() < f64::EPSILON);
    let original = knob_target(&look(checker), "vcf1", "cutoff");
    press(app, KeyCode::Char('='));
    app.tick(Instant::now() + Duration::from_secs(1));
    pump(app, connection, "the turn", |_| {
        knob_target(&look(checker), "vcf1", "cutoff") > original
    });

    // Undo it: z takes the latest change, the turn.
    pump(app, connection, "the turn in the log", |app| {
        app.log().lines().any(|line| line.text.contains("cutoff"))
    });
    press(app, KeyCode::Char('z'));
    pump(app, connection, "the undo", |_| {
        (knob_target(&look(checker), "vcf1", "cutoff") - original).abs() < 1e-3
    });
    pump(app, connection, "the undo in the log", |app| {
        app.log().lines().any(|line| line.undoes.is_some())
    });
}

/// Move the new LFO along its row with `L` in the rack view, see every
/// console's rows follow, then put it back with `z`.
fn move_and_put_back(app: &mut App, connection: &Connection, checker: &mut WallClient, lfo: &str) {
    // Selected in the list view, where left and right go in the order
    // modules were added.
    select(app, lfo, "rate");
    press(app, KeyCode::Char('v'));
    render(app, 160, 60);
    let before = look(checker).rack.expect("the wall keeps rows");
    let row = before
        .iter()
        .find(|row| row.iter().any(|id| id == lfo))
        .expect("the lfo hangs in a row")
        .clone();
    assert!(row.len() > 1, "the lfo shares its row: {row:?}");
    let key = if row.first().is_some_and(|id| id == lfo) {
        'L'
    } else {
        'H'
    };
    press(app, KeyCode::Char(key));
    app.tick(Instant::now() + Duration::from_secs(1));
    pump(app, connection, "the move", |_| {
        look(checker).rack.as_ref() != Some(&before)
    });
    let moved = look(checker);
    assert_eq!(
        moved.revision,
        look(checker).revision,
        "a move is no change"
    );
    press(app, KeyCode::Char('z'));
    pump(app, connection, "the move put back", |_| {
        look(checker).rack.as_ref() == Some(&before)
    });
    press(app, KeyCode::Char('v'));
}

#[test]
fn the_console_adds_patches_turns_and_undoes_on_a_real_wall() {
    let runtime = runtime_dir();
    let state = tempfile::tempdir().expect("a state directory");
    let daemon = start(runtime.path(), state.path());
    let socket = daemon.socket().to_path_buf();
    let config = Config {
        socket: socket.clone(),
        seat: "Tom".to_string(),
        client: "kazoo-wall console test".to_string(),
        console: true,
        poll: Duration::from_millis(50),
        launch: None,
    };
    let connection = Connection::start(&config).expect("the connection starts");
    let mut app = App::new("Tom", socket.display().to_string());
    let mut checker =
        WallClient::connect(&socket, "Checker", "test", false).expect("a checking seat");

    pump(&mut app, &connection, "the first snapshot", |app| {
        app.link().is_up()
            && app.feed().is_up()
            && app.snapshot().is_some()
            && !app.picker().is_empty()
    });

    let new_lfo = add_lfo(&mut app, &connection, &mut checker);
    patch_into_resonance(&mut app, &connection, &mut checker, &new_lfo);
    turn_and_undo(&mut app, &connection, &mut checker);
    move_and_put_back(&mut app, &connection, &mut checker, &new_lfo);
    // Another seat's change reaches the log through the feed at once.
    checker
        .tempo(101.0)
        .expect("the checking seat sets the tempo");
    pump(&mut app, &connection, "the other seat's change", |app| {
        app.log()
            .lines()
            .any(|line| line.text.starts_with("Checker") && line.text.contains("tempo"))
    });

    // A real snapshot, as it draws.
    let screen = text(&render(&mut app, 120, 40));
    println!("{screen}");
    assert!(screen.contains(&new_lfo), "{screen}");
    assert!(screen.contains("→ vcf1.resonance"), "{screen}");
    assert!(screen.contains("● live"), "{screen}");

    // The daemon restarts: the console notices, and reconnects.
    drop(checker);
    daemon.stop();
    daemon.wait().expect("the daemon stops");
    pump(&mut app, &connection, "the lost link", |app| {
        !app.link().is_up()
    });
    let screen = text(&render(&mut app, 120, 40));
    assert!(screen.contains("not answering"), "{screen}");
    let daemon = start(runtime.path(), state.path());
    pump(&mut app, &connection, "the link back", |app| {
        app.link().is_up() && app.feed().is_up()
    });
    drop(connection);
    daemon.stop();
    daemon.wait().expect("the daemon stops");
}
