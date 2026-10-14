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
    /// The lane the row's own moment is on.
    pub lane: usize,
}

/// One limb's lane: the rows it runs down, from its tip to the row it
/// ends on (the moment it grew from, where it joins its parent's lane, or
/// its own first moment).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    /// The limb.
    pub limb: usize,
    /// Its lane.
    pub lane: usize,
    /// The row of its tip.
    pub top: usize,
    /// The row it ends on.
    pub bottom: usize,
}

/// The lanes laid out: every row, and every lane's span.
///
/// A row's graph is drawn only when it is shown ([`Lanes::graph`]), so a tree of a million
/// moments lays out in one pass and draws only what is on screen.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Lanes {
    /// One row a moment, newest first.
    pub rows: Vec<Row>,
    /// Every limb's lane, by its top row.
    pub spans: Vec<Span>,
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

/// Lay out every moment as a row, newest first, each limb on a lane.
#[must_use]
pub fn layout(history: &History) -> Lanes {
    // Each moment's causal time: its own, held no earlier than the moment
    // it grew from (the wall dates a gap after changes it covers), and its
    // depth from the root; parents come before their children in the
    // history, so one pass fills both.
    let mut key: Vec<(i64, usize)> = Vec::with_capacity(history.moments.len());
    for moment in &history.moments {
        let (time, depth) = moment
            .parent
            .map_or((i64::MIN, 0), |p| (key[p].0, key[p].1 + 1));
        key.push((moment.time.max(time), depth));
    }
    let mut order: Vec<usize> = (0..history.moments.len())
        .filter(|&m| history.place_of(m).is_some())
        .collect();
    // Newest first: a child is never below the moment it grew from.
    order.sort_by(|&a, &b| {
        key[b]
            .cmp(&key[a])
            .then_with(|| history.moments[b].id.cmp(&history.moments[a].id))
    });
    // Where each limb's lane ends: the moment it grew from when that is on
    // the tree, else its own first moment.
    let ends: Vec<Option<usize>> = history
        .limbs
        .iter()
        .map(|limb| {
            limb.from
                .filter(|&m| history.place_of(m).is_some())
                .or_else(|| limb.moments.first().copied())
        })
        .collect();
    // Which limbs end on each moment, to free their lanes there.
    let mut ending: Vec<Vec<usize>> = vec![Vec::new(); history.moments.len()];
    for (l, end) in ends.iter().enumerate() {
        if let Some(m) = *end {
            ending[m].push(l);
        }
    }
    // The lane each limb holds while it runs. The trunk keeps the first
    // lane all the way down, so it is always the line on the left.
    let mut lane_of: Vec<Option<usize>> = vec![None; history.limbs.len()];
    let mut free: Vec<bool> = Vec::new();
    if !history.limbs.is_empty() {
        free.push(false);
        lane_of[0] = Some(0);
    }
    let mut span_of: Vec<Option<usize>> = vec![None; history.limbs.len()];
    let mut lanes = Lanes {
        rows: Vec::with_capacity(order.len()),
        spans: Vec::new(),
    };
    for (r, m) in order.into_iter().enumerate() {
        let Some((limb, _)) = history.place_of(m) else {
            continue;
        };
        let lane = lane_of[limb].unwrap_or_else(|| {
            let lane = free.iter().position(|&f| f).unwrap_or(free.len());
            if lane == free.len() {
                free.push(false);
            }
            free[lane] = false;
            lane_of[limb] = Some(lane);
            lane
        });
        if span_of[limb].is_none() {
            span_of[limb] = Some(lanes.spans.len());
            lanes.spans.push(Span {
                limb,
                lane,
                top: r,
                bottom: r,
            });
        }
        lanes.rows.push(Row { moment: m, lane });
        // Lanes that end on this row are free from the next one down.
        for &l in &ending[m] {
            if let Some(s) = span_of[l] {
                lanes.spans[s].bottom = r;
            }
            if let Some(lane) = lane_of[l].take() {
                free[lane] = true;
            }
        }
    }
    // A limb whose end was never reached runs to the last row.
    let last = lanes.rows.len().saturating_sub(1);
    for (l, span) in span_of.iter().enumerate() {
        if let Some(s) = *span
            && lane_of[l].is_some()
        {
            lanes.spans[s].bottom = last;
        }
    }
    lanes
}

impl Lanes {
    /// The spans that cross rows `from..to`: what [`Lanes::graph`] needs
    /// for those rows, found once a frame.
    #[must_use]
    pub fn crossing(&self, from: usize, to: usize) -> Vec<&Span> {
        self.spans
            .iter()
            .filter(|span| span.top < to && span.bottom >= from)
            .collect()
    }

    /// Row `r`'s graph: two characters a lane, from the spans crossing it
    /// (all of [`Lanes::crossing`] for a range holding `r` will do).
    #[must_use]
    pub fn graph(&self, history: &History, spans: &[&Span], r: usize) -> String {
        let Some(row) = self.rows.get(r) else {
            return String::new();
        };
        let Some((limb, _)) = history.place_of(row.moment) else {
            return String::new();
        };
        let width = spans
            .iter()
            .filter(|span| span.top <= r && span.bottom >= r)
            .map(|span| span.lane + 1)
            .max()
            .unwrap_or(0)
            .max(row.lane + 1);
        let mut marks = vec![Mark::Empty; width];
        let mut joining = Vec::new();
        for span in spans {
            if span.top > r || span.bottom < r {
                continue;
            }
            marks[span.lane] = if span.limb == limb {
                Mark::Here
            } else if history.limbs[span.limb].from == Some(row.moment) {
                joining.push(span.lane);
                Mark::Join
            } else {
                Mark::Pass
            };
        }
        draw_row(&marks, row.lane, &joining, history.here == Some(row.moment))
    }
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
