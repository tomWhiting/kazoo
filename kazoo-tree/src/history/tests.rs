use std::fmt::Write as _;
use std::io::Write;
use std::process::{Command, Stdio};

use super::*;

/// A `fast-import` stream being written, one commit at a time.
#[derive(Default)]
struct Stream {
    text: String,
    marks: u32,
}

impl Stream {
    /// Commit change `n` on `branch` (after mark `from`), with `files`
    /// written or (`None`) removed; answers its mark.
    fn commit(
        &mut self,
        branch: &str,
        from: Option<u32>,
        n: u32,
        files: &[(&str, Option<&str>)],
        sound: Option<&str>,
    ) -> u32 {
        let sound = sound.map_or_else(String::new, |s| format!("Kazoo-Sound: {s}\n"));
        let message = format!(
            "Tom turned vcf1 cutoff {n}\n\nKazoo-Kind: change\nKazoo-Seq: {n}\nKazoo-Beat: {}\nKazoo-Tempo: 100\n{sound}",
            f64::from(n) * 4.0
        );
        self.write(branch, from, 1_000 + n, &message, files)
    }

    /// Commit `message` on `branch` (after mark `from`) at second `at`,
    /// with `files` written or (`None`) removed; answers its mark.
    fn write(
        &mut self,
        branch: &str,
        from: Option<u32>,
        at: u32,
        message: &str,
        files: &[(&str, Option<&str>)],
    ) -> u32 {
        self.marks += 1;
        write!(
            self.text,
            "commit refs/heads/{branch}\nmark :{}\nauthor Tom <tom@kazoo.invalid> {at} +0000\ncommitter Tom <tom@kazoo.invalid> {at} +0000\ndata {}\n{message}\n",
            self.marks,
            message.len()
        )
        .expect("a String takes it");
        if let Some(from) = from {
            writeln!(self.text, "from :{from}").expect("a String takes it");
        }
        for (path, body) in files {
            // A module's file is a module's TOML, as the wall writes it; a
            // word given for one is its name, so each reads as itself.
            let module;
            let body = match (path.strip_prefix("modules/"), body) {
                (Some(file), Some(body)) if !body.starts_with("kind") => {
                    let kind = crate::colour::kind_of(file.trim_end_matches(".toml"));
                    module = format!("kind = \"{kind}\"\nname = \"{body}\"\n\n[knobs]\n");
                    Some(module.as_str())
                }
                _ => *body,
            };
            match body {
                Some(body) => write!(
                    self.text,
                    "M 100644 inline {path}\ndata {}\n{body}\n",
                    body.len()
                )
                .expect("a String takes it"),
                None => writeln!(self.text, "D {path}").expect("a String takes it"),
            }
        }
        self.text.push('\n');
        self.marks
    }

    /// Point `tag` at mark `at`.
    fn tag(&mut self, tag: &str, at: u32) {
        write!(self.text, "reset refs/tags/{tag}\nfrom :{at}\n\n").expect("a String takes it");
    }

    /// Feed the stream to `git fast-import` in `dir`.
    fn import(&self, dir: &std::path::Path) {
        let mut child = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["fast-import", "--quiet"])
            .stdin(Stdio::piped())
            .spawn()
            .expect("fast-import runs");
        child
            .stdin
            .take()
            .expect("its input")
            .write_all(self.text.as_bytes())
            .expect("the stream is written");
        assert!(child.wait().expect("fast-import ends").success());
    }
}

const BUD: &str = "bud/2/20261014T100000Z";
const SOUND: &str = "rms=-20.0 peak=-6.0 centroid=800 low=0.300 mid=0.600 high=0.100 onsets=2.0";

/// Plant a bare tree in a temporary folder and grow it with `fast-import`:
/// five moments on the trunk, a bud of two from the second, a mark and a
/// save, and HEAD on the bud.
fn planted() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("a temporary folder");
    kazoo_wall::tree::git::plant(dir.path()).expect("planted");
    let mut stream = Stream::default();
    let one = stream.commit(
        "trunk",
        None,
        1,
        &[("modules/vco1.toml", Some("a")), ("wall.toml", Some("t"))],
        None,
    );
    let two = stream.commit(
        "trunk",
        Some(one),
        2,
        &[("modules/vcf1.toml", Some("b"))],
        None,
    );
    let three = stream.commit(
        "trunk",
        Some(two),
        3,
        &[("modules/lfo1.toml", Some("c"))],
        Some(SOUND),
    );
    let four = stream.commit(
        "trunk",
        Some(three),
        4,
        &[("modules/vco1.toml", None)],
        None,
    );
    stream.commit("trunk", Some(four), 5, &[("wall.toml", Some("u"))], None);
    let six = stream.commit(BUD, Some(two), 6, &[("modules/env1.toml", Some("d"))], None);
    stream.commit(BUD, Some(six), 7, &[("wall.toml", Some("v"))], None);
    stream.tag("mark/before+the+rock", three);
    stream.tag("save/the+rock+100/20261014T101010Z", four);
    stream.import(dir.path());
    let status = Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["symbolic-ref", "HEAD", &format!("refs/heads/{BUD}")])
        .status()
        .expect("HEAD moves");
    assert!(status.success());
    dir
}

