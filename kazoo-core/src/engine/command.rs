//! Engine command types sent from the UI thread to the output callback.

use std::path::PathBuf;

use crate::mixer::TrackId;
use crate::mixer::clip::{ClipData, ClipId};
use crate::synthesis::SynthesisMode;
use crate::transport::TransportCommand;
use crate::{Db, Pan, Processor};

/// Commands that can be sent to the engine's output callback.
///
/// All variants are designed to be constructed on the UI thread and sent via
/// a `crossbeam_channel::Sender<EngineCommand>`. The output callback drains
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

    /// Set a parameter value on a track's primary synth processor (layer 0).
    SetSynthParameter {
        track_id: TrackId,
        param_index: usize,
        value: f32,
    },

    /// Add a new synth layer to a track.
    AddSynthLayer {
        track_id: TrackId,
        synthesis_mode: SynthesisMode,
        label: String,
    },

    /// Remove a synth layer from a track by index (layer 0 cannot be removed).
    RemoveSynthLayer {
        track_id: TrackId,
        layer_index: usize,
    },

    /// Set the gain of a synth layer.
    SetSynthLayerGain {
        track_id: TrackId,
        layer_index: usize,
        gain: Db,
    },

    /// Enable or disable a synth layer.
    SetSynthLayerEnabled {
        track_id: TrackId,
        layer_index: usize,
        enabled: bool,
    },

    /// Set a parameter value on a specific synth layer.
    SetSynthLayerParameter {
        track_id: TrackId,
        layer_index: usize,
        param_index: usize,
        value: f32,
    },

    /// Set the master bus volume.
    SetMasterVolume(Db),

    /// Begin recording the master output to a WAV file at the given path.
    StartRecording { path: PathBuf },

    /// Stop an active recording session and finalize the WAV file.
    StopRecording,

    /// Add a new audio clip to a track at the specified timeline position.
    AddClip {
        track_id: TrackId,
        clip_data: ClipData,
        position: u64,
    },

    /// Remove a clip from a track by clip ID.
    RemoveClip { track_id: TrackId, clip_id: ClipId },

    /// Move a clip to a new timeline position.
    MoveClip {
        track_id: TrackId,
        clip_id: ClipId,
        new_position: u64,
    },

    /// Trim samples from the start of a clip (non-destructive).
    TrimClipStart {
        track_id: TrackId,
        clip_id: ClipId,
        samples: usize,
    },

    /// Trim samples from the end of a clip (non-destructive).
    TrimClipEnd {
        track_id: TrackId,
        clip_id: ClipId,
        samples: usize,
    },

    /// Split a clip at the given timeline position into two clips.
    SplitClip {
        track_id: TrackId,
        clip_id: ClipId,
        split_position: u64,
    },

    /// Set the gain of a clip.
    SetClipGain {
        track_id: TrackId,
        clip_id: ClipId,
        gain: Db,
    },

    /// Mute or unmute a clip.
    SetClipMute {
        track_id: TrackId,
        clip_id: ClipId,
        muted: bool,
    },

    /// Duplicate a clip to a new timeline position.
    DuplicateClip {
        track_id: TrackId,
        clip_id: ClipId,
        new_position: u64,
    },

    /// Shut down the engine gracefully. All threads should terminate.
    Shutdown,
}

