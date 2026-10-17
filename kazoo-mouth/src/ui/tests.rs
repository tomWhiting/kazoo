use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use kazoo_core::voice::quantise::QuantiseSettings;
use kazoo_core::voice::{VOICE_HOP, VoiceFrame, hz_from_volts};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::{Buffer, Cell};
use ratatui::style::Modifier;

use super::{BRIGHT, DIM, Display, draw};
use crate::controls::{App, Control};

const RATE: u32 = 48_000;

pub(super) fn voice(at: u64, semitones: f32, snapped: f32) -> VoiceFrame {
    VoiceFrame {
        at,
        hz: hz_from_volts(semitones / 12.0),
        volts: semitones / 12.0,
        snapped_hz: hz_from_volts(snapped / 12.0),
        snapped_volts: snapped / 12.0,
        gate: true,
        onset: false,
        voiced: true,
        loudness: 0.7,
        breath: 0.25,
        confidence: 0.9,
    }
}

pub(super) fn render(
    app: &App,
    display: &Display,
    width: u16,
    height: u16,
    now: Instant,
) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| draw(frame, app, display, "playing direct: no desk", now))
        .unwrap();
    terminal.backend().buffer().clone()
}

pub(super) fn text(buffer: &Buffer) -> String {
    buffer
        .content
        .chunks(usize::from(buffer.area.width))
        .map(|row| row.iter().map(Cell::symbol).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn the_pitch_panel_shows_sung_and_snapped() {
    let app = App::new(QuantiseSettings::default(), false);
    let mut display = Display::new(RATE);
    let frame = voice(64, 9.12, 9.0);
    let expected_hz = format!("{:.1} Hz", hz_from_volts(frame.volts));
    display.push(frame);
    display.recentre();
    let buffer = render(&app, &display, 120, 40, Instant::now());
    let screen = text(&buffer);
    assert!(screen.contains("+12¢"), "{screen}");
    assert!(screen.contains(&expected_hz), "{screen}");
    assert_eq!(expected_hz, "443.1 Hz");
    assert!(screen.contains("→ A4"), "{screen}");
    assert!(screen.contains("440.0 Hz"), "{screen}");
    assert!(screen.contains("● kazoo mouth"), "{screen}");
}

#[test]
fn unvoiced_is_dim_and_held() {
    let app = App::new(QuantiseSettings::default(), false);
    let mut display = Display::new(RATE);
    display.push(voice(64, 9.12, 9.0));
    let mut unvoiced = voice(128, 0.0, 0.0);
    unvoiced.voiced = false;
    unvoiced.gate = false;
    display.push(unvoiced);
    display.recentre();
    let buffer = render(&app, &display, 120, 40, Instant::now());
    let screen = text(&buffer);
    assert!(screen.contains("sung A4"), "{screen}");
    assert!(screen.contains("snapped A4"), "{screen}");
    assert!(screen.contains("○ kazoo mouth"), "{screen}");
    for row in buffer.content.chunks(120).skip(3).take(5) {
        for cell in row.iter().take(82).filter(|cell| cell.symbol() != " ") {
            assert_eq!(cell.fg, DIM);
            assert!(cell.modifier.contains(Modifier::DIM));
        }
    }
}

#[test]
fn the_trail_draws_four_seconds() {
    let app = App::new(
        QuantiseSettings {
            blend: 1.0,
            ..QuantiseSettings::default()
        },
        false,
    );
    let mut display = Display::new(RATE);
    for index in 0..3000 {
        let semitones = (index as f32).mul_add(4.0 / 2999.0, -2.0);
        display.push(voice(
            (index as u64 + 1) * VOICE_HOP as u64,
            semitones,
            semitones.round(),
        ));
    }
    display.recentre();
    assert_eq!(display.frames.len(), 3000);
    let buffer = render(&app, &display, 120, 40, Instant::now());
    let rows: Vec<_> = buffer.content.chunks(120).skip(8).take(6).collect();
    for column in 0..41 {
        assert!(
            rows.iter().any(|row| {
                row[column].fg == BRIGHT
                    && row[column]
                        .symbol()
                        .chars()
                        .any(|ch| ('\u{2801}'..='\u{28ff}').contains(&ch))
            }),
            "no snapped point in column {column}"
        );
    }
    assert!(text(&buffer).contains("C4"));
    let capacity = display.frames.capacity();
    display.push(voice(192_064, 2.0, 2.0));
    assert_eq!(display.frames.len(), 3000);
    assert_eq!(display.frames.front().unwrap().at, 128);
    assert_eq!(display.frames.capacity(), capacity);
}

#[test]
fn a_small_terminal_says_so() {
    let app = App::new(QuantiseSettings::default(), false);
    for (width, height) in [(1, 1), (20, 8), (60, 20), (79, 24), (80, 23)] {
        let buffer = render(&app, &Display::new(RATE), width, height, Instant::now());
        assert_eq!(buffer.area.width, width);
        assert_eq!(buffer.area.height, height);
    }
}

#[test]
fn the_footer_and_help_name_every_key() {
    let mut app = App::new(QuantiseSettings::default(), false);
    let display = Display::new(RATE);
    let now = Instant::now();
    let screen = text(&render(&app, &display, 120, 40, now));
    let footer = screen.lines().last().unwrap();
    for key in ["Tab", "↑↓", "←→", "w", "Space", "b", "d", "?"] {
        assert!(footer.contains(key), "{footer}");
    }
    assert!(footer.contains(app.selected.help()));
    app.help = true;
    let lines = super::board::help_lines(&app);
    for key in [
        "SOURCE",
        "PITCH",
        "GATE",
        "FX",
        "LOOP",
        "Shift+w",
        "Ctrl-C",
        "Enter on a control",
    ] {
        assert!(lines.iter().any(|line| line.contains(key)), "{key}");
    }
    assert!(lines.iter().all(|line| line.chars().count() <= 50));
}

#[test]
fn busy_is_visible_for_one_second() {
    let (sender, receiver) = crossbeam_channel::bounded(64);
    let now = Instant::now();
    let mut app = App::new(QuantiseSettings::default(), false);
    let display = Display::new(RATE);
    for _ in 0..65 {
        app.handle_key(
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE),
            &sender,
            now,
        )
        .unwrap();
    }
    assert_eq!(receiver.len(), 64);
    assert!(text(&render(&app, &display, 80, 24, now)).contains("busy: change not sent"));
    assert!(
        !text(&render(
            &app,
            &display,
            80,
            24,
            now + std::time::Duration::from_secs(1)
        ))
        .contains("busy: change not sent")
    );
}

#[test]
fn the_trail_recentres_only_beyond_three_semitones() {
    let mut display = Display::new(RATE);
    display.push(voice(64, 0.0, 0.0));
    display.recentre();
    assert!(display.centre.abs() < f64::EPSILON);
    for index in 0..3000 {
        display.push(voice((index as u64 + 2) * 64, 3.0, 3.0));
    }
    display.recentre();
    assert!(display.centre.abs() < f64::EPSILON);
    for index in 0..3000 {
        display.push(voice((index as u64 + 3002) * 64, 3.1, 3.0));
    }
    display.recentre();
    assert!((display.centre - 3.1).abs() < 0.0001);
}

#[test]
fn display_gaps_expire_old_frames_and_invalid_pitch_is_not_drawn() {
    let app = App::new(QuantiseSettings::default(), true);
    let mut display = Display::new(RATE);
    display.push(voice(64, 9.0, 9.0));
    let mut invalid = voice(200_000, 0.0, 0.0);
    invalid.volts = f32::NAN;
    invalid.snapped_volts = f32::INFINITY;
    invalid.loudness = f32::NAN;
    invalid.breath = f32::INFINITY;
    invalid.confidence = f32::NEG_INFINITY;
    display.push(invalid);
    display.recentre();
    assert_eq!(display.frames.len(), 1);
    assert!(!display.latest.unwrap().voiced);
    let screen = text(&render(&app, &display, 120, 40, Instant::now()));
    assert!(screen.contains("silent"));
    assert!(screen.contains("sung A4"));
    assert!(!screen.contains("NaN"));
    assert!(!screen.contains("inf"));
}

#[test]
fn the_controls_meters_and_link_are_visible() {
    let mut app = App::new(QuantiseSettings::default(), false);
    app.mic_rms = 0.1;
    app.output_rms = 0.25;
    let screen = text(&render(&app, &Display::new(RATE), 120, 40, Instant::now()));
    for name in [
        "playing direct: no desk",
        "quantise [off]",
        "mic",
        "output",
        "-20 dBFS",
        "latency ≥ 0.0 ms",
    ] {
        assert!(screen.contains(name), "{screen}");
    }
    for control in Control::ALL {
        assert!(screen.contains(control.name()), "{screen}");
    }
}

#[test]
fn trail_capacity_tracks_the_device_rate_and_unvoiced_frames_leave_gaps() {
    for rate in [44_100, 48_000, 96_000] {
        let mut display = Display::new(rate);
        let count = rate as usize * 4 / VOICE_HOP;
        for index in 0..=count {
            display.push(voice((index as u64 + 1) * VOICE_HOP as u64, 9.0, 9.0));
        }
        assert_eq!(display.frames.len(), count);
        let mut silence = voice((count as u64 + 2) * VOICE_HOP as u64, 9.0, 9.0);
        silence.voiced = false;
        display.push(silence);
        assert_eq!(display.frames.len(), count);
        assert_eq!(display.points(false).len(), count - 1);
        assert_eq!(display.points(true).len(), count - 1);
    }
}

#[test]
fn the_fx_panel_shows_the_chain_and_the_selected_slots_knobs() {
    use crate::controls::Focus;
    use crate::engine::fx::Slot;
    let mut app = App::new(QuantiseSettings::default(), false);
    app.focus = Focus::Fx;
    app.selected_slot = Slot::Delay;
    app.accepted.fx.slots[Slot::Megaphone as usize].on = true;
    let screen = text(&render(&app, &Display::new(RATE), 120, 40, Instant::now()));
    let mut previous = 0;
    for name in [
        "[●] megaphone",
        "[○] distortion",
        "[○] delay",
        "[○] echo",
        "[○] reverb",
    ] {
        let index = screen.find(name).unwrap();
        assert!(index >= previous, "{screen}");
        previous = index;
    }
    for name in ["ltime", "feedback", "mix", "120.0"] {
        assert!(screen.contains(name), "{screen}");
    }
}

#[test]
fn both_panels_show_at_thirty_rows() {
    let app = App::new(QuantiseSettings::default(), false);
    for height in [24, 30, 40] {
        let screen = text(&render(
            &app,
            &Display::new(RATE),
            80,
            height,
            Instant::now(),
        ));
        for name in ["SOURCE", "PITCH", "GATE", "FX", "cutoff", "megaphone"] {
            assert!(screen.contains(name), "{screen}");
        }
    }
}

#[test]
fn help_scroll_stops_at_the_last_description_and_can_move_back() {
    let mut app = App::new(QuantiseSettings::default(), false);
    app.help = true;
    app.help_scroll.set(u16::MAX);
    let screen = text(&render(&app, &Display::new(RATE), 80, 24, Instant::now()));
    assert!(screen.contains("Enter on a control"));
    let end = app.help_scroll.get();
    assert!(end > 0 && end < u16::MAX);
    let (sender, receiver) = crossbeam_channel::bounded(64);
    app.handle_key(
        KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
        &sender,
        Instant::now(),
    )
    .unwrap();
    assert_eq!(app.help_scroll.get(), end - 1);
    assert!(receiver.is_empty());
}

#[test]
fn the_loop_strip_shows_state_length_layers_and_volume() {
    use crate::engine::looper::{Looper, State};
    use std::sync::atomic::Ordering;

    let mut app = App::new(QuantiseSettings::default(), false);
    let (looper, feed) = Looper::with_capacity(RATE, 192_000);
    let gauges = &feed.gauges;
    gauges.state.store(State::Playing as u8, Ordering::Relaxed);
    gauges.n.store(192_000, Ordering::Relaxed);
    gauges.head.store(100_800, Ordering::Relaxed);
    gauges.layers.store(3, Ordering::Relaxed);
    gauges.bars.store(4, Ordering::Relaxed);
    gauges
        .loop_bpm
        .store(120.0_f64.to_bits(), Ordering::Relaxed);
    for arc in gauges.arcs.iter().take(3) {
        arc[1].store(192_000, Ordering::Relaxed);
    }
    app.loop_feed = Some(feed);
    let now = Instant::now();
    let buffer = render(&app, &Display::new(RATE), 80, 24, now);
    let screen = text(&buffer);
    for expected in [
        "▶ 2.10 / 4.00 s",
        "4 bars ♩120.0",
        "layers 3",
        "vol 80%",
        "▓",
    ] {
        assert!(screen.contains(expected), "{expected}: {screen}");
    }
    assert!(buffer[(42, 22)].modifier.contains(Modifier::REVERSED));
    app.desk_bpm = Some(128.0);
    app.loop_gauges()
        .unwrap()
        .off_tempo
        .store(true, Ordering::Relaxed);
    assert!(text(&render(&app, &Display::new(RATE), 80, 24, now)).contains("· desk 128.0"));
    drop(looper);
}

#[test]
fn every_panel_is_on_screen() {
    let app = App::new(QuantiseSettings::default(), false);
    let screen = text(&render(&app, &Display::new(RATE), 120, 40, Instant::now()));
    for name in ["SOURCE", "PITCH", "GATE", "FX", "open", "close", "hold"] {
        assert!(screen.contains(name), "missing {name}: {screen}");
    }
    for control in Control::ALL {
        assert!(screen.contains(control.name()), "{screen}");
    }
}

#[test]
fn small_terminals_stack() {
    let mut app = App::new(QuantiseSettings::default(), false);
    app.selected = Control::Cutoff;
    app.focus = crate::controls::Focus::Synth;
    for (width, height) in [(80, 30), (50, 20)] {
        let buffer = render(&app, &Display::new(RATE), width, height, Instant::now());
        assert_eq!(
            buffer.content.len(),
            usize::from(width) * usize::from(height)
        );
        assert!(text(&buffer).contains("cutoff"));
    }
}

#[test]
fn help_is_short_and_grouped() {
    let mut app = App::new(QuantiseSettings::default(), false);
    app.help = true;
    let screen = text(&render(&app, &Display::new(RATE), 120, 80, Instant::now()));
    for title in ["SOURCE", "PITCH", "GATE", "FX"] {
        assert!(screen.contains(title), "{screen}");
    }
    assert!(
        super::board::help_lines(&app)
            .iter()
            .all(|line| line.chars().count() <= 50)
    );
    let (sender, receiver) = crossbeam_channel::bounded(64);
    for ch in "/gate".chars() {
        app.handle_key(
            KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE),
            &sender,
            Instant::now(),
        )
        .unwrap();
    }
    let lines = super::board::help_lines(&app);
    assert!(!lines.is_empty());
    assert!(
        lines
            .iter()
            .all(|line| line.to_lowercase().contains("gate"))
    );
    let filtered = text(&render(&app, &Display::new(RATE), 120, 40, Instant::now()));
    assert!(!filtered.contains("SOURCE"));
    assert!(receiver.is_empty());
}

