use crate::analysis::{cycle::CycleTracker, score::signals};
use std::f32::consts::{FRAC_PI_2, PI, TAU};

use super::{
    CycleEstimate, CycleSettings,
    early::{EARLY_CLARITY, EARLY_LAGS},
    fixtures,
};
use crate::{
    analysis::score::instrument::instrument_suite, host::live::InputProfile, voice::VoiceTracker,
};

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

#[test]
fn a_hint_overrides_a_reading_an_octave_up() {
    // A weak fundamental under a loud second harmonic: the NSDF at half the
    // period is about 0.92, past the half check, so the tracker reads 300 Hz.
    let signal: Vec<_> = (0..9_600)
        .map(|i| {
            let phase = TAU * 150.0 * i as f32 / 48_000.0;
            0.2_f32.mul_add(phase.sin(), (2.0 * phase).sin()) * 0.4
        })
        .collect();
    let mut tracker = CycleTracker::new(48_000);
    tracker.process(&signal[..4_800]);
    // An octave-up reading is of a lag that is not the signal's period, so it
    // wanders around 300 Hz with the window's phase; what matters is that the
    // tracker reads the octave above, not 150 Hz.
    assert!(
        tracker.estimate().is_some_and(|e| e.hz > 250.0),
        "{:?}",
        tracker.estimate()
    );
    tracker.hint(48_000.0 / 150.0);
    let before = tracker.marks();
    let mut corrected = false;
    for sample in &signal[4_800..] {
        tracker.push(*sample);
        corrected |= tracker.estimate().is_some_and(|e| cents(e.hz, 150.0) < 3.0);
        if tracker.marks() >= before + 4 {
            break;
        }
    }
    assert!(corrected, "{:?}", tracker.estimate());
}

#[test]
fn a_low_hum_stays_locked_once_found() {
    let take = named("hum_low");
    let mut tracker = CycleTracker::new(48_000);
    let mut first = None;
    let mut lost = 0;
    for (i, sample) in take.samples[9_600..38_400].iter().enumerate() {
        tracker.push(*sample);
        match tracker.estimate() {
            Some(estimate) => {
                first.get_or_insert(i);
                assert!(cents(estimate.hz, 65.406) <= 10.0, "{i}: {estimate:?}");
            }
            None if first.is_some() => lost += 1,
            None => {}
        }
    }
    assert!(first.is_some(), "never locked");
    assert_eq!(
        lost, 0,
        "unlocked for {lost} samples after locking at {first:?}"
    );
}

#[test]
fn a_candidate_off_its_peak_is_climbed_to_the_peak() {
    // 220 Hz is 218.18 samples at 48 kHz; a candidate 11 samples short sits
    // on the rising flank of the NSDF, where the three lags all climb.
    let mut tracker = CycleTracker::new(48_000);
    tracker.process(&sine(220.0, 48_000, 4_800, 0.0));
    let refined = tracker.refine(207.0);
    assert!(
        refined.is_none_or(|e| (e.period - 48_000.0 / 220.0).abs() < 0.05),
        "{refined:?}"
    );
}

#[test]
fn a_fading_note_never_reads_an_octave_down() {
    // Syllables dip 24 dB in 10 ms: candidates compared over different
    // windows let the longer lag, read further from the fade, win.
    let take = named("rearticulate");
    let mut tracker = CycleTracker::new(48_000);
    for (i, sample) in take.samples.iter().enumerate() {
        tracker.push(*sample);
        if let Some(estimate) = tracker.estimate() {
            assert!(cents(estimate.hz, 220.0) <= 100.0, "{i}: {estimate:?}");
        }
    }
}

