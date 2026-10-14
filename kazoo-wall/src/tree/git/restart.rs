use std::collections::BTreeSet;
use std::path::Path;

use super::{TreeError, checked, git_command};

/// Which of `candidates` (logged changes that are not goes) after
/// `through` have no commit in the tree at `dir`, by its own changes and
/// the gaps its whole commits named, in order. Git is asked only when a
/// candidate comes after `through`.
pub(in crate::tree) fn missing_changes(
    dir: &Path,
    candidates: &[u64],
    through: Option<u64>,
) -> Result<Vec<u64>, TreeError> {
    let after: BTreeSet<u64> = candidates
        .iter()
        .copied()
        .filter(|seq| *seq > through.unwrap_or(0))
        .collect();
    if after.is_empty() {
        return Ok(Vec::new());
    }
    let text = checked(git_command("log", Some(dir)).args([
        "--all", "--format=%(trailers:key=Kazoo-Kind,valueonly,separator=%x2C)|%(trailers:key=Kazoo-Seq,valueonly,separator=%x2C)|%(trailers:key=Kazoo-Gap,valueonly,separator=%x2C)",
    ]))?;
    let mut committed = BTreeSet::new();
    let mut gaps = Vec::new();
    for row in text.lines() {
        let mut fields = row.split('|');
        let kind = fields.next().unwrap_or("");
        let seq = fields.next().unwrap_or("");
        if crate::tree::thread::here::is_change(kind) {
            committed.insert(
                seq.parse::<u64>()
                    .map_err(|err| TreeError::Git(format!("commit sequence: {err}")))?,
            );
        }
        for gap in fields
            .next()
            .unwrap_or("")
            .split(',')
            .filter(|gap| !gap.is_empty())
        {
            let (first, last) = gap
                .split_once('-')
                .ok_or_else(|| TreeError::Git(format!("commit gap: {gap}")))?;
            let first = first
                .parse::<u64>()
                .map_err(|err| TreeError::Git(format!("gap start: {err}")))?;
            let last = last
                .parse::<u64>()
                .map_err(|err| TreeError::Git(format!("gap end: {err}")))?;
            gaps.push(first..=last);
        }
    }
    Ok(after
        .into_iter()
        .filter(|seq| !committed.contains(seq) && !gaps.iter().any(|gap| gap.contains(seq)))
        .collect())
}

pub(super) fn describe(missing: &[u64]) -> (String, String) {
    let mut names = Vec::new();
    let mut trailers = Vec::new();
    let mut at = 0;
    while let Some(&first) = missing.get(at) {
        let mut last = first;
        at += 1;
        while let Some(&next) = missing.get(at) {
            if last.checked_add(1) != Some(next) {
                break;
            }
            last = next;
            at += 1;
        }
        names.push(if first == last {
            first.to_string()
        } else {
            format!("{first} to {last}")
        });
        trailers.push(format!("Kazoo-Gap: {first}-{last}\n"));
    }
    (names.join(", "), trailers.concat())
}
