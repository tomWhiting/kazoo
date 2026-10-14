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
        self.marks += 1;
        let sound = sound.map_or_else(String::new, |s| format!("Kazoo-Sound: {s}\n"));
        let message = format!(
            "Tom turned vcf1 cutoff {n}\n\nKazoo-Kind: change\nKazoo-Seq: {n}\nKazoo-Beat: {}\nKazoo-Tempo: 100\n{sound}",
            f64::from(n) * 4.0
        );
        let at = 1_000 + n;
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
    stream.tag("save/the+rock+100", four);
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
    assert_eq!(stamp(dir.path()).expect("stamped"), "HEAD ");
}

#[test]
fn the_stamp_moves_when_the_tree_grows() {
    let dir = planted();
    let before = stamp(dir.path()).expect("stamped");
    let status = Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["symbolic-ref", "HEAD", "refs/heads/trunk"])
        .status()
        .expect("HEAD moves");
    assert!(status.success());
    assert_ne!(stamp(dir.path()).expect("stamped again"), before);
}

fn record(id: &str, parents: &str, body: &str, files: &str) -> String {
    format!("\u{1e}{id}\0{parents}\01000\0Tom\0{body}\u{1f}\n{files}")
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
fn a_bad_sound_is_an_error_not_silence() {
    let log = record("a", "", "one\n\nKazoo-Sound: rms=loud", "");
    let err = History::from_git(&log, "", None).expect_err("a bad sound");
    assert!(err.to_string().contains("Kazoo-Sound"), "{err}");
    let log = record("a", "", "one\n\nKazoo-Sound: rms=-3", "");
    let err = History::from_git(&log, "", None).expect_err("a short sound");
    assert!(err.to_string().contains("no peak"), "{err}");
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
