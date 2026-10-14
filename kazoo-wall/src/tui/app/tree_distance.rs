//! How far a listed moment is from the wall, in kazoo tree's own words.

use std::collections::{HashMap, HashSet};

use crate::protocol::TreeResult;
use crate::protocol::tree::LimbPlace;

const GO: &str = "Enter puts the wall here";

fn changes(count: usize) -> String {
    if count == 1 {
        "1 change".to_owned()
    } else {
        format!("{count} changes")
    }
}

enum Path {
    Found(usize, usize),
    Apart,
}

pub fn words(result: &TreeResult, commit: &str) -> String {
    let Some(here) = &result.here else {
        return GO.to_owned();
    };
    if here.commit == commit {
        return "the wall is here".to_owned();
    }
    match distance(result, commit, &here.commit) {
        Ok(Path::Found(0, on)) => format!("{} on from the wall · {GO}", changes(on)),
        Ok(Path::Found(back, 0)) => format!("{} back from the wall · {GO}", changes(back)),
        Ok(Path::Found(back, on)) => back.checked_add(on).map_or_else(
            || problem("the tree's distance overflows"),
            |total| {
                format!(
                    "{} from the wall: {back} back to where they part, {on} on · {GO}",
                    changes(total)
                )
            },
        ),
        Ok(Path::Apart) => format!("on no path from the wall · {GO}"),
        Err(message) => problem(message),
    }
}

fn problem(message: &str) -> String {
    format!("how far from the wall is not known: {message} · {GO}")
}

fn parent(result: &TreeResult, place: LimbPlace) -> Result<Option<LimbPlace>, &'static str> {
    result
        .limbs
        .get(place.limb)
        .map(|limb| limb.from)
        .ok_or("the tree sent an unknown limb")
}

fn step(distance: usize, position: usize) -> Result<usize, &'static str> {
    distance
        .checked_add(position)
        .and_then(|count| count.checked_add(1))
        .ok_or("the tree's distance overflows")
}

fn distance(result: &TreeResult, commit: &str, here: &str) -> Result<Path, &'static str> {
    let mut at = result.places.get(here).copied();
    if at.is_none() {
        return Err("the tree sent no position for the wall");
    }
    let mut down = HashMap::new();
    let mut back = 0;
    while let Some(place) = at {
        if down.insert(place.limb, (place.position, back)).is_some() {
            return Err("the tree sent a cycle in its limbs");
        }
        at = parent(result, place)?;
        if at.is_some() {
            back = step(back, place.position)?;
        }
    }
    let mut at = result.places.get(commit).copied();
    if at.is_none() {
        return Err("the tree sent no position for this moment");
    }
    let mut on: usize = 0;
    let mut seen = HashSet::new();
    while let Some(place) = at {
        if let Some(&(there, back)) = down.get(&place.limb) {
            let part = there.min(place.position);
            return Ok(Path::Found(
                back.checked_add(there - part)
                    .ok_or("the tree's distance overflows")?,
                on.checked_add(place.position - part)
                    .ok_or("the tree's distance overflows")?,
            ));
        }
        if !seen.insert(place.limb) {
            return Err("the tree sent a cycle in its limbs");
        }
        at = parent(result, place)?;
        if at.is_some() {
            on = step(on, place.position)?;
        }
    }
    Ok(Path::Apart)
}
