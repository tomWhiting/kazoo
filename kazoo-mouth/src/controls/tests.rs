use std::cell::RefCell;
use std::time::{Duration, Instant};

use crossbeam_channel::TrySendError;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use kazoo_core::synthesis::OscillatorShape;
use kazoo_core::voice::quantise::{NOTE_NAMES, QuantiseSettings, SCALE_NAMES};

use super::{Action, App, CommandSender, Control};
use crate::engine::Command;

#[derive(Debug, Default)]
struct FakeSender {
    commands: RefCell<Vec<Command>>,
}

impl CommandSender for FakeSender {
    fn try_send(&self, command: Command) -> Result<(), TrySendError<Command>> {
        let mut commands = self.commands.borrow_mut();
        if commands.len() == 64 {
            Err(TrySendError::Full(command))
        } else {
            commands.push(command);
            Ok(())
        }
    }
}

fn key(app: &mut App, sender: &impl CommandSender, code: KeyCode, shift: bool, now: Instant) {
    let modifiers = if shift {
        KeyModifiers::SHIFT
    } else {
        KeyModifiers::NONE
    };
    assert_eq!(
        app.handle_key(KeyEvent::new(code, modifiers), sender, now)
            .unwrap(),
        Action::Continue
    );
}

fn assert_whole_command(app: &App, sender: &FakeSender, control: Control) {
    let mut commands = sender.commands.borrow_mut();
    assert_eq!(commands.len(), 1, "{control:?}");
    match commands.remove(0) {
        Command::SetQuantise(settings) => {
            assert!(matches!(
                control,
                Control::Quantise
                    | Control::Scale
                    | Control::Root
                    | Control::Snap
                    | Control::Vibrato
                    | Control::Hysteresis
                    | Control::Blend
            ));
            assert_eq!(settings, app.accepted.quantise);
        }
        Command::SetSynth(settings) => {
            assert!(matches!(
                control,
                Control::Shape | Control::Cutoff | Control::Air | Control::Volume
            ));
            assert_eq!(settings, app.accepted.synth);
        }
        Command::SetSilent(silent) => {
            assert_eq!(control, Control::Silent);
            assert_eq!(silent, app.accepted.silent);
        }
        Command::Shutdown => panic!("a control sent shutdown"),
    }
}

const fn set_float(app: &mut App, control: Control, value: f32) {
    match control {
        Control::Snap => app.accepted.quantise.snap_ms = value,
        Control::Hysteresis => app.accepted.quantise.hysteresis_cents = value,
        Control::Blend => app.accepted.quantise.blend = value,
        Control::Cutoff => app.accepted.synth.cutoff_hz = value,
        Control::Air => app.accepted.synth.air = value,
        Control::Volume => app.accepted.synth.volume = value,
        _ => panic!("not a numeric control"),
    }
}

const fn get_float(app: &App, control: Control) -> f32 {
    match control {
        Control::Snap => app.accepted.quantise.snap_ms,
        Control::Hysteresis => app.accepted.quantise.hysteresis_cents,
        Control::Blend => app.accepted.quantise.blend,
        Control::Cutoff => app.accepted.synth.cutoff_hz,
        Control::Air => app.accepted.synth.air,
        Control::Volume => app.accepted.synth.volume,
        _ => panic!("not a numeric control"),
    }
}

fn assert_close(actual: f32, expected: f32) {
    assert!((actual - expected).abs() < 0.002, "{actual} != {expected}");
}

