use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::{Patch, error};
use crate::protocol::{ErrorCode, Request, WallError, What, valid_save_name};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Chain {
    pub name: String,
    pub modules: Vec<ChainModule>,
    pub cables: Vec<ChainCable>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChainModule {
    pub kind: String,
    pub label: Option<String>,
    pub knobs: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChainCable {
    pub from: (usize, String),
    pub to: (usize, String),
    pub amount: f32,
}

impl Patch {
    pub(super) fn undo_chain(&mut self, name: &str, changes: &[What]) -> Result<What, WallError> {
        let mut staged = self.clone();
        let mut inverse = Vec::with_capacity(changes.len());
        for change in changes.iter().rev() {
            inverse.push(staged.undo(change)?);
        }
        *self = staged;
        Ok(What::Chain {
            name: name.to_string(),
            changes: inverse,
        })
    }

    pub fn chain(&self, name: &str, selected: &[String]) -> Result<(Chain, usize), WallError> {
        if !valid_save_name(name) || selected.is_empty() {
            return Err(error(
                ErrorCode::BadRequest,
                "name a chain and select its modules",
            ));
        }
        let mut indices = BTreeMap::new();
        let mut modules = Vec::new();
        for id in selected {
            let module = self.find(id)?;
            if indices.contains_key(&module.id) {
                continue;
            }
            indices.insert(module.id.clone(), modules.len());
            modules.push(ChainModule {
                kind: module.kind.name().to_string(),
                label: module.name.clone(),
                knobs: module.knobs.clone(),
            });
        }
        let mut outside = 0;
        let mut cables = Vec::new();
        for cable in self.cables() {
            match (
                indices.get(&cable.from_module),
                indices.get(&cable.to_module),
            ) {
                (Some(&from), Some(&to)) => {
                    let record = self.cable_record(cable);
                    let (_, port) = record
                        .from
                        .split_once('.')
                        .ok_or_else(|| error(ErrorCode::Internal, "a cable has no output"))?;
                    let (_, jack) = record
                        .to
                        .split_once('.')
                        .ok_or_else(|| error(ErrorCode::Internal, "a cable has no input"))?;
                    cables.push(ChainCable {
                        from: (from, port.to_string()),
                        to: (to, jack.to_string()),
                        amount: cable.amount,
                    });
                }
                (Some(_), None) | (None, Some(_)) => outside += 1,
                (None, None) => {}
            }
        }
        Ok((
            Chain {
                name: name.to_string(),
                modules,
                cables,
            },
            outside,
        ))
    }
}

impl Chain {
    pub fn validate(&self) -> Result<(), WallError> {
        let bad = |message| error(ErrorCode::BrokenSave, message);
        if !valid_save_name(&self.name) || self.modules.is_empty() {
            return Err(bad(
                "the chain must have a valid name and at least one module",
            ));
        }
        let mut kinds = Vec::with_capacity(self.modules.len());
        for module in &self.modules {
            let kind = Patch::check_add(&module.kind, module.label.as_deref())?;
            if module.knobs.len() != kind.spec().knobs.len()
                || module
                    .knobs
                    .iter()
                    .zip(&kind.spec().knobs)
                    // A value its knob would change (out of range, a part
                    // step on a stepped knob, not a number) is refused.
                    .any(|(value, knob)| !value.is_finite() || knob.clamp(*value) != *value)
            {
                return Err(bad("the chain has invalid knob values"));
            }
            kinds.push(kind);
        }
        let mut inputs = BTreeSet::new();
        for cable in &self.cables {
            let from = kinds
                .get(cable.from.0)
                .ok_or_else(|| bad("the chain has a missing source module"))?;
            let to = kinds
                .get(cable.to.0)
                .ok_or_else(|| bad("the chain has a missing destination module"))?;
            if from.spec().output_index(&cable.from.1).is_none()
                || to.spec().jack(&cable.to.1).is_none()
                || !cable.amount.is_finite()
                || !(-1.0..=1.0).contains(&cable.amount)
                || !inputs.insert(cable.to.clone())
            {
                return Err(bad("the chain has an invalid or duplicate cable"));
            }
        }
        Ok(())
    }

    pub fn requests(&self, patch: &Patch) -> Result<Vec<Request>, WallError> {
        self.validate()?;
        let mut staged = patch.clone();
        let row = staged.rows().len();
        let mut ids = Vec::with_capacity(self.modules.len());
        let mut requests = Vec::with_capacity(self.modules.len() + self.cables.len());
        for module in &self.modules {
            // Keep the usual append placement within the chain's fresh row.
            let kind = Patch::check_add(&module.kind, module.label.as_deref())?;
            let mut place = staged.usual_place(kind);
            place.row = row;
            let added = staged.add(&module.kind, module.label.as_deref(), Some(&place))?;
            let What::Add { module: record } = added else {
                return Err(error(
                    ErrorCode::Internal,
                    "adding a chain module made no module",
                ));
            };
            ids.push(record.id);
            requests.push(Request::Add {
                kind: module.kind.clone(),
                name: module.label.clone(),
                place: Some(place),
            });
        }
        for cable in &self.cables {
            let from = format!("{}.{}", ids[cable.from.0], cable.from.1);
            let to = format!("{}.{}", ids[cable.to.0], cable.to.1);
            staged.plug(&from, &to, Some(f64::from(cable.amount)), None)?;
            requests.push(Request::Patch {
                from,
                to,
                amount: Some(f64::from(cable.amount)),
            });
        }
        Ok(requests)
    }
}
