use std::collections::BTreeMap;
use std::path::Path;
use std::sync::mpsc;

use super::*;
use crate::daemon::wall::Wall;
use crate::engine::{Engine, EngineConfig, engine};
use crate::protocol::{Change, What};
use crate::tree::git::{Commit, Importer, Planted};
use crate::tree::thread::{Answered, TREE_QUEUE, TreeHandle, TreeItem, WholeKind};

const AT: &str = "2026-10-13T09:01:07Z";

#[test]
fn listed_parents_cover_buds_beyond_the_recent_rows() {
    let mut rig = Rig::new();
    let root = rig.here();
    let first = rig.change(400.0);
    let second = rig.change(500.0);
    rig.go(first.clone(), None);
    let bud = rig.change(600.0);
    let line = read_line(rig.path()).unwrap().0;
    let listing = super::list(rig.path(), &line, 4, Some(1)).unwrap();
    assert_eq!(listing.recent.len(), 1);
    let value = serde_json::to_value(listing).unwrap();
    assert_eq!(value["parents"][&bud.commit], first.commit);
    assert_eq!(value["parents"][&second.commit], first.commit);
    assert_eq!(value["parents"][&first.commit], root.commit);
    assert!(value["parents"][&root.commit].is_null());
}

#[test]
fn a_tree_longer_than_the_bound_is_listed_with_its_newest_parents() {
    // The listing never refuses a long tree: it sends the newest parents
    // only, and the list says a moment further back is further than that.
    let mut rig = Rig::new();
    let root = rig.here();
    let places: Vec<_> = (0..4).map(|n| rig.change(400.0 + f64::from(n))).collect();
    let here = &places[3];
    let parents = super::ancestry::within(rig.path(), &BTreeMap::new(), Some(here), 2).unwrap();
    assert_eq!(parents.len(), 2, "{parents:?}");
    assert_eq!(
        parents[&here.commit].as_deref(),
        Some(places[2].commit.as_str())
    );
    assert_eq!(
        parents[&places[2].commit].as_deref(),
        Some(places[1].commit.as_str())
    );
    assert!(!parents.contains_key(&root.commit));
}

fn command(tree: &Path, args: &[&str]) -> String {
    let result = git::git_command(args[0], Some(tree))
        .args(&args[1..])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{args:?}: {}: {}",
        result.status,
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap().trim_end().into()
}

struct Rig {
    wall: Wall,
    engine: Option<Engine>,
    dir: Option<tempfile::TempDir>,
    ticket: u64,
}