// `EngineCommand` cannot derive `Debug` because `Box<dyn Processor>` is not
// Debug-compatible in every variant. Provide a manual implementation.
impl std::fmt::Debug for EngineCommand {
    #[allow(clippy::too_many_lines)]
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
            Self::AddSynthLayer {
                track_id,
                synthesis_mode,
                label,
            } => f
                .debug_struct("AddSynthLayer")
                .field("track_id", track_id)
                .field("synthesis_mode", synthesis_mode)
                .field("label", label)
                .finish(),
            Self::RemoveSynthLayer {
                track_id,
                layer_index,
            } => f
                .debug_struct("RemoveSynthLayer")
                .field("track_id", track_id)
                .field("layer_index", layer_index)
                .finish(),
            Self::SetSynthLayerGain {
                track_id,
                layer_index,
                gain,
            } => f
                .debug_struct("SetSynthLayerGain")
                .field("track_id", track_id)
                .field("layer_index", layer_index)
                .field("gain", gain)
                .finish(),
            Self::SetSynthLayerEnabled {
                track_id,
                layer_index,
                enabled,
            } => f
                .debug_struct("SetSynthLayerEnabled")
                .field("track_id", track_id)
                .field("layer_index", layer_index)
                .field("enabled", enabled)
                .finish(),
            Self::SetSynthLayerParameter {
                track_id,
                layer_index,
                param_index,
                value,
            } => f
                .debug_struct("SetSynthLayerParameter")
                .field("track_id", track_id)
                .field("layer_index", layer_index)
                .field("param_index", param_index)
                .field("value", value)
                .finish(),
            Self::SetMasterVolume(db) => f.debug_tuple("SetMasterVolume").field(db).finish(),
            Self::StartRecording { path } => f
                .debug_struct("StartRecording")
                .field("path", path)
                .finish(),
            Self::StopRecording => write!(f, "StopRecording"),
            Self::AddClip {
                track_id,
                clip_data,
                position,
            } => f
                .debug_struct("AddClip")
                .field("track_id", track_id)
                .field("clip_data", &clip_data.name())
                .field("position", position)
                .finish(),
            Self::RemoveClip { track_id, clip_id } => f
                .debug_struct("RemoveClip")
                .field("track_id", track_id)
                .field("clip_id", clip_id)
                .finish(),
            Self::MoveClip {
                track_id,
                clip_id,
                new_position,
            } => f
                .debug_struct("MoveClip")
                .field("track_id", track_id)
                .field("clip_id", clip_id)
                .field("new_position", new_position)
                .finish(),
            Self::TrimClipStart {
                track_id,
                clip_id,
                samples,
            } => f
                .debug_struct("TrimClipStart")
                .field("track_id", track_id)
                .field("clip_id", clip_id)
                .field("samples", samples)
                .finish(),
            Self::TrimClipEnd {
                track_id,
                clip_id,
                samples,
            } => f
                .debug_struct("TrimClipEnd")
                .field("track_id", track_id)
                .field("clip_id", clip_id)
                .field("samples", samples)
                .finish(),
            Self::SplitClip {
                track_id,
                clip_id,
                split_position,
            } => f
                .debug_struct("SplitClip")
                .field("track_id", track_id)
                .field("clip_id", clip_id)
                .field("split_position", split_position)
                .finish(),
            Self::SetClipGain {
                track_id,
                clip_id,
                gain,
            } => f
                .debug_struct("SetClipGain")
                .field("track_id", track_id)
                .field("clip_id", clip_id)
                .field("gain", gain)
                .finish(),
            Self::SetClipMute {
                track_id,
                clip_id,
                muted,
            } => f
                .debug_struct("SetClipMute")
                .field("track_id", track_id)
                .field("clip_id", clip_id)
                .field("muted", muted)
                .finish(),
            Self::DuplicateClip {
                track_id,
                clip_id,
                new_position,
            } => f
                .debug_struct("DuplicateClip")
                .field("track_id", track_id)
                .field("clip_id", clip_id)
                .field("new_position", new_position)
                .finish(),
            Self::Shutdown => write!(f, "Shutdown"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mixer::clip::{ClipData, ClipId};
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

    /// Helper to create test clip data for command tests.
    fn test_clip_data() -> ClipData {
        ClipData::new(vec![0.0; 100], "TestClip".into(), None, 44_100)
    }

    #[test]
    fn add_clip_debug() {
        let cmd = EngineCommand::AddClip {
            track_id: TrackId(0),
            clip_data: test_clip_data(),
            position: 1000,
        };
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("AddClip"));
        assert!(dbg.contains("TestClip"));
        assert!(dbg.contains("1000"));
    }

