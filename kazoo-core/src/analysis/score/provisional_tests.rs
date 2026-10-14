use super::{SCORE_STEP, Take, TrackerUnderTest, score};

struct Correction {
    step: usize,
}
impl TrackerUnderTest for Correction {
    fn name(&self) -> &'static str {
        "provisional correction"
    }
    fn push(&mut self, block: &[f32]) {
        self.step += block.len() / SCORE_STEP;
    }
    fn hz(&self) -> Option<f32> {
        Some(if self.step <= 20 { 220.0 } else { 110.0 })
    }
    fn provisional(&self) -> bool {
        self.step <= 20
    }
}

#[test]
fn corrected_provisional_frames_are_counted() {
    let take = Take {
        name: "correction".into(),
        rate: 48_000,
        samples: vec![0.0; 80 * SCORE_STEP],
        truth: vec![Some(110.0); 80],
        onsets: vec![0],
        exact: true,
    };
    let result = score(&mut Correction { step: 0 }, &take);
    assert!(
        result.octave_pct > 0.0,
        "the early correction was excluded: {result:?}"
    );
}
