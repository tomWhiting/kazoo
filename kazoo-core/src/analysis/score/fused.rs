use super::{TrackerUnderTest, gate::GateUnderTest};
use crate::analysis::{OnsetDetector, PitchDetector, PitchDetectorConfig};
use crate::voice::{
    VoiceFrame, VoiceTracker,
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
}
impl FusedUnderTest {
    /// Construct an offline adapter for this stream's sample rate.
    pub fn new(rate: u32) -> crate::Result<Self> {
        Self::with_judge(rate, PitchDetectorConfig::for_rate(rate))
    }

    /// Construct an offline adapter with the caller's pitch judge.
    pub fn with_judge(rate: u32, config: PitchDetectorConfig) -> crate::Result<Self> {
        let scale = PitchDetectorConfig::for_rate(rate).frame_length / 2048;
        let lag = config.hop_length as u64;
        let onsets = OnsetConfig::default();
        Ok(Self {
            tracker: VoiceTracker::new(rate),
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
        })
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
                    self.tracker.judge(result);
                }
            }
            let input = std::slice::from_ref(sample);
            self.tracker.process(input, |frame| {
                self.latest = Some(frame);
                self.onsets += u64::from(frame.onset);
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
}
impl GateUnderTest for FusedUnderTest {
    fn gate(&self) -> bool {
        self.latest.is_some_and(|f| f.gate)
    }
    fn onsets(&self) -> u64 {
        self.onsets
    }
}