fn numeric_ranges_and_steps(now: Instant, sender: &FakeSender) {
    for (control, min, max, start, step, big) in [
        (Control::Snap, 0.0, 400.0, 30.0, 5.0, 50.0),
        (Control::Hysteresis, 0.0, 50.0, 0.0, 5.0, 25.0),
        (Control::Blend, 0.0, 1.0, 0.5, 0.05, 0.25),
        (Control::Air, 0.0, 1.0, 0.25, 0.05, 0.25),
        (Control::Volume, 0.0, 1.0, 0.5, 0.05, 0.25),
        (Control::Cutoff, 100.0, 12_000.0, 2400.0, 0.0, 0.0),
    ] {
        let mut app = App::new(QuantiseSettings::default(), false);
        app.selected = control;
        for (value, code) in [(min, KeyCode::Left), (max, KeyCode::Right)] {
            set_float(&mut app, control, value);
            key(&mut app, sender, code, false, now);
            assert_close(get_float(&app, control), value);
            assert_whole_command(&app, sender, control);
        }
        for shift in [false, true] {
            for (code, direction) in [(KeyCode::Left, -1.0), (KeyCode::Right, 1.0)] {
                set_float(&mut app, control, start);
                key(&mut app, sender, code, shift, now);
                let expected = if control == Control::Cutoff {
                    start * (direction / if shift { 1.0_f32 } else { 12.0_f32 }).exp2()
                } else {
                    direction
                        .mul_add(if shift { big } else { step }, start)
                        .clamp(min, max)
                };
                assert_close(get_float(&app, control), expected);
                assert_whole_command(&app, sender, control);
            }
        }
    }
}

fn indexed_ranges_and_steps(now: Instant, sender: &FakeSender) {
    let mut app = App::new(QuantiseSettings::default(), false);
    for shift in [false, true] {
        app.selected = Control::Scale;
        app.accepted.quantise.scale = 0;
        key(&mut app, sender, KeyCode::Left, shift, now);
        assert_eq!(app.accepted.quantise.scale, 0);
        assert_whole_command(&app, sender, Control::Scale);
        key(&mut app, sender, KeyCode::Right, shift, now);
        assert_eq!(app.accepted.quantise.scale, 1);
        assert_whole_command(&app, sender, Control::Scale);
        app.accepted.quantise.scale = SCALE_NAMES.len() - 1;
        key(&mut app, sender, KeyCode::Right, shift, now);
        assert_eq!(app.accepted.quantise.scale, SCALE_NAMES.len() - 1);
        assert_whole_command(&app, sender, Control::Scale);
        app.selected = Control::Root;
        app.accepted.quantise.root = 0;
        key(&mut app, sender, KeyCode::Left, shift, now);
        assert_eq!(app.accepted.quantise.root, 0);
        assert_whole_command(&app, sender, Control::Root);
        key(&mut app, sender, KeyCode::Right, shift, now);
        assert_eq!(app.accepted.quantise.root, 1);
        assert_whole_command(&app, sender, Control::Root);
        app.accepted.quantise.root = (NOTE_NAMES.len() - 1) as u8;
        key(&mut app, sender, KeyCode::Right, shift, now);
        assert_eq!(app.accepted.quantise.root, 11);
        assert_whole_command(&app, sender, Control::Root);
        app.selected = Control::Shape;
        app.accepted.synth.shape = OscillatorShape::Sine;
        key(&mut app, sender, KeyCode::Left, shift, now);
        assert_eq!(app.accepted.synth.shape, OscillatorShape::Sine);
        assert_whole_command(&app, sender, Control::Shape);
        for shape in [
            OscillatorShape::Triangle,
            OscillatorShape::Saw,
            OscillatorShape::Square,
            OscillatorShape::Square,
        ] {
            key(&mut app, sender, KeyCode::Right, shift, now);
            assert_eq!(app.accepted.synth.shape, shape);
            assert_whole_command(&app, sender, Control::Shape);
        }
    }
}

fn toggles_in_both_directions(now: Instant, sender: &FakeSender) {
    for control in [Control::Quantise, Control::Vibrato, Control::Silent] {
        for shift in [false, true] {
            let mut app = App::new(QuantiseSettings::default(), false);
            app.selected = control;
            let before = app.accepted;
            key(&mut app, sender, KeyCode::Left, shift, now);
            assert_ne!(app.accepted, before);
            assert_whole_command(&app, sender, control);
            key(&mut app, sender, KeyCode::Right, shift, now);
            assert_eq!(app.accepted, before);
            assert_whole_command(&app, sender, control);
        }
    }
}