impl Rig {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        git::plant(dir.path()).unwrap();
        command(dir.path(), &["config", "core.logAllRefUpdates", "always"]);
        let file = crate::seed::seed().unwrap().to_file();
        let mut rig = Self::attach(dir, &file, None);
        rig.here();
        rig
    }

    fn attach(dir: tempfile::TempDir, file: &PatchFile, latch: Option<mpsc::Receiver<()>>) -> Self {
        let (engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), None);
        let patch = crate::patch::Patch::from_file(file).unwrap();
        let mut wall = Wall::new(patch, control, None, Vec::new(), None);
        let handle = latch.map_or_else(
            || TreeHandle::attach(dir.path()).unwrap(),
            |latch| TreeHandle::attach_paused(dir.path(), latch).unwrap(),
        );
        wall.attach_tree(handle);
        Self {
            wall,
            engine: Some(engine),
            dir: Some(dir),
            ticket: 0,
        }
    }

    fn path(&self) -> &Path {
        self.dir.as_ref().unwrap().path()
    }

    fn finish(mut self) -> (tempfile::TempDir, PatchFile) {
        let file = self.wall.tree_test_file();
        let dir = self.dir.take().unwrap();
        drop(self);
        (dir, file)
    }

    fn ask(&mut self, item: impl FnOnce(u64) -> TreeItem) -> Result<Answered, HereError> {
        self.ticket += 1;
        let ticket = self.ticket;
        assert!(self.wall.tree_test_ask(item(ticket)));
        loop {
            if let Some(i) = self
                .wall
                .tree_answered
                .iter()
                .position(|(t, _, _)| *t == ticket)
            {
                return self.wall.tree_answered.remove(i).2;
            }
            self.wall.tree_test_wait();
        }
    }

    fn here(&mut self) -> TreePlace {
        match self.ask(|ticket| TreeItem::Here { ticket }).unwrap() {
            Answered::Here(place) => place,
            other => panic!("unexpected answer: {other:?}"),
        }
    }

    fn mark(
        &mut self,
        name: &str,
        console: bool,
    ) -> Result<crate::protocol::MarkResult, HereError> {
        match self.ask(|ticket| TreeItem::Mark {
            ticket,
            name: name.into(),
            console,
        })? {
            Answered::Marked(mark) => Ok(mark),
            other => panic!("unexpected answer: {other:?}"),
        }
    }

    fn change(&mut self, value: f64) -> TreePlace {
        let mut file = self.wall.tree_test_file();
        let module = file.modules.iter_mut().find(|m| m.id == "vcf1").unwrap();
        let from = module.knobs.insert("cutoff".into(), value).unwrap();
        let what = What::Turn {
            module: "vcf1".into(),
            knob: "cutoff".into(),
            from,
            to: value,
            glide_beats: 0.0,
            after_beats: 0.0,
            curve: crate::protocol::GlideCurve::Linear,
        };
        let change = make_change(file.last_seq + 1, what);
        self.wall.tree_test_apply(&file, change);
        self.here()
    }

    /// A batch of one cutoff turn, committed as the live tree commits one
    /// (`Kazoo-Kind: batch`).
    fn batch(&mut self, value: f64) -> TreePlace {
        let mut file = self.wall.tree_test_file();
        let module = file.modules.iter_mut().find(|m| m.id == "vcf1").unwrap();
        let from = module.knobs.insert("cutoff".into(), value).unwrap();
        let what = What::Batch {
            note: "one idea".into(),
            changes: vec![What::Turn {
                module: "vcf1".into(),
                knob: "cutoff".into(),
                from,
                to: value,
                glide_beats: 0.0,
                after_beats: 0.0,
                curve: crate::protocol::GlideCurve::Linear,
            }],
            comments: Vec::new(),
        };
        let change = make_change(file.last_seq + 1, what);
        self.wall.tree_test_apply(&file, change);
        self.here()
    }

    fn go(&mut self, to: TreePlace, undoes: Option<u64>) -> TreePlace {
        let from = self.here();
        let file = *super::wall(self.path(), &to.commit).unwrap();
        let seq = self.wall.tree_test_file().last_seq + 1;
        let what = What::Go {
            to,
            from,
            backup: "before-go".into(),
            fingerprint: "0123456789abcdef-1".into(),
            modules: u32::try_from(file.modules.len()).unwrap(),
            cables: u32::try_from(file.cables.len()).unwrap(),
            tempo: file.tempo,
            by_console: true,
        };
        let mut change = make_change(seq, what);
        change.undoes = undoes;
        self.wall.tree_test_apply(&file, change);
        self.here()
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.wall.close(std::time::Instant::now());
        self.engine.take();
    }
}

fn make_change(seq: u64, what: What) -> Change {
    Change {
        notes: Vec::new(),
        seq,
        at: AT.into(),
        seat: "Tom".into(),
        summary: crate::change::summarise("Tom", &what, None),
        what,
        undoes: None,
        clock: None,
        comment: None,
    }
}

fn three(rig: &mut Rig) -> Vec<TreePlace> {
    [400.0, 500.0, 600.0]
        .into_iter()
        .map(|value| rig.change(value))
        .collect()
}

fn count(tree: &Path) -> String {
    command(tree, &["rev-list", "--all", "--count"])
}
fn names(tree: &Path) -> String {
    command(tree, &["for-each-ref", "--format=%(refname)"])
}

#[test]
fn changes_grow_the_line_head_names() {
    let mut rig = Rig::new();
    let mut previous = rig.here().commit;
    for value in [400.0, 500.0, 600.0] {
        let place = rig.change(value);
        assert_eq!(command(rig.path(), &["rev-parse", "trunk"]), place.commit);
        assert_eq!(
            command(rig.path(), &["symbolic-ref", "HEAD"]),
            "refs/heads/trunk"
        );
        assert_eq!(
            command(rig.path(), &["rev-parse", &format!("{}^", place.commit)]),
            previous
        );
        previous = place.commit;
    }
}

