use crate::analysis::{
    cycle::{CycleSettings, CycleTracker},
    score::{SCORE_STEP, Take, instrument::instrument_suite, signals},
};
use crate::host::live::InputProfile;
use crate::voice::{
    TrackSettings, VoiceFrame, VoiceTracker,
    gate::{GATE_OFF_DB, GATE_ON_DB, ONSET_RISE_DB},
    judge::JudgeResult,
};

const RATE: u32 = 48_000;

fn named<'a>(takes: &'a [Take], name: &str) -> &'a Take {
    takes
        .iter()
        .find(|take| take.name == name)
        .expect("instrument take")
}

fn frames(take: &Take, profile: InputProfile) -> Vec<VoiceFrame> {
    let mut tracker = VoiceTracker::with_settings(take.rate, profile.tracking());
    let mut output = Vec::with_capacity(take.samples.len() / 64);
    tracker.process(&take.samples, |frame| output.push(frame));
    output
}

fn cents(hz: f32, truth: f32) -> f32 {
    (1200.0 * (hz / truth).log2()).abs()
}

fn truth_at(take: &Take, sample: usize) -> Option<f32> {
    let position = sample.saturating_sub(SCORE_STEP / 2);
    let before = position / SCORE_STEP;
    let low = take.truth.get(before).copied().flatten()?;
    let high = take.truth.get(before + 1).copied().flatten().unwrap_or(low);
    let fraction = (position % SCORE_STEP) as f32 / SCORE_STEP as f32;
    Some(
        low.log2()
            .mul_add(1.0 - fraction, high.log2() * fraction)
            .exp2(),
    )
}

fn octave_error(hz: f32, truth: f32) -> bool {
    let difference = 1200.0 * (hz / truth).log2();
    let octaves = (difference / 1200.0).round();
    octaves != 0.0 && 1200.0_f32.mul_add(-octaves, difference).abs() <= 100.0
}

#[test]
fn voice_is_track_v_exactly() {
    assert_eq!(
        CycleSettings::VOICE,
        CycleSettings {
            min_hz: 50.0,
            max_hz: 4000.0,
            dc_hz: 30.0,
            peak_release_ms: 30.0,
            early: false,
        }
    );
    assert_eq!(
        TrackSettings::VOICE,
        TrackSettings {
            cycle: CycleSettings::VOICE,
            judge_frame_48k: 2048,
            gate_on_db: -50.0,
            gate_off_db: -56.0,
            onset_rise_db: 9.0,
            jump_onset_cents: None,
        }
    );
    assert_eq!(InputProfile::Voice.tracking(), TrackSettings::VOICE);
    assert!((TrackSettings::VOICE.gate_on_db - GATE_ON_DB).abs() < f32::EPSILON);
    assert!((TrackSettings::VOICE.gate_off_db - GATE_OFF_DB).abs() < f32::EPSILON);
    assert!((TrackSettings::VOICE.onset_rise_db - ONSET_RISE_DB).abs() < f32::EPSILON);
    for take in signals::suite(RATE) {
        let mut original = VoiceTracker::new(RATE);
        let mut explicit = VoiceTracker::with_settings(RATE, InputProfile::Voice.tracking());
        let mut original_cycle = CycleTracker::new(RATE);
        let mut explicit_cycle = CycleTracker::with_settings(RATE, CycleSettings::VOICE);
        for block in take.samples.chunks(64) {
            let mut first = None;
            let mut second = None;
            original.process(block, |frame| first = Some(frame));
            explicit.process(block, |frame| second = Some(frame));
            assert_eq!(first, second, "{}", take.name);
            original_cycle.process(block);
            explicit_cycle.process(block);
            assert_eq!(
                original_cycle.estimate(),
                explicit_cycle.estimate(),
                "{}",
                take.name
            );
        }
    }
}

