use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use super::{HereError, MomentView, TreePlace, checked, full_id, git};

/// The most moments whose first parents one listing sends: the newest,
/// across every listed line. The tree grows by a commit a change and the
/// list is asked for again as the wall changes, so the work and the line
/// it takes stay this size however old the tree is; a moment whose line
/// parts from the wall's further back is said to be further than the
/// tree counted, never refused.
const MAX_PARENTS: usize = 4096;

/// The first parent of the listed moments, the wall's place and their
/// ancestors, newest first, up to [`MAX_PARENTS`] moments.
pub(super) fn read(
    tree: &Path,
    moments: &BTreeMap<String, MomentView>,
    here: Option<&TreePlace>,
) -> Result<BTreeMap<String, Option<String>>, HereError> {
    within(tree, moments, here, MAX_PARENTS)
}

pub(super) fn within(
    tree: &Path,
    moments: &BTreeMap<String, MomentView>,
    here: Option<&TreePlace>,
    most: usize,
) -> Result<BTreeMap<String, Option<String>>, HereError> {
    let heads: BTreeSet<_> = moments
        .keys()
        .map(String::as_str)
        .chain(here.map(|place| place.commit.as_str()))
        .collect();
    if heads.is_empty() {
        return Ok(BTreeMap::new());
    }
    let text = checked(
        git::git_command("log", Some(tree))
            .args(["-z", "--first-parent", "--format=%H%x00%P"])
            .arg(format!("-n{most}"))
            .args(heads)
            .arg("--"),
    )?;
    let mut fields: Vec<_> = text.split('\0').collect();
    if fields.last() == Some(&"") {
        fields.pop();
    }
    if fields.len() % 2 != 0 {
        return Err(HereError::Broken(
            "the tree's parent metadata is incomplete".into(),
        ));
    }
    fields
        .chunks_exact(2)
        .map(|row| {
            let commit = row[0].trim_start_matches('\n');
            let parent = row[1].split_whitespace().next();
            if !full_id(commit) || parent.is_some_and(|id| !full_id(id)) {
                return Err(HereError::Broken(
                    "the tree returned an invalid parent commit id".into(),
                ));
            }
            Ok((commit.into(), parent.map(str::to_owned)))
        })
        .collect()
}
