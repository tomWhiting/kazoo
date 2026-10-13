use super::*;

#[test]
fn every_heard_module_is_timed_in_turn() {
    let (mut engine, mut control) = fresh();
    let mut model = Model::default();
    for slot in 0..6 {
        model.add(&mut control, slot, Kind::VCO);
    }
    control.send(Command::Order(Box::new(processing_order(
        &[0, 1, 2, 3, 4],
        &[],
        &[],
    ))));
    let mut buffer = vec![0.0; SUB_BLOCK * 2 * 10];
    on_audio_thread(|| engine.render(&mut buffer, 2));
    for slot in 0..5 {
        assert!(
            control
                .cost(slot)
                .is_some_and(|ns| ns.is_finite() && ns > 0.0)
        );
    }
    assert_eq!(control.cost(5), None);
    assert_eq!(control.cost(usize::MAX), None);
    model.add(&mut control, 0, Kind::VCO);
    engine.drain_commands();
    assert_eq!(control.cost(0), None);
    let (page, at) = engine.room.page(0).unwrap();
    control.set_cost(0, 1_000.0);
    page.note_cost(at, 1e9);
    assert_eq!(control.cost(0), Some(1_750.0));
    control.set_cost(0, 1_000.0);
    page.note_cost(at, 500.0);
    assert_eq!(control.cost(0), Some(750.0));
    for ns in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        page.note_cost(at, ns);
        assert_eq!(control.cost(0), Some(750.0));
        control.set_cost(1, ns);
        assert_eq!(control.cost(1), None);
    }
    control.forget_cost(0);
    assert_eq!(control.cost(0), None);
    on_audio_thread(|| engine.render_sub_block());
    assert!(control.cost(0).is_some());
}

/// Both clock reads can fall in one tick of the clock (about 42 ns on Apple
/// silicon), timing a cheap module at nothing. That sample measured
/// nothing: held, it would pin the cost at zero for good, since a rise is
/// clamped to a multiple of what is held, and a wake of the module would
/// then cost nothing whatever it costs later.
#[test]
fn a_timing_of_nothing_measures_nothing() {
    let (mut engine, mut control) = fresh();
    let mut model = Model::default();
    model.add(&mut control, 0, Kind::VCO);
    engine.drain_commands();
    let (page, at) = engine.room.page(0).unwrap();
    assert_eq!(control.cost(0), None);
    page.note_cost(at, 0.0);
    page.note_cost(at, 1_000.0);
    assert_eq!(control.cost(0), Some(1_000.0));
    for _ in 0..8 {
        page.note_cost(at, 0.0);
    }
    page.note_cost(at, 4_000.0);
    assert_eq!(control.cost(0), Some(1_750.0));
}
