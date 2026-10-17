//! Engine health counters: every place the engine has to drop data or reject
//! a request increments a counter here instead of failing silently.
//!
//! The counters are plain relaxed atomics, so they are safe to bump from the
//! real-time audio callbacks (no allocation, no locking, no I/O). The UI (or
//! any frontend) reads a consistent-enough [`EngineStatsSnapshot`] through
//! [`super::EngineHandle::stats`] and can surface non-zero values to the user.

use crate::host::audio::StreamHealth;
use crate::io::mic::{MicShared, MicStatus};
use crate::voice::judge::JudgeCounters;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

/// Shared engine health counters and supervisor status.
///
/// One instance is created per engine and shared (via `Arc`) between the
/// audio callbacks, the analysis and disk threads, the MIDI callback and the
/// [`super::EngineHandle`].
#[derive(Debug, Default)]
pub struct EngineStats {
    audio_health: OnceLock<StreamHealth>,
    mic: OnceLock<Arc<MicShared>>,
    /// The voice judge's own counters, read with the engine's.
    judge: OnceLock<JudgeCounters>,
    mic_samples_dropped: AtomicU64,
    analysis_samples_dropped: AtomicU64,
    analysis_results_dropped: AtomicU64,
    disk_samples_dropped: AtomicU64,
    display_frames_dropped: AtomicU64,
    commands_dropped: AtomicU64,
    disk_commands_dropped: AtomicU64,
    params_rejected: AtomicU64,
    clips_rejected: AtomicU64,
    disk_errors: AtomicU64,
    desk_requests_dropped: AtomicU64,
    desk_syncs_rejected: AtomicU64,
    tracks_rejected: AtomicU64,
    processors_rejected: AtomicU64,
    takes_unavailable: AtomicU64,
    take_samples_dropped: AtomicU64,
    callback_frees: AtomicU64,
}

/// A point-in-time copy of [`EngineStats`].
///
/// Every counter is monotonically non-decreasing for the lifetime of the
/// engine. A non-zero value means data was lost or a request was refused.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EngineStatsSnapshot {
    /// Native input configuration and recovery state.
    pub mic: MicStatus,
    /// Mic samples lost because the input → output ring buffer was full.
    pub mic_samples_dropped: u64,
    /// Samples lost because the analysis thread (the spectrum and formant
    /// display) fell behind.
    pub analysis_samples_dropped: u64,
    /// Spectrum and formant results lost because the output callback had not
    /// drained the previous ones. Only the display reads them; pitch is the
    /// judge's.
    pub analysis_results_dropped: u64,
    /// Recorded master-bus samples lost because the disk thread fell behind.
    /// Any non-zero value means the recorded WAV file has gaps.
    pub disk_samples_dropped: u64,
    /// Display snapshots lost because the UI was not polling fast enough.
    pub display_frames_dropped: u64,
    /// Engine commands (from the UI or MIDI) lost because the command
    /// channel was full.
    pub commands_dropped: u64,
    /// Start/stop commands that could not be delivered to the disk thread.
    pub disk_commands_dropped: u64,
    /// Synth or effect parameter changes rejected (bad index / target).
    pub params_rejected: u64,
    /// Clips (including finished recordings) that could not be placed on a
    /// track, because the track was full or no longer exists.
    pub clips_rejected: u64,
    /// Disk recorder failures (start, write or finalize).
    pub disk_errors: u64,
    /// Transport requests (play, stop, tempo) meant for the kazoo-mix desk
    /// that could not be queued, so the desk never heard them.
    pub desk_requests_dropped: u64,
    /// Transport syncs from the desk that could not be followed (invalid, or
    /// too many changes waiting), so this engine may be out of step.
    pub desk_syncs_rejected: u64,
    /// Tracks the engine refused: it already holds [`crate::MAX_TRACKS`],
    /// or the track was built for a different block size.
    pub tracks_rejected: u64,
    /// Synths, synth layers and effects the engine refused: prepared for
    /// another sample rate or a smaller block size, or no room left on the
    /// track.
    pub processors_rejected: u64,
    /// Armed tracks that could not start recording because no recording
    /// chunk was free (the reclaim thread had fallen far behind).
    pub takes_unavailable: u64,
    /// Recorded samples lost because no recording chunk was free; any
    /// non-zero value means a take has a gap.
    pub take_samples_dropped: u64,
    /// Objects freed on the audio thread because they could not be handed
    /// to the reclaim thread. The engine is built so this never happens;
    /// any non-zero value is an engine bug.
    pub callback_frees: u64,
    /// Samples the voice judge (pYIN and onsets) missed because its input
    /// ring was full: it fell behind.
    pub judge_samples_dropped: u64,
    /// Voice judge verdicts lost because the output callback had not drained
    /// the ones before.
    pub judge_results_dropped: u64,
    /// pYIN runs that failed in the voice judge, each read as an unvoiced
    /// frame. A validated configuration never fails one, so any is an
    /// engine fault; [`EngineStats::first_pitch_failure`] says why.
    pub pitch_runs_failed: u64,
}