#[test]
fn the_tree_reads_as_limbs_marks_and_where_the_wall_is() {
    let dir = planted();
    let history = History::read(dir.path()).expect("the tree reads");
    assert_eq!(history.moments.len(), 7);
    assert_eq!(history.limbs.len(), 2);
    let trunk = &history.limbs[0];
    assert_eq!(trunk.name, "trunk");
    assert_eq!(trunk.moments.len(), 5);
    assert_eq!(trunk.from, None);
    let bud = &history.limbs[1];
    assert_eq!(bud.name, "bud/2/20261014T100000Z");
    assert_eq!(bud.moments.len(), 2);
    let from = bud.from.expect("the bud grew from the trunk");
    assert_eq!(history.moments[from].seq, Some(2));
    let modules: Vec<u32> = trunk
        .moments
        .iter()
        .map(|&m| history.moments[m].modules)
        .collect();
    assert_eq!(modules, [1, 2, 3, 2, 2]);
    assert_eq!(history.moments[bud.moments[0]].modules, 3);
    assert_eq!(
        history.marks,
        [
            Mark {
                name: "before the rock".into(),
                save: false,
                moment: trunk.moments[2]
            },
            Mark {
                name: "the rock 100".into(),
                save: true,
                moment: trunk.moments[3]
            },
        ]
    );
    assert_eq!(history.here, Some(bud.moments[1]));
    let three = &history.moments[trunk.moments[2]];
    assert_eq!(three.seat, "Tom");
    assert_eq!(three.summary, "Tom turned vcf1 cutoff 3");
    assert_eq!(three.beat, Some(12.0));
    assert_eq!(three.tempo, Some(100.0));
    assert_eq!(
        three.sound,
        Some(Sound {
            rms_db: -20.0,
            peak_db: -6.0,
            centroid_hz: 800.0,
            low: 0.3,
            mid: 0.6,
            high: 0.1,
            onsets: 2.0
        })
    );
    assert_eq!(history.place_of(bud.moments[1]), Some((1, 1)));
}

#[test]
fn an_empty_tree_reads_empty() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    kazoo_wall::tree::git::plant(dir.path()).expect("planted");
    let history = History::read(dir.path()).expect("an empty tree reads");
    assert!(history.moments.is_empty());
    assert!(history.limbs.is_empty());
    assert_eq!(history.here, None);
    assert_eq!(
        Snapshot::read(dir.path()).expect("read"),
        Snapshot::default()
    );
}

#[test]
fn the_snapshot_moves_when_the_wall_moves() {
    let dir = planted();
    let before = Snapshot::read(dir.path()).expect("read");
    let status = Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["symbolic-ref", "HEAD", "refs/heads/trunk"])
        .status()
        .expect("HEAD moves");
    assert!(status.success());
    assert_ne!(Snapshot::read(dir.path()).expect("read again"), before);
}

fn record(id: &str, parents: &str, body: &str, files: &str) -> String {
    record_at(id, parents, 1000, body, files)
}

/// A log record as `grow` asks for it: `body` is the subject, then after a
/// blank line the trailers.
fn record_at(id: &str, parents: &str, time: i64, body: &str, files: &str) -> String {
    let (subject, trailers) = body.split_once("\n\n").unwrap_or((body, ""));
    format!("\u{1e}{id}\0{parents}\0{time}\0Tom\0{subject}\0{trailers}\u{1f}\n{files}")
}

#[test]
fn a_merge_is_refused() {
    let log = [
        record("a", "", "one", ""),
        record("b", "a", "two", ""),
        record("c", "a b", "three", ""),
    ]
    .concat();
    let err = History::from_git(&log, "", None).expect_err("a merge");
    assert!(err.to_string().contains("two parents"), "{err}");
}

#[test]
fn an_unknown_parent_is_refused() {
    let err = History::from_git(&record("b", "a", "two", ""), "", None).expect_err("no parent");
    assert!(err.to_string().contains("does not hold"), "{err}");
}

#[test]
fn a_bad_trailer_is_a_problem_said_not_silence_and_not_a_closed_tree() {
    let log = [
        record(
            "a",
            "",
            "one\n\nKazoo-Sound: rms=loud\nKazoo-Seq: seven",
            "",
        ),
        record("b", "a", "two\n\nKazoo-Sound: rms=-3", ""),
        record("c", "b", "three\n\nKazoo-Seq: 3", ""),
    ]
    .concat();
    let history = History::from_git(&log, "refs/heads/trunk\0c\0\n", None).expect("it opens");
    let one = &history.moments[0];
    assert_eq!((one.sound, one.seq), (None, None));
    assert_eq!(one.problems.len(), 2, "{:?}", one.problems);
    assert!(
        one.problems[0].contains("Kazoo-Sound"),
        "{:?}",
        one.problems
    );
    assert!(one.problems[1].contains("Kazoo-Seq"), "{:?}", one.problems);
    assert!(history.moments[1].problems[0].contains("no peak"));
    assert_eq!(history.moments[2].seq, Some(3));
    assert_eq!(history.problem_count(), 3);
}

