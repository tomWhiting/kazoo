use kazoo_wall::protocol::tree::MAX_LISTED;
use kazoo_wall::protocol::{
    LoadResult, MarkKind, MarkResult, MomentView, TreePlace, TreeResult, What,
};

use crate::channel::clean;

use super::line;

#[must_use]
pub fn marked(result: &MarkResult) -> String {
    let place = match (result.at.change, result.at.branch.as_deref()) {
        (Some(change), Some(branch)) => format!("after change #{change} on {branch}"),
        (Some(change), None) => format!("after change #{change}, off any line"),
        (None, _) => format!("moment {}", short(&result.at.commit)),
    };
    format!(
        "Marked the moment the wall is at as \"{}\" ({}, {place}). wall_go with \"{}\" puts the wall back here.",
        result.name, result.tag, result.name
    )
}

#[must_use]
pub fn went(result: &LoadResult) -> String {
    let mut out = super::loaded(result);
    // An undone go is itself a go, back to where the wall was: on a line or
    // off one. An undone load stays on the line it was on, and says no more.
    if let What::Go { to, .. } = &result.change.what {
        if let Some(branch) = &to.branch {
            out.push_str(if result.change.undoes.is_some() {
                " The wall is back on "
            } else {
                " The wall is on "
            });
            out.push_str(branch);
            out.push_str(": the next change carries it on.");
        } else {
            out.push_str(
                " The next change anyone makes grows a bud: a new branch from this moment.",
            );
        }
    }
    out
}

#[must_use]
pub fn tree(result: &TreeResult) -> String {
    let Some(here) = &result.here else {
        let behind = result.revision.saturating_sub(result.through);
        return if behind > 0 {
            format!(
                "The tree is {behind} {} behind and holds no moments yet.",
                changes(behind)
            )
        } else {
            "The tree has no moments yet: the first change makes one.".into()
        };
    };
    let mut out = String::new();
    location(&mut out, here);
    if result.through < result.revision {
        line(
            &mut out,
            format_args!(
                "The tree is {} {} behind the wall.",
                result.revision - result.through,
                changes(result.revision - result.through)
            ),
        );
    } else {
        line(
            &mut out,
            format_args!(
                "The tree has every change up to #{} (the wall's latest is #{}).",
                result.through, result.revision
            ),
        );
    }
    line(
        &mut out,
        format_args!("Branches ({}):", result.branch_count),
    );
    for branch in result.branches.iter().take(MAX_LISTED) {
        let grew = branch
            .grew_from
            .as_ref()
            .map_or_else(String::new, |from| format!(" (grew from {from})"));
        line(
            &mut out,
            format_args!("- {}{grew}: {}", branch.name, moment(&branch.tip)),
        );
    }
    more(&mut out, result.branch_count, result.branches.len());
    line(&mut out, format_args!("Marks ({}):", result.mark_count));
    for mark in result.marks.iter().take(MAX_LISTED) {
        let kind = match mark.kind {
            MarkKind::Mark => "mark".to_string(),
            MarkKind::Save => format!("save; wall_go \"{}\"", mark.tag),
        };
        line(
            &mut out,
            format_args!("- \"{}\" ({kind}): {}", mark.name, moment(&mark.moment)),
        );
    }
    more(&mut out, result.mark_count, result.marks.len());
    line(
        &mut out,
        format_args!(
            "Recent on this line ({}):",
            result.recent.len().min(MAX_LISTED)
        ),
    );
    for recent in result.recent.iter().take(MAX_LISTED) {
        line(&mut out, format_args!("- {}", moment(recent)));
    }
    more(&mut out, result.recent.len(), result.recent.len());
    out.push_str("wall_go with a mark, a branch, #<change> or a commit id puts the wall there.");
    out
}

const fn changes(count: u64) -> &'static str {
    if count == 1 { "change" } else { "changes" }
}

fn short(commit: &str) -> String {
    commit.chars().take(12).collect()
}

fn identity(place: &TreePlace) -> String {
    let short = short(&place.commit);
    place
        .change
        .map_or_else(|| short.clone(), |change| format!("#{change} {short}"))
}

