use super::super::*;
use crate::engine::{Engine, EngineConfig, engine};
use crate::protocol::{DEFAULT_CROSS_BEATS, TreePlace};

struct Rig {
    wall: Wall,
    engine: Engine,
    state: tempfile::TempDir,
}

fn place() -> TreePlace {
    serde_json::from_value(serde_json::json!({"commit":"0123456789abcdef", "mark":"dawn"})).unwrap()
}

fn patch() -> Patch {
    let mut patch = Patch::empty(120.0);
    patch.add("out", None, None).unwrap();
    patch.add("vco", None, None).unwrap();
    patch.turn("vco1", "level", 0.2, Some(0.0)).unwrap();
    patch.turn("vco1", "shape", 0.0, Some(0.0)).unwrap();
    patch.turn("out1", "level", 1.0, Some(0.0)).unwrap();
    patch.plug("vco1.out", "out1.left", None, None).unwrap();
    patch
}

impl Rig {
    fn new(patch: Patch) -> Self {
        let state = tempfile::tempdir_in(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/kz-logs"))
            .unwrap();
        let (engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), None);
        let wall = Wall::new(
            patch,
            control,
            None,
            Vec::new(),
            Some(Store::peek(state.path()).unwrap()),
        );
        let mut rig = Self {
            wall,
            engine,
            state,
        };
        rig.render(2_048);
        rig
    }

    fn incoming(&self) -> Patch {
        Patch::load(&self.wall.patch.to_file()).unwrap().0
    }

    fn go(&mut self, incoming: Patch, beats: f64) -> Outcome {
        self.wall
            .replace(
                ("Tom", true),
                incoming,
                &Source::Moment {
                    to: place(),
                    from: place(),
                    beats,
                },
                Vec::new(),
            )
            .unwrap()
    }

    fn render(&mut self, frames: usize) -> Vec<f32> {
        let mut output = Vec::with_capacity(frames);
        let mut buffer = [0.0; 1_024];
        for _ in 0..frames.div_ceil(512) {
            let before = assert_no_alloc::violation_count();
            assert_no_alloc::assert_no_alloc(|| self.engine.render(&mut buffer, 2));
            assert_eq!(
                before,
                assert_no_alloc::violation_count(),
                "audio thread allocated or freed"
            );
            assert!(buffer.iter().all(|sample| sample.is_finite()));
            output.extend(buffer.iter().step_by(2).copied());
            self.wall.tick(Instant::now());
        }
        output
    }

    fn ask(&mut self, value: serde_json::Value) -> Outcome {
        let request: Request = serde_json::from_value(value).unwrap();
        let outcome = self
            .wall
            .request_as("Tom", true, &request, Instant::now())
            .unwrap();
        let Some(ticket) = outcome.pending else {
            return outcome;
        };
        loop {
            self.wall.tree_test_wait();
            if let Some((_, answer)) = self
                .wall
                .tree_answers(Instant::now())
                .into_iter()
                .find(|(id, _)| *id == ticket)
            {
                return answer.unwrap();
            }
        }
    }

