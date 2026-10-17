use super::*;
use crate::protocol::GlideCurve;

fn moving(wait: u32, curve: GlideCurve) -> Glide {
    Glide {
        value: 0.0,
        position: 0.0,
        target: 1.0,
        remaining: 128,
        wait,
        start: 0.0,
        end: 1.0,
        total: 128,
        gone: 0,
        curve,
    }
}

#[test]
fn knob_waits_exact_frames_and_curves_land_exactly() {
    let spec = Kind::VCA.spec().knobs.first().unwrap();
    let mut glide = moving(33, GlideCurve::Linear);
    glide.advance(Some(spec));
    assert_eq!((glide.position).to_bits(), (0.0_f32).to_bits());
    assert_eq!(glide.wait, 1);
    glide.advance(Some(spec));
    assert_eq!((glide.position).to_bits(), (31.0_f32 / 128.0).to_bits());
    for (curve, expected) in [
        (GlideCurve::Linear, [0.25, 0.5, 0.75]),
        (GlideCurve::EaseIn, [0.015_625, 0.125, 0.421_875]),
        (GlideCurve::EaseOut, [0.578_125, 0.875, 0.984_375]),
        (GlideCurve::SCurve, [0.15625, 0.5, 0.84375]),
    ] {
        let mut glide = moving(0, curve);
        for value in expected {
            glide.advance(Some(spec));
            assert!(
                (glide.position - value).abs() < 1e-6,
                "{curve:?}: {} != {value}",
                glide.position
            );
        }
        glide.advance(Some(spec));
        assert_eq!((glide.value).to_bits(), (1.0_f32).to_bits());
        assert_eq!(glide.remaining, 0);
    }
}

#[test]
fn cable_waits_exact_frames_then_fades() {
    let mut tap = super::super::lines::Tap::EMPTY;
    let route = Route {
        wait: 33,
        glide: 32,
        ..Route::new(0, 0, 1.0, 1)
    };
    let mut out = [0.0; SUB_BLOCK];
    tap.carry_direct(&route, &[1.0; SUB_BLOCK], &mut out);
    assert_eq!(out.map(f32::to_bits), [(0.0_f32).to_bits(); SUB_BLOCK]);
    tap.carry_direct(&route, &[1.0; SUB_BLOCK], &mut out);
    assert_eq!((out[0]).to_bits(), (0.0_f32).to_bits());
    assert_eq!((out[1]).to_bits(), (1.0_f32 / 32.0).to_bits());
    assert_eq!((out[31]).to_bits(), (31.0_f32 / 32.0).to_bits());
    tap.carry_direct(&route, &[1.0; SUB_BLOCK], &mut out);
    assert_eq!((out[0]).to_bits(), (1.0_f32).to_bits());
}

#[test]
fn tempo_waits_then_moves() {
    let (mut engine, mut control) = fresh();
    let mut buffer = Vec::new();
    control.send(Command::TempoGlide {
        bpm: 90.0,
        frames: 64,
        wait_frames: 33,
    });
    render(&mut engine, 32, &mut buffer);
    assert_eq!((control.shared().bpm()).to_bits(), (120.0_f64).to_bits());
    render(&mut engine, 32, &mut buffer);
    assert_eq!(
        (control.shared().bpm()).to_bits(),
        (120.0_f64 - 30.0 * 31.0 / 64.0).to_bits()
    );
    render(&mut engine, 64, &mut buffer);
    assert_eq!((control.shared().bpm()).to_bits(), (90.0_f64).to_bits());
}

#[test]
fn a_new_turn_replaces_a_wait() {
    let (mut engine, mut control) = fresh();
    let mut model = Model::default();
    model.add(&mut control, 0, Kind::VCA);
    model.send_tables(&mut control);
    let mut buffer = Vec::new();
    render(&mut engine, 32, &mut buffer);
    let before = control.knob(0, 0).unwrap();
    control.send(Command::Knob {
        slot: 0,
        knob: 0,
        target: 0.0,
        glide_frames: 480,
        wait_frames: 48_000,
        curve: GlideCurve::EaseIn,
    });
    render(&mut engine, 64, &mut buffer);
    assert_eq!((control.knob(0, 0).unwrap()).to_bits(), (before).to_bits());
    control.send(Command::Knob {
        slot: 0,
        knob: 0,
        target: 0.5,
        glide_frames: 480,
        wait_frames: 0,
        curve: GlideCurve::Linear,
    });
    render(&mut engine, 480, &mut buffer);
    assert_eq!((control.knob(0, 0).unwrap()).to_bits(), (0.5_f32).to_bits());
}

