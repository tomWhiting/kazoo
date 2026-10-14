use super::*;
use crate::daemon::wall::Wall;
use crate::engine::{Engine, EngineConfig, engine};
use crate::patch::{Patch, RetiredWords, Words};
use crate::protocol::{Change, ChangeResult, Request};
use std::time::{Duration, Instant};

const MARK: &str = "born 6374.0 2026-10-13T08:48:24Z 5f3c0e2a91b4d7e8 91d0c3a2b4e5f607";
const FIXTURE: &[u8] = include_bytes!("fixtures/evening.json");

fn record(id: &str, kind: &str) -> ModuleRecord {
    ModuleRecord {
        id: id.into(),
        kind: kind.into(),
        name: None,
        knobs: BTreeMap::new(),
        birth: None,
    }
}

fn fixture() -> PatchFile {
    crate::migrate::read(FIXTURE).unwrap().0
}

fn sample() -> PatchFile {
    let mut file = Patch::empty(97.3).to_file();
    file.seed = u64::MAX;
    file.last_seq = 6373;
    file.wall = 6373;
    file.next_cable = 281;
    let mut filter = record("vcf1", "vcf");
    filter.name = Some("low \"brass\" \\".into());
    filter.birth = Some(MARK.into());
    filter.knobs = BTreeMap::from([
        ("cutoff".into(), 0.35),
        ("res".into(), 0.2),
        ("unknown".into(), -0.0),
    ]);
    file.modules = vec![record("lfo1", "lfo"), filter, record("speak1", "speak")];
    file.cables.push(CableRecord {
        id: 150,
        from: "lfo1.out".into(),
        to: "vcf1.cutoff".into(),
        amount: -0.4,
    });
    file.dye.insert(
        "vcf1".into(),
        BTreeMap::from([
            ("Cassio".into(), 2.166_666_671_357_773),
            ("Tom W.".into(), 1.0),
        ]),
    );
    file.dye.insert("lfo1".into(), BTreeMap::new());
    file.speech.insert(
        "speak1".into(),
        Words {
            text: "hello\n\"world\"\\\u{0}\u{7f}".into(),
            voice: Some("af_heart".into()),
        },
    );
    file.retired_speech.push(RetiredWords {
        module: "speak2".into(),
        words: Words {
            text: "old\nwords".into(),
            voice: None,
        },
    });
    file.next_ids.insert("unknown_kind".into(), 9);
    file.rack = vec![
        vec!["lfo1".into()],
        vec!["vcf1".into()],
        vec!["speak1".into()],
    ];
    file.groups
        .insert("brass".into(), vec!["lfo1".into(), "vcf1".into()]);
    file
}

fn wall(store: Option<crate::store::Store>) -> (Wall, Engine) {
    let (engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), None);
    (
        Wall::new(
            crate::seed::seed().unwrap(),
            control,
            None,
            Vec::new(),
            store,
        ),
        engine,
    )
}

fn ask(wall: &mut Wall, request: &Request, now: Instant) -> Change {
    let outcome = wall.request("Tom", request, now).unwrap();
    serde_json::from_value::<ChangeResult>(outcome.result)
        .unwrap()
        .change
}

fn add() -> Request {
    Request::Add {
        alias: None,
        kind: "lfo".into(),
        name: None,
        place: None,
    }
}

fn changed_module(change: &Change) -> &ModuleRecord {
    match &change.what {
        What::Add { module } | What::Remove { module, .. } | What::Restore { module, .. } => module,
        other => panic!("expected a module change, got {other:?}"),
    }
}

#[test]
fn a_birth_reads_back_exactly() {
    for text in [
        MARK,
        "found 3217 d4ae3c0ffee11223 vco3",
        "born 0.0 0000-01-01T00:00:00Z 0000000000000000 ffffffffffffffff",
    ] {
        assert_eq!(text.parse::<Birth>().unwrap().to_string(), text);
    }
    for bad in [
        "",
        "born x",
        "found  3217 d4ae3c0ffee11223 vco3",
        "found 03217 d4ae3c0ffee11223 vco3",
        "found 3217 D4ae3c0ffee11223 vco3",
        "found 3217 d4ae3c0ffee1122 vco3",
        "found 3217 d4ae3c0ffee11223 ../x",
        "unknown 1",
        "born 06374.0 2026-10-13T08:48:24Z 5f3c0e2a91b4d7e8 91d0c3a2b4e5f607",
        "born 6374.00 2026-10-13T08:48:24Z 5f3c0e2a91b4d7e8 91d0c3a2b4e5f607",
        "born 6374.0 2026-10-13T08:48:24 5f3c0e2a91b4d7e8 91d0c3a2b4e5f607",
        "born 1.0 2026-02-30T08:48:24Z 0000000000000000 0000000000000000",
    ] {
        assert!(
            matches!(bad.parse::<Birth>(), Err(TreeError::BadBirth(_))),
            "{bad}"
        );
    }
}

