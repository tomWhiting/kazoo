use std::collections::BTreeSet;
use std::time::Instant;

use super::super::worker::Job;
use super::{App, Tone};
use crate::protocol::{ChainSaveResult, ChainsResult, Request, valid_save_name};

impl App {
    pub(super) fn run_extra_command(
        &mut self,
        name: &str,
        args: &[&str],
        now: Instant,
    ) -> Result<(), String> {
        if matches!(name, "fold" | "unfold") {
            return self.run_fold(name, args, now);
        }
        if !self.link.is_up() {
            return Err("the wall is not answering".to_string());
        }
        if name == "chain" {
            return self.run_chain(args);
        }
        if !args.is_empty() {
            return Err("type :chains".to_string());
        }
        self.jobs.push(Job::Call(Request::Chains));
        Ok(())
    }

    pub(super) fn run_chain(&mut self, args: &[&str]) -> Result<(), String> {
        let [action, name] = args else {
            return Err("type :chain save <name> or :chain add <name>".to_string());
        };
        if !valid_save_name(name) {
            return Err("give the chain a valid save name".to_string());
        }
        let request = match *action {
            "save" => Request::ChainSave {
                name: (*name).to_string(),
                modules: self.chain_selection()?,
            },
            "add" => Request::ChainAdd {
                name: (*name).to_string(),
            },
            _ => return Err("chain takes save or add".to_string()),
        };
        self.jobs.push(Job::Call(request));
        Ok(())
    }

    fn chain_selection(&self) -> Result<Vec<String>, String> {
        let groups = self
            .groups()
            .ok_or_else(|| "wait for the wall's groups".to_string())?;
        if let Some(group) = self.group.as_ref().and_then(|group| groups.get(group)) {
            return Ok(group.clone());
        }
        if let Some(tile) = self
            .selected_index()
            .and_then(|index| self.folded_tile(index))
        {
            return groups
                .get(&tile.group)
                .cloned()
                .ok_or_else(|| "that group has gone".to_string());
        }
        if let Some(id) = self.selected.as_ref() {
            let under: Vec<_> = groups
                .values()
                .filter(|members| members.contains(id))
                .collect();
            match under.as_slice() {
                [members] => return Ok((*members).clone()),
                [] => {}
                _ => return Err("show the group with :group before saving its chain".to_string()),
            }
        }
        let tagged: BTreeSet<_> = groups.values().flatten().cloned().collect();
        if tagged.is_empty() {
            return Err("tag the modules or select a group before saving a chain".to_string());
        }
        Ok(tagged.into_iter().collect())
    }

    pub(super) fn chain_result(
        &mut self,
        request: &Request,
        value: serde_json::Value,
        now: Instant,
    ) {
        if matches!(request, Request::Chains) {
            if let Some(result) = self.parse::<ChainsResult>(request, value, now) {
                self.chain_names = result.names;
                let text = if self.chain_names.is_empty() {
                    "no saved chains".to_string()
                } else {
                    self.chain_names.join(" · ")
                };
                self.say(Tone::Info, text, now);
            }
        } else if let Some(result) = self.parse::<ChainSaveResult>(request, value, now) {
            self.say(
                Tone::Done,
                format!(
                    "saved {} modules, {} cables, {} {} to the outside left off",
                    result.modules,
                    result.cables,
                    result.outside,
                    if result.outside == 1 {
                        "cable"
                    } else {
                        "cables"
                    }
                ),
                now,
            );
        }
    }
}