fn location(out: &mut String, here: &TreePlace) {
    let at = identity(here);
    if let Some(branch) = &here.branch {
        line(out, format_args!("The wall is on {branch} at {at}."));
    } else {
        let mark = here
            .mark
            .as_ref()
            .map_or_else(String::new, |name| format!(", reached by mark \"{name}\""));
        line(
            out,
            format_args!("The wall is at {at}, off any line{mark}: the next change grows a bud."),
        );
    }
}

fn moment(moment: &MomentView) -> String {
    let change = moment
        .change
        .map_or_else(String::new, |change| format!("#{change} "));
    format!(
        "{change}{} {} {}: {}",
        moment.short,
        moment.at,
        moment.seat,
        clean(&moment.summary)
    )
}

fn more(out: &mut String, count: usize, listed: usize) {
    let remaining = count.saturating_sub(listed.min(MAX_LISTED));
    if remaining > 0 {
        line(out, format_args!("- and {remaining} more"));
    }
}

#[cfg(test)]
mod tests {
    use kazoo_wall::protocol::{
        BranchView, Change, LoadResult, MarkKind, MarkResult, MarkView, MomentView, TreePlace,
        TreeResult, What,
    };

    use super::{marked, tree, went};

    fn place() -> TreePlace {
        TreePlace {
            commit: "abcdef0123456789abcdef0123456789abcdef01".into(),
            branch: Some("trunk".into()),
            mark: None,
            change: Some(57),
        }
    }

    fn moment() -> MomentView {
        MomentView {
            commit: place().commit,
            short: "abcdef012345".into(),
            change: Some(57),
            seat: "Waffles".into(),
            at: "2026-10-13T02:00:00Z".into(),
            summary: "turned\n the cutoff\t to 800 Hz".into(),
        }
    }

    fn listing() -> TreeResult {
        let mut first = moment();
        first.change = None;
        first.summary = "first\0 moment".into();
        TreeResult {
            parents: std::collections::BTreeMap::new(),
            here: Some(place()),
            through: 57,
            revision: 57,
            branches: vec![
                BranchView {
                    name: "bud/dawn/20261013-020000".into(),
                    tip: moment(),
                    grew_from: Some("dawn".into()),
                },
                BranchView {
                    name: "trunk".into(),
                    tip: moment(),
                    grew_from: None,
                },
            ],
            branch_count: 2,
            marks: vec![
                MarkView {
                    name: "dawn".into(),
                    kind: MarkKind::Mark,
                    tag: "mark/dawn".into(),
                    moment: moment(),
                },
                MarkView {
                    name: "low end".into(),
                    kind: MarkKind::Save,
                    tag: "save/low%20end/20261013-020000".into(),
                    moment: moment(),
                },
            ],
            mark_count: 2,
            recent: vec![moment(), first],
        }
    }

    #[test]
    fn a_mark_says_its_name_tag_and_place() {
        let mut result = MarkResult {
            name: "dawn".into(),
            tag: "mark/dawn".into(),
            at: place(),
        };
        assert_eq!(
            marked(&result),
            "Marked the moment the wall is at as \"dawn\" (mark/dawn, after change #57 on trunk). wall_go with \"dawn\" puts the wall back here."
        );
        result.at.branch = None;
        assert_eq!(
            marked(&result),
            "Marked the moment the wall is at as \"dawn\" (mark/dawn, after change #57, off any line). wall_go with \"dawn\" puts the wall back here."
        );
        result.at.change = None;
        assert_eq!(
            marked(&result),
            "Marked the moment the wall is at as \"dawn\" (mark/dawn, moment abcdef012345). wall_go with \"dawn\" puts the wall back here."
        );
    }

