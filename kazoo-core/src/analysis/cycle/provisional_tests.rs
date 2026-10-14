use super::ProvisionalJudge;
use crate::{analysis::score::instrument::instrument_suite, host::live::InputProfile};

#[test]
fn a_repluck_discards_the_previous_search_history() {
    let takes = instrument_suite(48_000);
    let take = takes
        .iter()
        .find(|take| take.name == "bass_open_noise")
        .expect("bass take");
    let onset = take.onsets[1];
    let truth = 41.203_45;
    let mut judge = ProvisionalJudge::new(take.rate, InputProfile::Bass.tracking().cycle);
    let mut first = None;
    for (at, &sample) in take.samples.iter().enumerate().take(onset + 1800) {
        if let Some(estimate) = judge.push(sample) {
            if at >= onset {
                first = Some((at, estimate));
                break;
            }
        }
    }
    let (at, estimate) = first.expect("new long note has provisional evidence");
    let error = (1200.0 * (estimate.hz / truth).log2()).abs();
    assert!(
        error <= 50.0,
        "at={at} estimate={estimate:?} truth={truth} error={error}"
    );
}

#[test]
fn ringing_cancellation_sweep_measurements() {
    use crate::voice::{VoiceTracker, judge::JudgeResult};
    let takes = instrument_suite(48_000);
    let take = takes
        .iter()
        .find(|take| take.name == "sweep")
        .expect("sweep");
    let mut judge = ProvisionalJudge::new(take.rate, InputProfile::Guitar.tracking().cycle);
    let mut tracker = VoiceTracker::with_settings(take.rate, InputProfile::Guitar.tracking());
    let mut due = None;
    for (at, &sample) in take.samples.iter().enumerate() {
        if let Some(reading) = judge.push(sample) {
            tracker.judge(JudgeResult::Provisional(reading));
        }
        if let Some(pitch) = judge.pitch() {
            tracker.judge(pitch);
        }
        tracker.process(std::slice::from_ref(&sample), |frame| {
            if frame.onset {
                let note = take.onsets.partition_point(|&pluck| pluck <= frame.at as usize) - 1;
                let truth = take.truth[(take.onsets[note] + 32) / 32].expect("note truth");
                due = Some(frame.at + (2.0 * take.rate as f32 / truth).ceil() as u64);
            }
            let settled = due.is_some_and(|due| frame.at >= due);
            if frame.onset || settled {
                let mut maxima = Vec::new();
                judge.residual.tracker.key_maxima(judge.residual.available, &mut maxima);
                maxima.sort_unstable_by(|a, b| b.clarity.total_cmp(&a.clarity));
                maxima.truncate(3);
                println!("RESIDUAL onset={} two_periods={settled} delivery={} frame={frame:?} published_sources={:?} pre_attack_sources={:?} ringing={:?} combs={:?} top_three={maxima:?}", frame.onset, at + 1,
                    judge.residual.sources, judge.ringing, judge.residual.ringing, judge.residual.cancellation.combs);
            }
            if settled { due = None; }
            judge.observe(frame);
        });
    }
}

#[test]
fn the_residual_search_never_allocates() {
    let takes = instrument_suite(48_000);
    let take = takes
        .iter()
        .find(|take| take.name == "sweep")
        .expect("sweep");
    let mut judge = ProvisionalJudge::new(take.rate, InputProfile::Guitar.tracking().cycle);
    let before = assert_no_alloc::violation_count();
    assert_no_alloc::assert_no_alloc(|| {
        for &sample in &take.samples {
            std::hint::black_box(judge.push(sample));
        }
    });
    assert_eq!(assert_no_alloc::violation_count(), before);
}