#[test]
fn a_go_moves_head_and_writes_nothing() {
    let mut rig = Rig::new();
    let places = three(&mut rig);
    let before = count(rig.path());
    let place = rig.go(places[1].clone(), None);
    assert_eq!(place.commit, places[1].commit);
    assert_eq!(place.branch, None);
    assert_eq!(
        command(rig.path(), &["rev-parse", "trunk"]),
        places[2].commit
    );
    assert_eq!(count(rig.path()), before);
}

#[test]
fn the_first_change_after_a_go_grows_a_bud_named_for_its_moment_and_time() {
    for by_mark in [false, true] {
        let mut rig = Rig::new();
        let moment = rig.change(400.0);
        if by_mark {
            rig.mark("dawn", true).unwrap();
        }
        rig.change(500.0);
        let mut to = moment.clone();
        if by_mark {
            to.mark = Some("dawn".into());
        }
        rig.go(to, None);
        let bud = rig.change(700.0);
        let from = if by_mark {
            "dawn"
        } else {
            &moment.commit[..12]
        };
        assert_eq!(
            bud.branch.as_deref(),
            Some(format!("bud/{from}/20261013T090107Z").as_str())
        );
        assert_eq!(
            command(rig.path(), &["rev-parse", &format!("{}^", bud.commit)]),
            moment.commit
        );
        assert_eq!(
            super::files(rig.path(), &bud.commit).unwrap(),
            crate::tree::to_files(&rig.wall.tree_test_file()).unwrap()
        );
        assert_eq!(
            command(rig.path(), &["symbolic-ref", "HEAD"]),
            format!("refs/heads/{}", bud.branch.unwrap())
        );
    }
}

#[test]
fn with_no_change_after_a_go_nothing_grows() {
    let mut rig = Rig::new();
    let places = three(&mut rig);
    let before = names(rig.path());
    let commits = count(rig.path());
    rig.go(places[0].clone(), None);
    let back = rig.go(places[2].clone(), Some(4));
    assert_eq!(back.branch.as_deref(), Some("trunk"));
    assert_eq!(names(rig.path()), before);
    assert_eq!(count(rig.path()), commits);
    assert_eq!(
        command(rig.path(), &["symbolic-ref", "HEAD"]),
        "refs/heads/trunk"
    );
}

#[test]
fn undoing_a_go_puts_head_back_on_the_line_it_left() {
    let mut rig = Rig::new();
    let places = three(&mut rig);
    rig.go(places[0].clone(), None);
    rig.go(places[2].clone(), Some(4));
    let next = rig.change(800.0);
    assert_eq!(next.branch.as_deref(), Some("trunk"));
    assert_eq!(
        command(rig.path(), &["rev-parse", &format!("{}^", next.commit)]),
        places[2].commit
    );
}

#[test]
fn undoing_an_old_go_whose_line_grew_since_leaves_the_wall_at_its_moment() {
    let mut rig = Rig::new();
    let places = three(&mut rig);
    rig.go(places[0].clone(), None);
    rig.change(700.0);
    rig.go(places[2].clone(), Some(4));
    let advanced = rig.change(800.0);
    assert_eq!(advanced.branch.as_deref(), Some("trunk"));
    let back = rig.go(places[2].clone(), Some(4));
    assert_eq!(back.branch, None);
    assert_eq!(back.commit, places[2].commit);
    let bud = rig.change(900.0);
    assert!(bud.branch.unwrap().starts_with("bud/"));
    assert_eq!(
        command(rig.path(), &["rev-parse", &format!("{}^", bud.commit)]),
        places[2].commit
    );
}

