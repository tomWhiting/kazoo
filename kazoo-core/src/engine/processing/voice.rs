//! Callback-side voice tracking and pitch delivery to synth layers.

use super::{ProcessingIO, ProcessingState};
use crate::analysis::PitchEstimate;
use crate::voice::VoiceFrame;

/// No voice yet: unvoiced, silent, zero pitch.
pub(super) const INITIAL_FRAME: VoiceFrame = VoiceFrame {
    at: 0,
    hz: 0.0,
    volts: 0.0,
    snapped_hz: 0.0,
    snapped_volts: 0.0,
    gate: false,
    onset: false,
    voiced: false,
    provisional: false,
    loudness: 0.0,
    breath: 0.0,
    confidence: 0.0,
};

/// Which callback buffer [`feed_voice`] reads, and how many samples of it.
#[derive(Clone, Copy)]
pub(super) enum Input {
    Mic(usize),
    Clip(usize),
}

/// Feed one piece of mic (or, in playback, clip) audio to the judge and the
/// tracker, and hand a voiced frame's pitch to the synths that follow it.
///
/// The slice is selected here so the tracker and mixer can be borrowed
/// independently, without copying audio or moving callback-owned buffers.
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

/// Render `output` (interleaved stereo) in pieces that each end where the
/// voice tracker's next frame falls due, so every synth renders each
/// [`VOICE_HOP`](crate::voice::VOICE_HOP) samples with a fresh pitch,
/// whatever the callback size. Returns the last piece's input level (dB).
pub(super) fn render_hops(
    state: &mut ProcessingState,
    io: &mut ProcessingIO,
    output: &mut [f32],
) -> f32 {
    let mut start = 0;
    loop {
        let frames = state.voice.until_frame();
        if (output.len() - start) / 2 <= frames {
            return super::render_segment(state, io, &mut output[start..]);
        }
        let end = start + frames * 2;
        super::render_segment(state, io, &mut output[start..end]);
        start = end;
    }
}

/// The display's pitch: the tracker's, with its confidence.
pub(super) fn pitch(frame: VoiceFrame) -> PitchEstimate {
    let frequency = frame.voiced.then_some(frame.hz);
    PitchEstimate {
        frequency,
        voiced_probability: frame.confidence,
        midi_note: frequency.and_then(crate::frequency_to_midi_note),
    }
}