#[test]
fn a_semitone_step_is_never_carried_past_its_note() {
    // A step's transitional readings must not be taken for a glide and
    // extrapolated: nothing reads beyond either note by more than 20 cents.
    for (old, new) in [(220.0, 233.082), (233.082, 220.0)] {
        let mut tracker = CycleTracker::new(48_000);
        tracker.process(&sine(old, 48_000, 9_600, 0.0));
        let phase = TAU * old * 9_600.0 / 48_000.0;
        let low = f32::min(old, new) * (-20.0_f32 / 1200.0).exp2();
        let high = f32::max(old, new) * (20.0_f32 / 1200.0).exp2();
        for (i, sample) in sine(new, 48_000, 9_600, phase).into_iter().enumerate() {
            tracker.push(sample);
            if let Some(estimate) = tracker.estimate() {
                assert!(
                    (low..=high).contains(&estimate.hz),
                    "{old} -> {new}, {i}: {estimate:?}"
                );
            }
        }
    }
}

fn early_readings(
    take: &crate::analysis::score::Take,
    profile: InputProfile,
) -> Vec<(usize, CycleEstimate)> {
    let mut tracker = CycleTracker::with_settings(take.rate, profile.tracking().cycle);
    let mut output = Vec::new();
    for (index, &sample) in take.samples.iter().enumerate() {
        tracker.push(sample);
        if (index + 1) % crate::voice::VOICE_HOP == 0 {
            if let Some(estimate) = tracker.estimate() {
                output.push((index + 1, estimate));
            }
        }
    }
    output
}

#[test]
fn a_low_b_speaks_in_a_period_and_a_half() {
    assert!(InputProfile::Bass.tracking().cycle.early);
    let takes = instrument_suite(fixtures::RATE);
    let take = fixtures::named(&takes, "bass_open");
    for (note, limit_ms) in [(0, 50.6), (1, 38.4)] {
        let onset = take.onsets[note];
        let hz = fixtures::truth_at(take, onset + 32).expect("bass pitch");
        let signal = &take.samples[onset..fixtures::note_end(take, note)];
        let mut early = InputProfile::Bass.tracking();
        early.cycle.early = true;
        let mut normal = early;
        normal.cycle.early = false;
        for (settings, enabled) in [(early, true), (normal, false)] {
            let mut tracker = VoiceTracker::with_settings(take.rate, settings);
            let mut cycle = CycleTracker::with_settings(take.rate, settings.cycle);
            let mut first = None;
            let mut provisional = false;
            let mut normal_first = None;
            for (index, &sample) in signal.iter().enumerate() {
                cycle.push(sample);
                if let Some(estimate) = cycle.estimate() {
                    provisional |= estimate.provisional;
                    if cents(estimate.hz, hz) <= 50.0 {
                        normal_first.get_or_insert(index + 1);
                    }
                }
                tracker.process(std::slice::from_ref(&sample), |frame| {
                    if let Some(estimate) = cycle.estimate().filter(|estimate| estimate.provisional)
                    {
                        assert!(frame.voiced);
                        assert_eq!(frame.confidence.to_bits(), estimate.clarity.to_bits());
                        assert_eq!(frame.hz.to_bits(), estimate.hz.to_bits());
                    }
                    if frame.voiced && frame.gate && cents(frame.hz, hz) <= 50.0 {
                        first.get_or_insert(frame);
                    }
                });
            }
            let frame = first.expect("the bass speaks");
            let latency_ms = frame.at as f32 / 48.0;
            if enabled {
                assert!(
                    latency_ms <= limit_ms,
                    "{hz} Hz: {latency_ms} ms, {frame:?}"
                );
                assert!(provisional, "{hz} Hz never emitted an early estimate");
            } else {
                assert!(!provisional);
                assert!(
                    normal_first.expect("normal pitch") as f32 >= 2.0 * 48_000.0 / hz,
                    "{hz} Hz normal path arrived before two periods"
                );
            }
        }
    }
}