#[test]
fn a_go_by_branch_name_carries_on_that_line() {
    let mut rig = Rig::new();
    let places = three(&mut rig);
    rig.go(places[0].clone(), None);
    rig.change(700.0);
    let trunk = super::resolve(
        rig.path(),
        &super::read_line(rig.path()).unwrap().0,
        "trunk",
    )
    .unwrap();
    assert_eq!(rig.go(trunk, None).branch.as_deref(), Some("trunk"));
    assert_eq!(rig.change(800.0).branch.as_deref(), Some("trunk"));
}

#[test]
fn two_buds_from_one_moment_in_one_second_have_their_own_names() {
    let mut rig = Rig::new();
    let places = three(&mut rig);
    rig.go(places[0].clone(), None);
    let first = rig.change(700.0);
    rig.go(places[0].clone(), None);
    let second = rig.change(800.0);
    assert_eq!(second.branch, first.branch.map(|name| format!("{name}-2")));
}

#[test]
fn a_go_the_queue_dropped_is_still_taken_before_the_gap() {
    asks_that_cannot_queue_do_not_mark_the_tree_behind();
    let mut rig = Rig::new();
    let places = three(&mut rig);
    let (dir, file) = rig.finish();
    let (release, latch) = mpsc::channel();
    let mut rig = Rig::attach(dir, &file, Some(latch));
    for _ in 1..TREE_QUEUE {
        assert!(rig.wall.tree_test_ask(TreeItem::Whole {
            kind: WholeKind::Gap,
            changes: Vec::new(),
            seq: 0,
            clock: None,
            files: Files::new(),
            generation: 1
        }));
    }
    let to = places[0].clone();
    let mut file = *super::wall(rig.path(), &to.commit).unwrap();
    file.last_seq = 4;
    let what = What::Go {
        to: to.clone(),
        from: places[2].clone(),
        backup: "before-go".into(),
        fingerprint: "f".into(),
        modules: 0,
        cables: 0,
        tempo: 120.0,
        by_console: true,
    };
    rig.wall.tree_test_apply(&file, make_change(4, what));
    release.send(()).unwrap();
    while rig.wall.snapshot().tree.unwrap().through != Some(4) {
        rig.wall.tree_test_wait();
    }
    let place = rig.here();
    assert!(place.branch.unwrap().starts_with("bud/"));
    assert_eq!(
        command(rig.path(), &["rev-parse", &format!("{}^", place.commit)]),
        to.commit
    );
    assert_ne!(command(rig.path(), &["rev-parse", "trunk"]), place.commit);
}

fn asks_that_cannot_queue_do_not_mark_the_tree_behind() {
    let dir = tempfile::tempdir().unwrap();
    git::plant(dir.path()).unwrap();
    let (release, latch) = mpsc::channel();
    let mut handle = TreeHandle::attach_paused(dir.path(), latch).unwrap();
    for ticket in 0..TREE_QUEUE {
        assert!(handle.ask(TreeItem::Here {
            ticket: u64::try_from(ticket).unwrap()
        }));
    }
    assert!(!handle.ask(TreeItem::Here { ticket: 10_000 }));
    assert!(!handle.resync);
    assert_eq!(handle.behind_from, None);
    release.send(()).unwrap();
    handle.finish().unwrap();
}

#[test]
fn a_start_at_a_moment_with_the_same_wall_writes_nothing() {
    for changed in [false, true] {
        let mut rig = Rig::new();
        let places = three(&mut rig);
        rig.go(places[0].clone(), None);
        let before = count(rig.path());
        let moment = places[0].commit.clone();
        let (dir, _) = rig.finish();
        let mut file = *super::wall(dir.path(), &moment).unwrap();
        if changed {
            file.tempo = 140.0;
        }
        let mut rig = Rig::attach(dir, &file, None);
        let here = rig.here();
        if changed {
            assert!(here.branch.unwrap().starts_with("bud/"));
            assert_eq!(
                command(rig.path(), &["rev-parse", &format!("{}^", here.commit)]),
                moment
            );
        } else {
            assert_eq!(here.commit, moment);
            assert_eq!(here.branch, None);
            assert_eq!(count(rig.path()), before);
        }
    }
}

