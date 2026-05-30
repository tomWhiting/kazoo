//! kazoo-303 — TB-303 inspired acid bassline synthesizer.
//!
//! This instrument is fully procedural: no samples, wavetables, or recordings are
//! played back. The sound is generated from oscillator math, envelopes, glide,
//! accent dynamics, and a resonant low-pass filter.

mod sequencer;
mod synth;
mod ui;

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use sequencer::Sequencer;
use synth::{AcidSynth, AcidSynthParam, Waveform};

#[derive(Debug)]
enum AudioCommand {
    Play,
    Stop,
    SetBpm(f64),
    ToggleStep(usize),
    ToggleAccent(usize),
    ToggleSlide(usize),
    TransposeStep { step: usize, semitones: i8 },
    SetParam { param: AcidSynthParam, value: f32 },
    SetWaveform(Waveform),
    RandomizePattern,
}

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;

    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| color_eyre::eyre::eyre!("no audio output device found"))?;
    let config = device.default_output_config()?;
    let sample_rate = config.sample_rate() as f32;
    let channels = config.channels() as usize;

    let playback_step = Arc::new(AtomicUsize::new(0));
    let playing = Arc::new(AtomicBool::new(false));
    let (cmd_tx, cmd_rx) = crossbeam_channel::bounded::<AudioCommand>(256);

    let stream = build_audio_stream(
        &device,
        &config.into(),
        sample_rate,
        channels,
        cmd_rx,
        Arc::clone(&playback_step),
        Arc::clone(&playing),
    )?;
    stream.play()?;

    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new();
    let result = run_event_loop(&mut terminal, &mut app, &cmd_tx, &playback_step, &playing);

    terminal::disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    drop(stream);

    result
}

fn build_audio_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    sample_rate: f32,
    channels: usize,
    cmd_rx: crossbeam_channel::Receiver<AudioCommand>,
    playback_step: Arc<AtomicUsize>,
    playing_flag: Arc<AtomicBool>,
) -> color_eyre::Result<cpal::Stream> {
    let mut synth = AcidSynth::new(sample_rate);
    let mut sequencer = Sequencer::new(sample_rate);

    let stream = device.build_output_stream(
        config,
        move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
            while let Ok(cmd) = cmd_rx.try_recv() {
                match cmd {
                    AudioCommand::Play => {
                        sequencer.play();
                        playing_flag.store(true, Ordering::Release);
                    }
                    AudioCommand::Stop => {
                        sequencer.stop();
                        synth.release();
                        playing_flag.store(false, Ordering::Release);
                    }
                    AudioCommand::SetBpm(bpm) => sequencer.clock.set_bpm(bpm),
                    AudioCommand::ToggleStep(step) => sequencer.toggle_step(step),
                    AudioCommand::ToggleAccent(step) => sequencer.toggle_accent(step),
                    AudioCommand::ToggleSlide(step) => sequencer.toggle_slide(step),
                    AudioCommand::TransposeStep { step, semitones } => {
                        sequencer.transpose_step(step, semitones);
                    }
                    AudioCommand::SetParam { param, value } => synth.set_param(param, value),
                    AudioCommand::SetWaveform(waveform) => synth.set_waveform(waveform),
                    AudioCommand::RandomizePattern => sequencer.randomize_acid(),
                }
            }

            let mut i = 0;
            while i < data.len() {
                if let Some(event) = sequencer.tick() {
                    playback_step.store(event.step_index, Ordering::Release);
                    synth.note_on(event.note, event.accent, event.slide);
                }

                let sample = kazoo_core::soft_limit(synth.process() * 0.8);
                for ch in 0..channels {
                    if i + ch < data.len() {
                        data[i + ch] = sample;
                    }
                }
                i += channels;
            }
        },
        |err| eprintln!("audio stream error: {err}"),
        None,
    )?;

    Ok(stream)
}

#[derive(Debug)]
struct App {
    sequencer: Sequencer,
    synth: AcidSynth,
    cursor_step: usize,
    selected_param: usize,
    playback_step: usize,
    playing: bool,
    show_help: bool,
    should_quit: bool,
}

impl App {
    fn new() -> Self {
        Self {
            sequencer: Sequencer::new(44_100.0),
            synth: AcidSynth::new(44_100.0),
            cursor_step: 0,
            selected_param: 0,
            playback_step: 0,
            playing: false,
            show_help: false,
            should_quit: false,
        }
    }

    const fn selected_param(&self) -> AcidSynthParam {
        AcidSynthParam::ALL[self.selected_param]
    }

    fn adjust_selected_param(&mut self, delta: f32) -> (AcidSynthParam, f32) {
        let param = self.selected_param();
        let value = self.synth.adjust_param(param, delta);
        (param, value)
    }
}

fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    cmd_tx: &crossbeam_channel::Sender<AudioCommand>,
    playback_step: &Arc<AtomicUsize>,
    playing: &Arc<AtomicBool>,
) -> color_eyre::Result<()> {
    loop {
        app.playback_step = playback_step.load(Ordering::Acquire);
        app.playing = playing.load(Ordering::Acquire);
        terminal.draw(|frame| ui::draw(frame, app))?;

        if event::poll(Duration::from_millis(16))? {
            if let Event::Key(key) = event::read()? {
                if handle_key_event(app, key, cmd_tx) {
                    break;
                }
            }
        }

        if app.should_quit {
            break;
        }
    }
    Ok(())
}

fn handle_key_event(
    app: &mut App,
    key: KeyEvent,
    cmd_tx: &crossbeam_channel::Sender<AudioCommand>,
) -> bool {
    if is_quit_key(key.code, key.modifiers) {
        return true;
    }

    if app.show_help {
        app.show_help = false;
        return false;
    }

    match key.code {
        KeyCode::Char('q') => return true,
        KeyCode::Char('?') => app.show_help = true,
        KeyCode::Enter => {
            app.playing = !app.playing;
            let _ = cmd_tx.try_send(if app.playing {
                AudioCommand::Play
            } else {
                AudioCommand::Stop
            });
        }
        KeyCode::Left => app.cursor_step = app.cursor_step.saturating_sub(1),
        KeyCode::Right => app.cursor_step = (app.cursor_step + 1).min(sequencer::STEPS_PER_PATTERN - 1),
        KeyCode::Up => app.selected_param = app.selected_param.saturating_sub(1),
        KeyCode::Down => {
            app.selected_param = (app.selected_param + 1).min(AcidSynthParam::ALL.len() - 1);
        }
        KeyCode::Char(' ') => {
            app.sequencer.toggle_step(app.cursor_step);
            let _ = cmd_tx.try_send(AudioCommand::ToggleStep(app.cursor_step));
        }
        KeyCode::Char('a') => {
            app.sequencer.toggle_accent(app.cursor_step);
            let _ = cmd_tx.try_send(AudioCommand::ToggleAccent(app.cursor_step));
        }
        KeyCode::Char('s') => {
            app.sequencer.toggle_slide(app.cursor_step);
            let _ = cmd_tx.try_send(AudioCommand::ToggleSlide(app.cursor_step));
        }
        KeyCode::Char('z') => {
            app.sequencer.transpose_step(app.cursor_step, -1);
            let _ = cmd_tx.try_send(AudioCommand::TransposeStep {
                step: app.cursor_step,
                semitones: -1,
            });
        }
        KeyCode::Char('x') => {
            app.sequencer.transpose_step(app.cursor_step, 1);
            let _ = cmd_tx.try_send(AudioCommand::TransposeStep {
                step: app.cursor_step,
                semitones: 1,
            });
        }
        KeyCode::Char('+' | '=') => {
            let bpm = app.sequencer.clock.bpm() + 1.0;
            app.sequencer.clock.set_bpm(bpm);
            let _ = cmd_tx.try_send(AudioCommand::SetBpm(app.sequencer.clock.bpm()));
        }
        KeyCode::Char('-') => {
            let bpm = app.sequencer.clock.bpm() - 1.0;
            app.sequencer.clock.set_bpm(bpm);
            let _ = cmd_tx.try_send(AudioCommand::SetBpm(app.sequencer.clock.bpm()));
        }
        KeyCode::Char(',') => {
            let (param, value) = app.adjust_selected_param(-0.03);
            let _ = cmd_tx.try_send(AudioCommand::SetParam { param, value });
        }
        KeyCode::Char('.') => {
            let (param, value) = app.adjust_selected_param(0.03);
            let _ = cmd_tx.try_send(AudioCommand::SetParam { param, value });
        }
        KeyCode::Char('w') => {
            let waveform = app.synth.toggle_waveform();
            let _ = cmd_tx.try_send(AudioCommand::SetWaveform(waveform));
        }
        KeyCode::Char('r') => {
            app.sequencer.randomize_acid();
            let _ = cmd_tx.try_send(AudioCommand::RandomizePattern);
        }
        _ => {}
    }

    false
}

const fn is_quit_key(code: KeyCode, modifiers: KeyModifiers) -> bool {
    if matches!(code, KeyCode::Esc) {
        return true;
    }
    if matches!(code, KeyCode::Char('q' | 'Q')) {
        return true;
    }
    modifiers.contains(KeyModifiers::CONTROL)
        && matches!(code, KeyCode::Char('q' | 'Q' | 'c' | 'C' | 'd' | 'D'))
}