fn mouse(app: &mut App, control: Control, kind: crossterm::event::MouseEventKind, fraction: f32) {
    let buffer = render(app, &Display::new(RATE), 120, 40, Instant::now());
    assert!(text(&buffer).contains(control.name()));
    let hit = app
        .hits
        .borrow()
        .iter()
        .find(|hit| hit.target == crate::controls::panels::Target::Control(control))
        .copied()
        .expect("dial hit region missing");
    let (sender, receiver) = crossbeam_channel::bounded(64);
    let event = crossterm::event::MouseEvent {
        kind,
        column: hit.bar.x + (fraction * f32::from(hit.bar.width.saturating_sub(1))).round() as u16,
        row: hit.area.y,
        modifiers: KeyModifiers::NONE,
    };
    app.handle_mouse(event, &sender, Instant::now()).unwrap();
    assert!(!receiver.is_empty());
}

#[test]
fn a_click_sets_a_dial() {
    let mut app = App::new(QuantiseSettings::default(), false);
    mouse(
        &mut app,
        Control::Cutoff,
        crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
        0.75,
    );
    assert!((app.accepted.synth.cutoff_hz - 9025.0).abs() < 600.0);
}

#[test]
fn the_wheel_turns_a_dial() {
    let mut app = App::new(QuantiseSettings::default(), false);
    let before = app.accepted.synth.volume;
    for _ in 0..2 {
        mouse(
            &mut app,
            Control::Volume,
            crossterm::event::MouseEventKind::ScrollUp,
            0.5,
        );
    }
    assert!((app.accepted.synth.volume - before - 0.1).abs() < 0.001);
}

