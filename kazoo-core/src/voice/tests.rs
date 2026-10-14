use crate::analysis::{PitchDetectorConfig, PitchEstimate, score::signals};
use crate::voice::judge::{self, JudgeResult, OnsetConfig};
use crate::voice::{C4_HZ, VoiceFrame, VoiceTracker, hz_from_volts, volts_from_hz};
use std::f32::consts::TAU;

fn pitch(hz: Option<f32>, probability: f32) -> JudgeResult {
    JudgeResult::Pitch(PitchEstimate {
        frequency: hz,
        voiced_probability: probability,
        midi_note: None,
    })
}
fn sine(hz: f32, length: usize, amplitude: f32) -> Vec<f32> {
    (0..length)
        .map(|i| (TAU * hz * i as f32 / 48_000.0).sin() * amplitude)
        .collect()
}
fn take(name: &str) -> crate::analysis::score::Take {
    signals::suite(48_000)
        .into_iter()
        .find(|t| t.name == name)
        .expect("named take")
}
fn frames(samples: &[f32]) -> Vec<VoiceFrame> {
    let mut tracker = VoiceTracker::new(48_000);
    let mut output = Vec::new();
    tracker.process(samples, |frame| output.push(frame));
    output
}
#[test]
fn volts_are_one_per_octave_from_c4() {
    assert!(volts_from_hz(C4_HZ).abs() < f32::EPSILON);
    assert!((volts_from_hz(523.2511) - 1.0).abs() < 1e-5);
    assert!((volts_from_hz(130.81278) + 1.0).abs() < 1e-5);
    assert!((1200.0 * (hz_from_volts(volts_from_hz(440.0)) / 440.0).log2()).abs() < 0.01);
    for bad in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        assert!(volts_from_hz(bad).abs() < f32::EPSILON);
    }
    assert!(C4_HZ.mul_add(-256.0, hz_from_volts(20.0)).abs() < 0.01);
    assert!((hz_from_volts(-20.0) - C4_HZ / 256.0).abs() < 1e-5);
    assert!(hz_from_volts(f32::NAN).is_finite());
}
#[test]
fn a_frame_comes_every_64_samples() {
    let signal = sine(220.0, 48_000, 0.5);
    for size in [1, 37, 128, 4096] {
        let mut tracker = VoiceTracker::new(48_000);
        let mut count = 0_u64;
        for block in signal.chunks(size) {
            tracker.process(block, |frame| {
                count += 1;
                assert_eq!(frame.at, count * 64);
            });
        }
        assert_eq!(count, 750);
        tracker.reset();
        tracker.process(&signal[..64], |f| assert_eq!(f.at, 64));
    }
}
#[test]
fn the_judge_folds_an_octave_slip() {
    let signal = take("octave_bait");
    let mut tracker = VoiceTracker::new(48_000);
    let mut voiced = 0;
    for block in signal.samples.chunks(512) {
        tracker.judge(pitch(Some(150.0), 0.9));
        tracker.process(block, |f| {
            if f.voiced && f.at >= 14_400 && f.at < 38_400 {
                voiced += 1;
                assert!((1200.0 * (f.hz / 150.0).log2()).abs() < 10.0, "{f:?}");
            }
        });
    }
    assert!(voiced > 300);
    // A forced half-period hint exercises a fold even when this take already locks correctly.
    let mut tracker = VoiceTracker::new(48_000);
    let bait: Vec<_> = (0..24_000)
        .map(|i| {
            let phase = TAU * 150.0 * i as f32 / 48_000.0;
            0.4_f32.mul_add((2.0 * phase).sin(), 0.02 * phase.sin())
        })
        .collect();
    let mut folded = None;
    for block in bait.chunks(64) {
        if tracker.samples % 512 == 0 {
            tracker.judge(pitch(Some(150.0), 0.9));
        }
        tracker.process(block, |_| {});
        if tracker.fold != 0.0 && folded.is_none() {
            folded = Some(tracker.cycle.marks());
        }
        if let Some(mark) = folded {
            if tracker
                .cycle
                .estimate()
                .is_some_and(|e| (1200.0 * (e.hz / 150.0).log2()).abs() < 10.0)
            {
                assert!(tracker.cycle.marks() <= mark + 4);
                return;
            }
            assert!(
                tracker.cycle.marks() < mark + 4,
                "hint did not land within four marks"
            );
        }
    }
    panic!("no fold landed: {folded:?}");
}

#[test]
fn a_hint_landing_is_not_a_leap() {
    let bait: Vec<_> = (0..48_000)
        .map(|i| {
            let phase = TAU * 150.0 * i as f32 / 48_000.0;
            0.4_f32.mul_add((2.0 * phase).sin(), 0.02 * phase.sin())
        })
        .collect();
    let mut tracker = VoiceTracker::new(48_000);
    let mut hinted = None;
    for block in bait.chunks(64) {
        if tracker.samples % 512 == 0 {
            tracker.judge(pitch(Some(150.0), 0.9));
        }
        tracker.process(block, |_| {});
        if tracker.fold != 0.0 && hinted.is_none() {
            hinted = Some((tracker.samples, tracker.last_leap));
        }
        if let Some((at, last_leap)) = hinted {
            if tracker
                .cycle
                .estimate()
                .is_some_and(|e| (1200.0 * (e.hz / 150.0).log2()).abs() < 10.0)
            {
                assert!(tracker.samples - at <= 480, "hint landed after 10 ms");
                assert_eq!(
                    tracker.last_leap, last_leap,
                    "landing on the hinted period must not count as a sung leap"
                );
                tracker.reset();
                assert_eq!(tracker.last_leap, 0);
                return;
            }
        }
    }
    panic!("no hint landed: {hinted:?}");
}

