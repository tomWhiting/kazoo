use crate::analysis::{
    cycle::{CycleTracker, fixtures},
    score::{
        FusedUnderTest, SCORE_STEP, Take, TrackerUnderTest, fused::Recording,
        instrument::instrument_suite, score, signals, table,
    },
};
use crate::{host::live::InputProfile, voice::VoiceTracker};

#[path = "early_measurements.rs"]
mod measurements;

#[test]
fn voice_and_line_are_unchanged() -> crate::Result<()> {
    let takes = signals::suite(fixtures::RATE);
    // Independent takes own their detectors; bounded workers avoid making
    // the complete voice and line matrix one serial test.
    std::thread::scope(|scope| {
        let mut workers = Vec::with_capacity(4);
        for profile in [InputProfile::Voice, InputProfile::Line] {
            for group in takes.chunks(takes.len().div_ceil(2)) {
                workers.push(scope.spawn(move || {
                    for take in group {
                        let start = std::time::Instant::now();
                        compare_unchanged(take, profile)?;
                        println!("{} {profile:?}: {:?}", take.name, start.elapsed());
                    }
                    crate::Result::Ok(())
                }));
            }
        }
        for worker in workers {
            worker.join().expect("comparison worker panicked")?;
        }
        Ok(())
    })
}

fn compare_unchanged(take: &Take, profile: InputProfile) -> crate::Result<()> {
    let settings = profile.tracking();
    assert!(!settings.cycle.early);
    let mut off = settings;
    off.cycle.early = false;
    let mut cycle = CycleTracker::with_settings(take.rate, settings.cycle);
    let mut reference = CycleTracker::with_settings(take.rate, off.cycle);
    for &sample in &take.samples {
        cycle.push(sample);
        reference.push(sample);
        assert_eq!(
            cycle.estimate(),
            reference.estimate(),
            "{} {profile:?}",
            take.name
        );
        assert_eq!(
            cycle.last_clarity().to_bits(),
            reference.last_clarity().to_bits()
        );
        assert_eq!(cycle.marks(), reference.marks());
        assert!(
            cycle
                .estimate()
                .is_none_or(|estimate| !estimate.provisional)
        );
    }
    let mut voice = VoiceTracker::with_settings(take.rate, settings);
    let mut reference = VoiceTracker::with_settings(take.rate, off);
    for block in take.samples.chunks(crate::voice::VOICE_HOP) {
        let mut actual = None;
        let mut expected = None;
        voice.process(block, |frame| actual = Some(frame));
        reference.process(block, |frame| expected = Some(frame));
        assert_eq!(actual, expected, "{} {profile:?}", take.name);
        if let (Some(actual), Some(expected)) = (actual, expected) {
            for (actual, expected) in [
                (actual.hz, expected.hz),
                (actual.volts, expected.volts),
                (actual.snapped_hz, expected.snapped_hz),
                (actual.snapped_volts, expected.snapped_volts),
                (actual.loudness, expected.loudness),
                (actual.breath, expected.breath),
                (actual.confidence, expected.confidence),
            ] {
                assert_eq!(actual.to_bits(), expected.to_bits());
            }
        }
    }
    // Delayed judge verdicts exercise folding and voicing alongside
    // the callback's cycle and gate readings.
    // Early changes only cycle tracking. The judges see identical samples
    // and settings, so compute their verdicts once and replay the same
    // deliveries into an independent hard-off tracker at their sample times.
    let mut fused = FusedUnderTest::with_settings(take.rate, settings)?;
    fused.record();
    let actual = score(&mut fused, take);
    let mut replay = Replay {
        tracker: VoiceTracker::with_settings(take.rate, off),
        recording: fused.finish_recording(),
        latest: None,
        samples: 0,
    };
    let expected = score(&mut replay, take);
    assert!(replay.recording.verdicts.is_empty());
    assert!(replay.recording.frames.is_empty());
    for (actual, expected) in [
        (actual.onset_ms_median, expected.onset_ms_median),
        (actual.onset_ms_max, expected.onset_ms_max),
        (actual.step_ms_median, expected.step_ms_median),
        (actual.step_ms_max, expected.step_ms_max),
        (actual.cents_median, expected.cents_median),
        (actual.cents_p95, expected.cents_p95),
        (actual.octave_pct, expected.octave_pct),
        (actual.gross_pct, expected.gross_pct),
        (actual.missed_pct, expected.missed_pct),
        (actual.false_pct, expected.false_pct),
    ] {
        assert_eq!(
            actual.to_bits(),
            expected.to_bits(),
            "{} {profile:?}",
            take.name
        );
    }
    assert_eq!(actual.onsets_missed, expected.onsets_missed);
    assert_eq!(actual.steps, expected.steps);
    assert_eq!(actual.steps_missed, expected.steps_missed);
    assert_eq!(
        actual.aligned_lag_ms.map(f32::to_bits),
        expected.aligned_lag_ms.map(f32::to_bits)
    );
    assert_eq!(actual.events.len(), expected.events.len());
    for (actual, expected) in actual.events.iter().zip(&expected.events) {
        assert_eq!(actual.at_sample, expected.at_sample);
        assert_eq!(actual.kind, expected.kind);
        assert_eq!(actual.truth_hz.to_bits(), expected.truth_hz.to_bits());
        assert_eq!(
            actual.latency_ms.map(f32::to_bits),
            expected.latency_ms.map(f32::to_bits)
        );
    }
    Ok(())
}