    #[test]
    fn remove_clip_debug() {
        let cmd = EngineCommand::RemoveClip {
            track_id: TrackId(1),
            clip_id: ClipId(5),
        };
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("RemoveClip"));
        assert!(dbg.contains("5"));
    }

    #[test]
    fn move_clip_debug() {
        let cmd = EngineCommand::MoveClip {
            track_id: TrackId(0),
            clip_id: ClipId(3),
            new_position: 2000,
        };
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("MoveClip"));
        assert!(dbg.contains("3"));
        assert!(dbg.contains("2000"));
    }

    #[test]
    fn trim_clip_start_debug() {
        let cmd = EngineCommand::TrimClipStart {
            track_id: TrackId(0),
            clip_id: ClipId(1),
            samples: 500,
        };
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("TrimClipStart"));
        assert!(dbg.contains("500"));
    }

    #[test]
    fn trim_clip_end_debug() {
        let cmd = EngineCommand::TrimClipEnd {
            track_id: TrackId(2),
            clip_id: ClipId(7),
            samples: 300,
        };
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("TrimClipEnd"));
        assert!(dbg.contains("300"));
    }

    #[test]
    fn split_clip_debug() {
        let cmd = EngineCommand::SplitClip {
            track_id: TrackId(0),
            clip_id: ClipId(1),
            split_position: 5000,
        };
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("SplitClip"));
        assert!(dbg.contains("5000"));
    }

    #[test]
    fn set_clip_gain_debug() {
        let cmd = EngineCommand::SetClipGain {
            track_id: TrackId(0),
            clip_id: ClipId(2),
            gain: Db::new(-6.0),
        };
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("SetClipGain"));
    }

    #[test]
    fn set_clip_mute_debug() {
        let cmd = EngineCommand::SetClipMute {
            track_id: TrackId(1),
            clip_id: ClipId(4),
            muted: true,
        };
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("SetClipMute"));
        assert!(dbg.contains("true"));
    }

    #[test]
    fn duplicate_clip_debug() {
        let cmd = EngineCommand::DuplicateClip {
            track_id: TrackId(0),
            clip_id: ClipId(1),
            new_position: 8000,
        };
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("DuplicateClip"));
        assert!(dbg.contains("8000"));
    }

    #[test]
    fn add_synth_layer_debug() {
        let cmd = EngineCommand::AddSynthLayer {
            track_id: TrackId(0),
            synthesis_mode: SynthesisMode::Wavetable,
            label: "Pad".into(),
        };
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("AddSynthLayer"));
        assert!(dbg.contains("Wavetable"));
        assert!(dbg.contains("Pad"));
    }

    #[test]
    fn remove_synth_layer_debug() {
        let cmd = EngineCommand::RemoveSynthLayer {
            track_id: TrackId(1),
            layer_index: 2,
        };
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("RemoveSynthLayer"));
        assert!(dbg.contains("2"));
    }

    #[test]
    fn set_synth_layer_gain_debug() {
        let cmd = EngineCommand::SetSynthLayerGain {
            track_id: TrackId(0),
            layer_index: 1,
            gain: Db::new(-6.0),
        };
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("SetSynthLayerGain"));
    }

    #[test]
    fn set_synth_layer_enabled_debug() {
        let cmd = EngineCommand::SetSynthLayerEnabled {
            track_id: TrackId(0),
            layer_index: 1,
            enabled: false,
        };
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("SetSynthLayerEnabled"));
        assert!(dbg.contains("false"));
    }

    #[test]
    fn set_synth_layer_parameter_debug() {
        let cmd = EngineCommand::SetSynthLayerParameter {
            track_id: TrackId(0),
            layer_index: 0,
            param_index: 2,
            value: 0.75,
        };
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("SetSynthLayerParameter"));
        assert!(dbg.contains("0.75"));
    }
}
