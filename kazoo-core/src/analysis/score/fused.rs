use super::{TrackerUnderTest, gate::GateUnderTest};
use crate::analysis::cycle::ProvisionalJudge;
use crate::analysis::{OnsetDetector, PitchDetector, PitchDetectorConfig};
use crate::voice::{
    TrackSettings, VoiceFrame, VoiceTracker,
    judge::{JudgeResult, OnsetConfig},
};
use std::collections::VecDeque;

/// Voice fusion with synchronous judges delivered one judge hop late.
#[derive(Debug)]
pub struct FusedUnderTest {
    tracker: VoiceTracker,
    pitch: PitchDetector,
    onset: OnsetDetector,
    pending: VecDeque<(u64, JudgeResult)>,
    samples: u64,
    lag: u64,
    latest: Option<VoiceFrame>,
    onsets: u64,
    early: Option<ProvisionalJudge>,
    #[cfg(test)]
    recording: Option<Recording>,
}

#[cfg(test)]
#[derive(Debug, Default)]
pub(super) struct Recording {
    pub(super) verdicts: VecDeque<(u64, JudgeResult)>,
    pub(super) frames: VecDeque<VoiceFrame>,
}
impl FusedUnderTest {
    /// Construct an offline adapter for this stream's sample rate.
    pub fn new(rate: u32) -> crate::Result<Self> {
        Self::with_settings(rate, TrackSettings::VOICE)
    }

    /// Use the same profile for cycle tracking, gate and worker verdicts.
    pub fn with_settings(rate: u32, settings: TrackSettings) -> crate::Result<Self> {
        Self::assemble(rate, settings, settings.judge_config(rate))
    }

    /// Construct an offline adapter with the voice profile and the caller's pitch judge.
    pub fn with_judge(rate: u32, config: PitchDetectorConfig) -> crate::Result<Self> {
        Self::assemble(rate, TrackSettings::VOICE, config)
    }

    fn assemble(
        rate: u32,
        settings: TrackSettings,
        config: PitchDetectorConfig,
    ) -> crate::Result<Self> {
        let scale = PitchDetectorConfig::for_rate(rate).frame_length / 2048;
        let lag = config.hop_length as u64;
        let onsets = OnsetConfig::default();
        Ok(Self {
            tracker: VoiceTracker::with_settings(rate, settings),
            pitch: PitchDetector::new(config)?,
            // The judge thread's own onset settings, scaled as `judge::spawn` scales them.
            onset: OnsetDetector::new(
                onsets.onset_fft_size * scale,
                onsets.onset_fft_size * scale / 4,
                rate as f32,
                onsets.onset_threshold,
            ),
            pending: VecDeque::with_capacity(64),
            samples: 0,
            lag,
            latest: None,
            onsets: 0,
            early: settings
                .cycle
                .early
                .then(|| ProvisionalJudge::new(rate, settings.cycle)),
            #[cfg(test)]
            recording: None,
        })
    }

    #[cfg(test)]
    pub(super) fn record(&mut self) {
        self.recording = Some(Recording::default());
    }

    #[cfg(test)]
    pub(super) fn finish_recording(self) -> Recording {
        self.recording.expect("recording was started")
    }
}
impl TrackerUnderTest for FusedUnderTest {
    fn name(&self) -> &'static str {
        "voice"
    }
    fn push(&mut self, block: &[f32]) {
        for sample in block {
            while self
                .pending
                .front()
                .is_some_and(|(at, _)| *at <= self.samples)
            {
                if let Some((_, result)) = self.pending.pop_front() {
                    #[cfg(test)]
                    if let Some(recording) = self.recording.as_mut() {
                        recording.verdicts.push_back((self.samples, result));
                    }
                    self.tracker.judge(result);
                }
            }
            let input = std::slice::from_ref(sample);
            if let Some(early) = self.early.as_mut() {
                if let Some(estimate) = early.push(*sample) {
                    self.tracker.judge(JudgeResult::Provisional(estimate));
                }
            }
            self.tracker.process(input, |frame| {
                self.latest = Some(frame);
                self.onsets += u64::from(frame.onset);
                #[cfg(test)]
                if let Some(recording) = self.recording.as_mut() {
                    recording.frames.push_back(frame);
                }
            });
            self.samples += 1;
            if let Some(pitch) = self.pitch.push_samples(input) {
                self.pending
                    .push_back((self.samples + self.lag, JudgeResult::Pitch(pitch)));
            }
            for onset in self.onset.push_samples(input) {
                self.pending.push_back((
                    self.samples + self.lag,
                    JudgeResult::Onset {
                        strength: onset.strength,
                    },
                ));
            }
        }
    }
    fn hz(&self) -> Option<f32> {
        self.latest.filter(|f| f.voiced).map(|f| f.hz)
    }
    fn provisional(&self) -> bool {
        self.latest.is_some_and(|frame| frame.provisional)
    }
}
impl GateUnderTest for FusedUnderTest {
    fn gate(&self) -> bool {
        self.latest.is_some_and(|f| f.gate)
    }
    fn onsets(&self) -> u64 {
        self.onsets
    }
}
