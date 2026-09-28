//! Engine command types sent from the UI thread to the output callback.

use std::path::PathBuf;

use super::reclaim::Parcel;
use crate::mixer::clip::{ClipData, ClipId};
use crate::mixer::{Prepared, SynthLayer, Track, TrackId};
use crate::synthesis::SynthesisMode;
use crate::transport::TransportCommand;
use crate::{Db, Pan};

/// Commands that can be sent to the engine's output callback.
///
/// All variants are designed to be constructed on the UI thread and sent via
/// a `crossbeam_channel::Sender<EngineCommand>`. The output callback drains
/// the receiver each audio block and applies commands atomically.
///
/// The output callback never allocates, so everything a command adds —
/// tracks, synths, layers, effects, clip audio — is built complete and
/// prepared by the sender (see the [`super::EngineHandle`] helpers, which do
/// this); anything prepared for another sample rate or a smaller block
/// size is refused. Whatever the callback replaces, removes or refuses is
/// freed on the engine's reclaim thread, never on the audio thread.
pub enum EngineCommand {
    /// Forward a transport control command (play, stop, pause, record, seek, etc.).
    Transport(TransportCommand),

    /// Add a track built with [`Track::new`] at the engine's sample rate
    /// and buffer size (see [`super::EngineHandle::add_track`]).
    AddTrack {
        /// The track, carried in a parcel that the engine keeps to send the
        /// track out again when it is removed.
        track: Parcel<Track>,
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

    /// Change the synthesis mode of a specific track, replacing its primary
    /// synth with `synth` — built for `mode`, set to the engine's sample
    /// rate and prepared for its buffer size (see
    /// [`super::prepared_synth`] and
    /// [`super::EngineHandle::set_track_synthesis_mode`]).
    SetTrackSynthesisMode {
        track_id: TrackId,
        synth: Prepared,
        mode: SynthesisMode,
    },

    /// Append an effect processor, prepared at the engine's sample rate and
    /// buffer size, to a track's effect chain (see
    /// [`super::EngineHandle::add_effect`]).
    AddEffect { track_id: TrackId, effect: Prepared },

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

    /// Add a synth layer, its synth built and prepared at the engine's
    /// sample rate and buffer size (see
    /// [`super::EngineHandle::add_synth_layer`]).
    AddSynthLayer {
        track_id: TrackId,
        layer: SynthLayer,
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

    /// MIDI Note On — route to the first armed track's synth layers.
    ///
    /// `note` is a MIDI note number (0-127), `velocity` is 0-127.
    /// `channel` is the MIDI channel (0-15).
    MidiNoteOn { note: u8, velocity: u8, channel: u8 },

    /// MIDI Note Off — release the given note.
    MidiNoteOff { note: u8, channel: u8 },

    /// MIDI Control Change — map CC number to synth/effect parameter.
    MidiCC { cc: u8, value: u8, channel: u8 },

    /// MIDI Pitch Bend — 14-bit pitch bend value (0-16383, center 8192).
    MidiPitchBend { value: u16, channel: u8 },

    /// Shut down the engine gracefully. All threads should terminate.
    Shutdown,
}

/// Generate an exhaustive `Debug` match: struct-like variants print every
/// field under its own name, tuple-like variants print their fields in
/// order, unit variants print their name, and `custom` arms are passed
/// through verbatim. The compiler still checks the match for exhaustiveness.
macro_rules! debug_match {
    (
        $self:expr, $f:expr;
        structs { $( $sv:ident { $($sf:ident),* } ),* $(,)? }
        tuples { $( $tv:ident ( $($tf:ident),* ) ),* $(,)? }
        units { $( $uv:ident ),* $(,)? }
        custom { $( $pat:pat => $body:expr ),* $(,)? }
    ) => {
        match $self {
            $( Self::$sv { $($sf),* } => {
                $f.debug_struct(stringify!($sv))$(.field(stringify!($sf), $sf))*.finish()
            } )*
            $( Self::$tv ( $($tf),* ) => $f.debug_tuple(stringify!($tv))$(.field($tf))*.finish(), )*
            $( Self::$uv => $f.write_str(stringify!($uv)), )*
            $( $pat => $body, )*
        }
    };
}

// `EngineCommand` cannot derive `Debug`: `Box<dyn Processor>` is not `Debug`,
// and printing a clip's samples would be useless. Tracks, synths, effects
// and clips are shown by name.
impl std::fmt::Debug for EngineCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        debug_match!(self, f;
            structs {
                AddSynthLayer { track_id, layer },
                RemoveEffect { track_id, effect_index },
                SetEffectBypass { track_id, effect_index, bypassed },
                SetEffectParameter { track_id, effect_index, param_index, value },
                SetSynthParameter { track_id, param_index, value },
                RemoveSynthLayer { track_id, layer_index },
                SetSynthLayerGain { track_id, layer_index, gain },
                SetSynthLayerEnabled { track_id, layer_index, enabled },
                SetSynthLayerParameter { track_id, layer_index, param_index, value },
                StartRecording { path },
                RemoveClip { track_id, clip_id },
                MoveClip { track_id, clip_id, new_position },
                TrimClipStart { track_id, clip_id, samples },
                TrimClipEnd { track_id, clip_id, samples },
                SplitClip { track_id, clip_id, split_position },
                SetClipGain { track_id, clip_id, gain },
                SetClipMute { track_id, clip_id, muted },
                DuplicateClip { track_id, clip_id, new_position },
                MidiNoteOn { note, velocity, channel },
                MidiNoteOff { note, channel },
                MidiCC { cc, value, channel },
                MidiPitchBend { value, channel },
            }
            tuples {
                Transport(cmd),
                RemoveTrack(id),
                SetTrackVolume(id, db),
                SetTrackPan(id, pan),
                SetTrackMute(id, muted),
                SetTrackSolo(id, soloed),
                SetTrackArm(id, armed),
                SetMasterVolume(db),
            }
            units { StopRecording, Shutdown }
            custom {
                Self::AddTrack { track } => f
                    .debug_struct("AddTrack")
                    .field("track", &track.get().map(|t| (t.id(), t.name())))
                    .finish(),
                Self::SetTrackSynthesisMode { track_id, synth, mode } => f
                    .debug_struct("SetTrackSynthesisMode")
                    .field("track_id", track_id)
                    .field("synth", &synth.processor().name())
                    .field("mode", mode)
                    .finish(),
                Self::AddEffect { track_id, effect } => f
                    .debug_struct("AddEffect")
                    .field("track_id", track_id)
                    .field("effect", &effect.processor().name())
                    .finish(),
                Self::AddClip { track_id, clip_data, position } => f
                    .debug_struct("AddClip")
                    .field("track_id", track_id)
                    .field("clip_data", &clip_data.name())
                    .field("position", position)
                    .finish(),
            }
        )
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
        let synth = crate::engine::create_synth(SynthesisMode::PitchTracked, 44_100.0);
        let track = Track::new(
            TrackId(4),
            "Lead",
            synth,
            SynthesisMode::PitchTracked,
            44_100.0,
            64,
        );
        let cmd = EngineCommand::AddTrack {
            track: Parcel::new(track),
        };
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("AddTrack"));
        assert!(dbg.contains("Lead"));
        assert!(dbg.contains('4'));
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
        let cmd = EngineCommand::SetTrackSynthesisMode {
            track_id: TrackId(0),
            synth: crate::engine::prepared_synth(SynthesisMode::Granular, 44_100.0, 64),
            mode: SynthesisMode::Granular,
        };
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("SetTrackSynthesisMode"));
        assert!(dbg.contains("Granular"));
        assert!(dbg.contains("Granular Synth"));
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
        assert!(dbg.contains('5'));
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
        assert!(dbg.contains('3'));
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
            layer: SynthLayer::new(
                crate::engine::prepared_synth(SynthesisMode::Wavetable, 44_100.0, 64),
                SynthesisMode::Wavetable,
                "Pad".into(),
            ),
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
        assert!(dbg.contains('2'));
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