#[test]
fn cable_amount_wait_holds_and_repeated_tables_do_not_restart_it() {
    let mut tap = super::super::lines::Tap::EMPTY;
    let mut out = [0.0; SUB_BLOCK];
    tap.carry_direct(&Route::new(0, 0, 0.75, 1), &[1.0; SUB_BLOCK], &mut out);
    let route = Route {
        wait: 32,
        glide: 32,
        ..Route::new(0, 0, 0.25, 1)
    };
    tap.carry_direct(&route, &[1.0; SUB_BLOCK], &mut out);
    assert_eq!(out.map(f32::to_bits), [(0.75_f32).to_bits(); SUB_BLOCK]);
    tap.carry_direct(&route, &[1.0; SUB_BLOCK], &mut out);
    assert_eq!((out[31]).to_bits(), (0.25_f32).to_bits());
    tap.carry_direct(&route, &[1.0; SUB_BLOCK], &mut out);
    assert_eq!(out.map(f32::to_bits), [(0.25_f32).to_bits(); SUB_BLOCK]);
}

#[test]
fn a_new_source_with_the_same_amount_waits_in_silence() {
    let (mut engine, mut control) = fresh();
    let mut model = Model::default();
    model.add(&mut control, 0, Kind::LFO);
    model.add(&mut control, 1, Kind::VCA);
    model.plug(&mut control, (0, 0, 1, 0, 1.0));
    let mut buffer = Vec::new();
    render(&mut engine, 32, &mut buffer);
    let mut table = CableTable::new();
    assert!(table.set(
        1,
        0,
        Some(Route {
            wait: 64,
            glide: 32,
            ..Route::new(1, 0, 1.0, 0)
        })
    ));
    control.send(Command::Cables(Box::new(table)));
    render(&mut engine, 32, &mut buffer);
    assert_eq!(
        engine.room.output(1, 0).unwrap().map(f32::to_bits),
        [0.0_f32.to_bits(); SUB_BLOCK]
    );
}

#[test]
fn a_direct_new_source_with_the_same_amount_waits() {
    let mut tap = super::super::lines::Tap::EMPTY;
    let mut out = [0.0; SUB_BLOCK];
    tap.carry_direct(&Route::new(0, 0, 1.0, 0), &[1.0; SUB_BLOCK], &mut out);
    tap.carry_direct(
        &Route {
            wait: 32,
            glide: 32,
            ..Route::new(1, 0, 1.0, 0)
        },
        &[1.0; SUB_BLOCK],
        &mut out,
    );
    assert_eq!(out.map(f32::to_bits), [(0.0_f32).to_bits(); SUB_BLOCK]);
}

#[test]
fn delayed_line_keeps_the_same_wait_and_fade_as_a_direct_cable() {
    let mut line = Line::new(48_000.0).unwrap();
    let mut out = [0.0; SUB_BLOCK];
    let route = Route {
        wait: 33,
        glide: 32,
        ..Route::new(0, 0, 1.0, 7)
    };
    on_audio_thread(|| line.carry(&route, &[1.0; SUB_BLOCK], &mut out));
    assert_eq!(out.map(f32::to_bits), [(0.0_f32).to_bits(); SUB_BLOCK]);
    on_audio_thread(|| line.carry(&route, &[1.0; SUB_BLOCK], &mut out));
    assert_eq!(out[0].to_bits(), 0.0_f32.to_bits());
    assert_eq!(out[1].to_bits(), (1.0_f32 / 32.0).to_bits());
    on_audio_thread(|| line.carry(&route, &[1.0; SUB_BLOCK], &mut out));
    assert_eq!(out.map(f32::to_bits), [(1.0_f32).to_bits(); SUB_BLOCK]);
}