#[test]
fn the_gate_holds_its_order() {
    let mut app = App::new(QuantiseSettings::default(), false);
    mouse(
        &mut app,
        Control::GateClose,
        crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
        1.0,
    );
    assert!((app.accepted.gate.close_db - app.accepted.gate.open_db + 3.0).abs() < 0.001);
    assert!(text(&render(&app, &Display::new(RATE), 120, 40, Instant::now())).contains("-53 dBFS"));
}

#[test]
fn an_idle_effect_shows_flat() {
    use crate::engine::{
        Stats,
        fx::{FxChain, FxSettings, Slot},
    };
    use std::sync::atomic::Ordering;
    let mut settings = FxSettings::default();
    let slot = Slot::Delay;
    let mix = slot
        .kind(0)
        .unwrap()
        .params
        .iter()
        .position(|spec| spec.name == "mix")
        .unwrap();
    settings.slots[slot as usize].on = true;
    settings.slots[slot as usize].values[0][mix] = 0.0;
    let stats = Stats::default();
    let mut chain = FxChain::new(RATE, 256, &settings).unwrap();
    let input: Vec<_> = (0..256).map(|i| (i as f32 * 0.1).sin() * 0.25).collect();
    let mut left = [0.0; 256];
    let mut right = [0.0; 256];
    for _ in 0..20 {
        chain.process(
            kazoo_fx::Context { bpm: 120.0 },
            &input,
            [&mut left, &mut right],
            &stats,
        );
    }
    let read = || {
        (
            f32::from_bits(stats.fx_input[slot as usize].load(Ordering::Relaxed)),
            f32::from_bits(stats.fx_output[slot as usize].load(Ordering::Relaxed)),
        )
    };
    let (before, after) = read();
    assert!(before > 0.0, "slot RMS was not published");
    assert!((before - after).abs() < 0.000_01);
    chain.param(slot, 0, mix as u8, 1.0, &stats);
    for _ in 0..40 {
        chain.process(
            kazoo_fx::Context { bpm: 120.0 },
            &input,
            [&mut left, &mut right],
            &stats,
        );
    }
    let (before, after) = read();
    assert!((before - after).abs() > 0.000_01);
}