fn many_marks(tree: &Path, commit: &str, n: usize) {
    let planted = git::read(tree).unwrap();
    let mut importer = Importer::new(tree, &planted).unwrap();
    for i in 0..n {
        importer
            .tag(&format!("refs/tags/mark/m{i}"), commit)
            .unwrap();
    }
    importer.finish().unwrap();
}

#[test]
fn marks_name_where_the_wall_is_and_never_move() {
    let empty = tempfile::tempdir().unwrap();
    git::plant(empty.path()).unwrap();
    let mut handle = TreeHandle::attach(empty.path()).unwrap();
    assert!(handle.ask(TreeItem::Mark {
        ticket: 1,
        name: "first".into(),
        console: true
    }));
    while handle.answered.is_empty() {
        handle.wait_event();
    }
    assert!(
        matches!(handle.answered.remove(0).2, Err(HereError::UnknownMoment(ref why)) if why == "the tree has no moment yet: nothing has been committed")
    );
    handle.finish().unwrap();
    let mut rig = Rig::new();
    let at = rig.change(400.0);
    let mark = rig.mark("Dawn", false).unwrap();
    assert_eq!(mark.at, at);
    let before = command(rig.path(), &["rev-parse", "refs/tags/mark/Dawn"]);
    assert!(matches!(
        rig.mark("dawn", false),
        Err(HereError::MarkExists(_))
    ));
    assert_eq!(
        command(rig.path(), &["rev-parse", "refs/tags/mark/Dawn"]),
        before
    );
    many_marks(rig.path(), &at.commit, MAX_MARKS - 1);
    assert!(matches!(rig.mark("limit", false), Err(HereError::Full(_))));
    assert!(rig.mark("console", true).is_ok());
    let mut detached = at.clone();
    detached.branch = None;
    rig.go(detached, None);
    assert_eq!(rig.mark("detached", true).unwrap().at.commit, at.commit);
}

#[test]
fn mark_and_save_names_are_refs_and_read_back() {
    let dir = tempfile::tempdir().unwrap();
    git::plant(dir.path()).unwrap();
    let mut names: Vec<String> = [
        "a b",
        "a..b",
        "x.lock",
        "Gorgeous Toms",
        "a...b",
        "a.lock.lock",
    ]
    .map(str::to_string)
    .to_vec();
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789 _.-";
    let mut state = 0x51ed_270bu64;
    for i in 0..1_000 {
        let mut name = format!("a{i}");
        for _ in 0..40 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            name.push(char::from(alphabet[(state as usize) % alphabet.len()]));
        }
        name.push('z');
        names.push(name);
    }
    let mut rig = Rig::new();
    for name in ["a b", "a..b", "x.lock", "Gorgeous Toms"] {
        let marked = rig.mark(name, true).unwrap();
        assert_eq!(marked.tag, format!("mark/{}", ref_part(name)));
        assert!(rig.wall.tree_test_ask(TreeItem::SaveMark {
            name: name.into(),
            at: AT.into()
        }));
        rig.here();
        assert_eq!(
            command(
                rig.path(),
                &[
                    "rev-parse",
                    &format!(
                        "refs/tags/save/{}/20261013T090107Z",
                        ref_part(&name.to_ascii_lowercase())
                    )
                ]
            ),
            marked.at.commit
        );
    }
    for name in names {
        assert!(crate::protocol::valid_save_name(&name));
        let part = ref_part(&name);
        assert_eq!(name_of(&part), name);
        command(
            dir.path(),
            &["check-ref-format", &format!("refs/tags/mark/{part}")],
        );
        command(
            dir.path(),
            &[
                "check-ref-format",
                &format!(
                    "refs/tags/save/{}/20261013T090107Z",
                    ref_part(&name.to_ascii_lowercase())
                ),
            ],
        );
    }
}

