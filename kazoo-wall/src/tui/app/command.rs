//! The command line: `:` and a command, as in Neovim.
//!
//! What is typed is completed from the wall as it is: the commands, the
//! kinds in the catalogue, the modules on the wall, their outputs (where a
//! cable comes from), their inputs and knobs (where it goes), the knobs'
//! named positions, the cables plugged in and the saves. `Tab` completes
//! as far as every completion agrees, then goes through them one at a time
//! (`Shift-Tab` back); `↑↓` go through them too; `⏎` runs the line and
//! `Esc` lets it go. A command the console can tell is wrong says why on
//! the line and stays open; the wall's own refusals come back to the
//! status line, as they do for keys.

use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::protocol::{ModuleView, Request, valid_name};

use super::super::worker::Job;
use super::saves::save_name_problem;
use super::{App, MAX_ENTRY, Mode, Tone, TurnKind, View, jack_names};

/// Longest command line.
const MAX_LINE: usize = 4 * MAX_ENTRY;

/// Every command: its name, what it takes and what it does.
pub const COMMANDS: [(&str, &str, &str); 17] = [
    (
        "add",
        "[kind] [name]",
        "add a module; alone, open the picker",
    ),
    (
        "remove",
        "<module>",
        "take a module and its cables off the wall",
    ),
    (
        "patch",
        "<from.output> <to.input> [amount]",
        "plug a cable in (amount -1 to 1, 1 when left out)",
    ),
    ("unplug", "<cable number | to.input>", "pull a cable out"),
    (
        "turn",
        "<module.knob> <value>",
        "turn a knob to a value or named position",
    ),
    ("save", "<name>", "save the wall as it is (it plays on)"),
    (
        "load",
        "<name>",
        "load a save (the wall as it is is saved first; z undoes it)",
    ),
    ("undo", "", "undo the latest change"),
    ("keys", "", "every key and command"),
    (
        "tag",
        "<group> [module…]",
        "put modules in a group (alone: the selected one)",
    ),
    (
        "untag",
        "<group> [module…]",
        "take modules out of a group (alone: the selected one)",
    ),
    (
        "group",
        "[group]",
        "show only that group; alone, the whole wall",
    ),
    ("groups", "", "say every group and its modules"),
    (
        "chain",
        "save|replace|add <name>",
        "save the selected chain (replace saves over one), or add a saved copy",
    ),
    ("chains", "", "list saved chains"),
    (
        "fold",
        "[group]",
        "fold a group to one rack tile; alone, the group under the cursor",
    ),
    ("unfold", "<group>", "open a folded group on the rack"),
];

/// The command line, as typed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandLine {
    /// The line.
    pub text: String,
    /// While going through the completions: the line as typed, and the
    /// completion chosen.
    pub cycle: Option<(String, usize)>,
    /// Why the line could not run, until it is changed.
    pub error: Option<String>,
}

impl CommandLine {
    /// The line the completions are of: as typed, before any was chosen.
    #[must_use]
    pub fn typed(&self) -> &str {
        self.cycle.as_ref().map_or(&self.text, |(typed, _)| typed)
    }

    /// The completion chosen, if one is.
    #[must_use]
    pub fn chosen(&self) -> Option<usize> {
        self.cycle.as_ref().map(|(_, chosen)| *chosen)
    }
}

/// One way to finish the word being typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    /// The word, whole.
    pub word: String,
    /// What it is, in a few words.
    pub note: String,
    /// Whether more of the same word follows (a module's id before its
    /// port), so no space goes after it.
    pub open: bool,
}

/// The words of `line`, and where the word being typed starts (the end,
/// after a space).
fn split(line: &str) -> (Vec<&str>, usize) {
    let start = line.rfind(char::is_whitespace).map_or(0, |space| space + 1);
    (line.split_whitespace().collect(), start)
}

