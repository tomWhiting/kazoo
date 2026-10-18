//! `kzoo setup`: everything kazoo needs for a good time, in one go, all in
//! the terminal.
//!
//! The program installed to stay, the tree planted, the one MCP in Claude
//! Code and in Codex (no name in it: a seat picks its name when it joins a
//! wall), and the kazoo skill for Claude. Each step says what it did; a
//! step that cannot be done says why, and the rest still go. What is right
//! already is left; what is out of date is put right; what is somebody
//! else's is only reported.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use color_eyre::eyre::{WrapErr, eyre};

use crate::cli::SetupArgs;

/// The npm package the program comes in.
const NPM_PACKAGE: &str = "kzhz";

/// How the program is installed from source, as the README says it.
const CARGO_INSTALL: &str = "cargo install kzoo";

/// The skill's folder name under Claude Code's `skills`.
const SKILL_NAME: &str = "kazoo";

/// The file that marks a skill folder as kazoo's own. It holds the version
/// that wrote it and a hash of what it wrote: a folder without it is
/// somebody else's and is left alone.
const SKILL_MARK: &str = ".kzoo-skill";

/// Where a skill is put together before it is swapped in, beside the
/// skills folder so the skill loader never sees it half made.
const STAGING: &str = ".kzoo-skill-staging";

/// The skill's files, as they go under its folder.
const SKILL: &[(&str, &str)] = &[
    ("SKILL.md", include_str!("../skill/SKILL.md")),
    (
        "references/instruments.md",
        include_str!("../skill/references/instruments.md"),
    ),
    (
        "references/wall.md",
        include_str!("../skill/references/wall.md"),
    ),
    (
        "references/modular.md",
        include_str!("../skill/references/modular.md"),
    ),
    (
        "references/aphex.md",
        include_str!("../skill/references/aphex.md"),
    ),
];

/// What else a good time needs, said last: each thing and what it is for.
/// `$name` is the program's name as it was run.
const PACKING_LIST: &[(&str, &str)] = &[
    (
        "headphones, or the good speakers",
        "the wall plays forever; your housemates don't",
    ),
    (
        "$name wall",
        "start the wall. It's silent until you press m",
    ),
    (
        "$name mix",
        "the desk: every instrument you start gets a strip",
    ),
    (
        "a Claude or two",
        "\"join the wall as Flapjack\": each picks its own name",
    ),
    (
        "a friend with a Mac",
        "one copy of kazoo each, the same one",
    ),
    (
        "$name tree",
        "everything that ever played, as a tree you can walk",
    ),
    (
        "a mark at the good bit",
        "wall_mark. You will not find it again otherwise",
    ),
    ("water", "you will forget, three hours in"),
    (
        "half a bag of chips from last time",
        "gone a bit soft; eaten anyway",
    ),
    (
        "the one lead that works",
        "you know the one. Not that one. The other one",
    ),
    (
        "a note to future you",
        "a batch's note: what you were going for, before you forget",
    ),
];

/// The packing list as `name` says it, the first line saying how setup
/// went: a dry run, done, or steps still to do: `missed` that setup can do
/// again, and `left` for the person to see to first.
pub(crate) fn packing_list(name: &str, dry: bool, missed: usize, left: usize) -> String {
    let line = |done: bool, thing: &str, what: &str| {
        let thing = thing.replace("$name", name);
        format!("  [{}] {thing:<36}{what}", if done { 'x' } else { ' ' })
    };
    let what = if dry {
        format!("{name} setup, without --dry-run")
    } else {
        match (missed, left) {
            (0, 0) => "just now".to_owned(),
            (missed, 0) => format!("{missed} to finish: {name} setup again"),
            (0, left) => {
                format!("{left} left for you: do what it says above, then {name} setup again")
            }
            (missed, left) => format!(
                "{} to finish: do what each says above, then {name} setup again",
                missed + left
            ),
        }
    };
    let first = line(!dry && missed == 0 && left == 0, "kazoo, set up", &what);
    let mut lines = vec![
        "What you need for a good time".to_owned(),
        "-----------------------------".to_owned(),
        String::new(),
        first,
    ];
    lines.extend(
        PACKING_LIST
            .iter()
            .map(|(thing, what)| line(false, thing, what)),
    );
    lines.push(String::new());
    lines.push(format!("Start with: {name} wall"));
    lines.push(String::new());
    lines.join("\n")
}

/// This program's version.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The names this program is installed under: `kzoo`, `kzhz` (npm's name,
/// and its own crate's) and `kazoo`.
const NAMES: [&str; 3] = ["kzoo", "kzhz", "kazoo"];

/// What setup reads from where it runs: passed in whole, so a test can set
/// up a machine of its own.
#[derive(Debug, Clone)]
pub(crate) struct Machine {
    /// The folders programs are found in, in order.
    pub path: Vec<PathBuf>,
    /// The home folder.
    pub home: PathBuf,
    /// Claude Code's own folder, when `CLAUDE_CONFIG_DIR` moves it.
    pub claude_dir: Option<PathBuf>,
    /// The package runner this runs under (`npm_config_user_agent`), if any.
    pub agent: String,
    /// The folder setup runs in.
    pub folder: PathBuf,
    /// The wall's state folder.
    pub state: PathBuf,
    /// The tree's folder.
    pub tree: PathBuf,
    /// The wall's socket.
    pub socket: PathBuf,
}

