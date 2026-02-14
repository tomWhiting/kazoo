//! Engine command types sent from the UI thread to the processing thread.

use std::path::PathBuf;

use crate::mixer::TrackId;
use crate::synthesis::SynthesisMode;
use crate::transport::TransportCommand;
use crate::{Db, Pan, Processor};

/// Commands that can be sent to the engine's processing thread.
///
/// All variants are designed to be constructed on the UI thread and sent via
/// a `crossbeam_channel::Sender<EngineCommand>`. The processing thread drains
/// the receiver each audio block and applies commands atomically.
pub enum EngineCommand {
    /// Forward a transport control command (play, stop, pause, record, seek, etc.).
    Transport(TransportCommand),

    /// Create a new mixer track with the given name and synthesis mode.
    AddTrack {
        name: String,
        synthesis_mode: SynthesisMode,
    },

    /// Remove a mixer track by its identifier.
    RemoveTrack(TrackId),

    /// Set the volume of a specific track.
    SetTrackVolume(TrackId, Db),

    /// Set the stereo pan position of a specific track.
    SetTrackPan(TrackId, Pan),

    /// Mute or unmute a specific track.
    SetTrackMute(TrackId, bool),

    /// Solo or unsolo a specific track.
    SetTrackSolo(TrackId, bool),

    /// Arm or disarm a specific track for recording.
    SetTrackArm(TrackId, bool),

    /// Change the synthesis mode of a specific track.
    ///
    /// This replaces the track's synth processor with a new instance of the
    /// requested mode, initialised at the current sample rate.
    SetTrackSynthesisMode(TrackId, SynthesisMode),

    /// Append an effect processor to a track's effect chain.
    AddEffect {
        track_id: TrackId,
        effect: Box<dyn Processor>,
    },

    /// Remove an effect from a track's chain by index.
    RemoveEffect {
        track_id: TrackId,
        effect_index: usize,
    },

    /// Set the bypass state of an effect in a track's chain.
    SetEffectBypass {
        track_id: TrackId,
        effect_index: usize,
        bypassed: bool,
    },

    /// Set a parameter value on an effect in a track's chain.
    SetEffectParameter {
        track_id: TrackId,
        effect_index: usize,
        param_index: usize,
        value: f32,
    },

    /// Set a parameter value on a track's synth processor.
    SetSynthParameter {
        track_id: TrackId,
        param_index: usize,
        value: f32,
    },

    /// Set the master bus volume.
    SetMasterVolume(Db),

    /// Begin recording the master output to a WAV file at the given path.
    StartRecording { path: PathBuf },

    /// Stop an active recording session and finalize the WAV file.
    StopRecording,

    /// Shut down the engine gracefully. All threads should terminate.
    Shutdown,
}