/// The command `word` names: by name, or the only one it starts.
fn command(word: &str) -> Option<&'static str> {
    let word = word.to_lowercase();
    if let Some((name, ..)) = COMMANDS.iter().find(|(name, ..)| *name == word) {
        return Some(name);
    }
    let mut starting = COMMANDS.iter().filter(|(name, ..)| name.starts_with(&word));
    match (starting.next(), starting.next()) {
        (Some((name, ..)), None) if !word.is_empty() => Some(name),
        _ => None,
    }
}

/// What a command takes and does, for the line: `patch <from…> — plug…`.
#[must_use]
pub fn usage(line: &str) -> Option<String> {
    let name = command(line.split_whitespace().next()?)?;
    let (_, takes, does) = COMMANDS.iter().find(|(other, ..)| *other == name)?;
    Some(format!("{name} {takes} · {does}"))
}

/// The longest start every word in `words` shares.
fn common_start(words: &[Completion]) -> String {
    let Some(first) = words.first() else {
        return String::new();
    };
    let mut shared: Vec<char> = first.word.chars().collect();
    for other in &words[1..] {
        let agree = shared
            .iter()
            .zip(other.word.chars())
            .take_while(|(a, b)| **a == *b)
            .count();
        shared.truncate(agree);
    }
    shared.into_iter().collect()
}

/// Which jacks of a module a port word may name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Jacks {
    /// Its outputs: where a cable comes from.
    Outputs,
    /// Its inputs and knobs: where a cable goes.
    Inputs,
    /// Its knobs.
    Knobs,
    /// Its inputs and knobs with a cable in.
    Plugged,
}

impl App {
    /// `:`: open the command line, asking for the saves to complete their
    /// names when they have not come.
    pub(super) fn open_command(&mut self, now: Instant) {
        self.mode = Mode::Command(CommandLine::default());
        if self.link.is_up() && self.saves.result.is_none() && !self.saves.fetching {
            self.fetch_saves(now);
        }
    }