    fn tree(&mut self) -> tempfile::TempDir {
        let tree = tempfile::tempdir_in(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/kz-logs"))
            .unwrap();
        crate::tree::git::plant(tree.path()).unwrap();
        self.wall
            .attach_tree(crate::tree::thread::TreeHandle::attach(tree.path()).unwrap());
        tree
    }

    fn crossing(&mut self, incoming: Patch) -> Outcome {
        let swaps = self.wall.swaps_sent;
        let result = self.go(incoming, 4.0);
        assert_eq!(
            self.wall.swaps_sent, swaps,
            "a crossing must not hush and replace the engine"
        );
        result
    }
}

fn peak(samples: &[f32]) -> f32 {
    samples.iter().copied().map(f32::abs).fold(0.0, f32::max)
}

#[test]
fn shared_modules_stay_and_glide_every_knob() {
    let mut old = patch();
    old.add("lfo", None, None).unwrap();
    old.plug("lfo1.out", "out1.right", None, None).unwrap();
    assert!(old.mark_birth(
        "vco1",
        crate::tree::Birth::Born {
            seq: 1,
            n: 0,
            at: "2026-10-15T00:00:00Z".into(),
            print: 1,
            salt: 1
        }
    ));
    let mut rig = Rig::new(old);
    let slot = rig.wall.rack.slot_of("vco1").unwrap();
    let lfo = rig.wall.rack.slot_of("lfo1").unwrap();
    let tag = rig.wall.rack.slots[slot].as_ref().unwrap().1;
    let mut incoming = rig.incoming();
    incoming.turn("vco1", "level", 0.8, Some(0.0)).unwrap();
    incoming.turn("lfo1", "rate", 2.0, Some(0.0)).unwrap();
    let text = serde_json::to_string(&incoming.to_file())
        .unwrap()
        .replace("vco1", "vco9");
    incoming = Patch::load(&serde_json::from_str(&text).unwrap())
        .unwrap()
        .0;
    rig.crossing(incoming);
    assert_eq!(rig.wall.rack.slot_of("vco9"), Some(slot));
    assert_eq!(rig.wall.rack.slot_of("vco1"), None);
    assert_eq!(rig.wall.rack.slot_of("lfo1"), Some(lfo));
    rig.render(48_128);
    let knob = rig
        .wall
        .patch
        .module("vco9")
        .unwrap()
        .kind
        .spec()
        .knob_index("level")
        .unwrap();
    let middle = rig.wall.engine.knob(slot, knob).unwrap();
    assert!(middle > 0.2 && middle < 0.8, "{middle}");
    assert_eq!(rig.wall.rack.slots[slot].as_ref().unwrap().1, tag);
    rig.render(48_128);
    assert_eq!(
        rig.wall.engine.knob(slot, knob).unwrap().to_bits(),
        0.8_f32.to_bits()
    );
    let rate = rig
        .wall
        .patch
        .module("lfo1")
        .unwrap()
        .kind
        .spec()
        .knob_index("rate")
        .unwrap();
    assert_eq!(
        rig.wall.engine.knob(lfo, rate).unwrap().to_bits(),
        2.0_f32.to_bits()
    );
}
#[test]
fn a_module_only_the_moment_has_fades_in_from_silence() {
    let mut old = patch();
    old.remove("vco1", &[]).unwrap();
    let mut rig = Rig::new(old);
    let mut incoming = rig.incoming();
    incoming.add("vco", None, None).unwrap();
    let id = incoming.modules().last().unwrap().id.clone();
    incoming.turn(&id, "level", 0.2, Some(0.0)).unwrap();
    incoming
        .plug(&format!("{id}.out"), "out1.left", None, None)
        .unwrap();
    rig.crossing(incoming);
    let samples = rig.render(108_032);
    let full = peak(&samples[96_000..98_048]);
    assert!(peak(&samples[..2_048]) < 0.01);
    assert!((peak(&samples[47_000..49_048]) / full - 0.5).abs() < 0.125);
    for pair in samples.chunks_exact(2_048).collect::<Vec<_>>().windows(2) {
        assert!(peak(pair[1]) - peak(pair[0]) < full / 8.0);
    }
}

#[test]
fn a_module_the_moment_lacks_fades_out_then_goes() {
    let mut rig = Rig::new(patch());
    let slot = rig.wall.rack.slot_of("vco1").unwrap();
    let mut incoming = rig.incoming();
    incoming.remove("vco1", &[]).unwrap();
    rig.crossing(incoming);
    let samples = rig.render(110_080);
    assert!(peak(&samples[..2_048]) > 0.05);
    assert!(peak(&samples[96_000..]) < 0.001);
    assert!(rig.wall.rack.free.contains(&slot));
}

#[test]
fn an_input_that_changes_source_crosses_through_silence_at_the_half() {
    let mut old = patch();
    old.add("vco", None, None).unwrap();
    old.turn("vco2", "octave", 1.0, Some(0.0)).unwrap();
    old.turn("vco2", "shape", 0.0, Some(0.0)).unwrap();
    old.turn("vco2", "level", 0.2, Some(0.0)).unwrap();
    let mut rig = Rig::new(old);
    let mut incoming = rig.incoming();
    incoming.plug("vco2.out", "out1.left", None, None).unwrap();
    rig.crossing(incoming);
    let samples = rig.render(110_080);
    let full = peak(&samples[..2_048]);
    assert!(peak(&samples[58_368..60_416]) < full * 0.05);
    let crossings = |window: &[f32]| {
        window
            .windows(2)
            .filter(|pair| pair[0].is_sign_positive() != pair[1].is_sign_positive())
            .count()
    };
    let first = crossings(&samples[24_000..26_048]);
    let last = crossings(&samples[90_000..92_048]);
    assert!(
        last >= first * 18 / 10 && last <= first * 22 / 10,
        "{first}, {last}"
    );
    assert!(
        samples
            .chunks_exact(2_048)
            .all(|window| peak(window) <= full * 1.05)
    );
}

#[test]
fn a_cable_whose_amount_differs_glides_on_its_own_line() {
    let mut rig = Rig::new(patch());
    let mut incoming = rig.incoming();
    let id = incoming.cables()[0].id;
    incoming
        .plug("vco1.out", "out1.left", Some(0.25), Some(id + 1))
        .unwrap();
    rig.crossing(incoming);
    let samples = rig.render(110_080);
    let first = peak(&samples[..2_048]);
    let last = peak(&samples[96_000..98_048]);
    assert!((last / first - 0.25).abs() < 0.03);
    assert!(peak(&samples[47_000..49_048]) > last * 2.0);
}

#[test]
fn the_same_birth_in_another_kind_is_two_modules() {
    let mut rig = Rig::new(patch());
    let slot = rig.wall.rack.slot_of("vco1").unwrap();
    let mut file = rig.wall.patch.to_file();
    file.modules
        .iter_mut()
        .find(|module| module.id == "vco1")
        .unwrap()
        .kind = "vcf".into();
    file.cables.clear();
    let incoming = Patch::load(&file).unwrap().0;
    rig.crossing(incoming);
    assert_ne!(rig.wall.rack.slot_of("vco1"), Some(slot));
}

#[test]
fn speakers_match_only_with_their_id_and_words() {
    let mut old = patch();
    old.add("speak", None, None).unwrap();
    let mut rig = Rig::new(old);
    let slot = rig.wall.rack.slot_of("speak1");
    rig.crossing(rig.incoming());
    assert_eq!(rig.wall.rack.slot_of("speak1"), slot);
    let mut different = rig.incoming();
    different.speech.insert(
        "speak1".into(),
        Words {
            text: "hello".into(),
            voice: None,
        },
    );
    let plan = rig
        .wall
        .plan_crossing(&different, super::CrossTiming::go(96_000));
    assert!(plan.born.iter().any(|id| id == "speak1"));
    assert!(plan.leaving.iter().any(|(id, _, _)| id == "speak1"));
}

#[test]
fn a_seats_changes_during_the_crossing_are_made_to_the_moment() {
    let mut rig = Rig::new(patch());
    let mut incoming = rig.incoming();
    incoming.turn("vco1", "level", 0.8, Some(0.0)).unwrap();
    rig.crossing(incoming);
    rig.render(48_128);
    rig.wall
        .request(
            "Tom",
            &Request::Turn {
                module: "vco1".into(),
                knob: "level".into(),
                value: 0.4,
                glide_beats: Some(0.0),
            },
            Instant::now(),
        )
        .unwrap();
    rig.render(64_000);
    let module = rig.wall.patch.module("vco1").unwrap();
    assert_eq!(
        module.knobs[module.kind.spec().knob_index("level").unwrap()].to_bits(),
        0.4_f32.to_bits()
    );
    assert_eq!(
        Store::peek(rig.state.path())
            .unwrap()
            .load_log()
            .unwrap()
            .changes
            .len(),
        2
    );
}

#[test]
fn a_go_during_a_crossing_carries_on_from_where_the_engine_is() {
    let mut rig = Rig::new(patch());
    let mut b = rig.incoming();
    b.turn("vco1", "level", 0.8, Some(0.0)).unwrap();
    rig.crossing(b);
    rig.render(24_064);
    let mut c = rig.incoming();
    c.turn("vco1", "level", 0.5, Some(0.0)).unwrap();
    rig.crossing(c);
    let samples = rig.render(110_080);
    for pair in samples.chunks_exact(2_048).collect::<Vec<_>>().windows(2) {
        assert!((peak(pair[0]) - peak(pair[1])).abs() < 0.05);
    }
    assert!(rig.wall.fades.is_empty());
}

#[test]
fn a_cut_mid_crossing_ends_it_cleanly() {
    let mut rig = Rig::new(patch());
    rig.crossing(rig.incoming());
    rig.go(rig.incoming(), 0.0);
    assert_eq!(rig.wall.swaps_sent, 1);
    assert!(rig.wall.fades.is_empty());
}

#[test]
fn a_new_engine_mid_crossing_ends_it_cleanly() {
    let mut rig = Rig::new(patch());
    rig.crossing(rig.incoming());
    let (mut engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), None);
    rig.wall.attach(control, None);
    let mut buffer = [0.0; 1_024];
    engine.render(&mut buffer, 2);
    assert_eq!(rig.wall.rack.index.len(), rig.wall.patch.modules().len());
    assert!(rig.wall.fades.is_empty());
}

