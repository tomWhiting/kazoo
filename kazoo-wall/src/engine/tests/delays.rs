use super::*;

#[derive(Debug)]
struct Ramp;

impl Module for Ramp {
    fn process(&mut self, _tick: &Tick, io: Io<'_>) {
        for (frame, value) in io.outputs[0].iter_mut().enumerate() {
            *value = (frame + 1) as f32 / 64.0;
        }
    }

    fn reset(&mut self) {}
}

#[derive(Debug)]
struct AddOne;

impl Module for AddOne {
    fn process(&mut self, _tick: &Tick, io: Io<'_>) {
        for (value, input) in io.outputs[0].iter_mut().zip(&io.inputs[0]) {
            *value = *input + 1.0;
        }
    }

    fn reset(&mut self) {}
}

fn source(control: &mut EngineControl, slot: usize, module: Box<dyn Module>) {
    control.send(Command::Insert {
        slot,
        tag: 1,
        kind: Kind::VCO,
        module,
        knobs: [0.0; MAX_KNOBS],
    });
}

#[test]
fn a_cable_at_delay_zero_reads_its_source_directly_and_owns_no_line() {
    let (mut engine, mut control) = fresh();
    control.shared().set_audible(true);
    source(&mut control, 0, Box::new(Ramp));
    let mut model = Model::default();
    model.slots.push((0, Kind::VCO));
    model.add(&mut control, 1, Kind::OUT);
    model.plug(&mut control, (0, 0, 1, 0, 0.375));
    let mut buffer = Vec::new();
    render(&mut engine, SUB_BLOCK * 8, &mut buffer);
    assert!(peak(&buffer) > 0.0);
    for (got, value) in engine.inputs[0]
        .iter()
        .zip(engine.room.output(0, 0).unwrap())
    {
        assert_eq!(got.to_bits(), (*value * 0.375).to_bits());
    }
    assert_eq!(control.shared().lines(), 0);
}

#[test]
fn a_feedback_cable_hears_the_previous_sub_block_without_a_line() {
    let (mut engine, mut control) = fresh();
    source(&mut control, 0, Box::new(AddOne));
    source(&mut control, 1, Box::new(AddOne));
    let mut model = Model::default();
    model.slots.extend([(0, Kind::VCO), (1, Kind::VCO)]);
    model.add(&mut control, 2, Kind::OUT);
    model
        .cables
        .extend([(0, 0, 1, 0, 1.0), (1, 0, 0, 0, 1.0), (0, 0, 2, 0, 0.1)]);
    model.send_tables(&mut control);
    let mut buffer = Vec::new();
    let mut seen = Vec::new();
    for _ in 0..4 {
        render(&mut engine, SUB_BLOCK, &mut buffer);
        seen.push(engine.room.output(0, 0).unwrap()[0]);
    }
    // The lined feedback path produces 1, 3, 5, 7 on successive sub-blocks.
    assert_eq!(seen, [1.0, 3.0, 5.0, 7.0]);
    assert_eq!(control.shared().lines(), 0);
}

#[test]
fn a_line_placed_at_a_jack_delays_and_a_removed_module_takes_it_with_it() {
    let (mut engine, mut control) = fresh();
    source(&mut control, 0, Box::new(Ramp));
    let mut model = Model::default();
    model.slots.push((0, Kind::VCO));
    model.add(&mut control, 1, Kind::OUT);
    control.send(Command::Line {
        slot: 1,
        jack: 0,
        line: Box::new(Line::new(RATE as f32).unwrap()),
    });
    let mut table = CableTable::new();
    assert!(table.set(
        1,
        0,
        Some(Route {
            delay: 100,
            ..Route::new(0, 0, 1.0, 1)
        })
    ));
    control.send(Command::Cables(Box::new(table)));
    control.send(Command::Order(Box::new(Order::new(&[0, 1], &[1]))));
    let mut buffer = Vec::new();
    let mut signal = Vec::new();
    for block in 0..5 {
        if block == 2 {
            assert!(control.reserve_slots(1_000));
        }
        render(&mut engine, SUB_BLOCK, &mut buffer);
        signal.extend_from_slice(&engine.inputs[0]);
    }
    assert!(signal[..100].iter().all(|value| *value == 0.0));
    assert_eq!(signal[100].to_bits(), (1.0_f32 / 64.0).to_bits());
    assert_eq!(control.shared().lines(), 1);
    control.send(Command::Remove { slot: 1 });
    render(&mut engine, SUB_BLOCK, &mut buffer);
    assert_eq!(control.shared().lines(), 0);
    let mut lines = 0;
    while let Some(item) = control.retired.try_pop() {
        if matches!(item, Retired::Line(_)) {
            lines += 1;
        }
    }
    assert_eq!(lines, 1);
}

#[test]
fn lines_replace_retire_and_reserve_their_destination_page() {
    let (mut engine, mut control) = fresh();
    for _ in 0..2 {
        control.send(Command::Line {
            slot: 128,
            jack: MAX_JACKS - 1,
            line: Box::new(Line::new(RATE as f32).unwrap()),
        });
    }
    let mut buffer = Vec::new();
    render(&mut engine, SUB_BLOCK, &mut buffer);
    assert_eq!(control.shared().lines(), 1);
    assert_eq!(control.shared().ignored(), 0);
    let mut retired = 0;
    while let Some(item) = control.retired.try_pop() {
        if matches!(item, Retired::Line(_)) {
            retired += 1;
        }
    }
    assert_eq!(retired, 1);
    control.send(Command::Unline {
        slot: 128,
        jack: MAX_JACKS - 1,
    });
    control.send(Command::Line {
        slot: usize::MAX,
        jack: 0,
        line: Box::new(Line::new(RATE as f32).unwrap()),
    });
    render(&mut engine, SUB_BLOCK, &mut buffer);
    assert_eq!(control.shared().lines(), 0);
    assert_eq!(control.shared().ignored(), 1);
    assert_eq!(control.retired_waiting(), 2);
}