    #[test]
    fn a_go_says_how_to_undo_it_and_that_a_bud_will_grow() {
        let mut destination = place();
        destination.branch = None;
        let mut result = LoadResult {
            change: Change {
                seq: 80,
                at: "2026-10-13T02:00:00Z".into(),
                seat: "Waffles".into(),
                what: What::Go {
                    to: destination,
                    from: place(),
                    backup: "before-go-20261013-020000".into(),
                    fingerprint: "0123456789abcdef-2048".into(),
                    modules: 12,
                    cables: 30,
                    tempo: 96.0,
                    by_console: false,
                },
                summary: "Waffles put the wall at dawn".into(),
                undoes: None,
                clock: None,
            },
            backup: "before-go-20261013-020000".into(),
            notes: vec!["the desk keeps the tempo at 120 BPM".into()],
        };
        let load = crate::render::loaded(&result);
        assert_eq!(
            went(&result),
            format!(
                "{load} The next change anyone makes grows a bud: a new branch from this moment."
            )
        );
        assert!(load.starts_with("Change #80: Waffles put the wall at dawn."));
        assert!(load.contains("call wall_look before changing anything"));
        assert!(load.contains("wall_undo with change 80 brings back the wall as it was"));
        assert!(load.contains("\"before-go-20261013-020000\""));
        result.change.undoes = Some(79);
        assert_eq!(
            went(&result),
            format!(
                "{} The next change anyone makes grows a bud: a new branch from this moment.",
                crate::render::loaded(&result)
            )
        );
    }

    #[test]
    fn a_go_names_its_tree_backup_as_a_moment() {
        let result = LoadResult {
            change: Change {
                seq: 80,
                at: "2026-10-15T00:00:00Z".into(),
                seat: "Waffles".into(),
                what: What::Go {
                    to: place(),
                    from: place(),
                    backup: "tree:abc123def456".into(),
                    fingerprint: String::new(),
                    modules: 12,
                    cables: 30,
                    tempo: 96.0,
                    by_console: false,
                },
                summary: "went to trunk".into(),
                undoes: None,
                clock: None,
            },
            backup: "tree:abc123def456".into(),
            notes: Vec::new(),
        };
        let text = went(&result);
        assert!(
            text.contains("(it is also moment abc123def456 in the tree)"),
            "{text}"
        );
        assert!(!text.contains("kept as the save"), "{text}");
    }

    #[test]
    fn the_tree_reads_line_by_line_within_its_bounds() {
        let mut result = listing();
        assert_eq!(
            tree(&result),
            "The wall is on trunk at #57 abcdef012345.\n\
             The tree has every change up to #57 (the wall's latest is #57).\n\
             Branches (2):\n\
             - bud/dawn/20261013-020000 (grew from dawn): #57 abcdef012345 2026-10-13T02:00:00Z Waffles: turned the cutoff to 800 Hz\n\
             - trunk: #57 abcdef012345 2026-10-13T02:00:00Z Waffles: turned the cutoff to 800 Hz\n\
             Marks (2):\n\
             - \"dawn\" (mark): #57 abcdef012345 2026-10-13T02:00:00Z Waffles: turned the cutoff to 800 Hz\n\
             - \"low end\" (save; wall_go \"save/low%20end/20261013-020000\"): #57 abcdef012345 2026-10-13T02:00:00Z Waffles: turned the cutoff to 800 Hz\n\
             Recent on this line (2):\n\
             - #57 abcdef012345 2026-10-13T02:00:00Z Waffles: turned the cutoff to 800 Hz\n\
             - abcdef012345 2026-10-13T02:00:00Z Waffles: first moment\n\
             wall_go with a mark, a branch, #<change> or a commit id puts the wall there."
        );
        let mut here = place();
        here.branch = None;
        here.mark = Some("dawn".into());
        result.here = Some(here.clone());
        assert!(tree(&result).starts_with(
            "The wall is at #57 abcdef012345, off any line, reached by mark \"dawn\": the next change grows a bud.\n"
        ));
        here.mark = None;
        here.change = None;
        result.here = Some(here);
        assert!(tree(&result).starts_with(
            "The wall is at abcdef012345, off any line: the next change grows a bud.\n"
        ));
        result.revision = 60;
        assert!(tree(&result).contains("The tree is 3 changes behind the wall.\n"));
        let mut mark = result.marks[0].clone();
        mark.moment.summary = "界".repeat(400);
        result.marks = vec![mark; 205];
        result.mark_count = 205;
        result.branches = vec![result.branches[0].clone(); 205];
        result.branch_count = 205;
        result.recent = vec![moment(); 205];
        let text = tree(&result);
        assert!(text.contains("Marks (205):\n"));
        assert_eq!(text.matches("- \"dawn\" (mark):").count(), 200);
        assert_eq!(text.matches("- bud/dawn/").count(), 200);
        assert_eq!(text.matches("- #57 abcdef012345").count(), 200);
        assert_eq!(text.matches("and 5 more").count(), 3);
        assert!(text.contains(&crate::channel::clean(&"界".repeat(400))));
        result.marks.truncate(200);
        result.branches.truncate(200);
        assert_eq!(tree(&result).matches("and 5 more").count(), 3);
        result.here = None;
        result.through = 0;
        result.revision = 0;
        result.marks.clear();
        result.mark_count = 0;
        result.branches.clear();
        result.branch_count = 0;
        result.recent.clear();
        assert_eq!(
            tree(&result),
            "The tree has no moments yet: the first change makes one."
        );
    }

