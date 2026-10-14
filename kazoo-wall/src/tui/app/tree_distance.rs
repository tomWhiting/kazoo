use std::collections::BTreeMap;

use crate::protocol::TreeResult;

fn walk<'a>(
    start: &'a str,
    parents: &'a BTreeMap<String, Option<String>>,
) -> Result<BTreeMap<&'a str, usize>, &'static str> {
    let mut depths = BTreeMap::new();
    let mut at = start;
    loop {
        let depth = depths.len();
        if depths.insert(at, depth).is_some() {
            return Err("the tree sent a cycle in its parents");
        }
        match parents.get(at) {
            Some(Some(parent)) => at = parent,
            Some(None) => return Ok(depths),
            None => return Err("the tree did not send this moment's parent"),
        }
    }
}

const fn changes(count: usize) -> &'static str {
    if count == 1 { "change" } else { "changes" }
}

pub(super) fn words(result: &TreeResult, commit: &str) -> String {
    let Some(here) = &result.here else {
        return "the wall has no moment in the tree yet".into();
    };
    if here.commit == commit {
        return "the wall is here".into();
    }
    match distance(result, commit, &here.commit) {
        Ok((back, 0)) => format!("{back} {} back from the wall", changes(back)),
        Ok((0, on)) => format!("{on} {} on from the wall", changes(on)),
        Ok((back, on)) => {
            let total = back + on;
            format!(
                "{total} {} from the wall: {back} back to where they part, {on} on",
                changes(total)
            )
        }
        Err(problem) => format!("the distance from the wall is not known: {problem}"),
    }
}

fn distance(result: &TreeResult, commit: &str, here: &str) -> Result<(usize, usize), &'static str> {
    let wall = walk(here, &result.parents)?;
    let selected = walk(commit, &result.parents)?;
    selected
        .iter()
        .filter_map(|(id, on)| wall.get(id).map(|back| (*back, *on)))
        .min_by_key(|(back, on)| back + on)
        .ok_or("the moments have no shared parent")
}
