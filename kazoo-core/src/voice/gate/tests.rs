use super::Gate;
use crate::{analysis::cycle::CycleEstimate, host::live::InputProfile};

#[test]
fn a_lone_wrong_reading_is_no_onset() {
    let mut gate = Gate::with_settings(48_000, InputProfile::Guitar.tracking());
    assert!(gate.frame_with_pitch(64, true, -12.0, -12.0, 110.0).1);
    for hop in 2..=64 {
        assert!(!gate.frame_with_pitch(hop * 64, true, -12.0, -12.0, 110.0).1);
    }
    let wrong = CycleEstimate {
        period: 48_000.0 / 790.0,
        hz: 790.0,
        clarity: 0.82,
        provisional: false,
    };
    assert!(
        !gate
            .frame_with_pitch(65 * 64, true, -12.0, -12.0, wrong.hz)
            .1
    );
    for hop in 66..=70 {
        assert!(!gate.frame_with_pitch(hop * 64, true, -12.0, -12.0, 110.0).1);
    }
}