impl EngineStatsSnapshot {
    /// Whether every counter is zero (the engine has lost nothing).
    #[must_use]
    pub const fn is_clean(&self) -> bool {
        self.mic_samples_dropped == 0
            && self.analysis_samples_dropped == 0
            && self.analysis_results_dropped == 0
            && self.disk_samples_dropped == 0
            && self.display_frames_dropped == 0
            && self.commands_dropped == 0
            && self.disk_commands_dropped == 0
            && self.params_rejected == 0
            && self.clips_rejected == 0
            && self.disk_errors == 0
            && self.desk_requests_dropped == 0
            && self.desk_syncs_rejected == 0
            && self.tracks_rejected == 0
            && self.processors_rejected == 0
            && self.takes_unavailable == 0
            && self.take_samples_dropped == 0
            && self.callback_frees == 0
            && self.judge_samples_dropped == 0
            && self.judge_results_dropped == 0
            && self.pitch_runs_failed == 0
    }
}

/// Add `n` to a counter. Relaxed ordering: the counters are independent
/// statistics, not synchronisation points.
fn bump(counter: &AtomicU64, n: u64) {
    counter.fetch_add(n, Ordering::Relaxed);
}

/// Convert a sample count to `u64` for counting. `usize` is at most 64 bits
/// on every supported target; saturate defensively on anything wider.
fn count(samples: usize) -> u64 {
    u64::try_from(samples).unwrap_or(u64::MAX)
}

impl EngineStats {
    pub(super) fn set_audio_health(&self, health: StreamHealth) -> crate::Result<()> {
        self.audio_health
            .set(health)
            .map_err(|_| crate::Error::Config("audio health already initialized".into()))
    }

    pub(super) fn set_judge(&self, counters: JudgeCounters) -> crate::Result<()> {
        self.judge
            .set(counters)
            .map_err(|_| crate::Error::Config("judge counters already attached".into()))
    }

    /// Why the voice judge's first failed pYIN run failed, once one has.
    #[must_use]
    pub fn first_pitch_failure(&self) -> Option<&str> {
        self.judge
            .get()
            .and_then(JudgeCounters::first_pitch_failure)
    }

    pub(super) fn set_mic(&self, shared: Arc<MicShared>) -> crate::Result<()> {
        self.mic
            .set(shared)
            .map_err(|_| crate::Error::Config("mic status already initialized".into()))
    }

    pub(super) fn check_audio(&self) -> crate::Result<()> {
        if let Some(health) = self.audio_health.get() {
            health.check()?;
        }
        Ok(())
    }

    /// Create a zeroed set of counters.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record the outcome of a bulk ring-buffer push: `offered` samples were
    /// offered and `accepted` were stored.
    fn record_push(counter: &AtomicU64, offered: usize, accepted: usize) {
        let lost = offered.saturating_sub(accepted);
        if lost > 0 {
            bump(counter, count(lost));
        }
    }

    /// Mic ring push: `offered` samples offered, `accepted` stored.
    pub fn record_mic_push(&self, offered: usize, accepted: usize) {
        Self::record_push(&self.mic_samples_dropped, offered, accepted);
    }