#[test]
fn buds_are_claimed_in_the_order_they_grew() {
    let log = [
        record("a", "", "one", ""),
        record("b", "a", "two", ""),
        record("c", "b", "three", ""),
        record("d", "b", "late bud", ""),
        record("e", "a", "early bud", ""),
        record("f", "e", "on the early bud", ""),
    ]
    .concat();
    let refs = "refs/heads/trunk\0c\0\nrefs/heads/bud/b/20261014T120000Z\0d\0\nrefs/heads/bud/a/20261014T110000Z\0f\0\n";
    let history = History::from_git(&log, refs, Some("d")).expect("reads");
    let names: Vec<&str> = history.limbs.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(
        names,
        ["trunk", "bud/a/20261014T110000Z", "bud/b/20261014T120000Z"]
    );
    assert_eq!(history.limbs[1].moments.len(), 2);
    assert_eq!(history.limbs[1].from, Some(0));
    assert_eq!(history.limbs[2].from, Some(1));
    assert_eq!(history.here, Some(3));
}

/// The limb each moment is on, by name.
fn owners(history: &History) -> Vec<Option<&str>> {
    history
        .place
        .iter()
        .map(|p| p.map(|(limb, _)| history.limbs[limb].name.as_str()))
        .collect()
}

#[test]
fn a_branch_made_by_hand_keeps_its_place_as_it_grows() {
    // From the trunk's b: "zebra" grew first (z1 at 1003), "apple" later
    // (p1 at 1004), and "stack" on top of zebra's z1 (s1 at 1005). Then
    // zebra grows again, so its tip is the newest. Nothing changes hands.
    let early = [
        record_at("a", "", 1000, "one", ""),
        record_at("b", "a", 1001, "two", ""),
        record_at("c", "b", 1002, "three", ""),
        record_at("z1", "b", 1003, "zebra one", ""),
        record_at("p1", "b", 1004, "apple one", ""),
        record_at("s1", "z1", 1005, "stack one", ""),
    ]
    .concat();
    let refs = "refs/heads/trunk\0c\0\nrefs/heads/zebra\0z1\0\nrefs/heads/apple\0p1\0\nrefs/heads/stack\0s1\0\n";
    let before = History::from_git(&early, refs, None).expect("reads");
    let grown = [early, record_at("z2", "z1", 1006, "zebra two", "")].concat();
    let refs = "refs/heads/trunk\0c\0\nrefs/heads/zebra\0z2\0\nrefs/heads/apple\0p1\0\nrefs/heads/stack\0s1\0\n";
    let after = History::from_git(&grown, refs, None).expect("reads");
    let names = |h: &History| h.limbs.iter().map(|l| l.name.clone()).collect::<Vec<_>>();
    assert_eq!(names(&before), ["trunk", "stack", "apple", "zebra"]);
    assert_eq!(names(&after), names(&before), "the order holds");
    assert_eq!(
        owners(&after)[..6],
        owners(&before)[..],
        "no moment changes limb"
    );
    assert_eq!(owners(&after)[6], Some("zebra"));
}

#[test]
fn thousands_of_buds_are_all_kept() {
    let mut log = record_at("t0", "", 0, "root", "");
    let mut refs = String::from("refs/heads/trunk\0t0\0\n");
    for n in 0..3_000 {
        log.push_str(&record_at(&format!("b{n}"), "t0", n + 1, "a bud", ""));
        writeln!(refs, "refs/heads/bud/t0/20261014T{n:06}Z\0b{n}\0").expect("a String takes it");
    }
    let history = History::from_git(&log, &refs, None).expect("reads");
    assert_eq!(history.limbs.len(), 3_001);
    assert!(
        history.place.iter().all(Option::is_some),
        "every moment is on a limb"
    );
}

/// One more change on the trunk, made from outside, as the wall makes one.
fn grow_trunk(dir: &std::path::Path, n: u32) {
    let tip = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "refs/heads/trunk"])
        .output()
        .expect("rev-parse runs");
    let tip = String::from_utf8(tip.stdout).expect("text");
    let message = format!("Tom turned vcf1 cutoff {n}\n\nKazoo-Kind: change\nKazoo-Seq: {n}\n");
    let at = 1_000 + n;
    let text = format!(
        "commit refs/heads/trunk\ncommitter Tom <tom@kazoo.invalid> {at} +0000\ndata {}\n{message}\nfrom {}\nM 100644 inline modules/vco{n}.toml\ndata 1\nx\n\n",
        message.len(),
        tip.trim()
    );
    Stream { text, marks: 0 }.import(dir);
}

#[test]
fn a_change_made_during_a_read_waits_for_the_next_read() {
    let dir = planted();
    let mut history = History::default();
    let snapshot = Snapshot::read(dir.path()).expect("read");
    // The wall writes between the refs being read and the log.
    grow_trunk(dir.path(), 8);
    let added = history
        .grow(dir.path(), &snapshot, &[])
        .expect("no error, no race");
    assert_eq!(added, 7, "only what the snapshot names");
    assert_eq!(history.limbs[0].moments.len(), 5);
    let now = Snapshot::read(dir.path()).expect("read again");
    assert_ne!(now, snapshot);
    assert_eq!(history.grow(dir.path(), &now, &[]).expect("grows"), 1);
    assert_eq!(history.limbs[0].moments.len(), 6);
    let tip = history.limbs[0].moments[5];
    assert_eq!(history.moments[tip].modules, 3, "counted from its parent");
}

