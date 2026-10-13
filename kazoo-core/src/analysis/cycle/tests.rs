use crate::analysis::{cycle::CycleTracker, score::signals};
use std::f32::consts::{FRAC_PI_2, PI, TAU};

fn sine(hz: f32, rate: u32, length: u32, phase: f32) -> Vec<f32> {
    (0..length)
        .map(|i| (TAU * hz * i as f32 / rate as f32 + phase).sin() * 0.5)
        .collect()
}

fn cents(hz: f32, truth: f32) -> f32 {
    (1200.0 * (hz / truth).log2()).abs()
}

fn named(name: &str) -> crate::analysis::score::Take {
    signals::suite(48_000)
        .into_iter()
        .find(|t| t.name == name)
        .expect("take exists")
}

fn noise(length: u32, amplitude: f32) -> Vec<f32> {
    let mut state = 19_u32;
    (0..length)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state as f32 / u32::MAX as f32).mul_add(2.0, -1.0) * amplitude
        })
        .collect()
}

#[test]
fn a_sine_is_tracked_within_three_cents_at_every_rate() {
    for rate in [44_100, 48_000, 96_000, 192_000] {
        let mut tracker = CycleTracker::new(rate);
        for (i, sample) in sine(220.0, rate, rate / 5, 0.0).into_iter().enumerate() {
            tracker.push(sample);
            if i >= rate as usize / 20 {
                let estimate = tracker.estimate().expect("locked after 50 ms");
                assert!(
                    cents(estimate.hz, 220.0) <= 3.0,
                    "rate {rate}, {i}: {estimate:?}"
                );
                assert!(estimate.clarity >= 0.98, "{estimate:?}");
            }
        }
    }
}

#[test]
fn the_first_pitch_comes_within_two_periods() {
    for phase in [0.0, FRAC_PI_2, PI, 3.0 * FRAC_PI_2] {
        let mut tracker = CycleTracker::new(48_000);
        tracker.process(&noise(9_600, 0.000_316));
        let limit = (2.0_f32 * 48_000.0 / 220.0 + 48.0).floor() as u32;
        tracker.process(&sine(220.0, 48_000, limit, phase));
        let estimate = tracker.estimate().unwrap_or_else(|| {
            panic!(
                "phase {phase}, marks {}, clarity {}",
                tracker.marks(),
                tracker.last_clarity()
            )
        });
        assert!(
            cents(estimate.hz, 220.0) <= 50.0,
            "phase {phase}: {estimate:?}"
        );
    }
}

#[test]
fn a_note_change_lands_within_two_periods_of_the_new_note() {
    for (old, new) in [(220.0, 330.0), (330.0, 220.0)] {
        let mut tracker = CycleTracker::new(48_000);
        tracker.process(&sine(old, 48_000, 9_600, 0.0));
        let phase = TAU * old * 9_600.0 / 48_000.0;
        let limit = (2.0_f32 * 48_000.0 / new + 48.0).floor() as u32;
        tracker.process(&sine(new, 48_000, limit, phase));
        let estimate = tracker.estimate().expect("new note landed");
        assert!(
            cents(estimate.hz, new) <= 20.0,
            "{old} -> {new}: {estimate:?}"
        );
    }
}

#[test]
fn harmonics_do_not_double_the_pitch() {
    let take = named("octave_bait");
    let mut tracker = CycleTracker::new(48_000);
    let mut locked = 0;
    let mut doubled = 0;
    for sample in &take.samples[9_600..38_400] {
        tracker.push(*sample);
        if let Some(estimate) = tracker.estimate() {
            locked += 1;
            doubled += usize::from(cents(estimate.hz, 300.0) <= 100.0);
        }
    }
    assert!(locked > 0);
    assert!(doubled * 100 <= locked * 5, "{doubled}/{locked}");
}

#[test]
fn a_formant_rich_vowel_locks() {
    let take = named("vowel_ah");
    let mut tracker = CycleTracker::new(48_000);
    let limit = 9_600 + (3.0_f32 * 48_000.0 / 130.81 + 48.0).floor() as usize;
    tracker.process(&take.samples[..limit]);
    let estimate = tracker
        .estimate()
        .expect("vowel locked within three periods");
    assert!(cents(estimate.hz, 130.81) <= 50.0, "{estimate:?}");
}

#[test]
fn noise_never_locks() {
    let mut tracker = CycleTracker::new(48_000);
    let mut locked = 0;
    for block in noise(48_000, 0.25).chunks(32) {
        tracker.process(block);
        locked += usize::from(tracker.estimate().is_some());
    }
    assert!(locked <= 30, "{locked}/1500 noise steps locked");
    tracker.reset();
    tracker.process(&noise(48_000, 0.000_316));
    assert_eq!(tracker.marks(), 0);
}

#[test]
fn it_lets_go_after_two_periods_of_silence() {
    let mut tracker = CycleTracker::new(48_000);
    tracker.process(&sine(220.0, 48_000, 9_600, 0.0));
    assert!(tracker.estimate().is_some());
    tracker.process(&noise(
        (2.0_f32 * 48_000.0 / 220.0 + 48.0).floor() as u32,
        0.000_316,
    ));
    assert!(tracker.estimate().is_none(), "{:?}", tracker.estimate());
}

#[test]
fn a_hint_brings_an_octave_slip_back() {
    let mut tracker = CycleTracker::new(48_000);
    tracker.process(&sine(300.0, 48_000, 9_600, 0.0));
    assert!(tracker.estimate().is_some_and(|e| cents(e.hz, 300.0) < 3.0));
    let signal: Vec<_> = (0..4_800)
        .map(|i| {
            let phase = TAU * 150.0 * i as f32 / 48_000.0;
            1.5_f32.mul_add((2.0 * phase).sin(), phase.sin()) * 0.2
        })
        .collect();
    tracker.hint(48_000.0 / 150.0);
    let before = tracker.marks();
    let mut corrected = false;
    for sample in &signal {
        tracker.push(*sample);
        corrected |= tracker
            .estimate()
            .is_some_and(|e| cents(e.hz, 150.0) < 3.0 && e.clarity >= 0.8);
        if tracker.marks() >= before + 4 {
            break;
        }
    }
    assert!(corrected, "{:?}", tracker.estimate());
}

#[test]
fn nonsense_input_is_survived() {
    let mut tracker = CycleTracker::new(48_000);
    for sample in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1e30, -1e30, 1.0] {
        tracker.push(sample);
        if let Some(e) = tracker.estimate() {
            assert!(e.period.is_finite() && e.hz.is_finite() && e.clarity.is_finite());
        }
    }
    tracker.process(&sine(
        220.0,
        48_000,
        (3.0_f32 * 48_000.0 / 220.0).ceil() as u32,
        0.0,
    ));
    assert!(
        tracker
            .estimate()
            .is_some_and(|e| cents(e.hz, 220.0) < 50.0),
        "{:?}",
        tracker.estimate()
    );
}

#[test]
fn processing_never_allocates() {
    let take = named("vowel_ah");
    let body = &take.samples[9_600..48_000];
    let signal: Vec<_> = body.iter().copied().cycle().take(480_000).collect();
    let mut tracker = CycleTracker::new(48_000);
    let before = assert_no_alloc::violation_count();
    assert_no_alloc::assert_no_alloc(|| tracker.process(&signal));
    assert_eq!(assert_no_alloc::violation_count(), before);
}
