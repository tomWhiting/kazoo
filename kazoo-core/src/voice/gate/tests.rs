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

#[test]
fn a_held_wrong_reading_cannot_confirm_itself() {
    let mut gate = Gate::with_settings(48_000, InputProfile::Guitar.tracking());
    for hop in 1..=64 {
        gate.frame_with_pitch(hop * 64, true, -12.0, -12.0, 110.0);
    }
    assert!(!gate.frame_with_pitch(65 * 64, true, -12.0, -12.0, 790.0).1);
    for hop in 66..=80 {
        assert!(
            !gate
                .frame_with_level(hop * 64, true, -12.0, -12.0, 790.0, -12.0)
                .1,
            "a held reading confirmed at hop {hop}"
        );
        assert_eq!(gate.published_pitch(), Some(110.0));
    }
}

#[test]
fn a_recent_amplitude_onset_confirms_a_jump() {
    let mut gate = Gate::with_settings(48_000, InputProfile::Guitar.tracking());
    for hop in 1..=64 {
        gate.frame_with_pitch(hop * 64, true, -12.0, -12.0, 110.0);
    }
    assert!(gate.frame_with_pitch(4160, true, -6.0, -14.0, 110.0).1);
    assert!(!gate.frame_with_pitch(4480, true, -6.0, -6.0, 196.0).1);
    assert_eq!(gate.published_pitch(), Some(196.0));
}
