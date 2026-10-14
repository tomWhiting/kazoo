use std::cell::RefCell;
use std::time::{Duration, Instant};

use crossbeam_channel::TrySendError;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use kazoo_core::synthesis::OscillatorShape;
use kazoo_core::voice::quantise::{NOTE_NAMES, QuantiseSettings, SCALE_NAMES};

use super::{Action, App, CommandSender, Control};
use crate::engine::Command;
use ringbuf::traits::Consumer;

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
        Command::Shutdown
        | Command::FxOn { .. }
        | Command::FxKind { .. }
        | Command::FxParam { .. }
        | Command::FxOrder(_)
        | Command::OwnTempo(_)
        | Command::LoopRecord
        | Command::LoopUndo
        | Command::LoopClear
        | Command::LoopLength(_)
        | Command::LoopPlay(_)
        | Command::LoopVolume(_) => panic!("a synth control sent a different command"),
    }
}

#[test]
fn loop_keys_send_their_commands() {
    use crate::engine::looper::{LoopLength, Looper, State};
    use std::sync::atomic::Ordering;

    let now = Instant::now();
    for focus in [super::Focus::Synth, super::Focus::Fx] {
        let mut app = App::new(QuantiseSettings::default(), false);
        app.focus = focus;
        let (mut looper, feed) = Looper::with_capacity(48_000, 96_000);
        let sender = FakeSender::default();
        app.loop_feed = Some(feed);
        for character in ['r', 'u', 'C', 'l', 'o', '-', '=', '_', '+'] {
            key(&mut app, &sender, KeyCode::Char(character), false, now);
            let commands = sender.commands.borrow();
            assert_eq!(commands.len(), 1);
            match (character, commands[0]) {
                ('r', Command::LoopRecord)
                | ('u', Command::LoopUndo)
                | ('C', Command::LoopClear)
                | ('l', Command::LoopLength(LoopLength::Bars(1)))
                | ('o', Command::LoopPlay(false)) => {}
                ('-' | '=' | '_' | '+', Command::LoopVolume(volume)) => {
                    assert!((volume - app.accepted.loop_volume).abs() < f32::EPSILON)
                }
                unexpected => panic!("wrong loop command: {unexpected:?}"),
            }
            drop(commands);
            sender.commands.borrow_mut().clear();
        }
        let gauges = app.loop_gauges().unwrap();
        gauges.n.store(48_000, Ordering::Relaxed);
        gauges.state.store(State::Playing as u8, Ordering::Relaxed);
        key(&mut app, &sender, KeyCode::Char('w'), false, now);
        assert!(sender.commands.borrow().is_empty());
        looper.process(&[0.0; 128], &[f64::NAN; 128], 120.0, &mut [0.0; 128]);
        let crate::engine::looper::LoopOut::Refused(samples) =
            app.loop_feed.as_mut().unwrap().out.try_pop().unwrap()
        else {
            panic!("blank fake tape accepted a bounce");
        };
        assert_eq!(samples.len(), 48_000);
        key(&mut app, &sender, KeyCode::Char('w'), false, now);
        assert_eq!(app.notice(now), Some("already saving"));
        app.loop_gauges()
            .unwrap()
            .state
            .store(State::Stopped as u8, Ordering::Relaxed);
        key(&mut app, &sender, KeyCode::Char('o'), false, now);
        assert!(matches!(
            sender.commands.borrow_mut().remove(0),
            Command::LoopPlay(true)
        ));
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

#[test]
fn fx_keys_select_turn_bypass_and_move() {
    use crate::controls::Focus;
    use crate::engine::fx::{Slot, mouth_default};

    let sender = FakeSender::default();
    let now = Instant::now();
    let mut app = App::new(QuantiseSettings::default(), false);
    key(&mut app, &sender, KeyCode::Tab, false, now);
    assert_eq!(app.focus, Focus::Fx);
    key(&mut app, &sender, KeyCode::Char(']'), false, now);
    assert_eq!(app.selected_slot, Slot::Distortion);
    key(&mut app, &sender, KeyCode::Char('b'), false, now);
    assert!(matches!(
        sender.commands.borrow_mut().remove(0),
        Command::FxOn {
            slot: Slot::Distortion,
            on: true
        }
    ));
    key(&mut app, &sender, KeyCode::Right, false, now);
    assert!(matches!(
        sender.commands.borrow_mut().remove(0),
        Command::FxKind {
            slot: Slot::Distortion,
            kind: 1
        }
    ));
    key(&mut app, &sender, KeyCode::Char(']'), false, now);
    assert_eq!(app.selected_slot, Slot::Delay);
    let description = Slot::Delay.kind(0).unwrap();
    let index = description
        .params
        .iter()
        .position(|spec| spec.name == "feedback")
        .unwrap();
    let spec = &description.params[index];
    app.selected_cell = index + 1;
    let value = app.accepted.fx.slots[2].values[0][index];
    key(&mut app, &sender, KeyCode::Right, true, now);
    let expected = spec.clamp(value + (spec.max - spec.min) / 10.0);
    match sender.commands.borrow_mut().remove(0) {
        Command::FxParam {
            slot: Slot::Delay,
            kind: 0,
            index: sent,
            value,
        } => {
            assert_eq!(usize::from(sent), index);
            assert_close(value, expected);
        }
        command => panic!("unexpected {command:?}"),
    }
    app.accepted.fx.slots[2].values[0][index] = spec.max;
    key(&mut app, &sender, KeyCode::Right, true, now);
    assert_close(app.accepted.fx.slots[2].values[0][index], spec.max);
    sender.commands.borrow_mut().clear();
    key(&mut app, &sender, KeyCode::Char('}'), false, now);
    let mut order = Slot::ALL;
    order.swap(2, 3);
    assert!(
        matches!(sender.commands.borrow_mut().remove(0), Command::FxOrder(sent) if sent == order)
    );
    assert_eq!(app.selected_slot, Slot::Delay);
    key(&mut app, &sender, KeyCode::Char('d'), false, now);
    assert!(
        matches!(sender.commands.borrow_mut().remove(0), Command::FxParam { slot: Slot::Delay, value, .. } if (value - mouth_default(Slot::Delay, 0, spec)).abs() < f32::EPSILON)
    );
}

#[test]
fn the_tempo_keys_belong_to_the_desk_when_it_has_one() {
    let sender = FakeSender::default();
    let now = Instant::now();
    let mut app = App::new(QuantiseSettings::default(), false);
    key(&mut app, &sender, KeyCode::Char('.'), false, now);
    assert!(
        matches!(sender.commands.borrow_mut().remove(0), Command::OwnTempo(bpm) if (bpm - 121.0).abs() < f64::EPSILON)
    );
    app.desk_bpm = Some(128.0);
    key(&mut app, &sender, KeyCode::Char('.'), false, now);
    assert!(sender.commands.borrow().is_empty());
    assert_eq!(
        app.notice(now),
        Some("the tempo is the desk's (128.0): change it on the desk")
    );
    assert!(app.notice(now + Duration::from_secs(2)).is_none());
}

#[test]
fn a_full_fx_channel_keeps_every_displayed_value() {
    use crate::controls::Focus;
    use crate::engine::fx::Slot;

    let sender = FakeSender::default();
    let now = Instant::now();
    let mut app = App::new(QuantiseSettings::default(), false);
    for _ in 0..64 {
        key(&mut app, &sender, KeyCode::Char('v'), false, now);
    }
    app.focus = Focus::Fx;
    app.selected_slot = Slot::Delay;
    let before = app.accepted;
    for code in [
        KeyCode::Char('b'),
        KeyCode::Right,
        KeyCode::Char('}'),
        KeyCode::Char('.'),
    ] {
        key(&mut app, &sender, code, false, now);
        assert_eq!(app.accepted, before);
        assert!(app.busy(now));
    }
    assert!(app.dirty_at.is_some());
}

#[test]
fn saves_follow_the_last_accepted_change_and_failures_stay_visible() -> std::io::Result<()> {
    use crate::controls::settings;
    use std::fs;
    use std::path::PathBuf;

    let directory =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/kz-logs/mouth-settings-app-save");
    if directory.exists() {
        fs::remove_dir_all(&directory)?;
    }
    fs::create_dir_all(&directory)?;
    let path = directory.join("settings.json");
    let sender = FakeSender::default();
    let now = Instant::now();
    let mut app = App::loaded(settings::Loaded::default(), path.clone(), now);
    key(&mut app, &sender, KeyCode::Char('v'), false, now);
    key(
        &mut app,
        &sender,
        KeyCode::Char('v'),
        false,
        now + Duration::from_millis(500),
    );
    app.save_if_due(now + Duration::from_secs(1), false);
    assert!(!path.exists());
    app.save_if_due(now + Duration::from_millis(1500), false);
    assert_eq!(settings::load(&path).settings, app.accepted);
    fs::remove_file(&path)?;
    fs::remove_dir(&directory)?;
    fs::write(&directory, b"not a directory")?;
    app.save_if_due(now + Duration::from_secs(2), true);
    assert!(
        app.notice(now)
            .is_some_and(|notice| notice.starts_with("settings not saved:"))
    );
    fs::remove_file(&directory)?;
    app.save_if_due(now + Duration::from_secs(3), true);
    assert!(app.notice(now).is_none());
    assert_eq!(settings::load(&path).settings, app.accepted);
    Ok(())
}

#[test]
fn load_notes_take_five_seconds_each_and_full_commands_do_not_restart_saving() {
    use crate::controls::settings;

    let sender = FakeSender::default();
    let now = Instant::now();
    let loaded = settings::Loaded {
        notes: vec!["first note".into(), "second note".into()],
        ..settings::Loaded::default()
    };
    let mut app = App::loaded(loaded, std::path::PathBuf::new(), now);
    assert_eq!(app.notice(now), Some("first note"));
    assert_eq!(
        app.notice(now + Duration::from_secs(5)),
        Some("second note")
    );
    assert!(app.notice(now + Duration::from_secs(10)).is_none());
    for _ in 0..64 {
        key(&mut app, &sender, KeyCode::Char('v'), false, now);
    }
    let accepted_at = app.dirty_at;
    key(
        &mut app,
        &sender,
        KeyCode::Char('v'),
        false,
        now + Duration::from_millis(500),
    );
    assert_eq!(app.dirty_at, accepted_at);
}

#[test]
fn a_queued_tempo_is_not_replaced_by_an_older_callback_snapshot() {
    use crate::engine::Stats;
    use std::sync::atomic::Ordering;

    let sender = FakeSender::default();
    let now = Instant::now();
    let mut app = App::new(QuantiseSettings::default(), false);
    let stats = Stats::default();
    stats.own_bpm.store(120.0_f64.to_bits(), Ordering::Relaxed);
    key(&mut app, &sender, KeyCode::Char('.'), false, now);
    app.sync_stats(&stats, now);
    assert_eq!(app.accepted.own_bpm.to_bits(), 121.0_f64.to_bits());
    key(&mut app, &sender, KeyCode::Char('.'), false, now);
    stats.own_bpm.store(121.0_f64.to_bits(), Ordering::Relaxed);
    app.sync_stats(&stats, now);
    assert_eq!(app.accepted.own_bpm.to_bits(), 122.0_f64.to_bits());
    stats.own_bpm.store(122.0_f64.to_bits(), Ordering::Relaxed);
    app.sync_stats(&stats, now);
    stats.own_bpm.store(150.0_f64.to_bits(), Ordering::Relaxed);
    app.sync_stats(&stats, now);
    assert_eq!(app.accepted.own_bpm.to_bits(), 150.0_f64.to_bits());
}