impl Machine {
    /// This machine, as the environment says.
    fn here() -> color_eyre::Result<Self> {
        let home = std::env::var_os("HOME")
            .filter(|home| !home.is_empty())
            .map(PathBuf::from)
            .ok_or_else(|| eyre!("$HOME is not set"))?;
        let path = std::env::var_os("PATH")
            .map(|path| std::env::split_paths(&path).collect())
            .unwrap_or_default();
        let claude_dir = std::env::var_os("CLAUDE_CONFIG_DIR")
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from);
        let agent = std::env::var("npm_config_user_agent").unwrap_or_default();
        let folder = std::env::current_dir()
            .map_err(|err| eyre!("the folder this runs in cannot be found ({err})"))?;
        Ok(Self {
            path,
            home,
            claude_dir,
            agent,
            folder,
            state: kazoo_wall::paths::state_dir()?,
            tree: kazoo_tree::tree_dir()?,
            socket: kazoo_wall::paths::socket_path(),
        })
    }

    /// Claude Code's config file, holding its MCP entries.
    fn claude_config(&self) -> PathBuf {
        self.claude_dir.as_ref().map_or_else(
            || self.home.join(".claude.json"),
            |dir| dir.join(".claude.json"),
        )
    }

    /// Claude Code's own folder.
    fn claude_home(&self) -> PathBuf {
        self.claude_dir
            .clone()
            .unwrap_or_else(|| self.home.join(".claude"))
    }

    /// `program` as this machine finds it by name: the first match on the
    /// path, as written there (a link is kept, so an upgrade it points at
    /// follows), passing over a package runner's throwaway copies and a
    /// project's own (`node_modules/.bin`, npx's cache, bunx's temp).
    fn find(&self, program: &str) -> Option<PathBuf> {
        self.path
            .iter()
            .filter(|dir| dir.is_absolute() && !throwaway(dir))
            .map(|dir| dir.join(program))
            .find(|candidate| is_program(candidate))
    }
}

/// Whether `dir` holds programs a package runner keeps only for a while, or
/// a project keeps for itself.
fn throwaway(dir: &Path) -> bool {
    dir.components().any(|part| match part {
        Component::Normal(name) => {
            let name = name.to_string_lossy();
            name == "node_modules" || name == "_npx" || name.starts_with("bunx-")
        }
        _ => false,
    })
}