#[test]
fn moments_resolve_by_branch_id_mark_save_change_and_prefix() {
    let mut rig = Rig::new();
    let places = three(&mut rig);
    let at = places[2].clone();
    rig.mark("Dawn", true).unwrap();
    rig.mark(&at.commit[..12], true).unwrap();
    assert!(rig.wall.tree_test_ask(TreeItem::SaveMark {
        name: "Evening".into(),
        at: AT.into()
    }));
    rig.here();
    let line = read_line(rig.path()).unwrap().0;
    for name in [
        "trunk",
        &at.commit,
        "dawn",
        "save/evening/20261013T090107Z",
        "#3",
        &at.commit[..7],
    ] {
        assert_eq!(resolve(rig.path(), &line, name).unwrap().commit, at.commit);
    }
    assert_eq!(
        resolve(rig.path(), &line, &at.commit[..12])
            .unwrap()
            .mark
            .as_deref(),
        Some(&at.commit[..12])
    );
    assert_eq!(resolve(rig.path(), &line, &at.commit).unwrap().mark, None);
    assert!(matches!(
        resolve(rig.path(), &line, "#999"),
        Err(HereError::UnknownMoment(_))
    ));
    assert!(matches!(
        resolve(rig.path(), &line, "missing"),
        Err(HereError::UnknownMoment(_))
    ));
    let saved_again = rig.change(650.0);
    assert!(rig.wall.tree_test_ask(TreeItem::SaveMark {
        name: "evening".into(),
        at: AT.into()
    }));
    rig.here();
    assert_eq!(
        command(
            rig.path(),
            &["rev-parse", "refs/tags/save/evening/20261013T090107Z"]
        ),
        at.commit
    );
    assert_eq!(
        command(
            rig.path(),
            &["rev-parse", "refs/tags/save/evening/20261013T090107Z-2"]
        ),
        saved_again.commit
    );
    imported_change_resolves();
    rig.go(places[0].clone(), None);
    let bud = rig.change(700.0);
    assert_eq!(
        resolve(rig.path(), &line, bud.branch.as_ref().unwrap())
            .unwrap()
            .branch,
        bud.branch
    );
    let mut resolver = Resolver::default();
    resolver.resolve(rig.path(), &line, "#1").unwrap();
    let next = rig.change(800.0);
    resolver
        .extend(rig.path(), next.branch.as_ref().unwrap(), 1)
        .unwrap();
    assert_eq!(
        resolver
            .resolve(rig.path(), &line, &format!("#{}", next.change.unwrap()))
            .unwrap()
            .commit,
        next.commit
    );
    ambiguous_prefix(rig.path(), &line, &at.commit);
}

fn imported_change_resolves() {
    let state = tempfile::tempdir().unwrap();
    let tree = tempfile::tempdir().unwrap();
    git::plant(tree.path()).unwrap();
    let mut file = crate::seed::seed().unwrap().to_file();
    let module = file.modules.iter_mut().find(|m| m.id == "vcf1").unwrap();
    let from = module.knobs.insert("cutoff".into(), 400.0).unwrap();
    let change = make_change(
        1,
        What::Turn {
            module: "vcf1".into(),
            knob: "cutoff".into(),
            from,
            to: 400.0,
            glide_beats: 0.0,
            after_beats: 0.0,
            curve: crate::protocol::GlideCurve::Linear,
        },
    );
    file.last_seq = 1;
    std::fs::create_dir(state.path().join("saves")).unwrap();
    std::fs::write(
        state.path().join("patch.json"),
        serde_json::to_vec(&file).unwrap(),
    )
    .unwrap();
    std::fs::write(
        state.path().join("log.jsonl"),
        format!("{}\n", serde_json::to_string(&change).unwrap()),
    )
    .unwrap();
    crate::tree::rings::grow(state.path(), tree.path(), false).unwrap();
    let line = read_line(tree.path()).unwrap().0;
    let to = resolve(tree.path(), &line, "#1").unwrap();
    assert_eq!(to.change, Some(1));
    assert_eq!(
        moment(tree.path(), &to.commit).unwrap().summary,
        change.summary
    );
}