#[test]
fn reading_on_from_where_it_was_is_reading_it_whole() {
    let dir = planted();
    let mut history = History::read(dir.path()).expect("read");
    grow_trunk(dir.path(), 8);
    grow_trunk(dir.path(), 9);
    history
        .grow(dir.path(), &Snapshot::read(dir.path()).expect("read"), &[])
        .expect("grows");
    let whole = History::read(dir.path()).expect("read whole");
    let by_id = |h: &History| {
        let mut ids: Vec<(String, Option<String>, u32)> = h
            .moments
            .iter()
            .map(|m| {
                (
                    m.id.to_string(),
                    m.parent.map(|p| h.moments[p].id.to_string()),
                    m.modules,
                )
            })
            .collect();
        ids.sort();
        ids
    };
    assert_eq!(by_id(&history), by_id(&whole));
    let limbs = |h: &History| -> Vec<(String, Vec<String>)> {
        h.limbs
            .iter()
            .map(|l| {
                (
                    l.name.clone(),
                    l.moments
                        .iter()
                        .map(|&m| h.moments[m].id.to_string())
                        .collect(),
                )
            })
            .collect()
    };
    assert_eq!(limbs(&history), limbs(&whole));
    // And the same tree grows from both: every commit in the same place.
    let (a, b) = (
        crate::growth::Growth::grow(&history),
        crate::growth::Growth::grow(&whole),
    );
    for (m, moment) in history.moments.iter().enumerate() {
        let w = whole.moment_of(&moment.id).expect("held");
        assert_eq!(a.at[m], b.at[w], "{}", moment.id);
    }
}

#[test]
fn the_persons_git_settings_change_nothing() {
    let dir = planted();
    let before = History::read(dir.path()).expect("read");
    for (key, value) in [
        ("log.showRoot", "false"),
        ("log.showSignature", "true"),
        ("color.ui", "always"),
        ("core.quotePath", "true"),
        ("trailer.separators", "#"),
    ] {
        let status = Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(["config", key, value])
            .status()
            .expect("config runs");
        assert!(status.success());
    }
    let after = History::read(dir.path()).expect("read");
    let modules = |h: &History| h.moments.iter().map(|m| m.modules).collect::<Vec<_>>();
    assert_eq!(modules(&after), modules(&before));
    assert_eq!(modules(&after)[..5], [1, 2, 3, 2, 2]);
    let seqs = |h: &History| h.moments.iter().map(|m| m.seq).collect::<Vec<_>>();
    assert_eq!(seqs(&after), seqs(&before));
    assert!(
        after.moments.iter().all(|m| m.seq.is_some()),
        "every trailer still reads"
    );
    assert_eq!(after.marks, before.marks);
}

#[test]
fn a_save_without_its_time_is_said_and_still_shown() {
    let log = record("a", "", "one", "");
    let refs =
        "refs/heads/trunk\0a\0\nrefs/tags/save/old+name\0a\0\nrefs/tags/mark/nowhere\0f00\0\n";
    let history = History::from_git(&log, refs, None).expect("reads");
    assert_eq!(history.marks.len(), 1);
    assert_eq!(history.marks[0].name, "old name");
    assert_eq!(history.problems.len(), 2, "{:?}", history.problems);
    assert!(history.problems[0].contains("no time"));
    assert!(history.problems[1].contains("not a moment"));
}

#[test]
fn words_from_a_commit_cannot_drive_the_terminal() {
    let log = record("a", "", "Tom \u{1b}[2Jcleared\u{7}", "");
    let refs = "refs/heads/tru\u{1b}nk\0a\0\n";
    let history = History::from_git(&log, refs, None).expect("reads");
    assert_eq!(history.moments[0].summary, "Tom \u{fffd}[2Jcleared\u{fffd}");
    assert_eq!(history.limbs[0].name, "tru\u{fffd}nk");
}

#[test]
fn a_bud_from_a_bud_made_the_same_second_keeps_to_its_own_moments() {
    // Two buds named for the same second: the first grew from the trunk's
    // f…, the second from the first bud's b…. By name the second sorts
    // first ("bud/bbbb…" before "bud/ffff…"), yet it stops where its name
    // says it grew from, so the first bud keeps its own moment.
    let log = [
        record("ffffffffffffffff", "", "one", ""),
        record(
            "bbbbbbbbbbbbbbbb",
            "ffffffffffffffff",
            "on the first bud",
            "",
        ),
        record(
            "cccccccccccccccc",
            "bbbbbbbbbbbbbbbb",
            "on the second bud",
            "",
        ),
        record(
            "dddddddddddddddd",
            "cccccccccccccccc",
            "on the second bud",
            "",
        ),
    ]
    .concat();
    let refs = "refs/heads/trunk\0ffffffffffffffff\0\nrefs/heads/bud/ffffffffffff/20261014T120000Z\0bbbbbbbbbbbbbbbb\0\nrefs/heads/bud/bbbbbbbbbbbb/20261014T120000Z\0dddddddddddddddd\0\n";
    let history = History::from_git(&log, refs, None).expect("reads");
    let names: Vec<&str> = history.limbs.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "trunk",
            "bud/bbbbbbbbbbbb/20261014T120000Z",
            "bud/ffffffffffff/20261014T120000Z"
        ]
    );
    assert_eq!(history.limbs[1].moments, [2, 3]);
    assert_eq!(history.limbs[1].from, Some(1));
    assert_eq!(history.limbs[2].moments, [1], "the first bud keeps b");
    assert_eq!(history.limbs[2].from, Some(0));
    // Grown on, the first bud keeps b all the same.
    let log = [
        log,
        record(
            "eeeeeeeeeeeeeeee",
            "bbbbbbbbbbbbbbbb",
            "first bud grows",
            "",
        ),
    ]
    .concat();
    let refs = refs.replace("Z\0bbbbbbbbbbbbbbbb", "Z\0eeeeeeeeeeeeeeee");
    let grown = History::from_git(&log, &refs, None).expect("reads");
    assert_eq!(grown.limbs[2].moments, [1, 4]);
    assert_eq!(grown.limbs[1].moments, [2, 3]);
}

