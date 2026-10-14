//! Made-up trees for the tests: a trunk and buds of plain moments, grown
//! one second apart in the order they are added, as a wall grows them.

use crate::history::{History, Limb, Mark, Moment};

/// A moment `n`, made at second `time`, by `seat`, with `modules` alive.
pub fn moment(
    n: usize,
    parent: Option<usize>,
    time: i64,
    seat: &str,
    summary: &str,
    modules: u32,
) -> Moment {
    Moment {
        id: format!("{n:040x}"),
        parent,
        time,
        seat: seat.to_owned(),
        summary: summary.to_owned(),
        kind: "change".to_owned(),
        seq: Some(n as u64),
        beat: Some(n as f64),
        tempo: Some(100.0),
        sound: None,
        modules,
    }
}

fn add(history: &mut History, parent: Option<usize>, seat: &str) -> usize {
    let n = history.moments.len();
    let summary = format!("{seat} turned vcf{} cutoff {n}", (n / 5) % 3);
    history.moments.push(moment(
        n,
        parent,
        1_000 + i64::try_from(n).expect("a small tree"),
        seat,
        &summary,
        10 + (n % 40) as u32,
    ));
    history.place.push(None);
    n
}

/// Grow `limb` by `length` moments at its tip; a new trunk when the tree
/// has no limbs.
pub fn lengthen(history: &mut History, limb: usize, length: usize) {
    if history.limbs.is_empty() {
        history.limbs.push(Limb {
            name: "trunk".to_owned(),
            moments: Vec::new(),
            from: None,
        });
    }
    let seat = if limb == 0 { "Tom" } else { "Cally" };
    for _ in 0..length {
        let tip = history.limbs[limb]
            .moments
            .last()
            .copied()
            .or(history.limbs[limb].from);
        let m = add(history, tip, seat);
        let place = history.limbs[limb].moments.len();
        history.limbs[limb].moments.push(m);
        history.place[m] = Some((limb, place));
    }
    history.here = history.moments.len().checked_sub(1);
}

/// Grow a bud of `length` moments from `place` on `from_limb`; answers it.
pub fn bud(history: &mut History, from_limb: usize, place: usize, length: usize) -> usize {
    let from = history.limbs[from_limb].moments[place];
    let n = history.limbs.len();
    history.limbs.push(Limb {
        name: format!("bud/{from:x}/20261014T{n:06}Z"),
        moments: Vec::new(),
        from: Some(from),
    });
    lengthen(history, n, length);
    n
}

/// A trunk of `trunk` moments, then each bud as `(from limb, place, length)`.
pub fn grown(trunk: usize, buds: &[(usize, usize, usize)]) -> History {
    let mut history = History::default();
    lengthen(&mut history, 0, trunk);
    for &(from_limb, place, length) in buds {
        bud(&mut history, from_limb, place, length);
    }
    history
}

/// Hang a mark on moment `m`.
pub fn mark(history: &mut History, m: usize, name: &str, save: bool) {
    history.marks.push(Mark {
        name: name.to_owned(),
        save,
        moment: m,
    });
}