#[test]
fn harmony_effect_panel_keys_fit_without_cut_words() {
    use crate::controls::Focus;
    let mut app = App::new(QuantiseSettings::default(), false);
    app.focus = Focus::Fx;
    for (width, expected) in [
        (
            80,
            "[ ] effect · ↑↓ row · ←→ change · b on/off · { } move · d default",
        ),
        (40, "[ ] effect · ←→ change · b on/off"),
        (20, "[ ] effect"),
    ] {
        let mut terminal = Terminal::new(TestBackend::new(width, 18)).unwrap();
        terminal
            .draw(|frame| super::board::effects(frame, frame.area(), &app))
            .unwrap();
        let screen = text(terminal.backend().buffer());
        assert!(screen.contains(expected), "width {width}: {screen}");
    }
}

#[test]
fn harmony_rows_read_off_and_whole_semitones() {
    use crate::controls::Focus;
    use crate::engine::fx::Slot;
    let mut app = App::new(QuantiseSettings::default(), false);
    app.focus = Focus::Fx;
    app.selected_slot = Slot::Harmony;
    for (cell, name, value) in [
        (2, "voice 1 shift", "▏ -12 st"),
        (4, "voice 2 shift", "▏ +12 st"),
        (5, "voice 2 level", "▏ -3.00 dB"),
        (7, "voice 3 level", "▏ off"),
    ] {
        app.selected_cell = cell;
        let mut terminal = Terminal::new(TestBackend::new(60, 18)).unwrap();
        terminal
            .draw(|frame| super::board::effects(frame, frame.area(), &app))
            .unwrap();
        let screen = text(terminal.backend().buffer());
        let row = screen
            .lines()
            .find(|line| line.contains(name))
            .unwrap_or_else(|| panic!("{name} missing: {screen}"));
        assert!(row.trim_end().ends_with(value), "{row}");
    }
}

