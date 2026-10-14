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
    let takes = instrument_suite(48_000);
    let take = takes
        .iter()
        .find(|take| take.name == "sweep")
        .expect("sweep");
    let mut judge = ProvisionalJudge::new(take.rate, InputProfile::Guitar.tracking().cycle);
    let mut last_attack = None;
    for (at, &sample) in take.samples.iter().enumerate() {
        if let Some(estimate) = judge.push(sample) {
            if last_attack != Some(judge.attack_at) {
                let mut maxima = judge.maxima.clone();
                maxima.sort_unstable_by(|a, b| b.clarity.total_cmp(&a.clarity));
                maxima.truncate(3);
                println!(
                    "RESIDUAL attack={} delivery={} reading={estimate:?} ringing={:?} combs={:?} top_three={maxima:?}",
                    judge.attack_at,
                    at + 1,
                    judge.ringing,
                    judge.cancellation.combs
                );
                last_attack = Some(judge.attack_at);
            }
        }
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
