//! Kazoo's speech: a studio vocoder, a text-to-speech renderer and a player
//! for the phrases it renders, so the wall can sing words.
//!
//! - [`vocoder`]: a real-time channel vocoder, 8 to 40 Bark-spaced bands,
//!   with formant shift, consonant detection and hold.
//! - [`tts`]: renders text to mono `f32` at any engine rate through macOS
//!   `say`, off the audio thread, with a render cache on disk.
//! - [`player`]: plays a rendered phrase in real time on a gate, once,
//!   looped or held, with varispeed or pitch-preserving stretch.
//!
//! Nothing here depends on the wall; the wall adapts these into modules and
//! a `speak` operation.

pub mod bank;
pub mod player;
pub mod tts;
pub mod vocoder;

pub use player::{Phrase, PhraseFeed, SpeechPlayer};
pub use tts::{RenderRequest, SpeechError, Tts, TtsConfig, TtsWorker};
pub use vocoder::Vocoder;

#[cfg(test)]
mod testing;