    /// Analysis-input ring push: `offered` samples offered, `accepted` stored.
    pub fn record_analysis_push(&self, offered: usize, accepted: usize) {
        Self::record_push(&self.analysis_samples_dropped, offered, accepted);
    }

    /// Disk ring push: `offered` samples offered, `accepted` stored.
    pub fn record_disk_push(&self, offered: usize, accepted: usize) {
        Self::record_push(&self.disk_samples_dropped, offered, accepted);
    }

    /// One analysis result could not be queued.
    pub fn analysis_result_dropped(&self) {
        bump(&self.analysis_results_dropped, 1);
    }

    /// One display snapshot could not be queued.
    pub fn display_frame_dropped(&self) {
        bump(&self.display_frames_dropped, 1);
    }

    /// One engine command could not be queued.
    pub fn command_dropped(&self) {
        bump(&self.commands_dropped, 1);
    }

    /// One disk command could not be delivered.
    pub fn disk_command_dropped(&self) {
        bump(&self.disk_commands_dropped, 1);
    }

    /// One parameter change was rejected.
    pub fn param_rejected(&self) {
        bump(&self.params_rejected, 1);
    }

    /// One clip could not be placed on its track.
    pub fn clip_rejected(&self) {
        bump(&self.clips_rejected, 1);
    }

    /// One disk recorder operation failed.
    pub fn disk_error(&self) {
        bump(&self.disk_errors, 1);
    }

    /// One transport request for the desk could not be queued.
    pub fn desk_request_dropped(&self) {
        bump(&self.desk_requests_dropped, 1);
    }

    /// One transport sync from the desk could not be followed.
    pub fn desk_sync_rejected(&self) {
        bump(&self.desk_syncs_rejected, 1);
    }

    /// One track was refused by the engine.
    pub fn track_rejected(&self) {
        bump(&self.tracks_rejected, 1);
    }

    /// One synth, layer or effect was refused.
    pub fn processor_rejected(&self) {
        bump(&self.processors_rejected, 1);
    }

    /// One armed track could not start recording.
    pub fn take_unavailable(&self) {
        bump(&self.takes_unavailable, 1);
    }

    /// `samples` recorded samples were lost.
    pub fn take_samples_dropped(&self, samples: usize) {
        bump(&self.take_samples_dropped, count(samples));
    }

    /// One object was freed on the audio thread.
    pub fn callback_free(&self) {
        bump(&self.callback_frees, 1);
    }