#[test]
fn the_tempo_glides_unless_the_desk_keeps_it() {
    let mut rig = Rig::new(patch());
    let mut incoming = rig.incoming();
    incoming.tempo = 90.0;
    rig.crossing(incoming);
    rig.render(48_128);
    assert!((rig.wall.engine.shared().bpm() - 105.0).abs() < 0.2);
    rig.render(48_128);
    assert_eq!(rig.wall.engine.shared().bpm().to_bits(), 90.0_f64.to_bits());
}

#[test]
fn a_go_of_no_beats_is_todays_cut() {
    let mut rig = Rig::new(patch());
    let result = rig.go(rig.incoming(), 0.0);
    assert_eq!(rig.wall.swaps_sent, 1);
    assert!(!result.change.unwrap().summary.contains("crossing"));
}

#[test]
fn a_crossing_past_the_guard_or_without_memory_is_a_cut_and_says_so() {
    use super::cross_or_cut;
    assert_eq!(cross_or_cut(4.0, false, false, 0.0), Some("memory"));
    assert_eq!(cross_or_cut(4.0, true, false, 1.0), None);
    assert_eq!(cross_or_cut(4.0, true, true, LOAD_GUARD), None);
    assert_eq!(
        cross_or_cut(4.0, true, true, LOAD_GUARD + 0.001),
        Some("load")
    );
    let mut rig = Rig::new(patch());
    rig.wall.load_watch.on = true;
    rig.wall.engine.shared().set_render_load(0.99);
    let mut incoming = rig.incoming();
    incoming.add("hall", None, None).unwrap();
    let result = rig.go(incoming, 4.0);
    assert!(
        matches!(result.change.unwrap().what, What::Go { beats, .. } if beats.to_bits() == 0.0_f64.to_bits())
    );
}