#[test]
fn the_early_estimate_is_rarely_wrong() {
    let takes = instrument_suite(fixtures::RATE);
    let mut notes = 0;
    let mut early_notes = 0;
    let mut octaves = 0;
    let mut jumps = 0;
    for (name, profile) in [
        ("bass_open", InputProfile::Bass),
        ("guitar_open", InputProfile::Guitar),
    ] {
        let original = fixtures::named(&takes, name);
        for position in [0.25, 0.35, 0.5] {
            let take = fixtures::at_position(original, position);
            for (note, &onset) in take.onsets.iter().enumerate() {
                notes += 1;
                let mut tracker = CycleTracker::with_settings(take.rate, profile.tracking().cycle);
                let mut provisional = None;
                let mut handover = None;
                let mut octave = false;
                let truth = fixtures::truth_at(&take, onset + 32).expect("note pitch");
                for &sample in &take.samples[onset..fixtures::note_end(&take, note)] {
                    tracker.push(sample);
                    if let Some(estimate) = tracker.estimate() {
                        if estimate.provisional {
                            provisional.get_or_insert(estimate);
                            octave |= fixtures::octave_error(estimate.hz, truth);
                        } else if provisional.is_some() {
                            handover.get_or_insert(estimate);
                        }
                    }
                }
                if let Some(early) = provisional {
                    early_notes += 1;
                    let confirmed = handover.expect("normal marks confirm the early pitch");
                    jumps += usize::from(cents(confirmed.hz, early.hz) > 30.0);
                }
                octaves += usize::from(octave);
            }
        }
    }
    assert_eq!(notes, 33);
    assert!(early_notes > 0, "early coverage {early_notes}/{notes}");
    assert!(
        octaves * 100 <= notes * 2,
        "octave errors {octaves}/{notes}"
    );
    assert!(jumps * 100 <= notes * 5, "handover jumps {jumps}/{notes}");
}

#[test]
fn a_refused_candidate_costs_nothing() {
    for polarity in [-1.0, 1.0] {
        let signal: Vec<_> = (0..24_000)
            .map(|index| {
                let phase = (31.0 * f64::from(index) / 48_000.0).fract();
                polarity * if phase < 0.3 { 0.5 } else { -0.5 }
            })
            .collect();
        let settings = InputProfile::Bass.tracking().cycle;
        let mut early = CycleTracker::with_settings(fixtures::RATE, settings);
        let mut normal = CycleTracker::with_settings(
            fixtures::RATE,
            CycleSettings {
                early: false,
                ..settings
            },
        );
        let mut first_early = None;
        let mut first_normal = None;
        let mut probes_checked = false;
        let mut first_lobe = false;
        let mut range_checked = false;
        for (index, &sample) in signal.iter().enumerate() {
            // Check the lobe's NSDF at its deadline even when a normal mark
            // has already locked and prevents the early search from running.
            let candidate = early.early.pending;
            let previous = early.previous;
            early.push(sample);
            normal.push(sample);
            if !first_lobe && previous < 0.0 && early.previous >= 0.0 {
                first_lobe = true;
                let period = (2.0 * (early.last_rise - early.last_fall)) as f32;
                if !(early.min_period..=early.max_period).contains(&period) {
                    assert!(early.early.pending.is_none());
                    range_checked = true;
                }
            }
            if !probes_checked
                && candidate.is_some_and(|candidate| early.time - 1.0 >= candidate.deadline)
            {
                let candidate = candidate.expect("an armed lobe schedules a candidate");
                let window = super::lag_from(f64::from(candidate.period / 2.0)).expect("window");
                for factor in EARLY_LAGS {
                    let lag = super::lag_from(f64::from(candidate.period * factor)).expect("lag");
                    for nearby in [lag - 1, lag, lag + 1] {
                        assert!(
                            early.nsdf(nearby, window) < EARLY_CLARITY,
                            "square construction: polarity {polarity}, lag {nearby}"
                        );
                    }
                }
                probes_checked = true;
            }
            assert!(
                early
                    .estimate()
                    .is_none_or(|estimate| !estimate.provisional)
            );
            assert_eq!(
                early.estimate(),
                normal.estimate(),
                "sample {index}, polarity {polarity}"
            );
            if early.estimate().is_some() {
                first_early.get_or_insert(index);
            }
            if normal.estimate().is_some() {
                first_normal.get_or_insert(index);
            }
        }
        if polarity < 0.0 {
            assert!(
                probes_checked,
                "the in-range lobe's NSDF refusal was exercised"
            );
            assert!(!range_checked);
        } else {
            assert!(range_checked, "the out-of-range lobe was rejected");
            assert!(!probes_checked);
        }
        assert!(first_normal.is_some(), "normal marks eventually lock");
        assert_eq!(first_early, first_normal);
    }
}