fn git_in(dir: &std::path::Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .status()
        .expect("git runs");
    assert!(status.success(), "git {args:?}");
}

#[test]
fn only_the_walls_own_refs_are_moments() {
    let dir = planted();
    let before = History::read(dir.path()).expect("read");
    // A lantern's note, a stash-like ref and a remote's copy: none is a
    // moment of the wall.
    git_in(
        dir.path(),
        &[
            "notes",
            "--ref=lanterns",
            "add",
            "-m",
            "a lantern",
            "refs/heads/trunk",
        ],
    );
    git_in(
        dir.path(),
        &[
            "update-ref",
            "refs/remotes/away/trunk",
            "refs/heads/trunk~1",
        ],
    );
    let after = History::read(dir.path()).expect("read");
    assert_eq!(after.moments.len(), before.moments.len());
    assert_eq!(after.problems, before.problems);
    assert!(after.limbs.iter().all(|limb| limb.name != NO_BRANCH));
}

#[test]
fn a_deleted_branchs_moments_stay_drawn_and_are_said() {
    let dir = planted();
    let mut history = History::default();
    history
        .grow(dir.path(), &Snapshot::read(dir.path()).expect("read"), &[])
        .expect("grows");
    git_in(dir.path(), &["symbolic-ref", "HEAD", "refs/heads/trunk"]);
    git_in(
        dir.path(),
        &["update-ref", "-d", &format!("refs/heads/{BUD}")],
    );
    history
        .grow(dir.path(), &Snapshot::read(dir.path()).expect("read"), &[])
        .expect("grows");
    let lost: Vec<&Limb> = history
        .limbs
        .iter()
        .filter(|limb| limb.name == NO_BRANCH)
        .collect();
    assert_eq!(lost.len(), 1);
    assert_eq!(lost[0].moments.len(), 2, "the bud's two moments");
    assert!(
        lost[0].from.is_some_and(|m| history.place_of(m).is_some()),
        "it still grows from its moment on the trunk"
    );
    assert!(
        history
            .problems
            .iter()
            .any(|p| p.contains("2 moments are on no branch")),
        "{:?}",
        history.problems
    );
    assert!(
        history
            .moments
            .iter()
            .enumerate()
            .all(|(m, _)| history.place_of(m).is_some())
    );
}

#[test]
fn the_wall_writing_while_the_tree_is_read_never_leaves_its_place_off_the_tree() {
    let dir = planted();
    git_in(dir.path(), &["symbolic-ref", "HEAD", "refs/heads/trunk"]);
    let writer = {
        let path = dir.path().to_path_buf();
        std::thread::spawn(move || {
            for n in 10..40 {
                grow_trunk(&path, n);
            }
        })
    };
    let mut history = History::default();
    for _ in 0..30 {
        let snapshot = Snapshot::read(dir.path()).expect("read");
        history.grow(dir.path(), &snapshot, &[]).expect("grows");
        let here = history.here.expect("HEAD is a moment");
        assert!(
            history.place_of(here).is_some(),
            "the wall's place is on a limb"
        );
        assert!(history.limbs.iter().all(|limb| limb.name != NO_BRANCH));
    }
    writer.join().expect("the writer ends");
}

#[test]
fn a_new_branch_made_by_hand_never_takes_an_older_ones_moments() {
    let log = [
        record_at("a", "", 1000, "one", ""),
        record_at("b", "a", 1001, "two", ""),
        record_at("z1", "b", 1002, "three", ""),
        record_at("z2", "z1", 1003, "four", ""),
    ]
    .concat();
    let refs = "refs/heads/trunk\0b\0\nrefs/heads/zebra\0z2\0\n";
    let mut history = History::from_git(&log, refs, None).expect("reads");
    assert_eq!(history.by_hand, ["zebra"]);
    let zebra = |h: &History| -> Vec<String> {
        let limb = h.limbs.iter().find(|l| l.name == "zebra").expect("zebra");
        limb.moments
            .iter()
            .map(|&m| h.moments[m].id.to_string())
            .collect()
    };
    assert_eq!(zebra(&history), ["z1", "z2"]);
    let later = format!("{refs}refs/heads/apple\0z1\0\n");
    let known = history.by_hand.clone();
    history
        .lay_out_with(&later, None, &known)
        .expect("lays out");
    assert_eq!(zebra(&history), ["z1", "z2"], "zebra keeps what it had");
    assert_eq!(history.by_hand, ["zebra", "apple"]);
}

#[test]
fn buds_with_a_taken_name_sort_by_their_number() {
    assert!(bud_order("bud/aa/20261014T100000Z") < bud_order("bud/aa/20261014T100000Z-2"));
    assert!(bud_order("bud/aa/20261014T100000Z-2") < bud_order("bud/aa/20261014T100000Z-10"));
    assert!(bud_order("bud/aa/20261014T100000Z-10") < bud_order("bud/aa/20261014T100001Z"));
}

