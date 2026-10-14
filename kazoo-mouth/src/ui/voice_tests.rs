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
use crate::engine::capture::SAMPLES;

const RATE: u32 = 48_000;

fn sine(hz: f32, offset: usize) -> [f32; SAMPLES] {
    std::array::from_fn(|index| (TAU * hz * (index + offset) as f32 / RATE as f32).sin() * 0.7)
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
    app.scope_voice = sine(220.0, 0);
    app.scope_synth = sine(440.0, 75);
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
            KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE),
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
    let samples =
        std::array::from_fn(|index| [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.5][index % 4]);
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