struct Replay {
    tracker: VoiceTracker,
    recording: Recording,
    latest: Option<crate::voice::VoiceFrame>,
    samples: u64,
}

impl TrackerUnderTest for Replay {
    fn name(&self) -> &'static str {
        "voice"
    }

    fn push(&mut self, block: &[f32]) {
        for sample in block {
            while self
                .recording
                .verdicts
                .front()
                .is_some_and(|(at, _)| *at == self.samples)
            {
                let (_, verdict) = self.recording.verdicts.pop_front().expect("due verdict");
                self.tracker.judge(verdict);
            }
            self.tracker.process(std::slice::from_ref(sample), |frame| {
                let actual = self.recording.frames.pop_front().expect("recorded frame");
                assert_eq!(actual, frame);
                for (actual, expected) in [
                    (actual.hz, frame.hz),
                    (actual.volts, frame.volts),
                    (actual.snapped_hz, frame.snapped_hz),
                    (actual.snapped_volts, frame.snapped_volts),
                    (actual.loudness, frame.loudness),
                    (actual.breath, frame.breath),
                    (actual.confidence, frame.confidence),
                ] {
                    assert_eq!(actual.to_bits(), expected.to_bits());
                }
                self.latest = Some(frame);
            });
            self.samples += 1;
        }
    }

    fn hz(&self) -> Option<f32> {
        self.latest
            .filter(|frame| frame.voiced)
            .map(|frame| frame.hz)
    }
}

fn instrument_frames(
    take: &Take,
    profile: InputProfile,
    early: bool,
) -> Vec<crate::voice::VoiceFrame> {
    let mut settings = profile.tracking();
    settings.cycle.early = early;
    let mut tracker = VoiceTracker::with_settings(take.rate, settings);
    let mut output = Vec::new();
    let mut judge = crate::analysis::cycle::ProvisionalJudge::new(take.rate, settings.cycle);
    for &sample in &take.samples {
        if let Some(estimate) = judge.push(sample) {
            tracker.judge(crate::voice::judge::JudgeResult::Provisional(estimate));
        }
        if let Some(pitch) = judge.pitch() {
            tracker.judge(pitch);
        }
        tracker.process(std::slice::from_ref(&sample), |frame| {
            judge.observe(frame);
            output.push(frame);
        });
    }
    output
}

#[test]
fn a_provisional_low_e_is_right() {
    let takes = instrument_suite(fixtures::RATE);
    let take = fixtures::named(&takes, "guitar_open_noise");
    let frames = instrument_frames(take, InputProfile::Guitar, true);
    let provisional: Vec<_> = frames.iter().filter(|frame| frame.provisional).collect();
    assert!(
        !provisional.is_empty(),
        "guitar_open published no provisional readings"
    );
    for frame in provisional {
        let truth = fixtures::truth_at(take, frame.at as usize).expect("provisional note truth");
        let corrected = frames.iter().any(|later| {
            later.at >= frame.at
                && (later.at - frame.at) as f32 <= 2.0 * take.rate as f32 / truth
                && later.voiced
                && !later.provisional
                && fixtures::cents(later.hz, truth) <= 50.0
        });
        assert!(
            fixtures::cents(frame.hz, truth) <= 50.0 || corrected,
            "{frame:?}, truth {truth}"
        );
    }
}