fn ambiguous_prefix(tree: &Path, line: &Line, parent: &str) {
    let mut importer = Importer::new(
        tree,
        &Planted {
            branch: "refs/heads/collision".into(),
            tip: Some(parent.into()),
            through: None,
        },
    )
    .unwrap();
    let mut nonce = 0u64;
    let prefix = loop {
        let mut last = 0;
        for _ in 0..20_000 {
            nonce += 1;
            let change = make_change(1_000_000 + nonce, What::Unknown);
            last = importer
                .commit(&Commit::change(&change, Vec::new()).unwrap())
                .unwrap();
        }
        importer.checkpoint(last).unwrap();
        let mut prefixes = BTreeMap::new();
        let ids = command(tree, &["log", "--format=%H", "refs/heads/collision"]);
        let duplicate = ids.lines().find_map(|id| {
            let prefix = &id[..7];
            prefixes.insert(prefix, id).map(|_| prefix.to_string())
        });
        if let Some(prefix) = duplicate {
            break prefix;
        }
    };
    importer.finish().unwrap();
    assert_eq!(
        resolve(tree, line, &prefix),
        Err(HereError::UnknownMoment(format!(
            "{prefix} names more than one moment; give more of it"
        )))
    );
}

#[test]
fn a_resolved_moment_is_its_wall() {
    let mut rig = Rig::new();
    let place = rig.change(400.0);
    let expected =
        crate::tree::from_files(&super::files(rig.path(), &place.commit).unwrap()).unwrap();
    assert!(expected.modules.iter().any(|m| m.birth.is_some()));
    match rig
        .ask(|ticket| TreeItem::Resolve {
            ticket,
            moment: place.commit.clone(),
        })
        .unwrap()
    {
        Answered::Resolved { to, here, wall } => {
            assert_eq!(*wall, expected);
            assert_eq!(to.commit, place.commit);
            assert_eq!(here, place);
        }
        other => panic!("unexpected answer: {other:?}"),
    }
}

#[test]
fn the_tree_lists_within_its_bounds() {
    let mut rig = Rig::new();
    let at = rig.here();
    many_marks(rig.path(), &at.commit, 205);
    for n in 0..205 {
        rig.change(400.0 + f64::from(n));
    }
    match rig
        .ask(|ticket| TreeItem::List {
            ticket,
            recent: Some(500),
        })
        .unwrap()
    {
        Answered::Listed(list) => {
            assert_eq!(list.marks.len(), 200);
            assert_eq!(list.mark_count, 205);
            assert_eq!(list.recent.len(), 200);
            assert_eq!(list.through, 205);
            assert_eq!(list.branch_count, 1);
        }
        other => panic!("unexpected answer: {other:?}"),
    }
    rig.mark("Dawn", true).unwrap();
    let mut to = rig.here();
    to.mark = Some("Dawn".into());
    to.branch = None;
    rig.go(to, None);
    let bud = rig.change(900.0);
    let line = read_line(rig.path()).unwrap().0;
    let list = super::list(rig.path(), &line, 207, Some(0)).unwrap();
    assert_eq!(
        list.branches
            .iter()
            .find(|b| Some(&b.name) == bud.branch.as_ref())
            .unwrap()
            .grew_from
            .as_deref(),
        Some("Dawn")
    );
    assert!(list.recent.is_empty());
    let mut importer = Importer::new(rig.path(), &git::read(rig.path()).unwrap()).unwrap();
    for i in 0..205 {
        importer
            .tag(
                &format!("refs/heads/bud/source/20261013T090107Z-{i}"),
                &bud.commit,
            )
            .unwrap();
    }
    importer.finish().unwrap();
    let list = super::list(rig.path(), &line, 207, Some(500)).unwrap();
    assert_eq!(list.branch_count, 207);
    assert_eq!(list.branches.len(), MAX_LISTED);
    assert_eq!(list.marks.len(), MAX_LISTED);
}

