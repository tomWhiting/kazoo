use std::f32::consts::TAU;
use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::Color;

use kazoo_core::voice::quantise::QuantiseSettings;

use super::Display;
use super::latency::Latency;
use super::scope::{self, Mode};
use super::spectrum::{self, Spectrum};
use super::tests::{render, text, voice};
use crate::controls::App;
use crate::engine::capture::{Frames, frames};

const RATE: u32 = 48_000;

fn sine(hz: f32, offset: usize) -> Vec<f32> {
    captured(RATE, hz, offset)
}

#[test]
fn a_held_note_stands_still() {
    let mut crossings = Vec::new();
    for frame in 0..30 {
        let samples = sine(220.0, frame * 1600);
        let trace = scope::points(&samples, &samples, RATE, Some(220.0), 160);
        let column = trace
            .windows(2)
            .position(|pair| pair[0].1 <= 0.0 && pair[1].1 > 0.0)
            .unwrap();
        crossings.push(column);
    }
    assert!(
        crossings.iter().all(|column| *column == crossings[0]),
        "first crossings: {crossings:?}"
    );
}

#[test]
fn the_scope_draws_both() {
    let mut app = App::new(QuantiseSettings::default(), false);
    app.scope = Frames::from_samples(sine(220.0, 0), sine(440.0, 75));
    let display = Display::new(RATE);
    let buffer = render(&app, &display, 160, 45, Instant::now());
    assert!(
        buffer
            .content
            .iter()
            .any(|cell| cell.fg == scope::VOICE && cell.symbol() != " "),
        "voice trace missing"
    );
    assert!(
        buffer
            .content
            .iter()
            .any(|cell| cell.fg == scope::SYNTH && cell.symbol() != " "),
        "synth trace missing"
    );
    app.plots.mode = Mode::Voice;
    let buffer = render(&app, &display, 160, 45, Instant::now());
    assert!(
        buffer
            .content
            .iter()
            .any(|cell| cell.fg == scope::VOICE && cell.symbol() != " ")
    );
    assert!(
        !buffer
            .content
            .iter()
            .any(|cell| cell.fg == scope::SYNTH && cell.symbol() != " ")
    );
    app.plots.mode = Mode::Synth;
    let buffer = render(&app, &display, 160, 45, Instant::now());
    assert!(
        buffer
            .content
            .iter()
            .any(|cell| cell.fg == scope::SYNTH && cell.symbol() != " ")
    );
}

#[test]
fn the_spectrum_marks_the_fundamental() {
    let mut spectrum = Spectrum::default();
    spectrum.update(&sine(440.0, 0), RATE);
    let marked = spectrum::band(440.0).unwrap();
    let highest = spectrum
        .levels
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .unwrap()
        .0;
    assert_eq!(highest, marked);
    let mut terminal = Terminal::new(TestBackend::new(32, 2)).unwrap();
    terminal
        .draw(|frame| spectrum::draw(frame, frame.area(), &spectrum, Some(440.0)))
        .unwrap();
    assert_eq!(
        terminal.backend().buffer().content[marked].fg,
        Color::Yellow
    );
    let mut app = App::new(QuantiseSettings::default(), false);
    app.spectrum = spectrum;
    let mut display = Display::new(RATE);
    display.push(voice(64, 9.0, 9.0));
    let screen = text(&render(&app, &display, 160, 45, Instant::now()));
    assert!(
        screen.contains("fundamental 440 Hz"),
        "spectrum marker missing: {screen}"
    );
}

#[test]
fn latency_adds_its_parts() {
    let mut app = App::new(QuantiseSettings::default(), false);
    app.latency = Latency {
        input: Some(5.0),
        resampler: Some(2.7),
        block: Some(2.7),
        effects: Some(0.0),
        output: Some(10.0),
        device_buffers_only: false,
    };
    app.plots.latency_open = true;
    let screen = text(&render(&app, &Display::new(RATE), 160, 45, Instant::now()));
    assert!(
        screen.contains("20.4 ms · in 5.0 / dsp 5.4 / out 10.0"),
        "{screen}"
    );
    assert!(
        screen.contains("in 5.0 + resampler 2.7 + block 2.7 + fx 0.0 + out 10.0 ms"),
        "{screen}"
    );
    app.latency.output = None;
    let screen = text(&render(&app, &Display::new(RATE), 160, 45, Instant::now()));
    assert!(screen.contains("≥ 10.4 ms"), "{screen}");
    assert!(screen.contains("out ?"), "{screen}");
}

#[test]
fn plot_shortcuts_change_only_the_display() {
    let mut app = App::new(QuantiseSettings::default(), false);
    let (sender, receiver) = crossbeam_channel::bounded(1);
    let now = Instant::now();
    for mode in [Mode::Voice, Mode::Synth, Mode::Both] {
        app.handle_key(
            KeyEvent::new(KeyCode::Char('O'), KeyModifiers::SHIFT),
            &sender,
            now,
        )
        .unwrap();
        assert_eq!(app.plots.mode, mode);
    }
    app.handle_key(
        KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE),
        &sender,
        now,
    )
    .unwrap();
    assert!(!app.plots.spectrum_visible);
    app.handle_key(
        KeyEvent::new(KeyCode::Char('L'), KeyModifiers::SHIFT),
        &sender,
        now,
    )
    .unwrap();
    assert!(app.plots.latency_open);
    assert!(receiver.is_empty());
}