fn is_program(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

/// Run `kzoo setup` as `name`.
///
/// # Errors
///
/// When the program cannot be found or installed to stay at this version:
/// nothing else can point at it. Every other step says its own failure and
/// the rest go on; the run then fails, saying how many steps could not be
/// done.
pub fn run(name: &str, args: &SetupArgs) -> color_eyre::Result<()> {
    run_on(name, args, &Machine::here()?)
}

pub(crate) fn run_on(name: &str, args: &SetupArgs, machine: &Machine) -> color_eyre::Result<()> {
    let dry = args.dry_run;
    let Program {
        path: program,
        notes,
    } = program(name, machine, dry)?;
    say(name, &format!("the program: {}", program.display()));
    for note in &notes {
        say(name, note);
    }
    let mut missed = 0_usize;
    let mut left = 0_usize;
    let mut step = |what: &str, result: color_eyre::Result<String>| match result {
        Ok(done) => say(name, &format!("{what}: {done}")),
        Err(why) if why.downcast_ref::<LeftForYou>().is_some() => {
            left += 1;
            say(name, &format!("{what}: left for you: {why:#}"));
        }
        Err(why) => {
            missed += 1;
            say(name, &format!("{what}: not done: {why:#}"));
        }
    };
    step("the tree", tree(name, machine, dry));
    step("Claude Code", claude(machine, &program, dry));
    step("the kazoo skill", skill(name, machine, dry));
    step("Codex", codex(machine, &program, dry));
    println!();
    print!("{}", packing_list(name, dry, missed, left));
    match (missed, left) {
        (0, 0) => Ok(()),
        (missed, 0) => Err(eyre!(
            "{missed} of the steps above could not be done; each says why, and {name} setup can be run again"
        )),
        (missed, left) => Err(eyre!(
            "{} of the steps above could not be done; each says why and what to do, then {name} setup can be run again",
            missed + left
        )),
    }
}

/// A step setup leaves alone, as it is not setup's to change: run again,
/// setup would leave it again, until the person does what it says.
#[derive(Debug)]
struct LeftForYou(String);

impl std::fmt::Display for LeftForYou {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for LeftForYou {}

fn left_for_you(why: String) -> color_eyre::Report {
    color_eyre::Report::new(LeftForYou(why))
}

fn say(name: &str, line: &str) {
    println!("{name} setup: {line}");
}

/// How a package runner installs a package to stay, and where it puts
/// the programs.
struct Installer {
    /// The runner itself, as this machine finds it.
    tool: PathBuf,
    /// The command and its arguments, the package last.
    install: Vec<String>,
    /// The command that prints its global programs' folder, and what to add
    /// to what it prints.
    bin: (&'static [&'static str], &'static str),
    /// What to try when the install fails.
    hint: &'static str,
}

impl Installer {
    /// The installer of the runner this runs under, if it is one setup can
    /// use.
    fn of(machine: &Machine) -> Result<Option<Self>, String> {
        type Bin = (&'static [&'static str], &'static str);
        let agent = machine.agent.as_str();
        let package = format!("{NPM_PACKAGE}@{VERSION}");
        let (install, bin, hint): (&[&str], Bin, &'static str) = if agent.starts_with("npm/") {
            (
                &["npm", "install", "-g"],
                (&["npm", "prefix", "-g"], "bin"),
                "if it says EACCES, npm's global folder needs to be your own: https://docs.npmjs.com/resolving-eacces-permissions-errors-when-installing-packages-globally",
            )
        } else if agent.starts_with("bun/") {
            (
                &["bun", "add", "-g"],
                (&["bun", "pm", "bin", "-g"], ""),
                "bun keeps global programs in ~/.bun/bin: check it is yours to write in",
            )
        } else if agent.starts_with("pnpm/") {
            (
                &["pnpm", "add", "-g"],
                (&["pnpm", "bin", "-g"], ""),
                "if it says there is no global bin folder, run pnpm setup (it sets PNPM_HOME) and open a new terminal",
            )
        } else if agent.starts_with("yarn/") {
            return Err(format!(
                "setup does not install with yarn: npm install -g {package}, then setup again"
            ));
        } else {
            return Ok(None);
        };
        let tool = machine.find(install[0]).ok_or_else(|| {
            format!(
                "{} is not where this machine finds programs by name",
                install[0]
            )
        })?;
        let mut install: Vec<String> = install.iter().map(|&part| part.to_owned()).collect();
        install.push(package);
        Ok(Some(Self {
            tool,
            install,
            bin,
            hint,
        }))
    }

    fn line(&self) -> String {
        self.install.join(" ")
    }

    /// Where the runner puts its global programs.
    fn global_bin(&self) -> color_eyre::Result<PathBuf> {
        let (command, then) = self.bin;
        let out = Command::new(&self.tool)
            .args(&command[1..])
            .output()
            .wrap_err_with(|| format!("{} cannot be run", command.join(" ")))?;
        if !out.status.success() {
            return Err(eyre!("{} failed ({})", command.join(" "), out.status));
        }
        let printed = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        if printed.is_empty() {
            return Err(eyre!("{} said nothing", command.join(" ")));
        }
        // An empty `then` would add a trailing separator to the folder.
        let bin = PathBuf::from(printed);
        Ok(if then.is_empty() { bin } else { bin.join(then) })
    }
}

/// The program setup points everything at, and what it noticed on the way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Program {
    pub path: PathBuf,
    /// Things to know: another kazoo that is not this version, or a folder
    /// not on PATH.
    pub notes: Vec<String>,
}

/// The program as this machine runs it by name, installed to stay, at this
/// version: the first of [`NAMES`] it finds that says it is this version.
/// Run from a package runner (`npx`, `bunx`, `pnpm dlx`), which keeps it
/// only for the one run, it is installed globally first with the same
/// runner, at this version. Every other kazoo found by name that is not
/// this version is noted.
fn program(name: &str, machine: &Machine, dry: bool) -> color_eyre::Result<Program> {
    let mut current = None;
    let mut others = Vec::new();
    for found in NAMES.iter().filter_map(|program| machine.find(program)) {
        match version_of(&found) {
            Ok(version) if version == VERSION => {
                if current.is_none() {
                    current = Some(found);
                }
            }
            Ok(version) => others.push((found, format!("version {version}"))),
            Err(why) => others.push((found, why)),
        }
    }
    // One note for each way to remove them: one package owns all three
    // names, and is named once.
    let other_notes = |skip: Option<&Path>| -> Vec<String> {
        let kept: Vec<_> = others
            .iter()
            .filter(|(old, _)| skip.is_none_or(|bin| old.parent() != Some(bin)))
            .cloned()
            .collect();
        removals(&kept)
            .into_iter()
            .map(|(how, copies)| {
                let (are, run, names) = if copies.len() == 1 {
                    ("is another kazoo", "runs", "its name")
                } else {
                    ("are another kazoo", "run", "their names")
                };
                format!(
                    "note: {} {are}, not this version, and {run} by {names} instead of this one: {how} to be sure",
                    copies.join(", ")
                )
            })
            .collect()
    };
    if let Some(path) = current {
        return Ok(Program {
            path,
            notes: other_notes(None),
        });
    }
    let installer = Installer::of(machine).map_err(|why| eyre!("{why}"))?;
    let Some(installer) = installer else {
        let install = format!(
            "npm install -g {NPM_PACKAGE}@{VERSION} (or {CARGO_INSTALL}), then {name} setup"
        );
        return Err(if others.is_empty() {
            eyre!("kazoo is not installed where this machine finds programs by name: {install}")
        } else {
            // What is there first goes, or cargo refuses the new one.
            let others: Vec<String> = removals(&others)
                .into_iter()
                .map(|(how, copies)| format!("{}: {how}", copies.join(", ")))
                .collect();
            eyre!(
                "this is {VERSION}, and what is installed is not: {}; then {install}",
                others.join("; ")
            )
        });
    };
    let line = installer.line();
    let bin = installer.global_bin()?;
    let path = bin.join(NPM_PACKAGE);
    let mut notes = Vec::new();
    if !machine.path.iter().any(|dir| dir == &bin) {
        notes.push(format!(
            "note: {} is not on PATH, so by name this machine will not find what {} installed: add it to PATH (Claude Code and Codex are given its full path, so they will)",
            bin.display(),
            if dry { "would be" } else { "was" }
        ));
    }
    // What the runner installs replaces any kazoo in its own folder.
    notes.extend(other_notes(Some(&bin)));
    if dry {
        say(
            name,
            &format!(
                "would install the program to stay: {line} (into {})",
                bin.display()
            ),
        );
        return Ok(Program { path, notes });
    }
    say(name, &format!("installing the program to stay: {line}"));
    let status = Command::new(&installer.tool)
        .args(&installer.install[1..])
        .status()
        .wrap_err_with(|| format!("{line} cannot be run"))?;
    if !status.success() {
        return Err(eyre!("{line} failed ({status}); {}", installer.hint));
    }
    match version_of(&path) {
        Ok(version) if version == VERSION => Ok(Program { path, notes }),
        Ok(version) => Err(eyre!(
            "{line} finished, but {} says it is {version}, not {VERSION}",
            path.display()
        )),
        Err(why) => Err(eyre!("{line} finished, but {} {why}", path.display())),
    }
}

/// The old copies (each with what it said of its version), grouped by how
/// each is removed, in the order found: `[(how, ["path (what)", ...])]`.
fn removals(others: &[(PathBuf, String)]) -> Vec<(String, Vec<String>)> {
    let mut grouped: Vec<(String, Vec<String>)> = Vec::new();
    for (old, what) in others {
        let how = how_to_remove(old);
        let copy = format!("{} ({what})", old.display());
        match grouped.iter_mut().find(|(seen, _)| *seen == how) {
            Some((_, copies)) => copies.push(copy),
            None => grouped.push((how, vec![copy])),
        }
    }
    grouped
}

/// How to take away the program at `path`: with what installed it, when
/// that can be told, or by deleting it.
pub(crate) fn how_to_remove(path: &Path) -> String {
    if let Some(package) = cargo_package(path) {
        return format!("remove it (cargo uninstall {package})");
    }
    if let Some(remove) = runner_package(path) {
        return format!("remove it ({remove})");
    }
    format!("delete {}", path.display())
}

/// The cargo package that installed `path`, as the `.crates2.json` beside
/// its `bin` folder records it.
fn cargo_package(path: &Path) -> Option<String> {
    let bin = path.parent()?;
    let file = path.file_name()?.to_str()?;
    // No record, or one that cannot be read, cannot say: it is deleted.
    let Ok(record) = fs::read(bin.parent()?.join(".crates2.json")) else {
        return None;
    };
    let Ok(record) = serde_json::from_slice::<serde_json::Value>(&record) else {
        return None;
    };
    record
        .get("installs")?
        .as_object()?
        .iter()
        .find(|(_, install)| {
            install
                .get("bins")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|bins| bins.iter().any(|bin| bin.as_str() == Some(file)))
        })
        .and_then(|(package, _)| package.split_whitespace().next().map(str::to_owned))
}