    /// The completions of the word being typed at the end of `line`.
    #[must_use]
    pub fn completions(&self, line: &str) -> Vec<Completion> {
        let (words, start) = split(line);
        let word = &line[start..];
        let position = if word.is_empty() {
            words.len()
        } else {
            words.len() - 1
        };
        let lower = word.to_lowercase();
        if position == 0 {
            return COMMANDS
                .iter()
                .filter(|(name, ..)| name.starts_with(&lower))
                .map(|(name, takes, does)| Completion {
                    word: (*name).to_string(),
                    note: format!("{takes} · {does}"),
                    open: false,
                })
                .collect();
        }
        let Some(name) = command(words[0]) else {
            return Vec::new();
        };
        match (name, position) {
            ("chain", 1) => ["save", "replace", "add"]
                .into_iter()
                .filter(|action| action.starts_with(&lower))
                .map(|action| Completion {
                    word: action.to_string(),
                    note: "chain action".to_string(),
                    open: false,
                })
                .collect(),
            ("chain", 2) if matches!(words.get(1), Some(&("add" | "replace"))) => self
                .chain_names
                .iter()
                .filter(|name| name.to_lowercase().starts_with(&lower))
                .map(|name| Completion {
                    word: name.clone(),
                    note: "saved chain".to_string(),
                    open: false,
                })
                .collect(),
            ("add", 1) => self
                .catalogue
                .iter()
                .filter(|info| info.kind.starts_with(&lower))
                .map(|info| Completion {
                    word: info.kind.clone(),
                    note: info.about.clone(),
                    open: false,
                })
                .collect(),
            ("tag" | "untag" | "group" | "fold" | "unfold", 1) => self
                .groups()
                .into_iter()
                .flatten()
                .filter(|(group, _)| group.to_lowercase().starts_with(&lower))
                .map(|(group, members)| Completion {
                    word: group.clone(),
                    note: members.join(" "),
                    open: false,
                })
                .collect(),
            ("tag" | "untag", _) | ("remove", 1) => self
                .modules()
                .iter()
                .filter(|module| module.id.to_lowercase().starts_with(&lower))
                .map(|module| Completion {
                    word: module.id.clone(),
                    note: module_note(module),
                    open: false,
                })
                .collect(),
            ("patch", 1) => self.port_completions(word, Jacks::Outputs),
            ("patch", 2) => self.port_completions(word, Jacks::Inputs),
            ("unplug", 1) => self.port_completions(word, Jacks::Plugged),
            ("turn", 1) => self.port_completions(word, Jacks::Knobs),
            ("turn", 2) => self.label_completions(words[1], &lower),
            ("save" | "load", 1) => self
                .shown_names()
                .into_iter()
                .filter(|save| save.to_lowercase().starts_with(&lower))
                .map(|save| Completion {
                    word: save,
                    note: "a save".to_string(),
                    open: false,
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    /// The completions of a `module.port` word: the modules with such
    /// jacks (their id and a dot), then, after the dot, the jacks.
    fn port_completions(&self, word: &str, jacks: Jacks) -> Vec<Completion> {
        let plugged = |module: &ModuleView, jack: &str| {
            let to = format!("{}.{jack}", module.id);
            self.cables().into_iter().find(|cable| cable.to == to)
        };
        let names = |module: &ModuleView| -> Vec<(String, String)> {
            match jacks {
                Jacks::Outputs => module
                    .outputs
                    .iter()
                    .map(|name| (name.clone(), "output".to_string()))
                    .collect(),
                Jacks::Inputs => jack_names(module)
                    .into_iter()
                    .map(|name| {
                        let note = if module.inputs.iter().any(|input| input == name) {
                            "input"
                        } else {
                            "knob"
                        };
                        (name.to_string(), note.to_string())
                    })
                    .collect(),
                Jacks::Knobs => module
                    .knobs
                    .iter()
                    .map(|knob| (knob.name.clone(), "knob".to_string()))
                    .collect(),
                Jacks::Plugged => jack_names(module)
                    .into_iter()
                    .filter_map(|name| {
                        plugged(module, name).map(|cable| {
                            (
                                name.to_string(),
                                format!("cable {} from {}", cable.id, cable.from),
                            )
                        })
                    })
                    .collect(),
            }
        };
        if let Some((id, port)) = word.split_once('.') {
            let port = port.to_lowercase();
            let Some(module) = self.module(id) else {
                return Vec::new();
            };
            return names(module)
                .into_iter()
                .filter(|(name, _)| name.to_lowercase().starts_with(&port))
                .map(|(name, note)| Completion {
                    word: format!("{id}.{name}"),
                    note,
                    open: false,
                })
                .collect();
        }
        let lower = word.to_lowercase();
        self.modules()
            .iter()
            .filter(|module| module.id.to_lowercase().starts_with(&lower))
            .filter(|module| !names(module).is_empty())
            .map(|module| Completion {
                word: format!("{}.", module.id),
                note: module_note(module),
                open: true,
            })
            .collect()
    }

    /// The named positions of knob `jack` (`module.knob`) starting `lower`.
    fn label_completions(&self, jack: &str, lower: &str) -> Vec<Completion> {
        let Some((id, knob)) = jack.split_once('.') else {
            return Vec::new();
        };
        let Some(module) = self.module(id) else {
            return Vec::new();
        };
        self.knob_info(&module.kind, knob)
            .map(|info| {
                info.labels
                    .iter()
                    .filter(|label| label.to_lowercase().starts_with(lower))
                    .map(|label| Completion {
                        word: label.clone(),
                        note: format!("a position of {jack}"),
                        open: false,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// A key on the command line.
    pub(super) fn command_key(
        &mut self,
        key: KeyEvent,
        mut line: CommandLine,
        now: Instant,
    ) -> Mode {
        match key.code {
            KeyCode::Esc => return Mode::Normal,
            KeyCode::Enter => {
                // `:a` (or `:add`) with no kind opens the picker.
                let text = line.text.trim();
                if !text.contains(char::is_whitespace) && command(text) == Some("add") {
                    self.open_picker(now);
                    return std::mem::replace(&mut self.mode, Mode::Normal);
                }
                return match self.run_command(line.text.trim(), now) {
                    // A command may open another mode (`keys` opens the help).
                    Ok(()) => std::mem::replace(&mut self.mode, Mode::Normal),
                    Err(why) => {
                        line.cycle = None;
                        line.error = Some(why);
                        Mode::Command(line)
                    }
                };
            }
            KeyCode::Tab => self.complete(&mut line, true, true),
            KeyCode::BackTab => self.complete(&mut line, false, true),
            KeyCode::Down => self.complete(&mut line, true, false),
            KeyCode::Up => self.complete(&mut line, false, false),
            KeyCode::Backspace => {
                line.cycle = None;
                line.error = None;
                line.text.pop();
            }
            KeyCode::Char(c)
                if !c.is_control()
                    && !key.modifiers.contains(KeyModifiers::CONTROL)
                    && line.text.chars().count() < MAX_LINE =>
            {
                line.cycle = None;
                line.error = None;
                line.text.push(c);
            }
            _ => {}
        }
        Mode::Command(line)
    }

    /// Complete the word being typed: with `fill`, first as far as every
    /// completion agrees (the only one whole, with a space after it), then
    /// one completion at a time, `forward` or back.
    fn complete(&self, line: &mut CommandLine, forward: bool, fill: bool) {
        line.error = None;
        let typed = line.typed().to_string();
        let found = self.completions(&typed);
        if found.is_empty() {
            return;
        }
        let (_, start) = split(&typed);
        if fill && line.cycle.is_none() {
            if let [only] = found.as_slice() {
                line.text = format!("{}{}", &typed[..start], only.word);
                if !only.open {
                    line.text.push(' ');
                }
                return;
            }
            let shared = common_start(&found);
            if shared.len() > typed.len() - start {
                line.text = format!("{}{shared}", &typed[..start]);
                return;
            }
        }
        let count = found.len();
        let chosen = match line.chosen() {
            Some(chosen) if forward => (chosen + 1) % count,
            Some(chosen) => (chosen + count - 1) % count,
            None if forward => 0,
            None => count - 1,
        };
        line.text = format!("{}{}", &typed[..start], found[chosen].word);
        line.cycle = Some((typed, chosen));
    }

    /// Run a command line; why not, when it cannot be.
    fn run_command(&mut self, text: &str, now: Instant) -> Result<(), String> {
        let words: Vec<&str> = text.split_whitespace().collect();
        let Some(first) = words.first() else {
            return Err("type a command: Tab shows them".to_string());
        };
        let name =
            command(first).ok_or_else(|| format!("'{first}' is not a command: Tab lists them"))?;
        let args = &words[1..];
        if matches!(name, "fold" | "unfold" | "chain" | "chains") {
            return self.run_extra_command(name, args, now);
        }
        if !self.link.is_up() {
            return Err("the wall is not answering".to_string());
        }
        let rest = text
            .split_once(char::is_whitespace)
            .map_or("", |(_, rest)| rest.trim());
        let wants = |count: usize| -> Result<(), String> {
            if args.len() < count {
                Err(format!("type {}", usage(name).unwrap_or_default()))
            } else {
                Ok(())
            }
        };
        match name {
            "add" => {
                wants(1)?;
                self.run_add(args[0], rest[args[0].len()..].trim(), now)?;
            }
            "remove" => {
                wants(1)?;
                let module = self.on_wall(args[0])?;
                self.say(Tone::Info, format!("removing {module} (z undoes it)…"), now);
                self.jobs.push(Job::Call(Request::Remove { module }));
            }
            "patch" => {
                wants(2)?;
                self.run_patch(args, now)?;
            }
            "unplug" => {
                wants(1)?;
                let cable = self.cable_for(args[0])?;
                self.jobs.push(Job::Call(Request::Unpatch {
                    cable: Some(cable),
                    to: None,
                }));
            }
            "turn" => {
                wants(2)?;
                let jack = self.jack(args[0], Jacks::Knobs)?;
                let (module, knob) = jack.split_once('.').unwrap_or((&jack, ""));
                let value = self.typed_value(module, knob, rest[args[0].len()..].trim())?;
                let (module, knob) = (module.to_string(), knob.to_string());
                self.hold_turn(&module, &knob, value, TurnKind::Now, None, now);
                self.tick(now);
            }
            "save" => {
                wants(1)?;
                if let Some(problem) = save_name_problem(rest) {
                    return Err(problem);
                }
                self.save_as(rest, now);
            }
            "load" => {
                wants(1)?;
                self.say(
                    Tone::Info,
                    format!("loading \"{rest}\" (the wall as it is is saved first)…"),
                    now,
                );
                self.jobs.push(Job::Call(Request::Load {
                    name: rest.to_string(),
                }));
            }
            "keys" => self.mode = Mode::Help { scroll: 0 },
            "tag" | "untag" => {
                wants(1)?;
                let modules = if args.len() > 1 {
                    args[1..]
                        .iter()
                        .map(|module| self.on_whole_wall(module))
                        .collect::<Result<Vec<String>, String>>()?
                } else {
                    vec![
                        self.selected
                            .clone()
                            .ok_or_else(|| "select a module, or name the modules".to_string())?,
                    ]
                };
                self.jobs.push(Job::Call(Request::Group {
                    group: args[0].to_string(),
                    modules,
                    join: name == "tag",
                }));
            }
            "group" => self.run_group(args.first().copied(), now)?,
            "groups" => self.say_groups(now),
            _ => self.undo_latest(now),
        }
        Ok(())
    }

    /// `group`: show only `group`, or (`None`) the whole wall again.
    fn run_group(&mut self, group: Option<&str>, now: Instant) -> Result<(), String> {
        let Some(group) = group else {
            self.show_group(None);
            self.say(Tone::Info, "the whole wall".to_string(), now);
            return Ok(());
        };
        let found = self
            .groups()
            .and_then(|groups| groups.keys().find(|name| name.eq_ignore_ascii_case(group)))
            .cloned()
            .ok_or_else(|| format!("there is no group called '{group}' (:groups says them)"))?;
        self.show_group(Some(found.clone()));
        self.say(
            Tone::Info,
            format!("only {found} is shown · :group alone shows the whole wall"),
            now,
        );
        Ok(())
    }

    /// `groups`: say every group and its modules.
    fn say_groups(&mut self, now: Instant) {
        let words = match self.groups() {
            Some(groups) if !groups.is_empty() => groups
                .iter()
                .map(|(group, members)| format!("{group}: {}", members.join(" ")))
                .collect::<Vec<String>>()
                .join(" · "),
            _ => "no groups yet: :tag <group> <module…> makes one".to_string(),
        };
        self.say(Tone::Info, words, now);
    }

    /// `add`: a module of `kind`, named `given` (no name when empty).
    fn run_add(&mut self, kind: &str, given: &str, now: Instant) -> Result<(), String> {
        if self.kind_info(kind).is_none() {
            return Err(format!("there is no kind of module called '{kind}'"));
        }
        let module_name = if given.is_empty() {
            None
        } else if valid_name(given) {
            Some(given.to_string())
        } else {
            return Err("a name is 1 to 24 of A-Z a-z 0-9 space _ . -".to_string());
        };
        let place = if self.view == View::Rack {
            self.after_selected()
        } else {
            None
        };
        self.add_module(kind.to_string(), module_name, place, now);
        Ok(())
    }

    /// `patch`: a cable from `args[0]` to `args[1]`, at `args[2]` or 1.
    fn run_patch(&mut self, args: &[&str], now: Instant) -> Result<(), String> {
        let from = self.jack(args[0], Jacks::Outputs)?;
        let to = self.jack(args[1], Jacks::Inputs)?;
        let amount = match args.get(2) {
            None => 1.0,
            Some(text) => match text.parse::<f64>() {
                Ok(amount) if (-1.0..=1.0).contains(&amount) => amount,
                Ok(_) | Err(_) => {
                    return Err(format!("'{text}' is not an amount: type -1 to 1"));
                }
            },
        };
        self.say(Tone::Info, "plugging in…".to_string(), now);
        self.jobs.push(Job::Call(Request::Patch {
            from,
            to,
            amount: Some(amount),
        }));
        Ok(())
    }

    /// The cable `word` names: by number, or by the jack it is plugged
    /// into; or why there is none.
    fn cable_for(&self, word: &str) -> Result<u32, String> {
        let cables = self.cables();
        word.parse::<u32>().map_or_else(
            |_| {
                let to = self.jack(word, Jacks::Inputs)?;
                cables
                    .iter()
                    .find(|cable| cable.to == to)
                    .map(|cable| cable.id)
                    .ok_or_else(|| format!("nothing is plugged into {to}"))
            },
            |number| {
                cables
                    .iter()
                    .find(|cable| cable.id == number)
                    .map(|cable| cable.id)
                    .ok_or_else(|| format!("there is no cable {number}"))
            },
        )
    }

    /// The id of module `id` on the wall, or why not.
    /// As [`Self::on_wall`], but looking past the group shown.
    fn on_whole_wall(&self, id: &str) -> Result<String, String> {
        self.snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.modules.iter().find(|module| module.id == id))
            .map(|module| module.id.clone())
            .ok_or_else(|| format!("nothing on the wall is called '{id}'"))
    }

    fn on_wall(&self, id: &str) -> Result<String, String> {
        self.module(id)
            .map(|module| module.id.clone())
            .ok_or_else(|| format!("nothing on the wall is called '{id}'"))
    }

    /// `word` as a jack of the right sort on the wall (`module.port`), or
    /// why it is not one.
    fn jack(&self, word: &str, jacks: Jacks) -> Result<String, String> {
        let (id, port) = word
            .split_once('.')
            .ok_or_else(|| format!("'{word}' is not a jack: type module.port, e.g. lfo1.out"))?;
        let module = self
            .module(id)
            .ok_or_else(|| format!("nothing on the wall is called '{id}'"))?;
        let (found, sort) = match jacks {
            Jacks::Outputs => (module.outputs.iter().any(|name| name == port), "output"),
            Jacks::Inputs | Jacks::Plugged => (jack_names(module).contains(&port), "input or knob"),
            Jacks::Knobs => (module.knobs.iter().any(|knob| knob.name == port), "knob"),
        };
        if found {
            Ok(word.to_string())
        } else {
            Err(format!("{id} has no {sort} called '{port}'"))
        }
    }
}

/// A module in a few words: its kind, and its name when it has one.
fn module_note(module: &ModuleView) -> String {
    module.name.as_ref().map_or_else(
        || module.kind.clone(),
        |name| format!("{} \"{name}\"", module.kind),
    )
}

/// Keys to the command line, for the hints.
pub const COMMAND_KEYS: &str = "Tab completes · ↑↓ choose · ⏎ runs · Esc cancels";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_is_named_whole_or_by_its_only_start() {
        assert_eq!(command("patch"), Some("patch"));
        assert_eq!(command("pa"), Some("patch"));
        assert_eq!(command("u"), None, "unplug and undo");
        assert_eq!(command("unp"), Some("unplug"));
        assert_eq!(command(""), None);
        assert_eq!(command("nope"), None);
    }

    #[test]
    fn the_word_being_typed_starts_after_the_last_space() {
        assert_eq!(split("patch lfo1.o"), (vec!["patch", "lfo1.o"], 6));
        assert_eq!(split("patch "), (vec!["patch"], 6));
        assert_eq!(split(""), (vec![], 0));
    }
}
