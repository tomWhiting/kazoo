use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use super::{HereError, MomentView, TreePlace, checked, full_id, git};
use crate::protocol::tree::{LimbPlace, TreeLimb};

#[cfg(test)]
mod tests;

#[derive(Debug)]
pub(super) struct Limbs {
    pub limbs: Vec<TreeLimb>,
    pub places: BTreeMap<String, LimbPlace>,
}

/// Collapse first-parent runs into limbs. Only positions of visible
/// moments leave the tree thread; their ancestors are represented by
/// each limb's connection rather than by one entry per commit.
pub(super) fn read(
    tree: &Path,
    moments: &BTreeMap<String, MomentView>,
    here: Option<&TreePlace>,
    recent: &[MomentView],
) -> Result<Limbs, HereError> {
    let heads: BTreeSet<_> = moments
        .keys()
        .map(String::as_str)
        .chain(here.map(|place| place.commit.as_str()))
        .chain(recent.iter().map(|moment| moment.commit.as_str()))
        .collect();
    if heads.is_empty() {
        return Ok(Limbs {
            limbs: Vec::new(),
            places: BTreeMap::new(),
        });
    }
    let text = checked(
        git::git_command("log", Some(tree))
            .args([
                "-z",
                "--first-parent",
                "--topo-order",
                "--reverse",
                "--format=%H%x00%P",
            ])
            .args(&heads)
            .arg("--"),
    )?;
    let graph = graph(&text)?;
    if heads.iter().any(|id| !graph.contains_key(id)) {
        return Err(HereError::Broken("the tree omitted a listed moment".into()));
    }
    let mut ordered: Vec<_> = heads.iter().copied().collect();
    // The deepest head claims shared ancestors first, so an old mark
    // cannot turn a run into a separate limb just by being listed.
    ordered.sort_unstable_by_key(|id| std::cmp::Reverse(graph[id].1));
    let mut limbs = Vec::new();
    let mut places = HashMap::with_capacity(graph.len());
    for head in ordered {
        let mut own = Vec::new();
        let mut at = Some(head);
        let mut from = None;
        while let Some(id) = at {
            if let Some(place) = places.get(id) {
                from = Some(*place);
                break;
            }
            own.push(id);
            at = graph[id].0;
        }
        if !own.is_empty() {
            let limb = limbs.len();
            for (position, id) in own.into_iter().rev().enumerate() {
                places.insert(id, LimbPlace { limb, position });
            }
            limbs.push(TreeLimb { from });
        }
    }
    let places = heads
        .into_iter()
        .map(|id| {
            places
                .get(id)
                .copied()
                .map(|place| (id.to_owned(), place))
                .ok_or_else(|| HereError::Broken("the tree's moment has no limb position".into()))
        })
        .collect::<Result<_, _>>()?;
    Ok(Limbs { limbs, places })
}

fn graph(text: &str) -> Result<HashMap<&str, (Option<&str>, usize)>, HereError> {
    let mut fields: Vec<_> = text.split('\0').collect();
    if fields.last() == Some(&"") {
        fields.pop();
    }
    if fields.len() % 2 != 0 {
        return Err(HereError::Broken(
            "the tree's parent metadata is incomplete".into(),
        ));
    }
    let mut graph: HashMap<&str, (Option<&str>, usize)> = HashMap::with_capacity(fields.len() / 2);
    for row in fields.chunks_exact(2) {
        let commit = row[0].trim_start_matches('\n');
        let mut parents = row[1].split_whitespace();
        let parent = parents.next();
        if !full_id(commit) || parent.is_some_and(|id| !full_id(id)) || parents.next().is_some() {
            return Err(HereError::Broken(
                "the tree returned an invalid first-parent moment".into(),
            ));
        }
        let depth = match parent {
            Some(parent) => graph
                .get(parent)
                .and_then(|(_, depth)| depth.checked_add(1))
                .ok_or_else(|| {
                    HereError::Broken("the tree's parent is missing or its depth overflows".into())
                })?,
            None => 0,
        };
        if graph.insert(commit, (parent, depth)).is_some() {
            return Err(HereError::Broken(
                "the tree returned a repeated moment".into(),
            ));
        }
    }
    Ok(graph)
}
