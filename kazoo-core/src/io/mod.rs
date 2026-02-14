//! Audio I/O: mic input, audio output, file read/write.
//!
//! - [`device`] -- audio device enumeration and stream construction via `cpal`.
//! - [`file`] -- WAV reading/writing (`hound`) and multi-format decoding (`symphonia`).
//! - [`recorder`] -- streaming disk recorder for capturing audio to WAV files.

pub mod device;
pub mod file;
pub mod recorder;

// Re-export primary public types for convenient `use kazoo_core::io::*`.
pub use device::{AudioStreams, DeviceInfo, StreamConfig};
pub use device::{build_streams, enumerate_input_devices, enumerate_output_devices};
pub use file::{AudioBuffer, read_audio_file, read_wav, write_wav};
pub use recorder::DiskRecorder;
