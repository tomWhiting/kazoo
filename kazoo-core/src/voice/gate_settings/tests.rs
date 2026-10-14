use super::GateSettings;
use crate::voice::{TrackSettings, VOICE_HOP, gate::Gate};

#[test]
fn gate_controls_clamp_invalid_values_and_preserve_threshold_order() {
    for open in [f32::NAN, f32::INFINITY, -100.0, -70.0, -50.0, -10.0, 20.0] {
        for close in [f32::NAN, f32::NEG_INFINITY, -100.0, -80.0, -15.0, 0.0] {
            let held = GateSettings {
                open_db: open,
                close_db: close,
                hold_ms: f32::NAN,
            }
            .held();
            assert!((-70.0..=-10.0).contains(&held.open_db));
            assert!((-80.0..=-15.0).contains(&held.close_db));
            assert!(held.close_db <= held.open_db - 3.0);
            assert!((held.hold_ms - 40.0).abs() < f32::EPSILON);
        }
    }
}

#[test]
fn measured_gate_threshold_and_live_hold_follow_settings() {
    let mut gate = Gate::with_settings(48_000, TrackSettings::VOICE);
    let defaults = GateSettings::default();
    let amplitude = 10.0_f32.powf(defaults.open_db / 20.0);
    let measured = 20.0 * amplitude.log10();
    assert!((measured + 50.0).abs() < 0.0001, "measured {measured} dBFS");
    gate.set(defaults, 48_000);
    assert!(!gate.frame(64, true, measured - 0.1, measured - 0.1).0);
    assert!(gate.frame(128, true, measured, measured).0);
    for hop in 1..30 {
        assert!(
            gate.frame(128 + hop * VOICE_HOP as u64, false, -80.0, -80.0)
                .0
        );
    }
    assert!(
        !gate
            .frame(128 + 30 * VOICE_HOP as u64, false, -80.0, -80.0)
            .0
    );
    gate.set(
        GateSettings {
            open_db: -60.0,
            close_db: -66.0,
            hold_ms: 0.0,
        },
        48_000,
    );
    assert!(gate.frame(4096, true, -59.0, -59.0).0);
    assert!(!gate.frame(4160, false, -80.0, -80.0).0);
}