#[test]
fn births_order_found_first_then_by_time() {
    let found: Birth = "found 3217 d4ae3c0ffee11223 vco3".parse().unwrap();
    let early: Birth = MARK.parse().unwrap();
    let late: Birth = MARK
        .replace("6374.0", "1.0")
        .replace("08:48:24", "08:48:25")
        .parse()
        .unwrap();
    assert!(found < early && early < late);
    assert!(early < MARK.replace("6374.0", "6375.0").parse().unwrap());
    assert!(early < MARK.replace("6374.0", "6374.1").parse().unwrap());
}

#[test]
fn stamping_numbers_each_newborn_of_a_change() {
    let existing = record("lfo5", "lfo");
    let restore = What::Restore {
        module: existing,
        cables: vec![],
        replaced: vec![],
        skipped: vec![],
        place: None,
    };
    let mut marked = record("lfo4", "lfo");
    marked.birth = Some(MARK.into());
    let mut what = What::Chain {
        name: "test".into(),
        changes: vec![
            What::Add {
                module: record("lfo1", "lfo"),
            },
            What::Turn {
                module: "lfo1".into(),
                knob: "rate".into(),
                from: 1.0,
                to: 2.0,
                glide_beats: 0.0,
            },
            What::Add {
                module: record("lfo2", "lfo"),
            },
            What::Chain {
                name: "nested".into(),
                changes: vec![What::Add {
                    module: record("lfo3", "lfo"),
                }],
            },
            restore.clone(),
            What::Add {
                module: marked.clone(),
            },
        ],
    };
    let births = stamp_births(&mut what, 10, "2026-10-13T08:48:24Z", 1, 2);
    assert_eq!(births.len(), 3);
    for (n, (id, birth)) in births.iter().enumerate() {
        assert_eq!(id, &format!("lfo{}", n + 1));
        assert_eq!(
            birth.to_string(),
            format!("born 10.{n} 2026-10-13T08:48:24Z 0000000000000001 0000000000000002")
        );
    }
    let What::Chain { changes, .. } = what else {
        panic!()
    };
    assert_eq!(changes[4], restore);
    assert_eq!(changes[5], What::Add { module: marked });
    assert!(stamp_births(&mut changes[4].clone(), 11, "2026-10-13T08:48:24Z", 1, 2).is_empty());
}

#[test]
fn an_added_module_is_born_with_its_change() {
    let dir = tempfile::tempdir().unwrap();
    let store = crate::store::Store::open(dir.path()).unwrap();
    let (mut wall, _engine) = wall(Some(store));
    assert!(
        wall.patch()
            .modules()
            .iter()
            .all(|module| module.birth.is_some())
    );
    let check = |wall: &Wall, change: &Change, expected_print: u64| {
        let birth = changed_module(change)
            .birth
            .as_ref()
            .unwrap()
            .parse::<Birth>()
            .unwrap();
        let Birth::Born {
            seq,
            n,
            at,
            salt,
            print,
        } = birth
        else {
            panic!()
        };
        assert_eq!(seq, change.seq);
        assert_eq!(n, 0);
        assert_eq!(at, change.at);
        assert_eq!(print, expected_print);
        assert_eq!(
            wall.patch().fingerprint_word(),
            crate::chance::fnv1a64(&{
                let mut file = wall.patch().to_file();
                file.last_seq = 0;
                file.wall = 0;
                serde_json::to_vec(&file).unwrap()
            })
        );
        salt
    };
    let mut expected = wall.patch().clone();
    expected.add("lfo", None, None).unwrap();
    let first = ask(&mut wall, &add(), Instant::now());
    let first_salt = check(&wall, &first, expected.fingerprint_word());
    let mut expected = wall.patch().clone();
    expected.add("lfo", None, None).unwrap();
    let second = ask(&mut wall, &add(), Instant::now());
    assert_ne!(
        first_salt,
        check(&wall, &second, expected.fingerprint_word())
    );
    let log = std::fs::read_to_string(dir.path().join("log.jsonl")).unwrap();
    let line = log
        .lines()
        .find(|line| line.contains("\"op\":\"add\""))
        .unwrap();
    let logged: Change = serde_json::from_str(line).unwrap();
    assert_eq!(logged, first);
}