    #[test]
    fn a_go_to_trunk_carries_on_the_branch() {
        let result = LoadResult {
            change: Change {
                seq: 80,
                at: moment().at,
                seat: "Tom".into(),
                what: What::Go {
                    to: place(),
                    from: place(),
                    backup: "before-go".into(),
                    fingerprint: "abc".into(),
                    modules: 1,
                    cables: 0,
                    tempo: 120.0,
                    by_console: true,
                },
                summary: "went to trunk".into(),
                undoes: None,
                clock: None,
            },
            backup: "before-go".into(),
            notes: Vec::new(),
        };
        assert!(went(&result).contains("The wall is on trunk: the next change carries it on"));
        assert!(!went(&result).contains("grows a bud"));
    }

    #[test]
    fn save_marks_list_the_lookup_tag_and_distinguish_names() {
        let mut result = listing();
        result.marks[0].name = "low end".into();
        result.marks[1].tag = "save/low%20end/20261013-020000".into();
        let text = tree(&result);
        assert!(text.contains("- \"low end\" (mark):"));
        assert!(text.contains("- \"low end\" (save; wall_go \"save/low%20end/20261013-020000\"):"));
    }

    #[test]
    fn an_empty_tree_reports_lag_and_singular_changes() {
        let mut result = listing();
        result.here = None;
        result.through = 57;
        result.revision = 58;
        assert_eq!(
            tree(&result),
            "The tree is 1 change behind and holds no moments yet."
        );
        result.revision = 59;
        assert_eq!(
            tree(&result),
            "The tree is 2 changes behind and holds no moments yet."
        );
        result.here = Some(place());
        result.revision = 58;
        assert!(tree(&result).contains("The tree is 1 change behind the wall."));
    }
    #[test]
    fn an_undone_replacement_says_where_the_wall_now_is() {
        let go = |branch: Option<&str>| What::Go {
            to: TreePlace {
                branch: branch.map(str::to_string),
                ..place()
            },
            from: place(),
            backup: "before-undo".into(),
            fingerprint: "abc".into(),
            modules: 1,
            cables: 0,
            tempo: 120.0,
            by_console: false,
        };
        let undone = |what: What| LoadResult {
            change: Change {
                seq: 81,
                at: moment().at,
                seat: "Waffles".into(),
                what,
                summary: "undid change 80".into(),
                undoes: Some(80),
                clock: None,
            },
            backup: "before-undo".into(),
            notes: Vec::new(),
        };
        let load = undone(What::Load {
            save: None,
            backup: "before-undo".into(),
            fingerprint: "abc".into(),
            modules: 1,
            cables: 0,
            tempo: 120.0,
            by_console: false,
        });
        assert_eq!(went(&load), crate::render::loaded(&load));
        let back_on_trunk = went(&undone(go(Some("trunk"))));
        assert!(
            back_on_trunk.contains("The wall is back on trunk: the next change carries it on."),
            "{back_on_trunk}"
        );
        let back_off_line = went(&undone(go(None)));
        assert!(
            back_off_line.contains("The next change anyone makes grows a bud"),
            "{back_off_line}"
        );
        assert!(!back_off_line.contains("line it left"), "{back_off_line}");
    }
}