#[test]
fn harmony_help_says_the_copies_arrive_late_and_nothing_else() {
    use crate::engine::fx::harmony;
    assert!(harmony::KIND.description.contains("13 to 25 ms"));
    assert!(!harmony::KIND.description.contains("Enter"));
}

#[test]
fn mic_choice_and_desk_refusal_are_readable_together() {
    let mut app = App::new(QuantiseSettings::default(), false);
    app.input_label =
        "MacBook Pro Microphone (not the AirPods' mic: it would drop them to phone quality)".into();
    let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
    let display = Display::new(RATE);
    terminal
        .draw(|frame| {
            draw(
                frame,
                &app,
                &display,
                "not on the desk: it runs at 48000 Hz, the desk at 24000 Hz (use the same device)",
                Instant::now(),
            );
        })
        .unwrap();
    let screen = text(terminal.backend().buffer());
    assert!(screen.contains(
        "not on the desk: it runs at 48000 Hz, the desk at 24000 Hz (use the same device)"
    ));
    assert!(screen.contains(
        "mic: MacBook Pro Microphone (not the AirPods' mic: it would drop them to phone quality)"
    ));
}

#[test]
fn a_narrow_header_keeps_every_word_of_the_mic_and_the_desk() {
    let mut app = App::new(QuantiseSettings::default(), false);
    app.input_label =
        "MacBook Pro Microphone (not the AirPods' mic: it would drop them to phone quality)".into();
    let link = "not on the desk: it runs at 48000 Hz, the desk at 24000 Hz (use the same device)";
    for width in [24, 41, 60] {
        let mut terminal = Terminal::new(TestBackend::new(width, 40)).unwrap();
        let display = Display::new(RATE);
        terminal
            .draw(|frame| draw(frame, &app, &display, link, Instant::now()))
            .unwrap();
        let screen = text(terminal.backend().buffer()).replace('\n', "");
        assert!(screen.contains(link), "width {width}: {screen}");
        assert!(
            screen.contains(&format!("mic: {}", app.input_label)),
            "width {width}: {screen}"
        );
    }
}