#[test]
fn a_sung_octave_leap_is_not_folded_back() {
    let mut signal = sine(220.0, 19_200, 0.5);
    let phase = TAU * 220.0 * 19_200.0 / 48_000.0;
    signal.extend((0..19_200).map(|i| (phase + TAU * 440.0 * i as f32 / 48_000.0).sin() * 0.5));
    let mut tracker = VoiceTracker::new(48_000);
    let mut count = 0;
    for (i, block) in signal.chunks(512).enumerate() {
        let hz = if i * 512 < 19_200 + 2048 {
            220.0
        } else {
            440.0
        };
        tracker.judge(pitch(Some(hz), 0.9));
        tracker.process(block, |f| {
            if f.voiced && f.at >= 19_200 + 267 {
                count += 1;
                assert!((1200.0 * (f.hz / 440.0).log2()).abs() < 20.0, "{f:?}");
            }
        });
    }
    assert!(count > 250);
}
#[test]
fn three_unvoiced_verdicts_unlock_a_false_pitch() {
    let signal: Vec<_> = (0..8192)
        .map(|i| if i % 48 < 24 { 0.316 } else { -0.316 })
        .collect();
    let mut tracker = VoiceTracker::new(48_000);
    tracker.process(&signal[..4096], |_| {});
    assert!(tracker.cycle.estimate().is_some());
    // The first verdict arrives at 4096; within four judge hops of it the
    // frame is unvoiced, and it stays so while the verdicts keep coming.
    let deadline = 4096 + 4 * 512;
    let mut frames_checked = 0;
    for block in signal[4096..].chunks(512) {
        tracker.judge(pitch(None, 0.02));
        tracker.process(block, |f| {
            if f.at >= deadline {
                frames_checked += 1;
                assert!(!f.voiced && !f.gate, "{f:?}");
            }
        });
    }
    assert!(frames_checked >= 32);
}
#[test]
fn the_gate_has_hysteresis() {
    let mut signal = Vec::new();
    for db in [-45.0_f32, -53.0, -58.0] {
        signal.extend(sine(220.0, 9600, 10.0_f32.powf(db / 20.0)));
    }
    let output = frames(&signal);
    assert!(
        output
            .iter()
            .filter(|f| (2000..9600).contains(&f.at))
            .all(|f| f.gate)
    );
    assert!(
        output
            .iter()
            .filter(|f| (10_000..19_200).contains(&f.at))
            .all(|f| f.gate)
    );
    assert!(
        output
            .iter()
            .filter(|f| f.at >= 19_200 + 1013)
            .all(|f| !f.gate)
    );
}
#[test]
fn a_re_sung_syllable_is_an_onset_and_a_legato_step_is_not() {
    for (name, expected) in [
        ("rearticulate", 5),
        ("steps", 1),
        ("glide", 1),
        ("vibrato", 1),
    ] {
        let output = frames(&take(name).samples);
        assert_eq!(
            output.iter().filter(|f| f.onset).count(),
            expected,
            "{name}"
        );
    }
}
#[test]
fn nonsense_never_leaves_the_tracker() {
    let mut tracker = VoiceTracker::new(48_000);
    tracker.process(&sine(220.0, 4096, 0.5), |_| {});
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1e30] {
        tracker.process(&[bad; 4096], |f| {
            for value in [
                f.hz,
                f.volts,
                f.snapped_hz,
                f.snapped_volts,
                f.loudness,
                f.breath,
                f.confidence,
            ] {
                assert!(value.is_finite());
            }
            assert!(!f.gate && !f.voiced, "{f:?}");
        });
    }
}
#[test]
fn the_judge_thread_comes_and_goes() -> crate::Result<()> {
    fn is_copy<T: Copy>() {}
    is_copy::<JudgeResult>();
    assert!(!std::mem::needs_drop::<JudgeResult>());
    let (mut feed, mut results, judge) = judge::spawn(
        PitchDetectorConfig::for_rate(48_000),
        128,
        OnsetConfig::default(),
    )?;
    let signal = sine(220.0, 48_000, 0.5);
    let mut pitches = 0;
    let mut voiced = 0;
    for hop in signal.chunks(512) {
        for block in hop.chunks(128) {
            feed.push(block);
        }
        if hop.len() < 512 {
            continue;
        }
        // Pace the feed by the judge's own hops, never forever: a judge that
        // stops answering fails here instead of hanging the run.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            match results.pop() {
                Some(JudgeResult::Pitch(p)) => {
                    pitches += 1;
                    if let Some(hz) = p.frequency {
                        voiced += 1;
                        assert!((1200.0 * (hz / 220.0).log2()).abs() < 10.0, "{p:?}");
                    }
                    break;
                }
                Some(JudgeResult::Onset { .. }) => {}
                None => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "no verdict for hop {pitches}"
                    );
                    std::thread::yield_now();
                }
            }
        }
    }
    assert!(voiced >= 80, "{voiced} voiced of {pitches}");
    assert!(pitches >= 80);
    assert_eq!(
        judge.stats(),
        judge::JudgeStatsSnapshot {
            samples_dropped: 0,
            results_dropped: 0
        }
    );
    drop(feed);
    let start = std::time::Instant::now();
    assert!(judge.join().is_ok());
    assert!(start.elapsed() < std::time::Duration::from_millis(100));
    assert!(
        judge::spawn(
            PitchDetectorConfig::for_rate(0),
            128,
            OnsetConfig::default()
        )
        .is_err()
    );
    Ok(())
}
#[test]
fn processing_never_allocates() -> crate::Result<()> {
    let vowel = take("vowel_ah");
    let mut tracker = VoiceTracker::new(48_000);
    let (mut feed, mut results, judge) = judge::spawn(
        PitchDetectorConfig::for_rate(48_000),
        128,
        OnsetConfig::default(),
    )?;
    let before = assert_no_alloc::violation_count();
    // Fixture construction stays outside the callback region.
    let signal: Vec<_> = vowel
        .samples
        .iter()
        .copied()
        .cycle()
        .take(480_000)
        .collect();
    let mut frames = 0_u64;
    assert_no_alloc::assert_no_alloc(|| {
        for (i, block) in signal.chunks(64).enumerate() {
            // Everything the callback does: feed the judge, drain it, hand
            // over its verdicts (stub ones too, so both kinds are always
            // exercised), then track.
            feed.push(block);
            while let Some(result) = results.pop() {
                tracker.judge(result);
            }
            if i % 8 == 0 {
                tracker.judge(pitch(Some(130.81), 0.9));
                tracker.judge(JudgeResult::Onset { strength: 0.5 });
            }
            tracker.process(block, |f| {
                frames += 1;
                std::hint::black_box(f);
            });
        }
        tracker.reset();
    });
    assert_eq!(assert_no_alloc::violation_count(), before);
    assert_eq!(frames, 7500);
    drop(feed);
    drop(results);
    assert!(judge.join().is_ok());
    Ok(())
}