#[test]
fn every_control_moves_within_its_range() {
    let now = Instant::now();
    let sender = FakeSender::default();
    numeric_ranges_and_steps(now, &sender);
    indexed_ranges_and_steps(now, &sender);
    toggles_in_both_directions(now, &sender);
}

#[test]
fn space_remembers_the_blend() {
    let sender = FakeSender::default();
    let now = Instant::now();
    let mut app = App::new(
        QuantiseSettings {
            blend: 0.6,
            ..QuantiseSettings::default()
        },
        false,
    );
    key(&mut app, &sender, KeyCode::Char(' '), false, now);
    assert_close(app.accepted.quantise.blend, 0.0);
    assert_whole_command(&app, &sender, Control::Quantise);
    key(&mut app, &sender, KeyCode::Char(' '), false, now);
    assert_close(app.accepted.quantise.blend, 0.6);
    assert_whole_command(&app, &sender, Control::Quantise);
}

#[test]
fn a_full_channel_says_busy_and_keeps_the_engines_value() {
    let sender = FakeSender::default();
    let now = Instant::now();
    let mut app = App::new(
        QuantiseSettings {
            blend: 0.6,
            ..QuantiseSettings::default()
        },
        false,
    );
    for _ in 0..64 {
        key(&mut app, &sender, KeyCode::Char('v'), false, now);
    }
    let before = app.accepted;
    for control in Control::ALL {
        app.selected = control;
        key(&mut app, &sender, KeyCode::Right, false, now);
        assert_eq!(app.accepted, before, "{control:?}");
        assert!(app.busy(now));
    }
    assert!(!app.busy(now + Duration::from_secs(1)));
    sender.commands.borrow_mut().clear();
    key(&mut app, &sender, KeyCode::Char(' '), false, now);
    assert_whole_command(&app, &sender, Control::Quantise);
    key(&mut app, &sender, KeyCode::Char(' '), false, now);
    assert_close(app.accepted.quantise.blend, 0.6);
    assert_whole_command(&app, &sender, Control::Quantise);
}

#[test]
fn selection_help_and_shortcuts_have_no_hidden_commands() {
    let sender = FakeSender::default();
    let now = Instant::now();
    let mut app = App::new(QuantiseSettings::default(), false);
    key(&mut app, &sender, KeyCode::Up, false, now);
    assert_eq!(app.selected, Control::Silent);
    key(&mut app, &sender, KeyCode::Down, false, now);
    assert_eq!(app.selected, Control::Quantise);
    key(&mut app, &sender, KeyCode::Char('?'), false, now);
    assert!(app.help);
    key(&mut app, &sender, KeyCode::Right, false, now);
    assert!(sender.commands.borrow().is_empty());
    key(&mut app, &sender, KeyCode::Esc, false, now);
    assert!(!app.help);
    key(&mut app, &sender, KeyCode::Char('v'), false, now);
    assert!(!app.accepted.quantise.keep_vibrato);
    assert_whole_command(&app, &sender, Control::Vibrato);
    key(&mut app, &sender, KeyCode::Char('s'), false, now);
    assert!(app.accepted.silent);
    assert_whole_command(&app, &sender, Control::Silent);
    assert_eq!(
        app.handle_key(
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &sender,
            now
        )
        .unwrap(),
        Action::Quit
    );
    app.help = true;
    assert_eq!(
        app.handle_key(
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            &sender,
            now
        )
        .unwrap(),
        Action::Quit
    );
}

#[test]
fn a_disconnected_command_channel_is_reported() {
    let (sender, receiver) = crossbeam_channel::bounded(64);
    drop(receiver);
    let mut app = App::new(QuantiseSettings::default(), false);
    let before = app.accepted;
    assert!(matches!(
        app.handle_key(
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE),
            &sender,
            Instant::now()
        ),
        Err(TrySendError::Disconnected(Command::SetSilent(true)))
    ));
    assert_eq!(app.accepted, before);
}