fn check_instrument_frames(
    take: &Take,
    profile: InputProfile,
    early: bool,
    failures: &mut Vec<String>,
) {
    let frames = instrument_frames(take, profile, early);
    let mut errors = Vec::new();
    let mut octave_frames = 0;
    let mut voiced_frames = 0;
    for (note, &onset) in take.onsets.iter().enumerate() {
        let end = fixtures::note_end(take, note);
        let truth = fixtures::truth_at(take, onset + SCORE_STEP).expect("note truth");
        let first = frames.iter().find(|frame| {
            frame.at >= onset as u64
                && frame.at < end as u64
                && frame.voiced
                && frame.gate
                && fixtures::cents(frame.hz, truth) <= 50.0
        });
        let budget = if take.name.ends_with("_noise") {
            2500.0
        } else {
            1500.0
        } / truth
            + 2.0;
        let latency = first.map(|frame| (frame.at - onset as u64) as f32 / 48.0);
        if early && latency.is_none_or(|latency| latency > budget) {
            failures.push(format!(
                "{} note {note}: {latency:?} ms, budget {budget} ms",
                take.name
            ));
        }
        println!(
            "{} early={early} note={note} hz={truth:.5} onset_ms={latency:?} budget_ms={budget:.5}",
            take.name
        );
        let mut settled = Vec::new();
        for frame in frames
            .iter()
            .filter(|frame| frame.at >= onset as u64 && frame.at < end as u64 && frame.voiced)
        {
            voiced_frames += 1;
            octave_frames += usize::from(fixtures::octave_error(frame.hz, truth));
            if frame.at >= onset as u64 + 4800 {
                settled.push(fixtures::cents(frame.hz, truth));
            }
        }
        if early && settled.is_empty() {
            failures.push(format!(
                "{} note {note}: no voiced frames after 100 ms",
                take.name
            ));
        }
        settled.sort_by(f32::total_cmp);
        if early
            && settled
                .get(settled.len() / 2)
                .is_none_or(|&median| median > 5.0)
        {
            failures.push(format!(
                "{} note {note}: settled median exceeds 5 cents",
                take.name
            ));
        }
        errors.extend(settled);
    }
    errors.sort_by(f32::total_cmp);
    let median = errors.get(errors.len() / 2).copied();
    println!(
        "{} early={early}: settled cents median={median:?}, octave frames={octave_frames}/{voiced_frames}",
        take.name
    );
    if early && (voiced_frames == 0 || octave_frames * 100 > voiced_frames * 2) {
        failures.push(format!(
            "{}: octave frames {octave_frames}/{voiced_frames}",
            take.name
        ));
    }
}

#[test]
fn early_meets_its_targets() -> crate::Result<()> {
    assert!(InputProfile::Bass.tracking().cycle.early);
    assert!(InputProfile::Guitar.tracking().cycle.early);
    let takes = instrument_suite(fixtures::RATE);
    let mut rows = Vec::new();
    let mut failures = Vec::new();
    for (take_name, profile) in [
        ("bass_open", InputProfile::Bass),
        ("guitar_open", InputProfile::Guitar),
        ("bass_open_noise", InputProfile::Bass),
        ("guitar_open_noise", InputProfile::Guitar),
    ] {
        let take = fixtures::named(&takes, take_name);
        for early in [false, true] {
            let mut settings = profile.tracking();
            settings.cycle.early = early;
            let result = score(
                &mut FusedUnderTest::with_settings(take.rate, settings)?,
                take,
            );
            if early {
                assert_eq!(
                    result.events.len(),
                    take.onsets.len(),
                    "one event per pluck"
                );
                for event in &result.events {
                    let budget = if take_name.ends_with("_noise") {
                        2500.0
                    } else {
                        1500.0
                    } / event.truth_hz
                        + 2.0;
                    if event.latency_ms.is_none_or(|latency| latency > budget) {
                        failures.push(format!(
                            "{take_name} fused event {}: {:?} ms, budget {budget} ms",
                            event.at_sample, event.latency_ms
                        ));
                    }
                }
                if result.octave_pct > 2.0 {
                    failures.push(format!("{take_name} fused octaves {}%", result.octave_pct));
                }
            }
            rows.push((
                if early { "early on" } else { "early off" },
                take_name,
                result,
            ));
            check_instrument_frames(take, profile, early, &mut failures);
        }
    }
    println!("{}", table(&rows));
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    Ok(())
}