#[test]
fn no_branch_or_tag_ever_moves_backwards() {
    let mut rig = Rig::new();
    let places = three(&mut rig);
    rig.mark("Dawn", true).unwrap();
    let before = refs(rig.path()).unwrap();
    rig.go(places[0].clone(), None);
    rig.change(700.0);
    rig.go(places[2].clone(), Some(4));
    rig.change(800.0);
    for old in before {
        let current = command(rig.path(), &["rev-parse", &old.name]);
        if old.name.starts_with("refs/tags/") {
            assert_eq!(current, old.commit);
        } else {
            let result = git::git_command("merge-base", Some(rig.path()))
                .args(["--is-ancestor", &old.commit, &current])
                .status()
                .unwrap();
            assert!(result.success());
        }
    }
    for r in refs(rig.path()).unwrap() {
        let log = command(rig.path(), &["reflog", "show", "--format=%H", &r.name]);
        if r.name.starts_with("refs/tags/") {
            assert!(log.lines().count() <= 1);
        } else {
            let ids: Vec<_> = log.lines().collect();
            for pair in ids.windows(2) {
                assert!(
                    git::git_command("merge-base", Some(rig.path()))
                        .args(["--is-ancestor", pair[1], pair[0]])
                        .status()
                        .unwrap()
                        .success()
                );
            }
        }
    }
}

#[test]
fn here_runs_only_these_git_commands() {
    let source = include_str!("../here.rs");
    let allowed = [
        "symbolic-ref",
        "update-ref",
        "rev-parse",
        "for-each-ref",
        "log",
        "ls-tree",
        "cat-file",
        "check-ref-format",
    ];
    for tail in source.split("git_command(\"").skip(1) {
        let name = tail.split('"').next().unwrap();
        assert!(allowed.contains(&name), "{name}");
    }
    assert_eq!(source.matches("git_command(\"update-ref\"").count(), 1);
    let update = source.split("git_command(\"update-ref\"").nth(1).unwrap();
    let arguments = update
        .split(';')
        .next()
        .unwrap()
        .split_once(".args([")
        .unwrap()
        .1
        .split(']')
        .next()
        .unwrap()
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    assert_eq!(
        arguments,
        ["\"--no-deref\"", "\"-m\"", "&message", "\"HEAD\"", "commit"]
    );
    assert!(!source.contains("--force"));
}

#[test]
fn seq_map_benchmark_fixture_is_imported_changes() {
    let dir = tempfile::tempdir().unwrap();
    git::plant(dir.path()).unwrap();
    let mut importer = Importer::new(
        dir.path(),
        &Planted {
            branch: "refs/heads/trunk".into(),
            tip: None,
            through: None,
        },
    )
    .unwrap();
    let mut last = 0;
    for seq in 1..=20_000 {
        let change = make_change(seq, What::Unknown);
        last = importer
            .commit(&Commit::change(&change, Vec::new()).unwrap())
            .unwrap();
    }
    importer.checkpoint(last).unwrap();
    importer.finish().unwrap();
    let started = std::time::Instant::now();
    let map = changes(dir.path(), None).unwrap();
    let elapsed = started.elapsed();
    assert_eq!(map.len(), 20_000);
    println!("BENCH seq map: {elapsed:?}, 20,000 imported change commits");
}

#[test]
fn a_batch_moment_is_a_change_with_its_number() {
    // A batch is one change (T10a): its commit says `Kazoo-Kind: batch`,
    // and the tree reads its number as any change's, so a go to it, a
    // `#N` and a restart all know it.
    let mut rig = Rig::new();
    rig.change(400.0);
    let place = rig.batch(500.0);
    assert_eq!(
        command(
            rig.path(),
            &[
                "log",
                "-1",
                "--format=%(trailers:key=Kazoo-Kind,valueonly)",
                &place.commit
            ]
        ),
        "batch"
    );
    assert_eq!(place.change, Some(2));
    let resolved = match rig
        .ask(|ticket| TreeItem::Resolve {
            ticket,
            moment: "#2".into(),
        })
        .unwrap()
    {
        Answered::Resolved { to, .. } => to,
        other => panic!("unexpected answer: {other:?}"),
    };
    assert_eq!(resolved.commit, place.commit);
    assert_eq!(resolved.change, Some(2));
    let line = read_line(rig.path()).unwrap().0;
    let listing = super::list(rig.path(), &line, 2, Some(4)).unwrap();
    assert_eq!(listing.recent[0].change, Some(2));
    assert_eq!(
        crate::tree::git::missing_changes(rig.path(), &[1, 2], Some(0)).unwrap(),
        Vec::<u64>::new(),
        "a restart finds the batch's own commit"
    );
}

mod notes;