/// The package runner's command to remove the package `path` comes from,
/// when it is a runner's global program: a link into a `node_modules`
/// (npm, bun, yarn 1), or a small script that runs a file in one (pnpm:
/// on Unix the others link).
fn runner_package(path: &Path) -> Option<String> {
    let Ok(real) = fs::canonicalize(path) else {
        return None;
    };
    let real_text = real.to_string_lossy().into_owned();
    let (package, target) = if real_text.contains("node_modules/") {
        (package_at(&real_text, Path::new("/"))?, real_text.clone())
    } else {
        let script = small_script(path)?;
        let target = script_target(&script)?;
        // pnpm's shim, by its shape; any other script is someone's own,
        // and only deleting it takes it off the path.
        if !script.contains("basedir=$(dirname") || !target.starts_with("$basedir/") {
            return None;
        }
        let base = real.parent().unwrap_or_else(|| Path::new("/"));
        // A shim whose program has gone is left over: pnpm would refuse.
        if !Path::new(&target.replace("$basedir", &base.to_string_lossy())).exists() {
            return None;
        }
        (package_at(&target, base)?, target)
    };
    let runner = if real_text.contains("/.bun/") || target.contains("/.bun/") {
        "bun remove -g"
    } else if !real_text.contains("node_modules/") {
        "pnpm remove -g"
    } else {
        runner_of_link(&real_text)
    };
    Some(format!("{runner} {package}"))
}

/// Which runner put a global package where `real` (a link's end, in a
/// `node_modules`) is, by the folders each keeps them in.
fn runner_of_link(real: &str) -> &'static str {
    if real.contains("/install/global/node_modules/") {
        // bun's `$BUN_INSTALL/install/global`, wherever BUN_INSTALL is.
        "bun remove -g"
    } else if real.contains("/yarn/global/node_modules/") {
        "yarn global remove"
    } else if real.contains("/pnpm/") || real.contains("/.pnpm/") {
        "pnpm remove -g"
    } else {
        "npm uninstall -g"
    }
}

/// The most a runner's script is: anything bigger is a program itself.
const SCRIPT_MOST: u64 = 64 * 1024;

/// The text of `path` when it is a small script (`#!`, at most
/// [`SCRIPT_MOST`]), read no further than that.
fn small_script(path: &Path) -> Option<String> {
    use std::io::Read;
    let Ok(file) = fs::File::open(path) else {
        return None;
    };
    let mut text = Vec::new();
    let Ok(read) = file.take(SCRIPT_MOST + 1).read_to_end(&mut text) else {
        return None;
    };
    (u64::try_from(read).is_ok_and(|read| read <= SCRIPT_MOST) && text.starts_with(b"#!"))
        .then(|| String::from_utf8_lossy(&text).into_owned())
}

/// The file a runner's script runs: the last of its words (split at
/// spaces, quotes, `=` and the `:` between `NODE_PATH`'s folders) that is
/// in a `node_modules`, which is the `exec` line's.
fn script_target(script: &str) -> Option<String> {
    script
        .split(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '=' | ':'))
        .rfind(|word| word.contains("node_modules/"))
        .map(str::to_owned)
}

/// The package one path names: the folder after its last `node_modules/`,
/// a scoped one (`@scope/name`) whole, with `$basedir` read as `base`.
/// None is named for one inside another package (a `node_modules` before
/// it), except in pnpm's store, where the package must also be installed
/// globally beside the store (not a dependency of one that is).
fn package_at(path: &str, base: &Path) -> Option<String> {
    let path = path.replace("$basedir", &base.to_string_lossy());
    let (before, after) = path.rsplit_once("node_modules/")?;
    let mut parts = after.split('/');
    let first = parts.next().filter(|part| !part.is_empty())?;
    let package = if first.starts_with('@') {
        let name = parts.next().filter(|part| !part.is_empty())?;
        format!("{first}/{name}")
    } else {
        first.to_owned()
    };
    match before.split_once("/.pnpm/") {
        Some((store, _)) => Path::new(store).join(&package).exists().then_some(package),
        None if before.contains("node_modules/") => None,
        None => Some(package),
    }
}

/// The version `program --version` says (its last word), or why it says
/// none.
fn version_of(program: &Path) -> Result<String, String> {
    version_within(program, VERSION_WAIT)
}