#[test]
fn a_note_that_starts_mid_glide() {
    let takes = instrument_suite(fixtures::RATE);
    let take = fixtures::moving(fixtures::named(&takes, "bass_open"), |time| {
        41.203_445 * (500.0 * time as f32 / 2.0 / 1200.0).exp2()
    });
    let output = early_readings(&take, InputProfile::Bass);
    let &(at, estimate) = output
        .iter()
        .find(|(_, estimate)| estimate.provisional)
        .expect("early glide pitch");
    let truth = fixtures::truth_at(&take, at).expect("glide truth");
    assert!(
        cents(estimate.hz, truth) <= 30.0,
        "{at}: {estimate:?}, truth {truth}"
    );
    let mut steps = 0;
    for pair in output.windows(2) {
        let [(before, low), (after, high)] = pair else {
            panic!("two readings")
        };
        if *before >= at && *after < take.samples.len() - fixtures::RATE as usize / 5 {
            let low_truth = fixtures::truth_at(&take, *before).expect("previous truth");
            let high_truth = fixtures::truth_at(&take, *after).expect("next truth");
            let allowed = cents(high_truth, low_truth) + 2.0;
            assert!(
                cents(high.hz, low.hz) <= allowed,
                "{before}->{after}: {low:?} -> {high:?}, limit {allowed}"
            );
            steps += 1;
        }
    }
    assert!(steps > 1000, "the later glide remains voiced");
}

#[test]
fn a_vibrato_note_stays_smooth_through_the_handover() {
    let takes = instrument_suite(fixtures::RATE);
    let take = fixtures::moving(fixtures::named(&takes, "bass_open"), |time| {
        41.203_445 * (30.0 * (std::f64::consts::TAU * 6.0 * time).sin() as f32 / 1200.0).exp2()
    });
    let output = early_readings(&take, InputProfile::Bass);
    let mut handovers = 0;
    for pair in output.windows(2) {
        let [(_, low), (_, high)] = pair else {
            panic!("two readings")
        };
        if low.provisional && !high.provisional {
            assert!(cents(high.hz, low.hz) <= 10.0, "{low:?} -> {high:?}");
            handovers += 1;
        }
    }
    assert_eq!(handovers, 1, "exactly one provisional-to-normal handover");
}

#[test]
fn the_early_path_never_allocates() {
    let takes = instrument_suite(fixtures::RATE);
    let take = fixtures::named(&takes, "bass_open");
    let signal: Vec<_> = take.samples.iter().copied().cycle().take(480_000).collect();
    let mut tracker =
        CycleTracker::with_settings(fixtures::RATE, InputProfile::Bass.tracking().cycle);
    let mut voice = VoiceTracker::with_settings(fixtures::RATE, InputProfile::Bass.tracking());
    let before = assert_no_alloc::violation_count();
    let mut tried = false;
    assert_no_alloc::assert_no_alloc(|| {
        for block in signal.chunks(64) {
            tracker.process(block);
            tried |= tracker.early.tried;
            voice.process(block, |frame| {
                std::hint::black_box(frame);
            });
        }
        tracker.reset();
        voice.reset();
    });
    assert!(
        tried,
        "the early deadline was evaluated inside the allocation guard"
    );
    assert_eq!(assert_no_alloc::violation_count(), before);
}