/// Every module's birth, in wall order; each is there and none repeats.
fn births(patch: &Patch) -> Vec<Birth> {
    let births: Vec<Birth> = patch
        .modules()
        .iter()
        .map(|module| module.birth.clone().unwrap())
        .collect();
    assert_eq!(
        births.iter().collect::<HashSet<_>>().len(),
        births.len(),
        "{births:?}"
    );
    births
}

#[test]
fn births_survive_a_load_its_undo_and_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let (engine_now, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), None);
    let store = crate::store::Store::open(dir.path()).unwrap();
    let mut wall = Wall::new(
        crate::seed::seed().unwrap(),
        control,
        None,
        Vec::new(),
        Some(store),
    );
    wall.set_recordings_dir(dir.path().join("recordings"));
    let now = Instant::now();
    ask(&mut wall, &add(), now);
    let saved = births(wall.patch());
    wall.request(
        "Tom",
        &Request::Save {
            name: "kept".into(),
            overwrite: false,
        },
        now,
    )
    .unwrap();
    ask(&mut wall, &add(), now);
    let before_load = births(wall.patch());
    let outcome = wall
        .request(
            "Tom",
            &Request::Load {
                name: "kept".into(),
            },
            now,
        )
        .unwrap();
    let loaded: crate::protocol::LoadResult = serde_json::from_value(outcome.result).unwrap();
    assert_eq!(births(wall.patch()), saved, "a load carries its births");
    wall.request(
        "Tom",
        &Request::Undo {
            change: loaded.change.seq,
        },
        now,
    )
    .unwrap();
    assert_eq!(
        births(wall.patch()),
        before_load,
        "and its undo brings them back"
    );
    wall.save(now);
    drop((wall, engine_now));
    let store = crate::store::Store::peek(dir.path()).unwrap();
    let crate::store::Loaded::Patch { patch, .. } = store.load_patch().unwrap() else {
        panic!("no patch.json");
    };
    let (_engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), None);
    let restarted = Wall::new(*patch, control, None, Vec::new(), None);
    assert_eq!(
        births(restarted.patch()),
        before_load,
        "and a restart keeps them"
    );
}

#[test]
fn a_birth_survives_save_load_and_the_undo_of_a_remove() {
    let (mut wall, _engine) = wall(None);
    let now = Instant::now();
    let added = ask(&mut wall, &add(), now);
    let module = changed_module(&added);
    let removed = ask(
        &mut wall,
        &Request::Remove {
            module: module.id.clone(),
        },
        now,
    );
    assert_eq!(changed_module(&removed).birth, module.birth);
    let restored = ask(
        &mut wall,
        &Request::Undo {
            change: removed.seq,
        },
        now,
    );
    assert_eq!(changed_module(&restored).birth, module.birth);
    let file = wall.patch().to_file();
    assert_eq!(
        Patch::load(&file).unwrap().0.to_file().modules,
        file.modules
    );
}

#[test]
fn an_old_save_is_found_the_same_way_every_time() {
    let value: serde_json::Value = serde_json::from_slice(FIXTURE).unwrap();
    assert!(
        value["modules"]
            .as_array()
            .unwrap()
            .iter()
            .all(|module| module.get("birth").is_none())
    );
    assert_eq!(fixture().wall, 3217);
    let seedless = {
        let mut value = value;
        value.as_object_mut().unwrap().remove("seed");
        serde_json::to_vec(&value).unwrap()
    };
    let mut seeds = Vec::new();
    for bytes in [FIXTURE.to_vec(), seedless] {
        // Read whole twice, from the bytes: the same seed and marks.
        let read = || {
            let file = crate::migrate::read(&bytes).unwrap().0;
            (file.seed, Patch::load(&file).unwrap().0.to_file().modules)
        };
        let (seed, first) = read();
        assert_eq!(read(), (seed, first.clone()));
        assert_eq!(first.len(), 85);
        for module in first {
            assert_eq!(
                module.birth,
                Some(format!("found 3217 {seed:016x} {}", module.id))
            );
        }
        seeds.push(seed);
    }
    // The seedless copy's seed comes from its own bytes, not the save's.
    assert_ne!(seeds[0], seeds[1]);
}

