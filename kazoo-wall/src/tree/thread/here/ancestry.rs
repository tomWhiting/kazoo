use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use super::{HereError, MomentView, TreePlace, checked, full_id, git};

const MAX_PARENTS: usize = 200_000;

pub(super) fn read(
    tree: &Path,
    moments: &BTreeMap<String, MomentView>,
    here: Option<&TreePlace>,
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
            .arg(format!("-n{}", MAX_PARENTS + 1))
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
    if fields.len() / 2 > MAX_PARENTS {
        return Err(HereError::Full(format!(
            "the listed moments have more than {MAX_PARENTS} parents; their distances cannot be listed"
        )));
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
