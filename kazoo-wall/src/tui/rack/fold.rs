use std::collections::{BTreeMap, BTreeSet};

use crate::protocol::{CableRecord, ModuleView};

use super::geometry::{self, Density, Point, Rack};

const KIND: &str = "folded group";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tile {
    pub group: String,
    pub members: Vec<usize>,
}

pub fn tiles(
    modules: &[ModuleView],
    rows: Option<&[Vec<String>]>,
    groups: &BTreeMap<String, Vec<String>>,
    folded: &BTreeSet<String>,
) -> BTreeMap<usize, Tile> {
    let indices: BTreeMap<&str, usize> = modules
        .iter()
        .enumerate()
        .map(|(index, module)| (module.id.as_str(), index))
        .collect();
    let mut order = Vec::with_capacity(modules.len());
    let mut seen = BTreeSet::new();
    for id in rows.into_iter().flatten().flatten() {
        if let Some(&index) = indices.get(id.as_str()) {
            if seen.insert(index) {
                order.push(index);
            }
        }
    }
    order.extend((0..modules.len()).filter(|index| !seen.contains(index)));
    let mut owners = BTreeMap::new();
    // A module shared by folded groups belongs to the first name in order.
    for group in folded {
        if let Some(members) = groups.get(group) {
            for id in members {
                if let Some(&index) = indices.get(id.as_str()) {
                    owners.entry(index).or_insert(group);
                }
            }
        }
    }
    let mut by_group: BTreeMap<&str, (usize, Tile)> = BTreeMap::new();
    for index in order {
        if let Some(&group) = owners.get(&index) {
            by_group
                .entry(group)
                .or_insert_with(|| {
                    (
                        index,
                        Tile {
                            group: group.clone(),
                            members: Vec::new(),
                        },
                    )
                })
                .1
                .members
                .push(index);
        }
    }
    by_group.into_values().collect()
}

pub fn tile_size(module: &ModuleView) -> Option<(u16, u16)> {
    (module.kind == KIND).then(|| {
        let name = module.name.as_deref().unwrap_or_default();
        (
            u16::try_from(name.chars().count())
                .unwrap_or(u16::MAX - 4)
                .saturating_add(4)
                .max(24),
            6,
        )
    })
}

pub fn layout(
    modules: &[ModuleView],
    density: Density,
    view: (u16, u16),
    rows: Option<&[Vec<String>]>,
    tiles: &BTreeMap<usize, Tile>,
) -> Rack {
    let hidden: BTreeSet<usize> = tiles
        .values()
        .flat_map(|tile| tile.members.iter().skip(1).copied())
        .collect();
    let mut projected = Vec::with_capacity(modules.len() - hidden.len());
    let mut original = Vec::with_capacity(projected.capacity());
    for (index, module) in modules.iter().enumerate() {
        if hidden.contains(&index) {
            continue;
        }
        original.push(index);
        projected.push(tiles.get(&index).map_or_else(
            || module.clone(),
            |tile| ModuleView {
                id: module.id.clone(),
                kind: KIND.to_string(),
                name: Some(tile.group.clone()),
                knobs: Vec::new(),
                inputs: Vec::new(),
                outputs: Vec::new(),
            },
        ));
    }
    let mut rack = geometry::rack(&projected, density, view, rows);
    for plate in &mut rack.plates {
        plate.module = original[plate.module];
    }
    rack
}

pub fn cable_ends(
    rack: &Rack,
    modules: &[ModuleView],
    cables: &[CableRecord],
    tiles: &BTreeMap<usize, Tile>,
) -> Vec<(u32, Point, Point)> {
    if tiles.is_empty() {
        return geometry::cable_ends(rack, modules, cables);
    }
    let owners: BTreeMap<usize, usize> = tiles
        .iter()
        .flat_map(|(&index, tile)| tile.members.iter().map(move |&member| (member, index)))
        .collect();
    let indices: BTreeMap<&str, usize> = modules
        .iter()
        .enumerate()
        .map(|(index, module)| (module.id.as_str(), index))
        .collect();
    let socket = |jack: &str, output: bool| {
        let (id, name) = jack.split_once('.')?;
        let &index = indices.get(id)?;
        let owner = owners.get(&index).copied();
        let plate = rack.plate(owner.unwrap_or(index))?;
        let point = if owner.is_some() {
            Point {
                x: plate.at.x
                    + if output {
                        u32::from(plate.width) - 1
                    } else {
                        0
                    },
                y: plate.at.y + 3,
            }
        } else {
            plate.socket(name, output, &modules[index], rack.density)?
        };
        Some((owner, point))
    };
    cables
        .iter()
        .filter_map(|cable| {
            let (from_owner, from) = socket(&cable.from, true)?;
            let (to_owner, to) = socket(&cable.to, false)?;
            if from_owner.is_some() && from_owner == to_owner {
                None
            } else {
                Some((cable.id, from, to))
            }
        })
        .collect()
}
