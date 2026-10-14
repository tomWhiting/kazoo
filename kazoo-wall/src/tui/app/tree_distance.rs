//! How far a listed moment is from the wall, in kazoo tree's own words.

use std::collections::BTreeMap;

use crate::protocol::TreeResult;

/// What Enter does on a moment in the list, said after its distance as
/// kazoo tree says it.
const GO: &str = "Enter puts the wall here";

/// A moment's first-parent line as far as the tree sent it: each moment
/// on it with how many changes back from `start` it is.
struct Line<'a> {
    depths: BTreeMap<&'a str, usize>,
    /// The line reached a root. When false, the tree sent no more of it:
    /// it goes back further than the listing counts.
    whole: bool,
}

fn walk<'a>(
    start: &'a str,
    parents: &'a BTreeMap<String, Option<String>>,
) -> Result<Line<'a>, &'static str> {
    let mut depths = BTreeMap::new();
    let mut at = start;
    loop {
        let depth = depths.len();
        if depths.insert(at, depth).is_some() {
            return Err("the tree sent a cycle in its parents");
        }
        match parents.get(at) {
            Some(Some(parent)) => at = parent,
            Some(None) => {
                return Ok(Line {
                    depths,
                    whole: true,
                });
            }
            None => {
                return Ok(Line {
                    depths,
                    whole: false,
                });
            }
        }
    }
}

fn changes(count: usize) -> String {
    if count == 1 {
        "1 change".to_owned()
    } else {
        format!("{count} changes")
    }
}

/// How the moment `commit` stands to the wall.
enum Path {
    /// Changes back to where the two lines part, then on to the moment.
    Found(usize, usize),
    /// Both lines were sent whole and share no moment.
    Apart,
    /// The lines part further back than the tree sent them.
    Beyond,
}

/// The line under the list's cursor: how far `commit` is from the wall
/// and that Enter goes there, as kazoo tree's details say it.
pub(super) fn words(result: &TreeResult, commit: &str) -> String {
    let Some(here) = &result.here else {
        return GO.to_owned();
    };
    if here.commit == commit {
        return "the wall is here".to_owned();
    }
    match distance(result, commit, &here.commit) {
        Ok(Path::Found(0, on)) => format!("{} on from the wall · {GO}", changes(on)),
        Ok(Path::Found(back, 0)) => format!("{} back from the wall · {GO}", changes(back)),
        Ok(Path::Found(back, on)) => format!(
            "{} from the wall: {back} back to where they part, {on} on · {GO}",
            changes(back + on)
        ),
        Ok(Path::Apart) => format!("on no path from the wall · {GO}"),
        Ok(Path::Beyond) => format!("further from the wall than the tree counted · {GO}"),
        Err(problem) => format!("how far from the wall is not known: {problem} · {GO}"),
    }
}

fn distance(result: &TreeResult, commit: &str, here: &str) -> Result<Path, &'static str> {
    let wall = walk(here, &result.parents)?;
    let selected = walk(commit, &result.parents)?;
    // The nearest shared moment is where the lines part. Two first-parent
    // lines that share one moment share every moment behind it, so when
    // any shared moment was sent, the nearest was too.
    let parted = selected
        .depths
        .iter()
        .filter_map(|(id, on)| wall.depths.get(id).map(|back| (*back, *on)))
        .min_by_key(|(back, on)| back + on);
    Ok(match parted {
        Some((back, on)) => Path::Found(back, on),
        None if wall.whole && selected.whole => Path::Apart,
        None => Path::Beyond,
    })
}