#[test]
fn one_commit_in_bytes_that_are_not_text_is_its_own_problem() {
    let mut log = record("a", "", "one", "").into_bytes();
    let mut bad = record("b", "a", "tw", "").into_bytes();
    let at = bad.iter().position(|&b| b == b'w').expect("a w");
    bad[at] = 0xff;
    log.extend(bad);
    let mut history = History::default();
    history.append(&log, false).expect("reads");
    history
        .lay_out_with("refs/heads/trunk\0b\0\n", None, &[])
        .expect("lays out");
    assert_eq!(history.moments.len(), 2);
    assert!(history.moments[0].problems.is_empty());
    assert!(history.moments[1].problems[0].contains("not all UTF-8"));
    assert!(history.moments[1].summary.contains('\u{fffd}'));
}

#[test]
fn a_batch_reads_with_its_note_and_its_steps_in_order() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    kazoo_wall::tree::git::plant(dir.path()).expect("planted");
    let mut stream = Stream::default();
    let one = stream.commit("trunk", None, 1, &[("modules/vco1.toml", Some("a"))], None);
    let message = "Cassio: the brass breathes\n\nKazoo-Kind: batch\nKazoo-Seq: 2\nKazoo-Beat: 8\nKazoo-Tempo: 100\nKazoo-Note: the brass breathes\nKazoo-Step: added lfo3\nKazoo-Step: patched lfo3.out to vcf1.cutoff\nKazoo-Step: turned vcf1 res 0.4\nKazoo-Change: {\"seq\":2}\n";
    stream.write(
        "trunk",
        Some(one),
        1_002,
        message,
        &[("modules/lfo3.toml", Some("b"))],
    );
    stream.import(dir.path());
    let history = History::read(dir.path()).expect("it reads");
    let batch = history.moments[1].batch.as_deref().expect("a batch");
    assert_eq!(batch.note, "the brass breathes");
    assert_eq!(
        batch.steps,
        [
            "added lfo3",
            "patched lfo3.out to vcf1.cutoff",
            "turned vcf1 res 0.4"
        ]
    );
    assert_eq!(history.moments[1].kind, "batch");
    assert!(
        history.moments[1].problems.is_empty(),
        "{:?}",
        history.moments[1].problems
    );
    assert_eq!(history.moments[0].batch, None);
}

#[test]
fn a_batch_missing_its_words_is_said_and_still_shown() {
    let log = [
        record("a", "", "one", ""),
        record(
            "b",
            "a",
            "Cassio: lost note\n\nKazoo-Kind: batch\nKazoo-Seq: 2",
            "",
        ),
        record(
            "c",
            "b",
            "a change with a note\n\nKazoo-Kind: change\nKazoo-Note: kept\nKazoo-Step: stray",
            "",
        ),
    ]
    .concat();
    let history = History::from_git(&log, "refs/heads/trunk\0c\0\n", None).expect("it opens");
    let lost = &history.moments[1];
    let batch = lost.batch.as_deref().expect("still a batch");
    assert_eq!(batch.note, "Cassio: lost note");
    assert!(batch.steps.is_empty());
    assert_eq!(lost.problems.len(), 2, "{:?}", lost.problems);
    let stray = &history.moments[2];
    assert_eq!(stray.batch, None, "only a batch is a knot");
    assert_eq!(
        stray.problems,
        [
            "a note on a moment that is not a batch: kept",
            "a batch's change on a moment that is not a batch: stray"
        ]
    );
    assert_eq!(history.knots, [vec![1]]);
}

#[test]
fn a_record_mark_in_the_words_never_splits_a_commit_or_closes_the_tree() {
    let log = [
        record("a", "", "one", ":000000 100644 0000000000000000000000000000000000000000 1111111111111111111111111111111111111111 A\tmodules/vco1.toml\n"),
        record(
            "b",
            "a",
            "Cassio: take\u{1e}two\u{1f}three\n\nKazoo-Kind: batch\nKazoo-Note: take\u{1e}two\u{1f}three\nKazoo-Step: added lfo3",
            ":000000 100644 0000000000000000000000000000000000000000 1111111111111111111111111111111111111111 A\tmodules/lfo3.toml\n",
        ),
        record("c", "b", "after", ""),
    ]
    .concat();
    let history = History::from_git(&log, "refs/heads/trunk\0c\0\n", None).expect("it opens");
    assert_eq!(history.moments.len(), 3);
    let batch = history.moments[1].batch.as_deref().expect("a batch");
    assert_eq!(batch.note, "take\u{fffd}two\u{fffd}three");
    assert_eq!(batch.steps, ["added lfo3"]);
    assert_eq!(history.moments[1].modules, 2, "its files still read");
    assert_eq!(history.moments[2].summary, "after");
}

#[test]
fn a_record_mark_then_an_id_at_the_end_of_an_author_or_subject_never_splits_a_commit() {
    let hex = "0123456789abcdef0123456789abcdef01234567";
    let log = [
        record("a", "", "one", ":000000 100644 0000000000000000000000000000000000000000 1111111111111111111111111111111111111111 A\tmodules/vco1.toml\n"),
        format!(
            "\u{1e}b\0a\01000\0B\u{1e}{hex}\0sub\u{1e}abc\0Kazoo-Kind: change\n\u{1f}\n:000000 100644 0000000000000000000000000000000000000000 1111111111111111111111111111111111111111 A\tmodules/lfo3.toml\n"
        ),
        record("c", "b", "after", ""),
    ]
    .concat();
    let history = History::from_git(&log, "refs/heads/trunk\0c\0\n", None).expect("it opens");
    assert_eq!(history.moments.len(), 3);
    assert_eq!(history.moments[1].seat, format!("B\u{fffd}{hex}"));
    assert_eq!(history.moments[1].summary, "sub\u{fffd}abc");
    assert_eq!(history.moments[1].modules, 2);
}