#[test]
fn the_go_and_its_words_say_the_crossing() {
    let mut rig = Rig::new(patch());
    let result = rig.go(rig.incoming(), DEFAULT_CROSS_BEATS);
    let change = result.change.unwrap();
    assert!(change.summary.contains("; crossing over 16 beats"));
    let loaded: LoadResult = serde_json::from_value(result.result).unwrap();
    assert!(
        loaded
            .notes
            .iter()
            .any(|note| note.contains("2 modules glide, 0 fade in, 0 fade out"))
    );
    let mut old = serde_json::to_value(change.what).unwrap();
    old.as_object_mut().unwrap().remove("beats");
    assert!(
        matches!(serde_json::from_value::<What>(old).unwrap(), What::Go { beats, .. } if beats.to_bits() == 0.0_f64.to_bits())
    );
}

#[test]
fn nothing_is_built_or_freed_on_the_audio_thread() {
    a_module_only_the_moment_has_fades_in_from_silence();
    a_module_the_moment_lacks_fades_out_then_goes();
    an_input_that_changes_source_crosses_through_silence_at_the_half();
    a_go_during_a_crossing_carries_on_from_where_the_engine_is();
}

#[test]
fn undoing_a_go_crosses_back_over_its_beats() {
    let mut rig = Rig::new(patch());
    let tree = rig.tree();
    rig.ask(serde_json::json!({"op":"mark", "name":"before"}));
    rig.ask(serde_json::json!({"op":"turn", "module":"vco1", "knob":"level", "value":0.8, "glide_beats":0}));
    let before = rig.wall.patch.fingerprint();
    let swaps = rig.wall.swaps_sent;
    let go = rig
        .ask(serde_json::json!({"op":"go", "moment":"before"}))
        .change
        .unwrap();
    let result = rig.ask(serde_json::json!({"op":"undo", "change":go.seq}));
    assert_eq!(rig.wall.swaps_sent, swaps);
    assert!(
        matches!(result.change.unwrap().what, What::Go { beats, .. } if beats.to_bits() == 16.0_f64.to_bits())
    );
    assert_eq!(rig.wall.patch.fingerprint(), before);
    let module = rig.wall.patch.module("vco1").unwrap();
    assert_eq!(
        module.knobs[module.kind.spec().knob_index("level").unwrap()].to_bits(),
        0.8_f32.to_bits()
    );
    rig.wall.tree.as_mut().unwrap().finish().unwrap();
    drop(tree);
}