/// How long a program found by name has to say its version: it may not be
/// kazoo at all, and one that never answers must not stop setup.
const VERSION_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// [`version_of`], waiting at most `wait` for the answer. Whatever way it
/// ends, the program is stopped with everything it started that is still
/// in its process group; something that left the group (a daemon that
/// called `setsid`) cannot be told from the rest of the machine, and is
/// left alone.
fn version_within(program: &Path, wait: std::time::Duration) -> Result<String, String> {
    use std::io::Read;
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;
    let mut child = Command::new(program)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        // Its own group, so what it starts can be stopped with it.
        .process_group(0)
        .spawn()
        .map_err(|err| format!("cannot be run ({err})"))?;
    let heard = child.stdout.take().map(|mut stdout| {
        // Read as it writes, so a long answer cannot fill the pipe and
        // stall it. What it read comes back over `answer`; one setup has
        // stopped waiting for is left to the thread (its send then fails,
        // unheard).
        let (answer, heard) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("kzoo-version".to_owned())
            .spawn(move || {
                let mut said = Vec::new();
                answer.send(stdout.read_to_end(&mut said).map(|_| said))
            })
            .map(|_| heard)
    });
    let ended = match heard {
        None => Err("cannot be read".to_owned()),
        Some(Err(err)) => Err(format!("cannot be read ({err})")),
        Some(Ok(heard)) => answered(&child, &heard, wait),
    };
    // Its group is stopped while the program is still unreaped, which
    // keeps the group's id from being anyone else's; only then is it reaped.
    let killed = rustix::process::kill_process_group(
        rustix::process::Pid::from_child(&child),
        rustix::process::Signal::KILL,
    );
    // The program itself too: it may have moved to another group. It is
    // unreaped, so its pid is no one else's.
    let direct = child.kill();
    let said = match (ended, reap(&mut child, killed, direct)) {
        (Err(why), Ok(_)) => return Err(why),
        (Err(why), Err(err)) => return Err(format!("{why}, and could not be stopped ({err})")),
        (Ok(_), Err(err)) => return Err(format!("could not be stopped after it answered ({err})")),
        (Ok(said), Ok(status)) if status.success() => said,
        (Ok(_), Ok(status)) => return Err(format!("says no version ({status})")),
    };
    String::from_utf8_lossy(&said)
        .split_whitespace()
        .last()
        .map(str::to_owned)
        .ok_or_else(|| "says no version".to_owned())
}

/// Reap `child` once its group has been `killed` and it has been sent
/// SIGKILL itself (`direct`), never waiting on one that may still be
/// running: only a direct kill that went through (SIGKILL cannot be
/// ignored), or a program seen to have exited, is waited for. macOS may
/// refuse to signal an exited, unreaped program, which the exit check
/// covers. Otherwise it is left, and that is said.
fn reap(
    child: &mut std::process::Child,
    killed: rustix::io::Result<()>,
    direct: io::Result<()>,
) -> Result<std::process::ExitStatus, String> {
    use rustix::process::{Pid, WaitId, WaitIdOptions, waitid};
    let exited = matches!(
        waitid(
            WaitId::Pid(Pid::from_child(child)),
            WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
        ),
        Ok(Some(_))
    );
    if direct.is_ok() || exited {
        return child.wait().map_err(|err| err.to_string());
    }
    match child.try_wait() {
        Ok(Some(status)) => Ok(status),
        Ok(None) => Err(match (direct, killed) {
            (Err(err), _) => err.to_string(),
            (Ok(()), Err(err)) => io::Error::from(err).to_string(),
            (Ok(()), Ok(())) => "it is still running".to_owned(),
        }),
        Err(err) => Err(err.to_string()),
    }
}

/// What `child` said to `--version` over `heard`, once it has exited
/// (left unreaped), within `wait`.
fn answered(
    child: &std::process::Child,
    heard: &std::sync::mpsc::Receiver<io::Result<Vec<u8>>>,
    wait: std::time::Duration,
) -> Result<Vec<u8>, String> {
    use rustix::process::{Pid, WaitId, WaitIdOptions, waitid};
    use std::sync::mpsc::RecvTimeoutError;
    let pid = Pid::from_child(child);
    let started = std::time::Instant::now();
    loop {
        match waitid(
            WaitId::Pid(pid),
            WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
        ) {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < wait => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Ok(None) => {
                return Err(format!(
                    "did not answer --version within {:.1} s",
                    wait.as_secs_f64()
                ));
            }
            Err(err) => return Err(format!("could not be waited for ({err})")),
        }
    }
    // Something it left running may still hold its output open: the rest
    // of the wait is all the answer gets, with a moment's grace for one
    // that finished right at the end.
    let left = wait
        .saturating_sub(started.elapsed())
        .max(std::time::Duration::from_millis(250));
    match heard.recv_timeout(left) {
        Ok(said) => said.map_err(|err| format!("its answer could not be read ({err})")),
        Err(RecvTimeoutError::Timeout) => Err(format!(
            "did not finish its answer to --version within {:.1} s",
            wait.as_secs_f64()
        )),
        Err(RecvTimeoutError::Disconnected) => {
            Err("its answer could not be read (the reader stopped)".to_owned())
        }
    }
}

/// Plant the tree, as `kzoo tree` does, and say the same as it would.
fn tree(name: &str, machine: &Machine, dry: bool) -> color_eyre::Result<String> {
    use crate::entry::TreeIs;
    let dir = &machine.tree;
    let found = if dry {
        crate::entry::assess_tree(name, &machine.state, dir, &machine.socket)?
    } else {
        crate::entry::plant(name, &machine.state, dir, &machine.socket)?
    };
    Ok(match found {
        TreeIs::There => format!("planted already, at {}", dir.display()),
        TreeIs::ToPlant => format!("would plant it at {}", dir.display()),
        TreeIs::Planted => format!("planted just now, at {}", dir.display()),
        TreeIs::ForTheWall => {
            "the wall has played, and grows the tree from what it has played when it next starts"
                .to_owned()
        }
    })
}

/// A kazoo entry in Claude Code or Codex.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Entry {
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    /// What about it a command, arguments and environment cannot say (an
    /// http server, a working folder, a timeout...), when anything: such
    /// an entry could not be put back as it was, so it is never replaced.
    pub odd: Option<String>,
}

impl Entry {
    fn wanted(program: &Path) -> Self {
        Self {
            command: program.to_string_lossy().into_owned(),
            args: vec!["mcp".to_owned()],
            env: BTreeMap::new(),
            odd: None,
        }
    }