#[test]
fn nonfinite_display_samples_and_latency_stay_finite() {
    let samples: Vec<f32> = (0..frames(RATE))
        .map(|index| [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.5][index % 4])
        .collect();
    let points = scope::points(&samples, &samples, RATE, Some(f32::NAN), 80);
    assert!(points.iter().all(|(x, y)| x.is_finite() && y.is_finite()));
    let mut spectrum = Spectrum::default();
    spectrum.update(&samples, RATE);
    assert!(spectrum.levels.iter().all(|level| level.is_finite()));
    let latency = Latency {
        input: Some(f64::NAN),
        output: Some(f64::INFINITY),
        ..Latency::default()
    };
    assert_eq!(latency.header(), "≥ 0.0 ms · in ? / dsp ? / out ?");
}

/// What the engine captures at `rate`, as the scope is handed it.
fn captured(rate: u32, hz: f32, offset: usize) -> Vec<f32> {
    (0..frames(rate))
        .map(|index| (TAU * hz * (index + offset) as f32 / rate as f32).sin() * 0.7)
        .collect()
}

/// The lowest pitch the mouth tracks: the voice range's floor.
const LOWEST: f32 = kazoo_core::analysis::VOICE_MIN_HZ;

#[test]
fn the_lowest_pitch_stands_still_over_four_periods_at_every_rate() {
    for rate in [44_100, 48_000, 96_000, 192_000] {
        let mut starts = Vec::new();
        for frame in 0..24 {
            let samples = captured(rate, LOWEST, frame * 397);
            let trace = scope::points(&samples, &samples, rate, Some(LOWEST), 400);
            // Inside the first and last two columns, which sit on the trigger.
            let rising: Vec<_> = trace[2..trace.len() - 2]
                .windows(2)
                .enumerate()
                .filter(|(_, pair)| pair[0].1 <= 0.0 && pair[1].1 > 0.0)
                .map(|(column, _)| column + 2)
                .collect();
            // Four periods from a rising crossing: three more rise inside.
            assert_eq!(
                rising.len(),
                3,
                "{rate} Hz frame {frame}: rising crossings at {rising:?}"
            );
            assert!(
                trace[0].1.abs() < 0.02 && trace[1].1 > trace[0].1,
                "{rate} Hz frame {frame}: the trace starts at {:?}, not on a rising crossing",
                &trace[..2]
            );
            starts.push(rising[0]);
        }
        assert!(
            starts.iter().all(|column| *column == starts[0]),
            "{rate} Hz: {starts:?}"
        );
    }
}

#[test]
fn loop_playback_stays_on_o_and_the_scope_takes_shift_o() {
    let mut app = App::new(QuantiseSettings::default(), false);
    let (sender, receiver) = crossbeam_channel::bounded(4);
    let now = Instant::now();
    app.handle_key(
        KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE),
        &sender,
        now,
    )
    .unwrap();
    assert_eq!(app.plots.mode, Mode::Both, "o must not touch the scope");
    assert!(
        matches!(receiver.try_recv(), Ok(crate::engine::Command::LoopPlay(_))),
        "o must start or stop the loop"
    );
    app.handle_key(
        KeyEvent::new(KeyCode::Char('O'), KeyModifiers::SHIFT),
        &sender,
        now,
    )
    .unwrap();
    assert_eq!(app.plots.mode, Mode::Voice);
    assert!(receiver.is_empty(), "Shift+o only changes the display");
    let help = super::board::help_lines(&app).join("\n");
    assert!(help.contains("o: play or stop"), "{help}");
    assert!(
        help.contains("Shift+o: voice+synth / voice / synth scope"),
        "{help}"
    );
    assert!(!help.contains("p: play or stop"), "{help}");
    assert!(crate::run::USAGE.contains("o stops/starts"));
    assert!(crate::run::USAGE.contains("O cycles scope"));
}

#[test]
fn the_trail_stays_beside_the_scope_on_a_narrow_terminal() {
    let app = App::new(QuantiseSettings::default(), false);
    let mut display = Display::new(RATE);
    for index in 0..3000 {
        let semitones = (index as f32).mul_add(4.0 / 2999.0, -2.0);
        display.push(voice(
            (index as u64 + 1) * kazoo_core::voice::VOICE_HOP as u64,
            semitones,
            semitones.round(),
        ));
    }
    display.recentre();
    for width in [80_u16, 100, 118] {
        let buffer = render(&app, &display, width, 40, Instant::now());
        let trail = buffer
            .content
            .chunks(usize::from(width))
            .skip(6)
            .take(8)
            .flat_map(|row| row[..10].iter())
            .any(|cell| {
                cell.fg == super::BRIGHT
                    && cell
                        .symbol()
                        .chars()
                        .any(|ch| ('\u{2801}'..='\u{28ff}').contains(&ch))
            });
        assert!(
            trail,
            "no pitch trail at {width} columns:\n{}",
            text(&buffer)
        );
    }
}

#[test]
fn a_host_that_counts_buffers_only_shows_a_floor() {
    let latency = Latency {
        input: Some(2.7),
        resampler: Some(0.0),
        block: Some(2.7),
        effects: Some(0.0),
        output: Some(2.7),
        device_buffers_only: true,
    };
    assert_eq!(latency.header(), "≥ 8.1 ms · in ≥2.7 / dsp 2.7 / out ≥2.7");
    assert!(
        latency.breakdown().contains("Bluetooth"),
        "{}",
        latency.breakdown()
    );
    let unknown = Latency {
        output: None,
        ..latency
    };
    assert_eq!(unknown.header(), "≥ 5.4 ms · in ≥2.7 / dsp 2.7 / out ?");
    assert_eq!(
        kazoo_core::host::audio::LATENCY_COVERS
            == kazoo_core::host::audio::LatencyCovers::BufferOnly,
        cfg!(any(
            target_os = "macos",
            target_os = "ios",
            target_os = "windows"
        ))
    );
}
