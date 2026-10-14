use std::path::Path;

use kazoo_tree::app::{App, Grown};
use kazoo_tree::history::History;

use crate::protocol::TreeResult;
use crate::tree::git::{Commit, Importer, Planted, git_command};
use crate::tree::thread::here::{list, read_line};
use crate::tui::tree_distance_words;

struct Rig {
    dir: tempfile::TempDir,
    importer: Option<Importer>,
    count: u64,
    oldest: String,
    bud: Vec<String>,
}

fn run(path: &Path, args: &[&str]) -> String {
    let output = git_command(args[0], Some(path))
        .args(&args[1..])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().into()
}

fn importer(path: &Path, branch: &str, tip: Option<String>) -> Importer {
    Importer::new(
        path,
        &Planted {
            branch: branch.into(),
            tip,
            through: None,
        },
    )
    .unwrap()
}

fn commit(importer: &mut Importer, number: u64) -> u64 {
    importer
        .commit(&Commit {
            seat: "Test".into(),
            unix: 1_700_000_000,
            message: format!("moment {number}\n\nKazoo-Kind: change\n"),
            whole: false,
            files: Vec::new(),
        })
        .unwrap()
}

impl Rig {
    fn new(count: u64, fork: u64) -> Self {
        let dir = tempfile::tempdir_in(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/kz-logs"))
            .unwrap();
        crate::tree::git::plant(dir.path()).unwrap();
        let mut trunk = importer(dir.path(), "refs/heads/trunk", None);
        let first = commit(&mut trunk, 0);
        let oldest = trunk.checkpoint(first).unwrap();
        run(dir.path(), &["tag", "mark/oldest", &oldest]);
        let mut fork_id = oldest.clone();
        for n in 1..count {
            let mark = commit(&mut trunk, n);
            if n == fork {
                fork_id = trunk.checkpoint(mark).unwrap();
            }
        }
        trunk.checkpoint(count).unwrap();
        let branch = format!("refs/heads/bud/{}/20261015-070000", &fork_id[..12]);
        let mut grown = importer(dir.path(), &branch, Some(fork_id));
        let bud = (0..3)
            .map(|n| {
                let mark = commit(&mut grown, count + n);
                grown.checkpoint(mark).unwrap()
            })
            .collect();
        grown.finish().unwrap();
        Self {
            dir,
            importer: Some(trunk),
            count,
            oldest,
            bud,
        }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn listing(&self, wall: &str) -> TreeResult {
        run(self.path(), &["update-ref", "--no-deref", "HEAD", wall]);
        let line = read_line(self.path()).unwrap().0;
        list(self.path(), &line, 0, Some(3)).unwrap()
    }

    fn grow(&mut self, count: u64) {
        let trunk = self.importer.as_mut().unwrap();
        for n in self.count..count {
            commit(trunk, n);
        }
        trunk.checkpoint(count).unwrap();
        self.count = count;
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        if let Some(importer) = self.importer.take() {
            importer.finish().unwrap();
        }
    }
}

#[test]
fn oldest_trunk_distance_is_exact_beyond_ten_thousand_moments() {
    let rig = Rig::new(10_000, 5_000);
    let listing = rig.listing(&rig.bud[2]);
    assert_eq!(
        tree_distance_words(&listing, &rig.oldest),
        "5003 changes back from the wall · Enter puts the wall here"
    );
    let tip = run(rig.path(), &["rev-parse", "refs/heads/trunk"]);
    assert_eq!(
        tree_distance_words(&listing, &tip),
        "5002 changes from the wall: 3 back to where they part, 4999 on · Enter puts the wall here"
    );
}

#[test]
fn listing_size_depends_on_rows_and_limbs_not_trunk_length() {
    let mut rig = Rig::new(1_000, 500);
    let short = serde_json::to_vec(&rig.listing(&rig.bud[2])).unwrap();
    rig.grow(100_000);
    let listing = rig.listing(&rig.bud[2]);
    let long = serde_json::to_vec(&listing).unwrap();
    eprintln!("listing sizes: {} -> {} bytes", short.len(), long.len());
    assert!(
        long.len() <= short.len() + 16,
        "1000: {} bytes, 100000: {} bytes",
        short.len(),
        long.len()
    );
    let value = serde_json::to_value(listing).unwrap();
    assert!(value.get("parents").is_none());
    assert_eq!(value["limbs"].as_array().unwrap().len(), 2);
}

#[test]
fn console_words_match_kazoo_tree_on_the_same_five_moments() {
    let rig = Rig::new(30, 10);
    let mut orphan = importer(rig.path(), "refs/heads/other", None);
    let mark = commit(&mut orphan, 100);
    let apart = orphan.checkpoint(mark).unwrap();
    orphan.finish().unwrap();
    let listing = rig.listing(&rig.bud[1]);
    let history = History::read(rig.path()).unwrap();
    let wall = history.moment_of(&rig.bud[1]).unwrap();
    let tip = run(rig.path(), &["rev-parse", "refs/heads/trunk"]);
    let selected = [&rig.bud[1], &rig.bud[0], &rig.bud[2], &tip, &apart];
    let expected = [
        Some((0, 0)),
        Some((1, 0)),
        Some((0, 1)),
        Some((2, 19)),
        None,
    ];
    let app = App::showing(rig.path().into(), Grown::of(history.clone()));
    for (commit, path) in selected.into_iter().zip(expected) {
        let moment = history.moment_of(commit).unwrap();
        assert_eq!(history.path_between(wall, moment), path);
        assert_eq!(
            tree_distance_words(&listing, commit),
            app.whereabouts(moment)
        );
    }
    let value = serde_json::to_value(listing).unwrap();
    assert!(
        value.get("parents").is_none(),
        "the distance must travel by limbs"
    );
    assert!(value["limbs"].is_array());
}

#[test]
fn malformed_limb_metadata_reports_cycles_missing_places_and_overflow() {
    let rig = Rig::new(30, 10);
    let listing = rig.listing(&rig.bud[1]);
    let wall = listing.places[&rig.bud[1]];
    let check = |listing: &TreeResult, message: &str| {
        assert_eq!(
            tree_distance_words(listing, &rig.oldest),
            format!("how far from the wall is not known: {message} · Enter puts the wall here")
        );
    };
    let mut cycle = listing.clone();
    cycle.limbs[wall.limb].from = Some(wall);
    check(&cycle, "the tree sent a cycle in its limbs");
    let mut unknown = listing.clone();
    unknown.places.get_mut(&rig.bud[1]).unwrap().limb = usize::MAX;
    check(&unknown, "the tree sent an unknown limb");
    let mut overflow = listing.clone();
    overflow.places.get_mut(&rig.bud[1]).unwrap().position = usize::MAX;
    check(&overflow, "the tree's distance overflows");
    let mut missing = listing.clone();
    missing.places.remove(&rig.bud[1]);
    check(&missing, "the tree sent no position for the wall");
    let mut missing = listing;
    missing.places.remove(&rig.oldest);
    check(&missing, "the tree sent no position for this moment");
}