#[test]
fn profiles_use_the_declared_ranges_and_times() {
    for (profile, dc, release, frame, on, off, rise, jump) in [
        (
            InputProfile::Voice,
            30.0,
            30.0,
            2048,
            -50.0,
            -56.0,
            9.0,
            None,
        ),
        (
            InputProfile::Guitar,
            20.0,
            30.0,
            2048,
            -48.0,
            -54.0,
            6.0,
            Some(80.0),
        ),
        (
            InputProfile::Bass,
            10.0,
            80.0,
            4096,
            -48.0,
            -54.0,
            6.0,
            Some(80.0),
        ),
        (
            InputProfile::Line,
            10.0,
            80.0,
            4096,
            -50.0,
            -56.0,
            9.0,
            None,
        ),
    ] {
        let settings = profile.tracking();
        let (min_hz, max_hz) = profile.range_hz();
        assert_eq!(
            settings,
            TrackSettings {
                cycle: CycleSettings {
                    min_hz,
                    max_hz,
                    dc_hz: dc,
                    peak_release_ms: release,
                    early: matches!(profile, InputProfile::Guitar | InputProfile::Bass)
                },
                judge_frame_48k: frame,
                gate_on_db: on,
                gate_off_db: off,
                onset_rise_db: rise,
                jump_onset_cents: jump,
            }
        );
        for (rate, scale) in [(44_100, 1), (48_000, 1), (96_000, 2), (192_000, 4)] {
            let config = settings.judge_config(rate);
            assert_eq!(config.frame_length, frame * scale);
            assert_eq!(config.hop_length, 512 * scale);
            assert!((config.min_frequency - min_hz).abs() < f32::EPSILON);
            assert!((config.max_frequency - max_hz).abs() < f32::EPSILON);
        }
    }
}

#[test]
fn the_bass_profile_hears_low_b() {
    let takes = instrument_suite(RATE);
    let take = named(&takes, "bass_open");
    let output = frames(take, InputProfile::Bass);
    let mut octaves = 0;
    let mut voiced = 0;
    for (note, &onset) in take.onsets.iter().enumerate() {
        let end = take
            .onsets
            .get(note + 1)
            .copied()
            .unwrap_or(take.samples.len() - 9600);
        let hz = truth_at(take, onset + SCORE_STEP).expect("note truth");
        let first = output.iter().find(|frame| {
            frame.at >= onset as u64
                && frame.at < end as u64
                && frame.voiced
                && cents(frame.hz, hz) <= 50.0
        });
        assert!(
            first.is_some_and(|frame| (frame.at - onset as u64) as f32 / 48.0 <= 2000.0 / hz + 1.0),
            "note {hz}: first {first:?}"
        );
        let mut settled = Vec::new();
        let mut expected = 0;
        for frame in output
            .iter()
            .filter(|frame| frame.at >= onset as u64 && frame.at < end as u64)
        {
            let age = frame.at - onset as u64;
            if age >= 9600 {
                expected += 1;
            }
            if frame.voiced {
                voiced += 1;
                let octave = octave_error(frame.hz, hz);
                octaves += usize::from(octave);
                assert!(age < 4800 || !octave, "settled octave: {hz}, {frame:?}");
                if age >= 9600 {
                    settled.push(cents(frame.hz, hz));
                }
            }
        }
        assert!(settled.len() * 10 >= expected * 9 && !settled.is_empty());
        settled.sort_by(f32::total_cmp);
        assert!(settled[settled.len() / 2] <= 5.0, "note {hz}: {settled:?}");
    }
    assert!(
        voiced > 0 && octaves * 100 <= voiced * 2,
        "{octaves}/{voiced}"
    );
}

#[test]
fn the_voice_profile_does_not_hear_low_b() {
    let takes = instrument_suite(RATE);
    let take = named(&takes, "bass_open");
    let hz = truth_at(take, take.onsets[0] + SCORE_STEP).expect("low B truth");
    let output = frames(take, InputProfile::Voice);
    let low_b: Vec<_> = output
        .iter()
        .filter(|frame| frame.at >= take.onsets[0] as u64 && frame.at < take.onsets[1] as u64)
        .collect();
    assert!(!low_b.is_empty());
    assert!(
        low_b
            .iter()
            .all(|frame| !frame.voiced || cents(frame.hz, hz) > 50.0)
    );
}

#[test]
fn a_sweep_is_one_onset_per_note() {
    let takes = instrument_suite(RATE);
    let take = named(&takes, "sweep");
    let output = frames(take, InputProfile::Guitar);
    let onsets: Vec<_> = output.iter().filter(|frame| frame.onset).collect();
    assert_eq!(onsets.len(), 6, "{onsets:?}");
    for (frame, &pluck) in onsets.iter().zip(&take.onsets) {
        let hz = truth_at(take, pluck + SCORE_STEP).expect("pluck truth");
        assert!(
            frame.at >= pluck as u64
                && (frame.at - pluck as u64) as f32 / 48.0 <= 2000.0 / hz + 3.0,
            "pluck {pluck}, {hz} Hz: {frame:?}"
        );
    }
}

