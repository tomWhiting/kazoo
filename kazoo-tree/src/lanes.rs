//! The second view: lanes, the way serie shows a git history.
//!
//! One row per moment, newest at the top; each limb a lane from its tip down to the
//! moment it budded from, where it joins its parent's lane. For finding the
//! exact line: who, when, what, and a strip of how it sounded.

use crate::history::History;

/// One row of the lanes view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// The moment on this row.
    pub moment: usize,
    /// The graph: two characters a lane, left to right.
    pub graph: String,
}

/// What a lane is doing on one row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mark {
    /// The row's own moment, on this lane.
    Here,
    /// A lane passing by.
    Pass,
    /// A bud joining its parent on this row.
    Join,
    /// Nothing.
    Empty,
}

/// Lay out every moment as a row, newest first.
#[must_use]
pub fn layout(history: &History) -> Vec<Row> {
    let mut order: Vec<usize> = (0..history.moments.len())
        .filter(|&m| history.place_of(m).is_some())
        .collect();
    order.sort_by(|&a, &b| {
        history.moments[b]
            .time
            .cmp(&history.moments[a].time)
            .then(b.cmp(&a))
    });
    // The lane each active limb holds, and for each limb the moment its
    // lane ends on (the moment it grew from, or its own first moment).
    let mut lane_of: Vec<Option<usize>> = vec![None; history.limbs.len()];
    let mut lanes: Vec<Option<usize>> = Vec::new();
    // The trunk keeps the first lane all the way down, so it is always the
    // line on the left.
    if !history.limbs.is_empty() {
        lanes.push(Some(0));
        lane_of[0] = Some(0);
    }
    let ends: Vec<Option<usize>> = history
        .limbs
        .iter()
        .map(|limb| limb.from.or_else(|| limb.moments.first().copied()))
        .collect();
    // A lane is drawn only from its limb's tip down: the trunk's lane is
    // kept for it from the top, but empty until its tip.
    let mut started = vec![false; history.limbs.len()];
    let mut rows = Vec::with_capacity(order.len());
    for m in order {
        let Some((limb, _)) = history.place_of(m) else {
            continue;
        };
        if lane_of[limb].is_none() {
            let free = lanes
                .iter()
                .position(Option::is_none)
                .unwrap_or(lanes.len());
            if free == lanes.len() {
                lanes.push(None);
            }
            lanes[free] = Some(limb);
            lane_of[limb] = Some(free);
        }
        started[limb] = true;
        let mut marks = vec![Mark::Empty; lanes.len()];
        let mut joining = Vec::new();
        for (lane, held) in lanes.iter().enumerate() {
            let Some(l) = *held else { continue };
            if !started[l] {
                continue;
            }
            marks[lane] = if l == limb {
                Mark::Here
            } else if history.limbs[l].from == Some(m) {
                joining.push(lane);
                Mark::Join
            } else {
                Mark::Pass
            };
        }
        let own = lane_of[limb].unwrap_or(0);
        rows.push(Row {
            moment: m,
            graph: draw_row(&marks, own, &joining, history.here == Some(m)),
        });
        // Lanes that end on this row are free from the next one down.
        for held in &mut lanes {
            if let Some(l) = *held
                && ends[l] == Some(m)
                && (l != limb || history.limbs[l].from.is_none())
            {
                *held = None;
                lane_of[l] = None;
            }
        }
    }
    rows
}

/// One row's graph: two characters a lane. The row's own moment on its
/// lane; a bud joining it turns into it from its own lane (`╯` from the
/// right, `╰` from the left) with a line across the lanes between.
fn draw_row(marks: &[Mark], own: usize, joining: &[usize], here: bool) -> String {
    let left = joining.iter().copied().min().unwrap_or(own).min(own);
    let right = joining.iter().copied().max().unwrap_or(own).max(own);
    let mut out = String::new();
    for (lane, mark) in marks.iter().enumerate() {
        let between = lane > left && lane < right;
        let across = if lane >= left && lane < right {
            '─'
        } else {
            ' '
        };
        let glyph = match mark {
            Mark::Here => {
                if here {
                    '◉'
                } else {
                    '●'
                }
            }
            Mark::Join if lane > own => '╯',
            Mark::Join => '╰',
            Mark::Pass if between => '┼',
            Mark::Pass => '│',
            Mark::Empty if between => '─',
            Mark::Empty => ' ',
        };
        out.push(glyph);
        out.push(across);
    }
    out.trim_end().to_owned()
}

#[cfg(test)]
mod tests;
