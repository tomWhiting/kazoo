use super::CycleTracker;
use crate::analysis::score::signals;

#[test]
fn a_breathy_high_note_holds_its_period() {
    let mut noise = 19_u32;
    let mut tracker = CycleTracker::new(48_000);
    let mut voiced = 0;
    let mut steps = 0;
    for index in 0..48_000 {
        noise ^= noise << 13;
        noise ^= noise >> 17;
        noise ^= noise << 5;
        let grain = (noise as f32 / u32::MAX as f32).mul_add(2.0, -1.0) * 0.08;
        let phase = std::f32::consts::TAU * 1500.0 * index as f32 / 48_000.0;
        tracker.push(0.5_f32.mul_add(phase.sin(), grain));
        if index >= 2400 && (index + 1) % 32 == 0 {
            steps += 1;
            if let Some(estimate) = tracker.estimate() {
                voiced += 1;
                let cents = (1200.0 * (estimate.hz / 1500.0).log2()).abs();
                assert!(cents < 1100.0, "{index}: {estimate:?}");
            }
        }
    }
    assert!(voiced * 100 >= steps * 98, "{voiced}/{steps}");
}

#[test]
fn a_high_note_lets_go_after_three_ms() {
    let mut tracker = CycleTracker::new(48_000);
    for index in 0..24_000 {
        let phase = std::f32::consts::TAU * 2000.0 * index as f32 / 48_000.0;
        tracker.push(0.5 * phase.sin());
    }
    for index in 0..168 {
        tracker.push(0.000_316);
        if index == 95 {
            assert!(tracker.estimate().is_some(), "released before two ms");
        }
    }
    assert!(
        tracker.estimate().is_none(),
        "held beyond three ms plus a period"
    );
}

#[test]
fn a_splice_at_a_high_note_is_bridged() {
    let mut tracker = CycleTracker::new(48_000);
    for index in 0..48_000 {
        let phase_index = if index >= 24_000 { index + 128 } else { index };
        let phase = std::f32::consts::TAU * 2000.0 * phase_index as f32 / 48_000.0;
        tracker.push(0.5 * phase.sin());
        if (24_000..24_144).contains(&index) {
            assert!(tracker.estimate().is_some(), "splice unlock at {index}");
        }
        if index == 24_143 {
            let estimate = tracker.estimate().expect("splice holds");
            assert!((1200.0 * (estimate.hz / 2000.0).log2()).abs() <= 50.0);
        }
    }
}

#[test]
fn high_cycle_rows_preserve_the_octave() {
    for take in signals::high_suite(48_000) {
        let mut tracker = CycleTracker::new(take.rate);
        let mut count = 0;
        for (index, &sample) in take.samples.iter().enumerate() {
            tracker.push(sample);
            if index > 14_400
                && index < take.samples.len() - 9600
                && index % 32 == 0
                && let Some(estimate) = tracker.estimate()
            {
                let truth = take.truth[index / 32].expect("body truth");
                let cents = 1200.0 * (estimate.hz / truth).log2();
                let octave = (cents / 1200.0).round();
                assert!(
                    octave.abs() < f32::EPSILON || 1200.0_f32.mul_add(-octave, cents).abs() > 100.0,
                    "{} {index}: {estimate:?}, truth {truth}",
                    take.name
                );
                count += 1;
            }
        }
        assert!(count > 4000, "{} voiced steps {count}", take.name);
    }
}
