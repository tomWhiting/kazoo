use kazoo_wall::catalogue::Kind;
use kazoo_wall::dsp::build;
use kazoo_wall::engine::{
    CableTable, Command, Engine, EngineConfig, EngineControl, Route, engine, processing_order,
};
use kazoo_wall::{MAX_INPUTS, MAX_JACKS, MAX_KNOBS};

pub type Cable = (usize, usize, usize, usize, f32);

#[derive(Debug)]
pub struct Fixture {
    pub engine: Engine,
    pub control: EngineControl,
    pub next_slot: usize,
    pub next_tag: u32,
    pub slots: Vec<(usize, Kind)>,
    pub cables: Vec<Cable>,
    pub placed_routes: usize,
    pub table_routes: usize,
}

impl Default for Fixture {
    fn default() -> Self {
        Self::new()
    }
}

impl Fixture {
    #[must_use]
    pub fn new() -> Self {
        let (engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), None);
        Self {
            engine,
            control,
            next_slot: 0,
            next_tag: 0,
            slots: Vec::new(),
            cables: Vec::new(),
            placed_routes: 0,
            table_routes: 0,
        }
    }

    pub fn add(&mut self, kind: Kind) -> usize {
        let slot = self.next_slot;
        self.next_slot += 1;
        self.next_tag += 1;
        let mut knobs = [0.0; MAX_KNOBS];
        for (target, value) in knobs.iter_mut().zip(kind.spec().defaults()) {
            *target = value;
        }
        self.control.send(Command::Insert {
            slot,
            tag: self.next_tag,
            kind,
            module: build(kind, 48_000.0),
            knobs,
        });
        self.slots.push((slot, kind));
        slot
    }

    pub fn plug(&mut self, from: usize, port: usize, to: usize, jack: usize, amount: f32) {
        self.cables.retain(|cable| cable.2 != to || cable.3 != jack);
        self.cables.push((from, port, to, jack, amount));
    }

    pub fn commit(&mut self) {
        let mut table = CableTable::with_slots(self.next_slot).unwrap();
        self.placed_routes = 0;
        for (line, &(from, port, to, jack, amount)) in self.cables.iter().enumerate() {
            assert!(table.set(
                to,
                jack,
                Some(Route::new(
                    u32::try_from(from).unwrap(),
                    u8::try_from(port).unwrap(),
                    amount,
                    u32::try_from(line).unwrap(),
                    u32::try_from(line + 1).unwrap()
                ))
            ));
            self.placed_routes += 1;
        }
        self.table_routes = (0..table.slots())
            .map(|slot| {
                (0..MAX_JACKS)
                    .filter(|jack| table.route(slot, *jack).is_some())
                    .count()
            })
            .sum();
        let slots: Vec<usize> = self.slots.iter().map(|(slot, _)| *slot).collect();
        let outs: Vec<usize> = self
            .slots
            .iter()
            .filter_map(|(slot, kind)| (*kind == Kind::OUT).then_some(*slot))
            .collect();
        let edges: Vec<(usize, usize)> =
            self.cables.iter().map(|cable| (cable.0, cable.2)).collect();
        self.control.send(Command::Cables(Box::new(table)));
        self.control.send(Command::Order(Box::new(processing_order(
            &slots, &edges, &outs,
        ))));
        let mut buffer = [0.0; 512 * 2];
        // Large fixtures outgrow the command ring before the first callback.
        loop {
            self.engine.render(&mut buffer, 2);
            assert!(
                self.control.pump().is_empty(),
                "fixture produced an engine fault"
            );
            if self.control.backlog() == 0 {
                break;
            }
        }
        self.engine.render(&mut buffer, 2);
        assert!(
            self.control.pump().is_empty(),
            "fixture produced an engine fault"
        );
        assert_eq!(self.control.shared().ignored(), 0);
    }

    pub fn render(&mut self, out: &mut [f32]) {
        self.engine.render(out, 2);
    }

    #[must_use]
    pub fn kind(&self, slot: usize) -> Kind {
        self.slots
            .iter()
            .find(|(number, _)| *number == slot)
            .unwrap()
            .1
    }

    #[must_use]
    pub fn port(&self, slot: usize, name: &str) -> usize {
        self.kind(slot)
            .spec()
            .outputs
            .iter()
            .position(|port| port.name == name)
            .unwrap()
    }

    #[must_use]
    pub fn input(&self, slot: usize, name: &str) -> usize {
        self.kind(slot)
            .spec()
            .inputs
            .iter()
            .position(|port| port.name == name)
            .unwrap()
    }

    #[must_use]
    pub fn knob(&self, slot: usize, name: &str) -> usize {
        MAX_INPUTS + self.kind(slot).spec().knob_index(name).unwrap()
    }

    #[must_use]
    pub fn outs_fed_by(&self, name: &str) -> usize {
        self.cables
            .iter()
            .filter(|cable| self.kind(cable.0).name() == name && self.kind(cable.2) == Kind::OUT)
            .count()
    }
}
