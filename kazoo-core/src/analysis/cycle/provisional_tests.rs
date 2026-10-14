use super::ProvisionalJudge;
use crate::{analysis::score::instrument::instrument_suite, host::live::InputProfile};

#[test]
fn a_repluck_discards_the_previous_search_history() {
    let takes = instrument_suite(48_000);
    let take = takes
        .iter()
        .find(|take| take.name == "bass_open")
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