    /// Take a copy of every counter and the supervisor's mic status.
    /// This reads the mic's status mutex and belongs on the UI thread.
    #[must_use]
    pub fn snapshot(&self) -> EngineStatsSnapshot {
        let load = |c: &AtomicU64| c.load(Ordering::Relaxed);
        let judge = self
            .judge
            .get()
            .map(JudgeCounters::snapshot)
            .unwrap_or_default();
        EngineStatsSnapshot {
            mic: self
                .mic
                .get()
                .map_or_else(MicStatus::default, |shared| shared.status()),
            mic_samples_dropped: load(&self.mic_samples_dropped),
            analysis_samples_dropped: load(&self.analysis_samples_dropped),
            analysis_results_dropped: load(&self.analysis_results_dropped),
            disk_samples_dropped: load(&self.disk_samples_dropped),
            display_frames_dropped: load(&self.display_frames_dropped),
            commands_dropped: load(&self.commands_dropped),
            disk_commands_dropped: load(&self.disk_commands_dropped),
            params_rejected: load(&self.params_rejected),
            clips_rejected: load(&self.clips_rejected),
            disk_errors: load(&self.disk_errors),
            desk_requests_dropped: load(&self.desk_requests_dropped),
            desk_syncs_rejected: load(&self.desk_syncs_rejected),
            tracks_rejected: load(&self.tracks_rejected),
            processors_rejected: load(&self.processors_rejected),
            takes_unavailable: load(&self.takes_unavailable),
            take_samples_dropped: load(&self.take_samples_dropped),
            callback_frees: load(&self.callback_frees),
            judge_samples_dropped: judge.samples_dropped,
            judge_results_dropped: judge.results_dropped,
            pitch_runs_failed: judge.pitch_runs_failed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_judges_counters_count_against_a_clean_engine() {
        for snapshot in [
            EngineStatsSnapshot {
                judge_samples_dropped: 1,
                ..EngineStatsSnapshot::default()
            },
            EngineStatsSnapshot {
                judge_results_dropped: 1,
                ..EngineStatsSnapshot::default()
            },
            EngineStatsSnapshot {
                pitch_runs_failed: 1,
                ..EngineStatsSnapshot::default()
            },
        ] {
            assert!(!snapshot.is_clean(), "{snapshot:?}");
        }
    }

    #[test]
    fn new_stats_are_clean() {
        let stats = EngineStats::new();
        assert!(stats.snapshot().is_clean());
        assert_eq!(stats.snapshot(), EngineStatsSnapshot::default());
    }

    #[test]
    fn mic_status_is_separate_from_loss_counters() {
        let shared = Arc::new(MicShared::default());
        let stats = EngineStats::new();
        stats.set_mic(Arc::clone(&shared)).unwrap();
        assert_eq!(stats.snapshot().mic, shared.status());
        assert!(stats.set_mic(shared).is_err());
        let snapshot = EngineStatsSnapshot {
            mic: MicStatus {
                state: crate::io::mic::MicState::Lost,
                ..MicStatus::default()
            },
            ..EngineStatsSnapshot::default()
        };
        assert!(snapshot.is_clean());
    }

    #[test]
    fn partial_push_counts_only_lost_samples() {
        let stats = EngineStats::new();
        stats.record_mic_push(128, 128);
        assert!(stats.snapshot().is_clean());

        stats.record_mic_push(128, 100);
        stats.record_analysis_push(64, 0);
        stats.record_disk_push(10, 3);
        let snap = stats.snapshot();
        assert_eq!(snap.mic_samples_dropped, 28);
        assert_eq!(snap.analysis_samples_dropped, 64);
        assert_eq!(snap.disk_samples_dropped, 7);
        assert!(!snap.is_clean());
    }

    #[test]
    fn accepted_greater_than_offered_does_not_underflow() {
        let stats = EngineStats::new();
        stats.record_disk_push(4, 8);
        assert_eq!(stats.snapshot().disk_samples_dropped, 0);
    }

    #[test]
    fn event_counters_increment_independently() {
        let stats = EngineStats::new();
        stats.analysis_result_dropped();
        stats.display_frame_dropped();
        stats.display_frame_dropped();
        stats.command_dropped();
        stats.disk_command_dropped();
        stats.param_rejected();
        stats.clip_rejected();
        stats.disk_error();
        stats.desk_request_dropped();
        stats.desk_sync_rejected();
        stats.desk_sync_rejected();
        stats.track_rejected();
        stats.take_unavailable();
        stats.take_unavailable();
        stats.take_unavailable();
        stats.callback_free();
        stats.processor_rejected();
        stats.take_samples_dropped(40);

        let snap = stats.snapshot();
        assert_eq!(snap.analysis_results_dropped, 1);
        assert_eq!(snap.display_frames_dropped, 2);
        assert_eq!(snap.commands_dropped, 1);
        assert_eq!(snap.disk_commands_dropped, 1);
        assert_eq!(snap.params_rejected, 1);
        assert_eq!(snap.clips_rejected, 1);
        assert_eq!(snap.disk_errors, 1);
        assert_eq!(snap.desk_requests_dropped, 1);
        assert_eq!(snap.desk_syncs_rejected, 2);
        assert_eq!(snap.tracks_rejected, 1);
        assert_eq!(snap.takes_unavailable, 3);
        assert_eq!(snap.callback_frees, 1);
        assert_eq!(snap.processors_rejected, 1);
        assert_eq!(snap.take_samples_dropped, 40);
        assert_eq!(snap.mic_samples_dropped, 0);
    }
}
