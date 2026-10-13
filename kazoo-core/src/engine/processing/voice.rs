//! Callback-side voice tracking and pitch delivery to synth layers.

use super::{ProcessingIO, ProcessingState};
use crate::analysis::PitchEstimate;
use crate::voice::VoiceFrame;

pub(super) const INITIAL_FRAME: VoiceFrame = VoiceFrame {
    at: 0,
    hz: 0.0,
    volts: 0.0,
    snapped_hz: 0.0,
    snapped_volts: 0.0,
    gate: false,
    onset: false,
    voiced: false,
    loudness: 0.0,
    breath: 0.0,
    confidence: 0.0,
};

#[derive(Clone, Copy)]
pub(super) enum Input {
    Mic(usize),
    Clip(usize),
}

// Select the resident slice here so the tracker and mixer can be borrowed
// independently, without copying audio or moving callback-owned buffers.
pub(super) fn feed_voice(io: &mut ProcessingIO, state: &mut ProcessingState, input: Input) {
    let playback = state.transport.is_playing() && !state.transport.is_recording();
    let block = match input {
        Input::Mic(len) => &state.mic_block[..len],
        Input::Clip(len) => {
            let clips = state.mixer.clip_mix_buffer();
            &clips[..len.min(clips.len())]
        }
    };
    io.judge.push(block);
    while let Some(result) = io.judge_results.pop() {
        state.voice.judge(result);
    }
    let mut last = state.last_frame;
    state.voice.process(block, |frame| last = frame);
    state.last_frame = last;
    if last.voiced {
        for track in state.mixer.tracks_mut() {
            if playback || track.is_armed() {
                for layer in track.layers_mut() {
                    layer.synth_mut().set_pitch(last.hz);
                }
            }
        }
    }
}

pub(super) fn pitch(frame: VoiceFrame) -> PitchEstimate {
    let frequency = frame.voiced.then_some(frame.hz);
    PitchEstimate {
        frequency,
        voiced_probability: frame.confidence,
        midi_note: frequency.and_then(crate::frequency_to_midi_note),
    }
}