    /// The entry from Claude Code's JSON for it.
    pub(crate) fn of_claude(server: &serde_json::Value) -> Self {
        let mut odd = Vec::new();
        if let Some(object) = server.as_object() {
            for key in object.keys() {
                if !matches!(key.as_str(), "type" | "command" | "args" | "env") {
                    odd.push(format!("its {key}"));
                }
            }
        } else {
            odd.push("a shape setup cannot read".to_owned());
        }
        match server.get("type").map(serde_json::Value::as_str) {
            None | Some(Some("stdio")) => {}
            Some(Some(kind)) => odd.push(format!("a type other than stdio ({kind})")),
            Some(None) => odd.push("a type that is not text".to_owned()),
        }
        Self::with(server, odd)
    }

    /// The entry from Codex's JSON for it (`codex mcp get --json`). Any
    /// setting beyond a command, its arguments and environment makes it odd,
    /// a key Codex has added since included; what Codex says of the entry
    /// (`name`, `disabled_reason`, `auth_status`) is not a setting.
    pub(crate) fn of_codex(json: &serde_json::Value) -> Self {
        let mut odd = Vec::new();
        let set = |value: &serde_json::Value| !value.is_null();
        let transport = json.get("transport").unwrap_or(&serde_json::Value::Null);
        if transport.get("type").and_then(serde_json::Value::as_str) != Some("stdio") {
            odd.push("a transport other than stdio".to_owned());
        }
        if let Some(object) = json.as_object() {
            for (key, value) in object {
                match key.as_str() {
                    "name" | "disabled_reason" | "auth_status" | "transport" => {}
                    "enabled" => {
                        if value.as_bool() != Some(true) {
                            odd.push("its enabled set to false".to_owned());
                        }
                    }
                    _ if set(value) => odd.push(format!("its {key}")),
                    _ => {}
                }
            }
        } else {
            odd.push("a shape setup cannot read".to_owned());
        }
        if let Some(object) = transport.as_object() {
            for (key, value) in object {
                match key.as_str() {
                    "type" | "command" | "args" | "env" => {}
                    "env_vars" if value.as_array().is_some_and(Vec::is_empty) => {}
                    _ if set(value) => odd.push(format!("its {key}")),
                    _ => {}
                }
            }
        }
        Self::with(transport, odd)
    }

    fn with(server: &serde_json::Value, mut odd: Vec<String>) -> Self {
        fn text(odd: &mut Vec<String>, what: &str, value: &serde_json::Value) -> String {
            value.as_str().map_or_else(
                || {
                    odd.push(format!("{what} that is not text"));
                    String::new()
                },
                str::to_owned,
            )
        }
        let command = server
            .get("command")
            .map_or_else(String::new, |command| text(&mut odd, "a command", command));
        let args = match server.get("args") {
            None | Some(serde_json::Value::Null) => Vec::new(),
            Some(serde_json::Value::Array(args)) => args
                .iter()
                .map(|arg| text(&mut odd, "an argument", arg))
                .collect(),
            Some(_) => {
                odd.push("its args, which are not a list".to_owned());
                Vec::new()
            }
        };
        let env = match server.get("env") {
            None | Some(serde_json::Value::Null) => BTreeMap::new(),
            Some(serde_json::Value::Object(env)) => env
                .iter()
                .map(|(key, value)| (key.clone(), text(&mut odd, "an environment value", value)))
                .collect(),
            Some(_) => {
                odd.push("its env, which is not a table".to_owned());
                BTreeMap::new()
            }
        };
        if command.is_empty() {
            odd.push("no command".to_owned());
        }
        odd.dedup();
        Self {
            command,
            args,
            env,
            odd: (!odd.is_empty()).then(|| odd.join(", ")),
        }
    }

    /// How it reads: its environment, the command and its arguments.
    fn said(&self) -> String {
        self.env
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .chain([self.command.clone()])
            .chain(self.args.iter().cloned())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Claude Code's `mcp add` arguments for it, for you in every folder.
    fn claude_add(&self) -> Vec<OsString> {
        // The name comes first: `-e` takes every value up to the `--`.
        let mut args: Vec<OsString> = ["mcp", "add", "--scope", "user", "kazoo"]
            .iter()
            .map(OsString::from)
            .collect();
        for (key, value) in &self.env {
            args.push("-e".into());
            args.push(format!("{key}={value}").into());
        }
        args.extend(["--".into(), (&self.command).into()]);
        args.extend(self.args.iter().map(OsString::from));
        args
    }

    /// Codex's `mcp add` arguments for it.
    fn codex_add(&self) -> Vec<OsString> {
        let mut args: Vec<OsString> = ["mcp", "add", "kazoo"].iter().map(OsString::from).collect();
        for (key, value) in &self.env {
            args.push("--env".into());
            args.push(format!("{key}={value}").into());
        }
        args.extend(["--".into(), (&self.command).into()]);
        args.extend(self.args.iter().map(OsString::from));
        args
    }
}

/// Claude Code's kazoo entries: the one for you in every folder, and every
/// folder's own (which Claude Code uses there instead).
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ClaudeEntries {
    pub everywhere: Option<Entry>,
    pub folders: Vec<String>,
}

/// Claude Code's kazoo entries in `config`, and the `.mcp.json` files from
/// `folder` up that name one.
pub(crate) fn claude_entries(config: &Path, folder: &Path) -> color_eyre::Result<ClaudeEntries> {
    let mut entries = ClaudeEntries::default();
    let text = match fs::read_to_string(config) {
        Ok(text) => Some(text),
        Err(err) if err.kind() == io::ErrorKind::NotFound => None,
        Err(err) => return Err(eyre!("{} cannot be read ({err})", config.display())),
    };
    if let Some(text) = text {
        let json: serde_json::Value = serde_json::from_str(&text)
            .map_err(|err| eyre!("{} is not JSON ({err})", config.display()))?;
        entries.everywhere = json.pointer("/mcpServers/kazoo").map(Entry::of_claude);
        if let Some(projects) = json.get("projects").and_then(serde_json::Value::as_object) {
            entries.folders.extend(
                projects
                    .iter()
                    .filter(|(_, project)| project.pointer("/mcpServers/kazoo").is_some())
                    .map(|(path, _)| path.clone()),
            );
        }
    }
    for dir in folder.ancestors() {
        let file = dir.join(".mcp.json");
        let text = match fs::read_to_string(&file) {
            Ok(text) => text,
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                ) =>
            {
                continue;
            }
            Err(err) => return Err(eyre!("{} cannot be read ({err})", file.display())),
        };
        let json: serde_json::Value = serde_json::from_str(&text)
            .map_err(|err| eyre!("{} is not JSON ({err})", file.display()))?;
        if json.pointer("/mcpServers/kazoo").is_some() {
            entries.folders.push(file.display().to_string());
        }
    }
    entries.folders.sort();
    entries.folders.dedup();
    Ok(entries)
}