/// The health row sits in the header, wrapped to the width and red while
/// it rises: on an 80 by 24 screen its words are whole, the footer is whole,
/// and the rest of the header still fits.
#[test]
fn the_health_row_wraps_in_the_header_and_leaves_the_footer_alone() {
    use crate::health::Losses;
    use ratatui::style::Color;
    let mut app = App::new(QuantiseSettings::default(), false);
    let display = Display::new(RATE);
    let now = Instant::now();
    let losses = Losses {
        mic_dropped: 28,
        display_dropped: 3,
        judge: kazoo_core::voice::judge::JudgeStatsSnapshot {
            pitch_runs_failed: 1,
            ..Default::default()
        },
        ..Losses::default()
    };
    let why = "pyin's forward FFT: the input and output buffers are the wrong length";
    app.observe_losses(losses, Some(why), now);
    let buffer = render(&app, &display, 80, 24, now);
    let screen = text(&buffer);
    let joined: String = screen.lines().map(str::trim_end).collect();
    assert!(
        joined.contains(&format!(
            "28 mic samples dropped (+2 others); pYIN failed: {why}"
        )),
        "{screen}"
    );
    let footer = screen.lines().last().unwrap();
    assert!(footer.starts_with(super::FOOTER), "{footer}");
    assert!(!footer.contains("lost"), "{footer}");
    // The health row takes three rows here; the rest still fit.
    let health_rows = screen
        .lines()
        .filter(|line| {
            line.contains("mic samples") || line.contains("FFT") || line.contains("length")
        })
        .count();
    assert!(health_rows >= 2, "{screen}");
    // The rest of the header still fits, after the health rows.
    assert!(screen.contains("mic: "), "{screen}");
    assert!(screen.contains("playing direct: no desk"), "{screen}");
    let row = screen
        .lines()
        .position(|line| line.contains("28 mic samples dropped"))
        .unwrap();
    let start = usize::from(buffer.area.width) * row;
    assert_eq!(buffer.content[start].fg, Color::Red, "fresh is red");

    // Once settled, the totals stay, amber.
    let later = now + crate::health::RISE_LIFETIME;
    let buffer = render(&app, &display, 80, 24, later);
    let screen = text(&buffer);
    let row = screen
        .lines()
        .position(|line| line.contains("lost so far: mic 28 · pyin 1 · screen 3"))
        .unwrap_or_else(|| panic!("{screen}"));
    let start = usize::from(buffer.area.width) * row;
    assert_eq!(buffer.content[start].fg, Color::Yellow, "settled is amber");
}

#[test]
fn nothing_lost_draws_no_health_row() {
    let app = App::new(QuantiseSettings::default(), false);
    let screen = text(&render(&app, &Display::new(RATE), 120, 40, Instant::now()));
    assert!(!screen.contains("lost"), "{screen}");
}

/// On a short screen the header is cut from the bottom, so the health row,
/// straight after the title, is the last of it to go.
#[test]
fn a_short_screen_keeps_the_health_row_before_the_latency() {
    use crate::health::Losses;
    let mut app = App::new(QuantiseSettings::default(), false);
    app.plots.latency_open = true;
    let now = Instant::now();
    app.observe_losses(
        Losses {
            mic_dropped: 28,
            ..Losses::default()
        },
        None,
        now,
    );
    let screen = text(&render(&app, &Display::new(RATE), 80, 12, now));
    assert!(screen.contains("28 mic samples dropped"), "{screen}");
}