// `EngineCommand` cannot derive `Debug` because `Box<dyn Processor>` is not
// Debug-compatible in every variant. Provide a manual implementation.
impl std::fmt::Debug for EngineCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(cmd) => f.debug_tuple("Transport").field(cmd).finish(),
            Self::AddTrack {
                name,
                synthesis_mode,
            } => f
                .debug_struct("AddTrack")
                .field("name", name)
                .field("synthesis_mode", synthesis_mode)
                .finish(),
            Self::RemoveTrack(id) => f.debug_tuple("RemoveTrack").field(id).finish(),
            Self::SetTrackVolume(id, db) => {
                f.debug_tuple("SetTrackVolume").field(id).field(db).finish()
            }
            Self::SetTrackPan(id, pan) => {
                f.debug_tuple("SetTrackPan").field(id).field(pan).finish()
            }
            Self::SetTrackMute(id, m) => f.debug_tuple("SetTrackMute").field(id).field(m).finish(),
            Self::SetTrackSolo(id, s) => f.debug_tuple("SetTrackSolo").field(id).field(s).finish(),
            Self::SetTrackArm(id, a) => f.debug_tuple("SetTrackArm").field(id).field(a).finish(),
            Self::SetTrackSynthesisMode(id, mode) => f
                .debug_tuple("SetTrackSynthesisMode")
                .field(id)
                .field(mode)
                .finish(),
            Self::AddEffect { track_id, effect } => f
                .debug_struct("AddEffect")
                .field("track_id", track_id)
                .field("effect", &effect.name())
                .finish(),
            Self::RemoveEffect {
                track_id,
                effect_index,
            } => f
                .debug_struct("RemoveEffect")
                .field("track_id", track_id)
                .field("effect_index", effect_index)
                .finish(),
            Self::SetEffectBypass {
                track_id,
                effect_index,
                bypassed,
            } => f
                .debug_struct("SetEffectBypass")
                .field("track_id", track_id)
                .field("effect_index", effect_index)
                .field("bypassed", bypassed)
                .finish(),
            Self::SetEffectParameter {
                track_id,
                effect_index,
                param_index,
                value,
            } => f
                .debug_struct("SetEffectParameter")
                .field("track_id", track_id)
                .field("effect_index", effect_index)
                .field("param_index", param_index)
                .field("value", value)
                .finish(),
            Self::SetSynthParameter {
                track_id,
                param_index,
                value,
            } => f
                .debug_struct("SetSynthParameter")
                .field("track_id", track_id)
                .field("param_index", param_index)
                .field("value", value)
                .finish(),
            Self::SetMasterVolume(db) => f.debug_tuple("SetMasterVolume").field(db).finish(),
            Self::StartRecording { path } => f
                .debug_struct("StartRecording")
                .field("path", path)
                .finish(),
            Self::StopRecording => write!(f, "StopRecording"),
            Self::Shutdown => write!(f, "Shutdown"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::TransportCommand;

    #[test]
    fn transport_command_debug() {
        let cmd = EngineCommand::Transport(TransportCommand::Play);
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("Transport"));
    }

    #[test]
    fn add_track_command_debug() {
        let cmd = EngineCommand::AddTrack {
            name: "Lead".into(),
            synthesis_mode: SynthesisMode::PitchTracked,
        };
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("Lead"));
        assert!(dbg.contains("PitchTracked"));
    }

    #[test]
    fn set_master_volume_command_debug() {
        let cmd = EngineCommand::SetMasterVolume(Db::new(-6.0));
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("SetMasterVolume"));
    }

    #[test]
    fn start_recording_command_debug() {
        let cmd = EngineCommand::StartRecording {
            path: PathBuf::from("/tmp/test.wav"),
        };
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("StartRecording"));
        assert!(dbg.contains("test.wav"));
    }

    #[test]
    fn shutdown_command_debug() {
        let cmd = EngineCommand::Shutdown;
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("Shutdown"));
    }

    #[test]
    fn remove_track_debug() {
        let cmd = EngineCommand::RemoveTrack(TrackId(3));
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("RemoveTrack"));
    }

    #[test]
    fn set_track_volume_debug() {
        let cmd = EngineCommand::SetTrackVolume(TrackId(1), Db::new(-12.0));
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("SetTrackVolume"));
    }

    #[test]
    fn set_track_pan_debug() {
        let cmd = EngineCommand::SetTrackPan(TrackId(0), Pan::CENTER);
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("SetTrackPan"));
    }

    #[test]
    fn set_track_mute_debug() {
        let cmd = EngineCommand::SetTrackMute(TrackId(2), true);
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("SetTrackMute"));
    }

    #[test]
    fn set_track_solo_debug() {
        let cmd = EngineCommand::SetTrackSolo(TrackId(0), false);
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("SetTrackSolo"));
    }

    #[test]
    fn set_track_arm_debug() {
        let cmd = EngineCommand::SetTrackArm(TrackId(1), true);
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("SetTrackArm"));
    }

    #[test]
    fn set_track_synthesis_mode_debug() {
        let cmd = EngineCommand::SetTrackSynthesisMode(TrackId(0), SynthesisMode::Granular);
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("SetTrackSynthesisMode"));
        assert!(dbg.contains("Granular"));
    }

    #[test]
    fn remove_effect_debug() {
        let cmd = EngineCommand::RemoveEffect {
            track_id: TrackId(0),
            effect_index: 2,
        };
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("RemoveEffect"));
    }

    #[test]
    fn set_effect_bypass_debug() {
        let cmd = EngineCommand::SetEffectBypass {
            track_id: TrackId(1),
            effect_index: 0,
            bypassed: true,
        };
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("SetEffectBypass"));
    }

    #[test]
    fn set_effect_parameter_debug() {
        let cmd = EngineCommand::SetEffectParameter {
            track_id: TrackId(0),
            effect_index: 1,
            param_index: 0,
            value: 0.75,
        };
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("SetEffectParameter"));
    }

    #[test]
    fn set_synth_parameter_debug() {
        let cmd = EngineCommand::SetSynthParameter {
            track_id: TrackId(0),
            param_index: 0,
            value: 440.0,
        };
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("SetSynthParameter"));
    }

    #[test]
    fn stop_recording_debug() {
        let cmd = EngineCommand::StopRecording;
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("StopRecording"));
    }
}