/// The one MCP in Claude Code, for you in every folder (its user scope), as
/// `kazoo`, running `program mcp`.
fn claude(machine: &Machine, program: &Path, dry: bool) -> color_eyre::Result<String> {
    let local = machine.claude_home().join("local").join("claude");
    let claude = machine
        .find("claude")
        .or_else(|| is_program(&local).then_some(local))
        .ok_or_else(|| {
            eyre!("Claude Code is not installed (https://claude.com/claude-code); run this again once it is")
        })?;
    let entries = claude_entries(&machine.claude_config(), &machine.folder)?;
    let wanted = Entry::wanted(program);
    let done = put_right(
        &claude,
        entries.everywhere.as_ref(),
        &wanted,
        dry,
        &["mcp", "remove", "kazoo", "--scope", "user"],
        Entry::claude_add,
    )?;
    if entries.folders.is_empty() {
        return Ok(done);
    }
    Ok(format!(
        "{done}; these folders have kazoo entries of their own, which Claude Code uses in them instead (left as they are): {}",
        entries.folders.join(", ")
    ))
}

/// The same MCP in Codex, when Codex is here.
fn codex(machine: &Machine, program: &Path, dry: bool) -> color_eyre::Result<String> {
    let Some(codex) = machine.find("codex") else {
        return Ok("not installed here, so nothing to do".to_owned());
    };
    let got = Command::new(&codex)
        .args(["mcp", "get", "kazoo", "--json"])
        .output()
        .wrap_err("codex mcp get cannot be run")?;
    let said = String::from_utf8_lossy(&got.stderr);
    let found = if got.status.success() {
        let json: serde_json::Value = serde_json::from_slice(&got.stdout)
            .map_err(|err| eyre!("codex mcp get said something that is not JSON ({err})"))?;
        Some(Entry::of_codex(&json))
    } else if got.status.code() == Some(1) && said.contains("No MCP server named") {
        None
    } else {
        return Err(eyre!(
            "codex mcp get failed ({}): {}",
            got.status,
            one_line(&said)
        ));
    };
    put_right(
        &codex,
        found.as_ref(),
        &Entry::wanted(program),
        dry,
        &["mcp", "remove", "kazoo"],
        Entry::codex_add,
    )
}

/// Put `tool`'s kazoo entry right: left when it is `wanted` already, added
/// when there is none, and replaced (`remove`, then add) when it is
/// something else, said with what it was. When the add fails after the
/// remove, the old entry is put back.
fn put_right(
    tool: &Path,
    found: Option<&Entry>,
    wanted: &Entry,
    dry: bool,
    remove: &[&str],
    add: fn(&Entry) -> Vec<OsString>,
) -> color_eyre::Result<String> {
    match found {
        Some(entry) if entry == wanted => Ok("kazoo is in it already".to_owned()),
        Some(Entry { odd: Some(odd), .. }) => Err(left_for_you(format!(
            "its kazoo entry has {odd}, which setup could not put back as it was, so it is left as it is: remove it yourself to have setup add the one MCP"
        ))),
        None if dry => Ok(format!("would add kazoo ({})", wanted.said())),
        Some(old) if dry => Ok(format!(
            "would put kazoo's entry right (it is {}; it would be {})",
            old.said(),
            wanted.said()
        )),
        None => {
            run_tool(tool, add(wanted))?;
            Ok(format!(
                "kazoo is added ({}): for you, in every folder, joining a wall by the name each seat picks",
                wanted.said()
            ))
        }
        Some(old) => {
            run_tool(tool, remove)?;
            if let Err(why) = run_tool(tool, add(wanted)) {
                return Err(match run_tool(tool, add(old)) {
                    Ok(()) => eyre!(
                        "the new entry cannot be added ({why:#}), so the old one ({}) is put back",
                        old.said()
                    ),
                    Err(back) => eyre!(
                        "the new entry cannot be added ({why:#}), and the old one ({}) cannot be put back ({back:#})",
                        old.said()
                    ),
                });
            }
            Ok(format!(
                "kazoo's entry is put right (it was {}; it is {})",
                old.said(),
                wanted.said()
            ))
        }
    }
}

/// Run `tool` with `args`, its output kept unless it fails.
fn run_tool<I, S>(tool: &Path, args: I) -> color_eyre::Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let name = tool.file_name().map_or_else(
        || tool.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    let out = Command::new(tool)
        .args(args)
        .output()
        .wrap_err_with(|| format!("{name} cannot be run"))?;
    if out.status.success() {
        return Ok(());
    }
    let said = one_line(&String::from_utf8_lossy(&out.stderr));
    Err(eyre!(
        "{name} failed ({}){}",
        out.status,
        if said.is_empty() {
            String::new()
        } else {
            format!(": {said}")
        }
    ))
}

/// `text` on one line.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The skill's files' hash: FNV-1a over every path and its text.
fn skill_hash() -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for (path, text) in SKILL {
        for byte in path.bytes().chain([0]).chain(text.bytes()).chain([0]) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    }
    format!("{hash:016x}")
}