/// A filter's file as the wall writes it: its cutoff, its seats' dye and
/// its birth as given.
fn filter(cutoff: f64, dye: f64, birth: &str) -> String {
    format!(
        "kind = \"vcf\"\nname = \"low\"\nbirth = \"{birth}\"\n\n[knobs]\ncutoff = {cutoff:?}\nres = 0.2\n\n[inputs]\ncutoff = {{ cable = 150, from = \"lfo1.out\", amount = -0.4 }}\n\n[dye]\nCassio = {dye:?}\n"
    )
}

#[test]
fn a_moments_tint_is_its_modules_shades_from_their_settings() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    kazoo_wall::tree::git::plant(dir.path()).expect("planted");
    let born = "born 7308.0 2026-10-14T14:06:06Z 1165dea1b321acbe aa83f0f62b7bc3c3";
    let first = filter(1000.0, 1.0, born);
    // Turned and dyed, then turned back with more dye: the same settings.
    let turned = filter(1100.0, 2.0, born);
    let back = filter(1000.0, 3.5, born);
    let mut stream = Stream::default();
    let one = stream.commit(
        "trunk",
        None,
        1,
        &[("modules/vco1.toml", Some("a")), ("wall.toml", Some("t"))],
        None,
    );
    let two = stream.commit(
        "trunk",
        Some(one),
        2,
        &[("modules/vcf1.toml", Some(&first))],
        None,
    );
    let three = stream.commit(
        "trunk",
        Some(two),
        3,
        &[("modules/vcf1.toml", Some(&turned))],
        None,
    );
    let four = stream.commit(
        "trunk",
        Some(three),
        4,
        &[("modules/vcf1.toml", Some(&back))],
        None,
    );
    let five = stream.commit("trunk", Some(four), 5, &[("modules/vco1.toml", None)], None);
    let six = stream.commit(
        "trunk",
        Some(five),
        6,
        &[("modules/out1.toml", Some("o"))],
        None,
    );
    stream.commit(
        "trunk",
        Some(six),
        7,
        &[("modules/lfo9.toml", Some("kind = [broken"))],
        None,
    );
    stream.import(dir.path());
    let history = History::read(dir.path()).expect("the tree reads");
    let moments = &history.limbs[0].moments;
    let tints: Vec<Tint> = moments.iter().map(|&m| history.moments[m].tint).collect();
    assert_eq!(tints[0].count, 1);
    assert_eq!(tints[1].count, 2);
    // A turn is a new shade, near the old; turned back, the old exactly,
    // whatever the dye says.
    assert_ne!(tints[2], tints[1]);
    assert_eq!(tints[3], tints[1]);
    // The vco taken away takes exactly its shade with it.
    assert_eq!(tints[4], tints[3].minus(&tints[0]));
    // Plumbing has no shade.
    assert_eq!(tints[5], tints[4]);
    for &m in &moments[..6] {
        assert!(
            history.moments[m].problems.is_empty(),
            "{:?}",
            history.moments[m].problems
        );
    }
    // A module file that does not read is said, and still counted.
    let last = &history.moments[moments[6]];
    assert_eq!(last.modules, 3, "vcf1, out1 and lfo9");
    assert_eq!(last.tint.count, 2);
    assert!(
        last.problems
            .iter()
            .any(|p| p.contains("could not be read")),
        "{:?}",
        last.problems
    );
}

#[test]
fn a_file_line_that_is_not_raw_or_not_a_modules_is_said() {
    let zero = "0".repeat(40);
    let one = "1".repeat(40);
    let log = record(
        "a",
        "",
        "one",
        &format!(
            "A\tmodules/vco1.toml\n:000000 100644 {zero} {one} A\t\"modules/a\\tb.toml\"\n:000000 100644 {zero} {one} A\tmodules/x/y.toml\n:000000 100644 {zero} {one} U\tmodules/vcf1.toml\n:000000 100644 {zero} {one} A\tmodules/lfo1.toml\n"
        ),
    );
    let history = History::from_git(&log, "refs/heads/trunk\0a\0\n", None).expect("it opens");
    let moment = &history.moments[0];
    assert_eq!(moment.modules, 1, "only lfo1 is a module added");
    assert_eq!(moment.tint.count, 1);
    let said = moment.problems.join("\n");
    for words in [
        "did not write as expected",
        "had to quote",
        "is no module's file",
        "does not know (U)",
    ] {
        assert!(said.contains(words), "{words}: {said}");
    }
}