#[test]
fn a_change_during_the_crossing_is_the_buds_first_commit() {
    let mut rig = Rig::new(patch());
    let tree = rig.tree();
    let marked: crate::protocol::MarkResult = serde_json::from_value(
        rig.ask(serde_json::json!({"op":"mark", "name":"before"}))
            .result,
    )
    .unwrap();
    rig.ask(serde_json::json!({"op":"turn", "module":"vco1", "knob":"level", "value":0.8, "glide_beats":0}));
    let swaps = rig.wall.swaps_sent;
    rig.ask(serde_json::json!({"op":"go", "moment":"before"}));
    assert_eq!(rig.wall.swaps_sent, swaps);
    rig.render(192_000);
    rig.ask(serde_json::json!({"op":"turn", "module":"vco1", "knob":"level", "value":0.4, "glide_beats":0}));
    rig.wall.tree.as_mut().unwrap().finish().unwrap();
    let parent = crate::tree::git::git_command("rev-parse", Some(tree.path()))
        .arg("HEAD^")
        .output()
        .unwrap();
    assert!(
        parent.status.success(),
        "{}",
        String::from_utf8_lossy(&parent.stderr)
    );
    assert_eq!(
        String::from_utf8(parent.stdout).unwrap().trim(),
        marked.at.commit
    );
    let branch = crate::tree::git::git_command("symbolic-ref", Some(tree.path()))
        .arg("HEAD")
        .output()
        .unwrap();
    assert!(branch.status.success());
    assert!(
        String::from_utf8(branch.stdout)
            .unwrap()
            .starts_with("refs/heads/bud/")
    );
}