#[test]
fn slides_bends_and_vibrato_are_not_onsets() {
    let takes = instrument_suite(RATE);
    for profile in [InputProfile::Guitar, InputProfile::Bass] {
        for name in ["slide", "bend", "vibrato", "glide"] {
            let take = named(&takes, name);
            let output = frames(take, profile);
            let onsets: Vec<_> = output.iter().filter(|frame| frame.onset).collect();
            assert_eq!(onsets.len(), 1, "{profile:?} {name}: {onsets:?}");
            let pluck = take.onsets[0];
            let hz = truth_at(take, pluck + SCORE_STEP).expect("first pluck truth");
            let latency_ms = (onsets[0].at - pluck as u64) as f32 / 48.0;
            let limit_ms = 2000.0 / hz + 3.0;
            println!(
                "{profile:?} {name}: onsets={} onset_ms={latency_ms:.6} limit_ms={limit_ms:.6}",
                onsets.len()
            );
            assert!(
                onsets[0].at >= pluck as u64 && latency_ms <= limit_ms,
                "{profile:?} {name}: onset {latency_ms} ms, limit {limit_ms} ms"
            );
        }
    }
}

fn within_one_period(take: &Take, frame: VoiceFrame, limit: f32) {
    let at = usize::try_from(frame.at).expect("fixture frame time");
    let truth = truth_at(take, at).expect("voiced truth");
    let period = (take.rate as f32 / truth).ceil() as usize;
    let error = (at.saturating_sub(period)..=at)
        .filter_map(|sample| truth_at(take, sample))
        .map(|truth| cents(frame.hz, truth))
        .fold(f32::INFINITY, f32::min);
    assert!(error <= limit, "{}: {frame:?}, error {error}", take.name);
}

#[test]
fn the_frames_follow_a_glide_and_a_vibrato_without_steps() {
    let takes = instrument_suite(RATE);
    for name in ["glide", "vibrato"] {
        let take = named(&takes, name);
        let output = frames(take, InputProfile::Guitar);
        let mut count = 0;
        let mut previous: Option<VoiceFrame> = None;
        for frame in output {
            let at = usize::try_from(frame.at).expect("fixture frame time");
            if frame.voiced && truth_at(take, at).is_some() {
                count += 1;
                within_one_period(take, frame, 5.0);
                if let Some(before) = previous {
                    let old = truth_at(take, before.at as usize).expect("previous truth");
                    let now = truth_at(take, at).expect("current truth");
                    assert!(
                        cents(frame.hz, before.hz) <= cents(now, old) + 2.0,
                        "{name}: {before:?} -> {frame:?}"
                    );
                }
                previous = Some(frame);
            } else {
                previous = None;
            }
        }
        assert!(count > 500, "{name}: {count} voiced frames");
    }
}

#[test]
fn slides_are_continuous() {
    let takes = instrument_suite(RATE);
    let take = named(&takes, "slide");
    let output = frames(take, InputProfile::Guitar);
    let mut voiced = 0;
    for frame in output {
        assert_eq!(frame.snapped_hz.to_bits(), frame.hz.to_bits());
        assert_eq!(frame.snapped_volts.to_bits(), frame.volts.to_bits());
        if frame.voiced && truth_at(take, frame.at as usize).is_some() {
            voiced += 1;
            within_one_period(take, frame, 10.0);
        }
    }
    assert!(voiced > 500);
}

#[test]
fn hum_is_not_a_note() {
    let takes = instrument_suite(RATE);
    let take = named(&takes, "noise_hum");
    for profile in InputProfile::ALL {
        let output = frames(take, profile);
        let high = output.iter().filter(|frame| frame.gate).count();
        assert!(
            !output.is_empty() && high * 100 <= output.len(),
            "{profile:?}: {high}/{}",
            output.len()
        );
    }
}