#[test]
fn a_tree_read_in_two_goes_has_the_tints_of_one_read_whole() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    kazoo_wall::tree::git::plant(dir.path()).expect("planted");
    let born = "born 7308.0 2026-10-14T14:06:06Z 1165dea1b321acbe aa83f0f62b7bc3c3";
    let mut stream = Stream::default();
    let one = stream.commit("trunk", None, 1, &[("wall.toml", Some("t"))], None);
    let two = stream.commit(
        "trunk",
        Some(one),
        2,
        &[("modules/vcf1.toml", Some(&filter(800.0, 1.0, born)))],
        None,
    );
    stream.import(dir.path());
    let mut history = History::default();
    let first = Snapshot::read(dir.path()).expect("read");
    history.grow(dir.path(), &first, &[]).expect("grows");
    // Later: the filter turned (its old file was read in the first go),
    // then taken away.
    let mut later = Stream {
        text: String::new(),
        marks: two,
    };
    let three = later.write(
        "trunk",
        Some(two),
        1_003,
        "Tom turned vcf1 cutoff\n\nKazoo-Kind: change\nKazoo-Seq: 3\n",
        &[("modules/vcf1.toml", Some(&filter(4000.0, 2.0, born)))],
    );
    later.write(
        "trunk",
        Some(three),
        1_004,
        "Tom removed vcf1\n\nKazoo-Kind: change\nKazoo-Seq: 4\n",
        &[("modules/vcf1.toml", None)],
    );
    later.text = later.text.replace(
        &format!("from :{two}\n"),
        &format!("from {}\n", history.moments[history.limbs[0].moments[1]].id),
    );
    later.import(dir.path());
    let second = Snapshot::read(dir.path()).expect("read again");
    history.grow(dir.path(), &second, &[]).expect("grows on");
    let whole = History::read(dir.path()).expect("read whole");
    let tints = |history: &History| -> Vec<Tint> {
        history.limbs[0]
            .moments
            .iter()
            .map(|&m| history.moments[m].tint)
            .collect()
    };
    assert_eq!(tints(&history).len(), 4);
    assert_eq!(tints(&history), tints(&whole));
    assert_ne!(tints(&history)[2], tints(&history)[1]);
    assert_eq!(tints(&history)[3], Tint::default());
}

#[test]
fn an_invisible_format_character_is_left_out_of_what_is_shown() {
    // A right-to-left override would show "txt.exe" as "exe.txt"; a
    // zero-width space hides between letters.
    assert_eq!(clean("a\u{202E}b\u{200B}c\u{FEFF}d"), "abcd");
    assert_eq!(clean("one\ttwo"), "one\u{fffd}two");
    assert_eq!(clean("Chopin 🎹 · é"), "Chopin 🎹 · é");
}

#[test]
fn a_trailer_in_another_case_reads_and_a_second_one_is_said_not_put_over_the_first() {
    let log = [
        record(
            "a",
            "",
            "one\n\nkazoo-seq: 7\nKAZOO-TEMPO: 96\nKazoo-Seq: 9",
            "",
        ),
        record("b", "a", "two\n\nKazoo-Seq: 8\nKazoo-Gap: 3", ""),
    ]
    .concat();
    let history = History::from_git(&log, "refs/heads/trunk\0b\0\n", None).expect("it opens");
    let one = &history.moments[0];
    assert_eq!((one.seq, one.tempo), (Some(7), Some(96.0)));
    assert_eq!(one.problems.len(), 1, "{:?}", one.problems);
    assert!(
        one.problems[0].contains("Kazoo-Seq is given more than once")
            && one.problems[0].ends_with(": 9"),
        "{:?}",
        one.problems
    );
    // A trailer the tree has no use for is no problem.
    assert!(
        history.moments[1].problems.is_empty(),
        "{:?}",
        history.moments[1].problems
    );
    let sound = record("c", "", "x\n\nKazoo-Sound: rms=-3 rms=-4 peak=0", "");
    let twice = History::from_git(&sound, "refs/heads/trunk\0c\0\n", None).expect("it opens");
    assert!(
        twice.moments[0].problems[0].contains("gives rms twice"),
        "{:?}",
        twice.moments[0].problems
    );
}

#[test]
fn a_ref_line_that_does_not_read_is_said() {
    let log = [record("a", "", "one", ""), record("b", "a", "two", "")].concat();
    let refs = "refs/heads/trunk\0b\0\nrefs/heads/broken\n\0a\0\n";
    let history = History::from_git(&log, refs, None).expect("it opens");
    let said = history
        .problems
        .iter()
        .filter(|p| p.contains("refs did not read"))
        .count();
    assert_eq!(said, 2, "{:?}", history.problems);
}

#[test]
fn moments_on_no_branch_are_shared_out_the_same_whatever_order_they_were_read_in() {
    // b is on no branch, and two tips on no branch (x and y, made in the
    // same second) both grew from it: whichever was read first, x (the
    // lesser commit) holds b.
    let a = record_at("a", "", 1000, "one", "");
    let b = record_at("b", "a", 1001, "two", "");
    let x = record_at("x", "b", 1002, "x", "");
    let y = record_at("y", "b", 1002, "y", "");
    let refs = "refs/heads/trunk\0a\0\n";
    let owner = |log: String| {
        let history = History::from_git(&log, refs, None).expect("it opens");
        let b = history.moment_of("b").expect("b");
        let (limb, _) = history.place_of(b).expect("b is drawn");
        let tip = *history.limbs[limb].moments.last().expect("a tip");
        history.moments[tip].id.to_string()
    };
    assert_eq!(
        owner([a.clone(), b.clone(), x.clone(), y.clone()].concat()),
        "x"
    );
    assert_eq!(owner([a, b, y, x].concat()), "x");
}