#[test]
fn a_damaged_or_doubled_birth_is_found_afresh_and_said() {
    let mut file = Patch::empty(120.0).to_file();
    file.modules = vec![
        record("lfo1", "lfo"),
        record("lfo2", "lfo"),
        record("lfo3", "lfo"),
    ];
    file.modules[0].birth = Some(MARK.into());
    file.modules[1].birth = Some(MARK.into());
    file.modules[2].birth = Some("born x".into());
    let (patch, notes) = Patch::load(&file).unwrap();
    assert_eq!(
        patch.find("lfo1").unwrap().record().birth,
        Some(MARK.into())
    );
    for id in ["lfo2", "lfo3"] {
        assert_eq!(
            patch.find(id).unwrap().record().birth,
            Some(format!("found 0 0000000000000000 {id}"))
        );
    }
    assert_eq!(notes.len(), 2);
    assert!(notes.iter().any(|note| note.contains("birth mark of lfo1")));
    assert!(
        notes
            .iter()
            .any(|note| note.contains("does not read ('born x')"))
    );
    let mut restored = Patch::empty(120.0);
    let What::Restore { module, .. } = restored.restore(&record("lfo7", "lfo"), &[], None).unwrap()
    else {
        panic!()
    };
    assert_eq!(module.birth, Some("found 0 0000000000000000 lfo7".into()));
}

#[test]
fn a_found_mark_belongs_to_the_module_it_names() {
    // A found mark names its module; on another module it is that module's
    // mark, and the module it names is found to it: births stay unique.
    let mut file = Patch::empty(120.0).to_file();
    let mut forged = record("lfo2", "lfo");
    forged.birth = Some("found 0 0000000000000000 lfo1".into());
    file.modules = vec![forged.clone(), record("lfo1", "lfo")];
    let (patch, notes) = Patch::load(&file).unwrap();
    for id in ["lfo1", "lfo2"] {
        assert_eq!(
            patch.find(id).unwrap().record().birth,
            Some(format!("found 0 0000000000000000 {id}"))
        );
    }
    assert_eq!(
        notes,
        vec!["module 'lfo2' had the birth mark of lfo1; it is found afresh".to_string()]
    );
    let mut restored = Patch::empty(120.0);
    restored.restore(&forged, &[], None).unwrap();
    restored.restore(&record("lfo1", "lfo"), &[], None).unwrap();
    for id in ["lfo1", "lfo2"] {
        assert_eq!(
            restored.find(id).unwrap().record().birth,
            Some(format!("found 0 0000000000000000 {id}"))
        );
    }
}

#[test]
fn old_records_and_lines_read_without_a_birth() {
    let json = r#"{"id":"lfo1","kind":"lfo","knobs":{}}"#;
    let module: ModuleRecord = serde_json::from_str(json).unwrap();
    assert_eq!(module.birth, None);
    assert_eq!(serde_json::to_string(&module).unwrap(), json);
    let change = format!(
        r#"{{"seq":1,"at":"2026-10-13T08:48:24Z","seat":"Tom","what":{{"op":"add","module":{json}}},"summary":"added"}}"#
    );
    assert_eq!(
        changed_module(&serde_json::from_str::<Change>(&change).unwrap()).birth,
        None
    );
}

fn roundtrip(file: &PatchFile, compare_files: bool) {
    let files = to_files(file).unwrap();
    let decoded = from_files(&files).unwrap();
    if compare_files {
        assert_eq!(to_files(&decoded).unwrap(), files);
    } else {
        assert_eq!(
            serde_json::to_vec_pretty(&decoded).unwrap(),
            serde_json::to_vec_pretty(file).unwrap()
        );
    }
}