/// A version's numbers, to tell newer from older.
fn version_numbers(version: &str) -> Option<Vec<u64>> {
    let numbers: Result<Vec<u64>, _> = version
        .split(['.', '-', '+'])
        .take(3)
        .map(str::parse)
        .collect();
    match numbers {
        Ok(numbers) if numbers.len() == 3 => Some(numbers),
        Ok(_) | Err(_) => None,
    }
}

/// The kazoo skill in Claude Code's `skills/kazoo`, as this version has it.
fn skill(name: &str, machine: &Machine, dry: bool) -> color_eyre::Result<String> {
    let claude = machine.claude_home();
    if dry {
        return skill_held(name, &claude, true);
    }
    // One setup at a time puts the skill in, so two never swap over each
    // other.
    fs::create_dir_all(&claude)
        .map_err(|err| eyre!("{} cannot be made ({err})", claude.display()))?;
    let path = claude.join(".kzoo-skill.lock");
    let lock = {
        use std::os::unix::fs::OpenOptionsExt;
        fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .mode(0o600)
            .open(&path)
            .map_err(|err| eyre!("{} cannot be opened ({err})", path.display()))?
    };
    rustix::fs::flock(&lock, rustix::fs::FlockOperation::LockExclusive)
        .map_err(|errno| eyre!("{} cannot be locked ({errno})", path.display()))?;
    let done = skill_held(name, &claude, false);
    drop(lock);
    done
}

/// The skill's files as they are in `dir`, hashed as [`skill_hash`] does;
/// `None` when any is missing or cannot be read.
fn hash_in(dir: &Path) -> Option<String> {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for (path, _) in SKILL {
        let text = match fs::read(dir.join(path)) {
            Ok(text) => text,
            Err(_missing) => return None,
        };
        for byte in path.bytes().chain([0]).chain(text).chain([0]) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    }
    Some(format!("{hash:016x}"))
}

fn skill_held(name: &str, claude: &Path, dry: bool) -> color_eyre::Result<String> {
    let skills = claude.join("skills");
    let dir = skills.join(SKILL_NAME);
    let mark = dir.join(SKILL_MARK);
    let written = format!("{VERSION} {}", skill_hash());
    let mut mended = false;
    let ours = match fs::read_to_string(&mark) {
        Ok(there) if there.trim() == written => {
            if hash_in(&dir).as_deref() == Some(skill_hash().as_str()) {
                return Ok(format!("this version's is in {} already", dir.display()));
            }
            mended = true;
            true
        }
        Ok(there) => {
            let theirs = there.split_whitespace().next().unwrap_or_default();
            if let (Some(theirs_n), Some(ours_n)) =
                (version_numbers(theirs), version_numbers(VERSION))
                && theirs_n > ours_n
            {
                return Err(left_for_you(format!(
                    "kazoo {theirs} put a newer skill in {}; this is {VERSION}, so it is left as it is: set up with kazoo {theirs} or later instead",
                    dir.display()
                )));
            }
            true
        }
        Err(err)
            if matches!(
                err.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            false
        }
        Err(err) => return Err(eyre!("{} cannot be read ({err})", mark.display())),
    };
    if !ours && dir.symlink_metadata().is_ok() {
        return Err(left_for_you(format!(
            "{} is there and is not kazoo's, so it is left as it is: move it away to have setup put the kazoo skill there",
            dir.display()
        )));
    }
    if dry {
        return Ok(if mended {
            format!(
                "would put it back whole in {}: some of its files are changed or gone",
                dir.display()
            )
        } else {
            format!("would put it in {}", dir.display())
        });
    }
    let staging = claude.join(STAGING);
    // What a run that was stopped left.
    remove(name, &staging);
    fs::create_dir_all(&skills)
        .map_err(|err| eyre!("{} cannot be made ({err})", skills.display()))?;
    // Written whole beside it, then swapped in: a skill is never half there.
    let new = staging.join("new");
    let old = staging.join("old");
    write_skill(&new, &written).inspect_err(|_| remove(name, &staging))?;
    if ours {
        fs::rename(&dir, &old)
            .map_err(|err| eyre!("the skill there cannot be moved aside ({err})"))
            .inspect_err(|_| remove(name, &staging))?;
    }
    if let Err(err) = fs::rename(&new, &dir) {
        if ours && let Err(back) = fs::rename(&old, &dir) {
            return Err(eyre!(
                "the new skill cannot go in ({err}), and the one that was there cannot be put back from {} ({back})",
                old.display()
            ));
        }
        remove(name, &staging);
        return Err(eyre!("the new skill cannot go in ({err})"));
    }
    remove(name, &staging);
    Ok(if mended {
        format!(
            "put back whole in {}: some of its files were changed or gone",
            dir.display()
        )
    } else {
        format!("in {}: Claude reads it when kazoo comes up", dir.display())
    })
}

fn write_skill(dir: &Path, written: &str) -> color_eyre::Result<()> {
    for (path, text) in SKILL {
        let file = dir.join(path);
        if let Some(parent) = file.parent() {
            fs::create_dir_all(parent)
                .map_err(|err| eyre!("{} cannot be made ({err})", parent.display()))?;
        }
        fs::write(&file, text)
            .map_err(|err| eyre!("{} cannot be written ({err})", file.display()))?;
    }
    let mark = dir.join(SKILL_MARK);
    fs::write(&mark, format!("{written}\n"))
        .map_err(|err| eyre!("{} cannot be written ({err})", mark.display()))
}

/// Take away a folder this run made, saying so if it cannot.
fn remove(name: &str, dir: &Path) {
    let gone = match fs::remove_dir_all(dir) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        gone => gone,
    };
    if let Err(err) = gone {
        eprintln!("{name} setup: {} is left behind ({err})", dir.display());
    }
}

#[cfg(test)]
mod tests;