#[test]
fn a_sustained_rise_does_not_reuse_flux_onsets() {
    let mut gate = super::gate::Gate::new(48_000);
    let mut count = u64::from(gate.frame(64, true, -12.0, -30.0).1);
    for frame in 2_u64..100 {
        let at = frame * 64;
        if frame % 8 == 0 {
            gate.flux(at);
        }
        count += u64::from(gate.frame(at, true, -12.0, -20.0).1);
    }
    assert_eq!(count, 1, "a sustained rise is one articulation");
    gate.frame(6400, true, -20.0, -20.0);
    gate.flux(6464);
    assert!(gate.frame(6464, true, -15.0, -20.0).1);
}
#[test]
fn a_hot_input_over_full_scale_is_still_a_voice() {
    // A float input with gain before it can run a few dB over full scale;
    // that is a loud note, not a fault, and it must not reset the tracker.
    let output = frames(&sine(220.0, 24_000, 1.4));
    let after = output.iter().filter(|f| f.at >= 2400);
    let mut count = 0;
    for f in after {
        count += 1;
        assert!(f.gate && f.voiced, "{f:?}");
        assert!((1200.0 * (f.hz / 220.0).log2()).abs() < 10.0, "{f:?}");
    }
    assert!(count > 300);
    let onsets = output.iter().filter(|f| f.onset).count();
    assert_eq!(onsets, 1);
}
#[test]
fn the_rest_of_a_recovery_is_the_same_syllable() {
    // A re-sung syllable fires on its ≥ 9 dB rise part-way up the ramp; the
    // ramp then carries on above the reset valley, and the judge's flux for
    // the same syllable arrives about 30 ms late. None of that is a second
    // syllable once the refractory time is over.
    let mut gate = super::gate::Gate::new(48_000);
    assert!(gate.frame(64, true, -6.0, -70.0).1);
    for frame in 2_u64..50 {
        assert!(!gate.frame(frame * 64, true, -6.0, -6.0).1);
    }
    // The dip, then the first part of the recovery (past the refractory time).
    assert!(!gate.frame(50 * 64, true, -30.0, -30.0).1);
    assert!(gate.frame(51 * 64, true, -20.0, -30.0).1);
    // The valley is reset to the fast level at the onset; the ramp goes on,
    // 5 dB over it, and the flux arrives 22 frames (29 ms) later. Old code
    // paired the two once the refractory time ended, at frame 96.
    let mut onsets = 0;
    for frame in 52_u64..130 {
        let at = frame * 64;
        if frame == 74 {
            gate.flux(at);
        }
        onsets += u32::from(gate.frame(at, true, -6.0, -11.0).1);
    }
    assert_eq!(onsets, 0, "one syllable, one onset");
}