fn random_roundtrips(compare_files: bool) {
    roundtrip(&fixture(), compare_files);
    roundtrip(&sample(), compare_files);
    for value in [
        0.0,
        -0.0,
        f64::MAX,
        f64::MIN,
        f64::MIN_POSITIVE,
        f64::from_bits(1),
        1.0e-100,
        -1.0e-100,
    ] {
        let mut file = sample();
        file.modules[0].knobs.insert("edge".into(), value);
        file.speech.get_mut("speak1").unwrap().text = (0..=31)
            .chain([127])
            .map(|code| char::from_u32(code).unwrap())
            .collect();
        roundtrip(&file, compare_files);
    }
    for seed in 0..64 {
        let (mut wall, mut engine) = wall(None);
        let start = Instant::now();
        let mut random = kazoo_fx::dsp::splitmix64(seed);
        let mut undo = None;
        for step in 0..50 {
            random = kazoo_fx::dsp::splitmix64(random);
            let file = wall.patch().to_file();
            let module = file.modules.iter().find(|m| m.kind == "lfo");
            let request = match (random % 6, module) {
                (1, Some(module)) => Request::Remove {
                    module: module.id.clone(),
                },
                (2, Some(module)) => Request::Turn {
                    module: module.id.clone(),
                    knob: "rate".into(),
                    value: 0.35,
                    glide_beats: Some(0.0),
                },
                (3, Some(module)) => Request::Patch {
                    from: format!("{}.out", module.id),
                    to: "vcf1.cutoff".into(),
                    amount: Some(-0.4),
                },
                (4, _) if !file.cables.is_empty() => Request::Unpatch {
                    cable: Some(file.cables[0].id),
                    to: None,
                },
                (5, _) if undo.is_some() => Request::Undo {
                    change: undo.unwrap(),
                },
                _ => add(),
            };
            match wall.request("Tom", &request, start + Duration::from_secs(step)) {
                Ok(outcome) => {
                    if let Some(change) = outcome.change {
                        undo = Some(change.seq);
                    }
                }
                Err(error) => assert!(
                    matches!(request, Request::Undo { .. }),
                    "{request:?}: {error}"
                ),
            }
            let mut output = [0.0; 64];
            engine.render(&mut output, 1);
            births(wall.patch());
            roundtrip(&wall.patch().to_file(), compare_files);
        }
    }
}

#[test]
fn save_to_files_to_save_is_identical() {
    random_roundtrips(false);
}
#[test]
fn files_to_save_to_files_is_byte_identical() {
    random_roundtrips(true);
}

#[test]
fn the_files_are_written_as_shown() {
    let mut file = sample();
    file.modules.pop();
    file.modules[1].name = Some("low brass".into());
    file.modules[1].knobs.remove("unknown");
    file.speech.clear();
    file.retired_speech.clear();
    file.groups.clear();
    file.next_ids.clear();
    file.next_ids.insert("bbd".into(), 2);
    file.dye.insert(
        "vcf1".into(),
        BTreeMap::from([("Cassio".into(), 3.21), ("Tom W".into(), 1.0)]),
    );
    file.rack = vec![vec!["lfo1".into()], vec!["vcf1".into()]];
    let files = to_files(&file).unwrap();
    assert_eq!(
        String::from_utf8(files["wall.toml"].clone()).unwrap(),
        "version = 2\ntempo = 97.3\nlast_seq = 6373\nwall = 6373\nseed = \"ffffffffffffffff\"\nnext_cable = 281\nmodules = [\n    \"lfo1\",\n    \"vcf1\",\n]\nrack = [\n    [\"lfo1\"],\n    [\"vcf1\"],\n]\n\n[next_ids]\nbbd = 2\n\n[groups]\n"
    );
    assert_eq!(
        String::from_utf8(files["modules/vcf1.toml"].clone()).unwrap(),
        format!(
            "kind = \"vcf\"\nname = \"low brass\"\nbirth = \"{MARK}\"\n\n[knobs]\ncutoff = 0.35\nres = 0.2\n\n[inputs]\ncutoff = {{ cable = 150, from = \"lfo1.out\", amount = -0.4 }}\n\n[dye]\nCassio = 3.21\n\"Tom W\" = 1.0\n"
        )
    );
}

#[test]
fn every_cable_lives_in_the_module_it_feeds() {
    let file = fixture();
    assert_eq!((file.modules.len(), file.cables.len()), (85, 121));
    assert!(file.speech.is_empty() && file.retired_speech.is_empty());
    let files = to_files(&file).unwrap();
    for cable in file.cables {
        let id = cable.to.split_once('.').unwrap().0;
        let needle = format!("cable = {},", cable.id);
        let holders: Vec<_> = files
            .iter()
            .filter(|(_, bytes)| std::str::from_utf8(bytes).unwrap().contains(&needle))
            .map(|(path, _)| path.clone())
            .collect();
        assert_eq!(holders, vec![format!("modules/{id}.toml")]);
    }
}