#[test]
fn nonsense_never_leaves_the_tracker() {
    let takes = instrument_suite(RATE);
    let signal = &named(&takes, "vibrato").samples[9600..13_696];
    for profile in InputProfile::ALL {
        let mut tracker = VoiceTracker::with_settings(RATE, profile.tracking());
        tracker.process(signal, |_| {});
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1e30] {
            tracker.judge(JudgeResult::Pitch(crate::analysis::PitchEstimate {
                frequency: Some(bad),
                voiced_probability: bad,
                midi_note: None,
            }));
            tracker.judge(JudgeResult::Onset { strength: bad });
            tracker.process(&[bad; 4096], |frame| {
                for value in [
                    frame.hz,
                    frame.volts,
                    frame.snapped_hz,
                    frame.snapped_volts,
                    frame.loudness,
                    frame.breath,
                    frame.confidence,
                ] {
                    assert!(value.is_finite(), "{profile:?}: {frame:?}");
                }
                assert!(!frame.gate && !frame.voiced, "{profile:?}: {frame:?}");
            });
        }
    }
}

#[test]
fn processing_never_allocates_with_any_profile() {
    let takes = instrument_suite(RATE);
    for name in ["bass_open", "sweep"] {
        let signal: Vec<_> = named(&takes, name)
            .samples
            .iter()
            .copied()
            .cycle()
            .take(480_000)
            .collect();
        for profile in InputProfile::ALL {
            let mut tracker = VoiceTracker::with_settings(RATE, profile.tracking());
            let mut cycle = CycleTracker::with_settings(RATE, profile.tracking().cycle);
            let before = assert_no_alloc::violation_count();
            let mut count = 0;
            assert_no_alloc::assert_no_alloc(|| {
                for block in signal.chunks(64) {
                    tracker.judge(JudgeResult::Pitch(crate::analysis::PitchEstimate {
                        frequency: Some(110.0),
                        voiced_probability: 0.9,
                        midi_note: None,
                    }));
                    tracker.judge(JudgeResult::Onset { strength: 0.5 });
                    tracker.process(block, |frame| {
                        count += 1;
                        std::hint::black_box(frame);
                    });
                    cycle.process(block);
                }
                tracker.reset();
                cycle.reset();
            });
            assert_eq!(
                assert_no_alloc::violation_count(),
                before,
                "{profile:?} {name}"
            );
            assert_eq!(count, 7500);
        }
    }
}

#[test]
fn jumps_need_voiced_history_and_obey_the_refractory_time() {
    use crate::voice::gate::Gate;
    let mut gate = Gate::with_settings(RATE, InputProfile::Guitar.tracking());
    assert!(gate.frame_with_pitch(64, true, -12.0, -12.0, 110.0).1);
    for hop in 2_u64..=48 {
        assert!(!gate.frame_with_pitch(hop * 64, true, -12.0, -12.0, 110.0).1);
    }
    assert!(gate.frame_with_pitch(49 * 64, true, -12.0, -12.0, 130.81).1);
    assert!(!gate.frame_with_pitch(50 * 64, true, -12.0, -12.0, 146.83).1);
    for hop in 51_u64..=96 {
        assert!(
            !gate
                .frame_with_pitch(hop * 64, true, -12.0, -12.0, 146.83)
                .1
        );
    }
    assert!(!gate.frame_with_pitch(97 * 64, false, -12.0, -12.0, 0.0).1);
    assert!(gate.frame_with_pitch(98 * 64, true, -12.0, -12.0, 196.0).1);
    gate.reset();
    assert!(gate.frame_with_pitch(64, true, -12.0, -12.0, 110.0).1);
    for hop in 2_u64..=48 {
        gate.frame_with_pitch(hop * 64, true, -12.0, -12.0, 110.0);
    }
    assert!(!gate.frame_with_pitch(49 * 64, false, -12.0, -12.0, 0.0).1);
    assert!(!gate.frame_with_pitch(50 * 64, true, -12.0, -12.0, 110.0).1);
    assert!(!gate.frame_with_pitch(51 * 64, true, -12.0, -12.0, 196.0).1);
}

#[test]
fn dc_does_not_open_a_quiet_instrument() {
    let signal: Vec<_> = (0..48_000)
        .map(|sample| {
            10.0_f32.powf(-55.0 / 20.0).mul_add(
                (std::f32::consts::TAU * 110.0 * sample as f32 / 48_000.0).sin(),
                0.2,
            )
        })
        .collect();
    for profile in [InputProfile::Guitar, InputProfile::Bass] {
        let mut tracker = VoiceTracker::with_settings(RATE, profile.tracking());
        let mut high = 0;
        tracker.process(&signal, |frame| high += usize::from(frame.gate));
        assert_eq!(high, 0, "{profile:?}: DC must not supply the opening level");
    }
}
