use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use kazoo_core::voice::quantise::QuantiseSettings;
use kazoo_core::voice::{VOICE_HOP, VoiceFrame, hz_from_volts};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::{Buffer, Cell};
use ratatui::style::{Color, Modifier};

use super::{BRIGHT, DIM, Display, draw};
use crate::controls::{App, Control};

const RATE: u32 = 48_000;

fn voice(at: u64, semitones: f32, snapped: f32) -> VoiceFrame {
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

fn render(app: &App, display: &Display, width: u16, height: u16, now: Instant) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| draw(frame, app, display, "playing direct: no desk", now))
        .unwrap();
    terminal.backend().buffer().clone()
}

fn text(buffer: &Buffer) -> String {
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
    let buffer = render(&app, &display, 80, 24, Instant::now());
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
    let buffer = render(&app, &display, 80, 24, Instant::now());
    let screen = text(&buffer);
    assert!(screen.contains("sung A4"), "{screen}");
    assert!(screen.contains("snapped A4"), "{screen}");
    assert!(screen.contains("○ kazoo mouth"), "{screen}");
    for row in buffer.content.chunks(80).skip(1).take(5) {
        for cell in row.iter().filter(|cell| cell.symbol() != " ") {
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
    let buffer = render(&app, &display, 80, 24, Instant::now());
    let rows: Vec<_> = buffer.content.chunks(80).skip(6).take(9).collect();
    for column in 0..80 {
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
    let mut app = App::new(QuantiseSettings::default(), false);
    app.help = true;
    let display = Display::new(RATE);
    for (width, height) in [(60, 20), (79, 24), (80, 23)] {
        let buffer = render(&app, &display, width, height, Instant::now());
        assert_eq!(text(&buffer).trim(), "make the terminal at least 80×24");
    }
}

#[test]
fn the_footer_and_help_name_every_key() {
    let mut app = App::new(QuantiseSettings::default(), false);
    let display = Display::new(RATE);
    let now = Instant::now();
    let screen = text(&render(&app, &display, 80, 24, now));
    let footer = screen.lines().last().unwrap();
    for name in [
        "↑↓",
        "←→",
        "space",
        "v vibrato",
        "s silent",
        "tab fx",
        "? help",
    ] {
        assert!(footer.contains(name), "{name}: {footer}");
    }
    app.help = true;
    let help = text(&render(&app, &display, 100, 100, now));
    for name in [
        "Up",
        "Down",
        "Left",
        "Right",
        "Shift+Left",
        "Shift+Right",
        "Space",
        "v",
        "s",
        "?",
        "Esc",
        "Ctrl-C",
        "Tab",
        "[ / ]",
        "b:",
        "{ / }",
        "d:",
        ", / .",
        "< / >",
        "Settings:",
    ] {
        assert!(help.contains(name), "{name}: {help}");
    }
    let unbordered = help.replace('│', " ");
    let help_words = unbordered.split_whitespace().collect::<Vec<_>>().join(" ");
    for control in Control::ALL {
        assert!(help.contains(control.name()), "{help}");
        assert!(help_words.contains(control.help()), "{help}");
    }
    for slot in crate::engine::fx::Slot::ALL {
        for kind in 0..slot.ids().len() {
            assert!(
                help_words.contains(slot.kind(kind).unwrap().description),
                "{help}"
            );
        }
    }
    app.help = false;
    app.focus = crate::controls::Focus::Fx;
    let screen = text(&render(&app, &display, 80, 24, now));
    let footer = screen.lines().last().unwrap();
    for name in [
        "[ ]",
        "↑↓",
        "←→",
        "b on/off",
        "{ }",
        "d default",
        "tab synth",
        "? help",
    ] {
        assert!(footer.contains(name), "{name}: {footer}");
    }
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
    let screen = text(&render(&app, &display, 80, 24, Instant::now()));
    assert!(screen.contains("silent"));
    assert!(screen.contains("sung A4"));
    assert!(!screen.contains("NaN"));
    assert!(!screen.contains("inf"));
}

#[test]
fn the_controls_meters_and_link_are_visible() {
    let app = App::new(QuantiseSettings::default(), false);
    let mut display = Display::new(RATE);
    display.push(voice(64, 9.0, 9.0));
    let buffer = render(&app, &display, 80, 24, Instant::now());
    let screen = text(&buffer);
    assert!(screen.contains("playing direct: no desk"));
    assert!(screen.contains("quantise off: hertz for hertz"));
    assert!(screen.contains("loud ███████████░░░░░"));
    assert!(screen.contains("breath ████░░░░░░░░░░░░"));
    assert!(screen.contains("sure ██████████████░░"));
    assert!(screen.contains("gate ●"));
    for control in Control::ALL {
        assert!(screen.contains(control.name()), "{screen}");
    }
    for cell in &buffer.content[17 * 80..17 * 80 + 40] {
        assert_eq!(cell.bg, BRIGHT);
        assert_eq!(cell.fg, Color::Black);
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
    app.accepted.fx.slots[0].on = true;
    let buffer = render(&app, &Display::new(RATE), 80, 24, Instant::now());
    let screen = text(&buffer);
    let mut previous = 0;
    for name in ["megaphone ●", "fuzz ○", "delay ○", "echo ○", "plate ○"] {
        let index = screen.find(name).unwrap();
        assert!(index >= previous, "{screen}");
        previous = index;
    }
    for name in ["1/8d·1/4", "ltime", "feedback", "mix", "♩120.0 own"] {
        assert!(screen.contains(name), "{name}: {screen}");
    }
    app.moving = 1 << Slot::Delay as usize;
    assert!(text(&render(&app, &Display::new(RATE), 80, 24, Instant::now())).contains("delay ◐"));
}

#[test]
fn both_panels_show_at_thirty_rows() {
    let display = Display::new(RATE);
    let now = Instant::now();
    let mut app = App::new(QuantiseSettings::default(), false);
    let tall = text(&render(&app, &display, 80, 30, now));
    assert!(tall.contains("cutoff"));
    assert!(tall.contains("megaphone"));
    let short = text(&render(&app, &display, 80, 24, now));
    assert!(short.contains("cutoff"));
    assert!(!short.contains("megaphone"));
    app.focus = crate::controls::Focus::Fx;
    let focused = text(&render(&app, &display, 80, 24, now));
    assert!(focused.contains("megaphone"));
    assert!(!focused.contains("cutoff"));
    let tall = text(&render(&app, &display, 80, 30, now));
    assert!(tall.contains("cutoff"));
    assert!(tall.contains("megaphone"));
}

#[test]
fn help_scroll_stops_at_the_last_description_and_can_move_back() {
    let mut app = App::new(QuantiseSettings::default(), false);
    app.help = true;
    app.help_scroll.set(u16::MAX);
    let display = Display::new(RATE);
    let now = Instant::now();
    let last = text(&render(&app, &display, 80, 24, now));
    assert!(last.contains("spring"), "{last}");
    let end = app.help_scroll.get();
    assert!(end < u16::MAX);
    let (sender, receiver) = crossbeam_channel::bounded(64);
    app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE), &sender, now)
        .unwrap();
    assert_eq!(app.help_scroll.get(), end - 1);
    assert!(receiver.is_empty());
}
