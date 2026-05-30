//! kazoo-juno — procedural Juno-inspired chorus polysynth.

mod app;
mod input;
mod ui;

use std::io;
use std::time::{Duration, Instant};

use app::{App, WAVEFORM_BUF_SIZE};
use color_eyre::eyre::WrapErr;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossterm::ExecutableCommand;
use crossterm::event::{
    self, Event, KeyCode, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use kazoo_juno::{JunoSynth, NUM_VOICES, SynthParams, VoiceStatus};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

const TARGET_FPS: u64 = 30;

#[derive(Debug)]
enum AudioCommand {
    NoteOn { note: u8, velocity: f32 },
    NoteOff { note: u8 },
    UpdateParams(SynthParams),
    AllNotesOff,
}

struct DisplaySnapshot {
    voice_status: [VoiceStatus; NUM_VOICES],
    waveform: [f32; WAVEFORM_BUF_SIZE],
}

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;

    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| color_eyre::eyre::eyre!("no audio output device found"))?;
    let supported_config = device.default_output_config()?;
    let sample_rate = supported_config.sample_rate() as f32;
    let channels = supported_config.channels() as usize;

    let (cmd_tx, cmd_rx) = crossbeam_channel::bounded::<AudioCommand>(256);
    let (display_tx, display_rx) = crossbeam_channel::bounded::<DisplaySnapshot>(2);

    let stream = build_audio_stream(
        &device,
        &supported_config.into(),
        sample_rate,
        channels,
        cmd_rx,
        display_tx,
    )?;
    stream.play().wrap_err("failed to start audio stream")?;

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    stdout.execute(EnterAlternateScreen)?;
    let _ = stdout.execute(PushKeyboardEnhancementFlags(
        KeyboardEnhancementFlags::REPORT_EVENT_TYPES,
    ));

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let mut app = App::new(sample_rate as u32);
    let frame_duration = Duration::from_millis(1000 / TARGET_FPS);

    loop {
        let frame_start = Instant::now();

        while let Ok(snapshot) = display_rx.try_recv() {
            app.voice_status = snapshot.voice_status;
            app.waveform_buf = snapshot.waveform;
        }

        terminal.draw(|frame| ui::draw(frame, &app))?;

        let timeout = frame_duration.saturating_sub(frame_start.elapsed());
        if event::poll(timeout)? {
            process_event(&mut app, &cmd_tx)?;
            while event::poll(Duration::ZERO)? {
                process_event(&mut app, &cmd_tx)?;
            }
        }

        if app.should_quit {
            break;
        }
    }

    let _ = cmd_tx.send(AudioCommand::AllNotesOff);
    drop(stream);
    let _ = io::stdout().execute(crossterm::event::PopKeyboardEnhancementFlags);
    disable_raw_mode()?;
    io::stdout().execute(LeaveAlternateScreen)?;

    Ok(())
}

fn process_event(
    app: &mut App,
    cmd_tx: &crossbeam_channel::Sender<AudioCommand>,
) -> color_eyre::Result<()> {
    if let Event::Key(key) = event::read()? {
        match key.kind {
            KeyEventKind::Press => {
                handle_key_press(app, key.code, key.modifiers, cmd_tx);
            }
            KeyEventKind::Release => handle_key_release(app, key.code, cmd_tx),
            KeyEventKind::Repeat => {}
        }
    }
    Ok(())
}

fn handle_key_press(
    app: &mut App,
    code: KeyCode,
    modifiers: KeyModifiers,
    cmd_tx: &crossbeam_channel::Sender<AudioCommand>,
) {
    if is_quit_key(code, modifiers) {
        app.should_quit = true;
        return;
    }

    if let Some(note) = input::key_to_note(code) {
        if let Some(key_index) = key_index(code) {
            if app.key_note_map[key_index].is_some() {
                return;
            }
            app.key_note_map[key_index] = Some(note);
        }
        app.add_held_note(note);
        let _ = cmd_tx.send(AudioCommand::NoteOn {
            note,
            velocity: if modifiers.contains(KeyModifiers::SHIFT) {
                1.0
            } else {
                0.82
            },
        });
    } else {
        match code {
            KeyCode::Tab => app.next_section(),
            KeyCode::BackTab => app.prev_section(),
            KeyCode::Down | KeyCode::Char('j') => app.next_param(),
            KeyCode::Up | KeyCode::Char('k') => app.prev_param(),
            KeyCode::Right | KeyCode::Char('l') => update_param(app, 1.0, cmd_tx),
            KeyCode::Left | KeyCode::Char('h') => update_param(app, -1.0, cmd_tx),
            KeyCode::Char(' ') => {
                app.key_note_map.fill(None);
                app.held_notes.fill(None);
                let _ = cmd_tx.send(AudioCommand::AllNotesOff);
            }
            _ => {}
        }
    }
}

fn handle_key_release(
    app: &mut App,
    code: KeyCode,
    cmd_tx: &crossbeam_channel::Sender<AudioCommand>,
) {
    if let Some(note) = key_index(code).and_then(|idx| app.key_note_map[idx].take()) {
        app.remove_held_note(note);
        let _ = cmd_tx.send(AudioCommand::NoteOff { note });
    }
}

const fn key_index(code: KeyCode) -> Option<usize> {
    if let KeyCode::Char(ch) = code {
        let ascii = ch as u32;
        if ascii < 128 {
            return Some(ascii as usize);
        }
    }
    None
}

const fn is_quit_key(code: KeyCode, modifiers: KeyModifiers) -> bool {
    if matches!(code, KeyCode::Esc) {
        return true;
    }
    modifiers.contains(KeyModifiers::CONTROL)
        && matches!(code, KeyCode::Char('q' | 'Q' | 'c' | 'C' | 'd' | 'D'))
}

fn update_param(app: &mut App, delta: f32, cmd_tx: &crossbeam_channel::Sender<AudioCommand>) {
    app.adjust_param(delta);
    let _ = cmd_tx.send(AudioCommand::UpdateParams(app.params.clone()));
}

fn build_audio_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    sample_rate: f32,
    channels: usize,
    cmd_rx: crossbeam_channel::Receiver<AudioCommand>,
    display_tx: crossbeam_channel::Sender<DisplaySnapshot>,
) -> color_eyre::Result<cpal::Stream> {
    let mut synth = JunoSynth::new(sample_rate);
    let mut waveform = [0.0_f32; WAVEFORM_BUF_SIZE];
    let mut waveform_pos = 0;
    let mut display_counter = 0_u32;

    let stream = device.build_output_stream(
        config,
        move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
            while let Ok(cmd) = cmd_rx.try_recv() {
                match cmd {
                    AudioCommand::NoteOn { note, velocity } => synth.note_on(note, velocity),
                    AudioCommand::NoteOff { note } => synth.note_off(note),
                    AudioCommand::UpdateParams(params) => {
                        synth.params = params;
                        synth.apply_params();
                    }
                    AudioCommand::AllNotesOff => synth.all_notes_off(),
                }
            }

            for frame in data.chunks_mut(channels) {
                let sample = synth.process_sample();
                waveform[waveform_pos] = sample;
                waveform_pos = (waveform_pos + 1) % WAVEFORM_BUF_SIZE;
                for output in frame {
                    *output = sample;
                }
            }

            display_counter = display_counter.wrapping_add(1);
            if display_counter % 3 == 0 {
                let _ = display_tx.try_send(DisplaySnapshot {
                    voice_status: synth.voice_status(),
                    waveform,
                });
            }
        },
        move |err| eprintln!("audio stream error: {err}"),
        None,
    )?;

    Ok(stream)
}