#[test]
fn what_is_not_one_wall_is_refused() {
    let file = sample();
    let mut bads = vec![];
    let mut bad = file.clone();
    bad.modules[0].id = "../x".into();
    bads.push(bad);
    let mut bad = file.clone();
    bad.modules.push(bad.modules[0].clone());
    bads.push(bad);
    for to in ["missing.in", "vcf1.", "vcf1", ".in"] {
        let mut bad = file.clone();
        bad.cables[0].to = to.into();
        bads.push(bad);
    }
    let mut bad = file.clone();
    let mut cable = bad.cables[0].clone();
    cable.id += 1;
    bad.cables.push(cable);
    bads.push(bad);
    let mut bad = file.clone();
    let mut cable = bad.cables[0].clone();
    cable.to = "vcf1.res".into();
    bad.cables.push(cable);
    bads.push(bad);
    let mut bad = file.clone();
    let mut cable = bad.cables[0].clone();
    cable.id -= 1;
    cable.to = "vcf1.res".into();
    bad.cables.push(cable);
    bads.push(bad);
    let mut bad = file.clone();
    bad.dye.insert("missing".into(), BTreeMap::new());
    bads.push(bad);
    let mut bad = file.clone();
    bad.speech.insert(
        "missing".into(),
        Words {
            text: String::new(),
            voice: None,
        },
    );
    bads.push(bad);
    for bad in bads {
        assert!(matches!(to_files(&bad), Err(TreeError::NotOneWall(_))));
    }
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let mut bad = file.clone();
        bad.tempo = value;
        assert!(matches!(to_files(&bad), Err(TreeError::NotANumber { .. })));
        let mut bad = file.clone();
        bad.modules[0].knobs.insert("rate".into(), value);
        assert!(matches!(to_files(&bad), Err(TreeError::NotANumber { .. })));
        let mut bad = file.clone();
        bad.cables[0].amount = value;
        assert!(matches!(to_files(&bad), Err(TreeError::NotANumber { .. })));
        let mut bad = file.clone();
        bad.dye
            .get_mut("vcf1")
            .unwrap()
            .insert("Tom W.".into(), value);
        assert!(matches!(to_files(&bad), Err(TreeError::NotANumber { .. })));
    }
    for field in ["wall", "last_seq"] {
        let mut bad = file.clone();
        if field == "wall" {
            bad.wall = u64::MAX;
        } else {
            bad.last_seq = u64::MAX;
        }
        assert!(matches!(to_files(&bad), Err(TreeError::TooLarge { .. })));
    }
    refused_files(&file);
}

fn refused_files(file: &PatchFile) {
    let files = to_files(file).unwrap();
    let mut bad = files.clone();
    bad.insert("other".into(), vec![]);
    assert!(matches!(from_files(&bad), Err(TreeError::UnknownFile(_))));
    let mut bad = files.clone();
    bad.insert("modules/../x.toml".into(), vec![]);
    assert!(matches!(from_files(&bad), Err(TreeError::UnknownFile(_))));
    for path in ["wall.toml", "modules/lfo1.toml"] {
        let mut bad = files.clone();
        bad.remove(path);
        assert!(matches!(from_files(&bad), Err(TreeError::Missing(_))));
    }
    let mut bad = files.clone();
    bad.insert(
        "modules/extra.toml".into(),
        b"kind = \"lfo\"\n[knobs]\n".to_vec(),
    );
    assert!(matches!(from_files(&bad), Err(TreeError::UnknownFile(_))));
    for (path, old, new) in [
        ("wall.toml", "version = 2", "version = nope"),
        ("wall.toml", "version = 2", "unexpected = 2\nversion = 2"),
        ("wall.toml", "ffffffffffffffff", "FFFFFFFFFFFFFFFF"),
        ("modules/vcf1.toml", MARK, "born x"),
        ("modules/vcf1.toml", "[knobs]", "unexpected = 2\n[knobs]"),
        (
            "modules/vcf1.toml",
            "cable = 150,",
            "oops = 1, cable = 150,",
        ),
    ] {
        let mut bad = files.clone();
        let text = String::from_utf8(bad[path].clone())
            .unwrap()
            .replace(old, new);
        bad.insert(path.into(), text.into_bytes());
        let error = from_files(&bad).unwrap_err();
        if new == "born x" {
            assert!(matches!(error, TreeError::BadBirth(_)));
        } else {
            assert!(
                matches!(error, TreeError::Parse { .. }),
                "{path}: {new}: {error}"
            );
        }
    }
    let mut bad = files;
    bad.insert("modules/lfo1.toml".into(), b"kind = \"lfo\"\n[knobs]\n[inputs]\nrate = { cable = 150, from = \"lfo1.out\", amount = 1.0 }\n".to_vec());
    assert!(matches!(from_files(&bad), Err(TreeError::NotOneWall(_))));
}
