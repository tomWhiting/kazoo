use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use super::{
    ClaudeEntries, Entry, LeftForYou, Machine, SKILL, SKILL_MARK, STAGING, VERSION, claude,
    claude_entries, codex, how_to_remove, is_program, packing_list, program, reap, run_on, skill,
    skill_hash, throwaway, tree, version_within,
};
use crate::cli::SetupArgs;

/// A machine with `home` as its home and only `bin` on its path.
fn machine(home: &Path, bin: &Path, agent: &str) -> Machine {
    Machine {
        path: vec![bin.to_path_buf()],
        home: home.to_path_buf(),
        claude_dir: None,
        agent: agent.to_owned(),
        folder: home.to_path_buf(),
        state: home.join(".kazoo").join("wall"),
        tree: home.join(".kazoo").join("tree"),
        socket: home.join("no-wall.sock"),
    }
}

/// A stand-in program: a shell script.
fn fake(dir: &Path, name: &str, body: &str) -> PathBuf {
    fs::create_dir_all(dir).unwrap();
    let path = dir.join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// A stand-in that writes each call's arguments as a line of `log`, and
/// fails when its arguments hold `fail`.
fn logging(dir: &Path, name: &str, log: &Path, fail: &str) -> PathBuf {
    fake(
        dir,
        name,
        &format!(
            "echo \"$*\" >> '{}'\ncase \"$*\" in *'{fail}'*) echo 'it went wrong' >&2; exit 1;; esac",
            log.display()
        ),
    )
}

fn calls(log: &Path) -> Vec<String> {
    match fs::read_to_string(log) {
        Ok(text) => text.lines().map(str::to_owned).collect(),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(err) => panic!("{err}"),
    }
}

fn skill_dir(home: &Path) -> PathBuf {
    home.join(".claude").join("skills").join("kazoo")
}

fn mark() -> String {
    format!("{VERSION} {}\n", skill_hash())
}

#[test]
fn the_skill_goes_in_whole_and_marked_as_kazoos() {
    let home = tempfile::tempdir().unwrap();
    let on = machine(home.path(), home.path(), "");
    let said = skill("kzoo", &on, false).unwrap();
    assert!(said.starts_with("in "), "{said}");
    let dir = skill_dir(home.path());
    for (path, text) in SKILL {
        assert_eq!(fs::read_to_string(dir.join(path)).unwrap(), *text, "{path}");
    }
    assert_eq!(fs::read_to_string(dir.join(SKILL_MARK)).unwrap(), mark());
    // Nothing is left beside it, nor in the staging folder.
    let beside: Vec<_> = fs::read_dir(dir.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(beside, ["kazoo"]);
    assert!(!home.path().join(".claude").join(STAGING).exists());
    // Again, it is there already.
    let again = skill("kzoo", &on, false).unwrap();
    assert!(again.contains("already"), "{again}");
}

#[test]
fn a_skill_of_kazoos_that_differs_is_replaced_whole() {
    let same_version = format!("{VERSION} 0000000000000000");
    for written in ["0.0.1 0000000000000000", same_version.as_str()] {
        let home = tempfile::tempdir().unwrap();
        let dir = skill_dir(home.path());
        fs::create_dir_all(dir.join("references")).unwrap();
        fs::write(dir.join(SKILL_MARK), format!("{written}\n")).unwrap();
        fs::write(dir.join("references").join("gone.md"), "old").unwrap();
        skill("kzoo", &machine(home.path(), home.path(), ""), false).unwrap();
        assert!(
            !dir.join("references").join("gone.md").exists(),
            "{written}"
        );
        assert_eq!(fs::read_to_string(dir.join(SKILL_MARK)).unwrap(), mark());
    }
}

#[test]
fn a_newer_skill_of_kazoos_is_left_alone() {
    let home = tempfile::tempdir().unwrap();
    let dir = skill_dir(home.path());
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(SKILL_MARK), "999.0.0 0000000000000000\n").unwrap();
    let why = skill("kzoo", &machine(home.path(), home.path(), ""), false)
        .unwrap_err()
        .to_string();
    assert!(why.contains("newer"), "{why}");
    assert_eq!(
        fs::read_to_string(dir.join(SKILL_MARK)).unwrap(),
        "999.0.0 0000000000000000\n"
    );
}

#[test]
fn a_skill_place_that_is_not_kazoos_is_left_alone() {
    let home = tempfile::tempdir().unwrap();
    let dir = skill_dir(home.path());
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("SKILL.md"), "someone else's").unwrap();
    let on = machine(home.path(), home.path(), "");
    let why = skill("kzoo", &on, false).unwrap_err().to_string();
    assert!(why.contains("not kazoo's"), "{why}");
    assert_eq!(
        fs::read_to_string(dir.join("SKILL.md")).unwrap(),
        "someone else's"
    );
    // A file where the folder would be is not kazoo's either.
    let other = tempfile::tempdir().unwrap();
    fs::create_dir_all(skill_dir(other.path()).parent().unwrap()).unwrap();
    fs::write(skill_dir(other.path()), "a file").unwrap();
    let why = skill("kzoo", &machine(other.path(), other.path(), ""), false)
        .unwrap_err()
        .to_string();
    assert!(why.contains("not kazoo's"), "{why}");
}

#[test]
fn what_a_stopped_run_left_is_cleared_and_claude_config_dir_is_followed() {
    let home = tempfile::tempdir().unwrap();
    let config = home.path().join("elsewhere");
    let left = config.join(STAGING).join("new");
    fs::create_dir_all(&left).unwrap();
    fs::write(left.join("SKILL.md"), "half").unwrap();
    let mut on = machine(home.path(), home.path(), "");
    on.claude_dir = Some(config.clone());
    skill("kzoo", &on, false).unwrap();
    assert!(
        config
            .join("skills")
            .join("kazoo")
            .join("SKILL.md")
            .is_file()
    );
    assert!(!config.join(STAGING).exists());
    assert!(!home.path().join(".claude").exists());
}

#[test]
fn a_dry_run_writes_nothing() {
    let home = tempfile::tempdir().unwrap();
    let said = skill("kzoo", &machine(home.path(), home.path(), ""), true).unwrap();
    assert!(said.starts_with("would"), "{said}");
    assert!(!home.path().join(".claude").exists());
}

#[test]
fn claude_codes_entries_are_read_for_everywhere_and_for_every_folder() {
    let home = tempfile::tempdir().unwrap();
    let config = home.path().join(".claude.json");
    let folder = home.path().join("a").join("b");
    fs::create_dir_all(&folder).unwrap();
    assert_eq!(
        claude_entries(&config, &folder).unwrap(),
        ClaudeEntries::default()
    );
    fs::write(
        &config,
        r#"{
            "mcpServers": {"kazoo": {"type": "stdio", "command": "/bin/kzoo", "args": ["mcp"], "env": {"A": "1"}}},
            "projects": {
                "/a/project": {"mcpServers": {"kazoo": {"command": "kazoo", "args": ["mcp", "--seat", "Cassio"]}}},
                "/another": {"mcpServers": {}}
            }
        }"#,
    )
    .unwrap();
    let project = home.path().join("a").join(".mcp.json");
    fs::write(
        &project,
        r#"{"mcpServers": {"kazoo": {"command": "kzoo"}}}"#,
    )
    .unwrap();
    let entries = claude_entries(&config, &folder).unwrap();
    assert_eq!(
        entries.everywhere,
        Some(Entry {
            command: "/bin/kzoo".to_owned(),
            args: vec!["mcp".to_owned()],
            env: [("A".to_owned(), "1".to_owned())].into(),
            odd: None,
        })
    );
    let mut folders = vec![project.display().to_string(), "/a/project".to_owned()];
    folders.sort();
    assert_eq!(entries.folders, folders);
    fs::write(&config, "not json").unwrap();
    let why = claude_entries(&config, &folder).unwrap_err().to_string();
    assert!(why.contains("not JSON"), "{why}");
}

#[test]
fn a_package_runners_copies_are_passed_over() {
    for dir in [
        "/Users/x/.npm/_npx/0a1b/node_modules/.bin",
        "/Users/x/project/node_modules/.bin",
        "/private/tmp/bunx-501-kzhz@latest/node_modules/.bin",
    ] {
        assert!(throwaway(Path::new(dir)), "{dir}");
    }
    assert!(!throwaway(Path::new("/opt/homebrew/bin")));
    let home = tempfile::tempdir().unwrap();
    let npx = home.path().join("_npx").join("node_modules").join(".bin");
    let kept = home.path().join("bin");
    fake(&npx, "kzoo", "echo kzoo 0");
    let stays = fake(&kept, "kzoo", "echo kzoo 0");
    let mut on = machine(home.path(), &npx, "npm/10");
    on.path.push(kept);
    assert_eq!(on.find("kzoo"), Some(stays));
}

#[test]
fn only_this_version_of_the_program_is_taken() {
    let home = tempfile::tempdir().unwrap();
    let bin = home.path().join("bin");
    fake(&bin, "kazoo", "echo kazoo-tui 0.1.0");
    let at = fake(&bin, "kzhz", &format!("echo kzhz {VERSION}"));
    let on = machine(home.path(), &bin, "");
    let found = program("kzoo", &on, false).unwrap();
    assert_eq!(found.path, at);
    // The old one beside it, under another name, is said.
    let old = bin.join("kazoo");
    assert_eq!(found.notes.len(), 1, "{:?}", found.notes);
    assert!(
        found.notes[0].starts_with(&format!(
            "note: {} (version 0.1.0) is another kazoo",
            old.display()
        )),
        "{:?}",
        found.notes
    );
    // With none at this version and no runner to install it, it says what
    // it found.
    fs::remove_file(&at).unwrap();
    let why = program("kzoo", &on, false).unwrap_err().to_string();
    assert!(
        why.contains("version 0.1.0") && why.contains(VERSION),
        "{why}"
    );
}

#[test]
fn run_by_npx_it_installs_the_program_to_stay() {
    let home = tempfile::tempdir().unwrap();
    let bin = home.path().join("bin");
    let prefix = home.path().join("global");
    let log = home.path().join("npm.log");
    fake(
        &bin,
        "npm",
        &format!(
            "echo \"$*\" >> '{log}'\n\
             if [ \"$1\" = prefix ]; then echo '{prefix}'; exit 0; fi\n\
             mkdir -p '{prefix}/bin'\n\
             printf '#!/bin/sh\\necho kzhz {VERSION}\\n' > '{prefix}/bin/kzhz'\n\
             chmod +x '{prefix}/bin/kzhz'",
            log = log.display(),
            prefix = prefix.display()
        ),
    );
    let on = machine(home.path(), &bin, "npm/10.9.0 node/v22");
    // A dry run says where it would go, and installs nothing.
    let dry = program("kzoo", &on, true).unwrap();
    assert_eq!(dry.path, prefix.join("bin").join("kzhz"));
    assert!(!prefix.join("bin").exists());
    let installed = program("kzoo", &on, false).unwrap();
    assert_eq!(installed.path, prefix.join("bin").join("kzhz"));
    // The runner's folder is not on this machine's PATH, which is said.
    for (found, word) in [(&dry, "would be"), (&installed, "was")] {
        assert_eq!(
            found.notes,
            [format!(
                "note: {} is not on PATH, so by name this machine will not find what {word} installed: add it to PATH (Claude Code and Codex are given its full path, so they will)",
                prefix.join("bin").display()
            )]
        );
    }
    assert!(
        calls(&log).contains(&format!("install -g kzhz@{VERSION}")),
        "{:?}",
        calls(&log)
    );
}

#[test]
fn claude_code_gets_kazoo_added_left_or_put_right() {
    let home = tempfile::tempdir().unwrap();
    let bin = home.path().join("bin");
    let log = home.path().join("claude.log");
    let program = Path::new("/new/kzoo");
    logging(&bin, "claude", &log, "never");
    let on = machine(home.path(), &bin, "");
    let config = home.path().join(".claude.json");
    // None: it is added.
    let said = claude(&on, program, false).unwrap();
    assert!(said.starts_with("kazoo is added"), "{said}");
    assert_eq!(calls(&log), ["mcp add --scope user kazoo -- /new/kzoo mcp"]);
    // Right already: nothing is run.
    fs::write(
        &config,
        r#"{"mcpServers": {"kazoo": {"command": "/new/kzoo", "args": ["mcp"]}}}"#,
    )
    .unwrap();
    fs::remove_file(&log).unwrap();
    assert!(claude(&on, program, false).unwrap().contains("already"));
    assert!(calls(&log).is_empty());
    // Something else: replaced, saying what it was.
    fs::write(
        &config,
        r#"{"mcpServers": {"kazoo": {"command": "kazoo", "args": ["mcp", "--seat", "Tom"]}}}"#,
    )
    .unwrap();
    let said = claude(&on, program, false).unwrap();
    assert!(said.contains("it was kazoo mcp --seat Tom"), "{said}");
    assert_eq!(
        calls(&log),
        [
            "mcp remove kazoo --scope user",
            "mcp add --scope user kazoo -- /new/kzoo mcp"
        ]
    );
    // A dry run runs nothing.
    fs::remove_file(&log).unwrap();
    assert!(claude(&on, program, true).unwrap().starts_with("would"));
    assert!(calls(&log).is_empty());
}

#[test]
fn an_entry_that_cannot_be_replaced_is_put_back() {
    let home = tempfile::tempdir().unwrap();
    let bin = home.path().join("bin");
    let log = home.path().join("claude.log");
    logging(&bin, "claude", &log, "/new/kzoo");
    fs::write(
        home.path().join(".claude.json"),
        r#"{"mcpServers": {"kazoo": {"command": "kazoo", "args": ["mcp"], "env": {"K": "v"}}}}"#,
    )
    .unwrap();
    let on = machine(home.path(), &bin, "");
    let why = claude(&on, Path::new("/new/kzoo"), false)
        .unwrap_err()
        .to_string();
    assert!(
        why.contains("is put back") && why.contains("it went wrong"),
        "{why}"
    );
    assert_eq!(
        calls(&log),
        [
            "mcp remove kazoo --scope user",
            "mcp add --scope user kazoo -- /new/kzoo mcp",
            "mcp add --scope user kazoo -e K=v -- kazoo mcp"
        ]
    );
}

#[test]
fn claude_code_not_on_the_path_is_looked_for_where_it_installs_itself() {
    let home = tempfile::tempdir().unwrap();
    let log = home.path().join("claude.log");
    logging(
        &home.path().join(".claude").join("local"),
        "claude",
        &log,
        "never",
    );
    let on = machine(home.path(), &home.path().join("bin"), "");
    claude(&on, Path::new("/new/kzoo"), false).unwrap();
    assert_eq!(calls(&log).len(), 1);
    // And with none, it says so.
    let bare = tempfile::tempdir().unwrap();
    let why = claude(
        &machine(bare.path(), bare.path(), ""),
        Path::new("/k"),
        false,
    )
    .unwrap_err()
    .to_string();
    assert!(why.contains("not installed"), "{why}");
}

#[test]
fn codex_is_told_apart_from_a_codex_that_fails() {
    let home = tempfile::tempdir().unwrap();
    let bin = home.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    let on = machine(home.path(), &bin, "");
    let program = Path::new("/new/kzoo");
    assert!(
        codex(&on, program, false)
            .unwrap()
            .contains("not installed")
    );
    let log = home.path().join("codex.log");
    // Absent: added.
    fake(
        &bin,
        "codex",
        &format!(
            "echo \"$*\" >> '{}'\n\
             if [ \"$2\" = get ]; then echo \"Error: No MCP server named 'kazoo' found.\" >&2; exit 1; fi",
            log.display()
        ),
    );
    assert!(
        codex(&on, program, false)
            .unwrap()
            .starts_with("kazoo is added")
    );
    assert_eq!(
        calls(&log),
        ["mcp get kazoo --json", "mcp add kazoo -- /new/kzoo mcp"]
    );
    // Right already.
    fake(
        &bin,
        "codex",
        r#"echo '{"transport": {"type": "stdio", "command": "/new/kzoo", "args": ["mcp"]}}'"#,
    );
    assert!(codex(&on, program, false).unwrap().contains("already"));
    // Any other failure is said, and nothing is added.
    fake(&bin, "codex", "echo 'config.toml is broken' >&2; exit 2");
    let why = codex(&on, program, false).unwrap_err().to_string();
    assert!(why.contains("config.toml is broken"), "{why}");
}

#[test]
fn only_an_executable_file_is_a_program() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("kzoo");
    fs::write(&file, "#!/bin/sh\n").unwrap();
    assert!(!is_program(&file));
    fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(is_program(&file));
    assert!(!is_program(dir.path()));
    assert!(!is_program(&dir.path().join("missing")));
}

#[test]
fn the_skill_names_itself() {
    let (_, main) = SKILL[0];
    assert!(main.starts_with("---\nname: kazoo\n"), "{main}");
    assert!(SKILL.iter().all(|(_, text)| !text.trim().is_empty()));
}

#[test]
fn the_packing_list_says_how_setup_went_and_remembers_the_chips() {
    let done = packing_list("kzhz", false, 0, 0);
    assert!(done.contains("  [x] kazoo, set up"), "{done}");
    assert!(done.contains("half a bag of chips from last time"));
    assert!(done.contains("[ ] kzhz wall") && done.ends_with("Start with: kzhz wall\n"));
    assert!(!done.contains("{name}"));
    let missed = packing_list("kzoo", false, 2, 0);
    assert!(missed.contains("[ ] kazoo, set up"), "{missed}");
    assert!(missed.contains("2 to finish: kzoo setup again"), "{missed}");
    let left = packing_list("kzoo", false, 0, 1);
    assert!(
        left.contains("1 left for you: do what it says above, then kzoo setup again"),
        "{left}"
    );
    let both = packing_list("kzoo", false, 1, 2);
    assert!(
        both.contains("3 to finish: do what each says above, then kzoo setup again"),
        "{both}"
    );
    let dry = packing_list("kzoo", true, 0, 0);
    assert!(dry.contains("kzoo setup, without --dry-run"), "{dry}");
    // Every item's words start in one column, whatever the name.
    for list in [
        done,
        missed,
        left,
        both,
        dry,
        packing_list("kazoo", false, 0, 0),
    ] {
        for line in list.lines().filter(|line| line.starts_with("  [")) {
            let chars: Vec<char> = line.chars().collect();
            assert_eq!(chars.get(41), Some(&' '), "{line}");
            assert!(chars.get(42).is_some_and(|c| *c != ' '), "{line}");
        }
    }
}

#[test]
fn a_whole_run_sets_up_everything_and_counts_what_it_missed() {
    let home = tempfile::tempdir().unwrap();
    let bin = home.path().join("bin");
    let kzoo = fake(&bin, "kzoo", &format!("echo kzoo {VERSION}"));
    let claude_log = home.path().join("claude.log");
    let codex_log = home.path().join("codex.log");
    logging(&bin, "claude", &claude_log, "never");
    fake(
        &bin,
        "codex",
        &format!(
            "echo \"$*\" >> '{}'\n\
             if [ \"$2\" = get ]; then echo \"No MCP server named 'kazoo'\" >&2; exit 1; fi",
            codex_log.display()
        ),
    );
    let on = machine(home.path(), &bin, "");
    run_on("kzoo", &SetupArgs { dry_run: false }, &on).unwrap();
    assert!(kazoo_wall::tree::git::is_bare(&on.tree).unwrap());
    assert!(skill_dir(home.path()).join("SKILL.md").is_file());
    let add = format!("mcp add --scope user kazoo -- {} mcp", kzoo.display());
    assert_eq!(calls(&claude_log), [add]);
    assert_eq!(calls(&codex_log).len(), 2);
    // A step that fails is counted, and the rest still go.
    logging(&bin, "claude", &claude_log, "add");
    fs::remove_dir_all(skill_dir(home.path())).unwrap();
    let why = run_on(
        "kzoo",
        &SetupArgs { dry_run: false },
        &machine(home.path(), &bin, ""),
    )
    .unwrap_err()
    .to_string();
    assert!(why.starts_with("1 of the steps"), "{why}");
    assert!(skill_dir(home.path()).join("SKILL.md").is_file());
}

#[test]
fn a_wall_that_has_played_grows_its_own_tree() {
    let home = tempfile::tempdir().unwrap();
    let on = machine(home.path(), home.path(), "");
    fs::create_dir_all(&on.state).unwrap();
    fs::write(on.state.join(kazoo_wall::store::LOG_FILE), "{\"seq\":1}\n").unwrap();
    for dry in [true, false] {
        let said = tree("kzoo", &on, dry).unwrap();
        assert!(said.contains("grows the tree"), "{said}");
    }
    assert!(!on.tree.exists());
    // With nothing played, a dry run would plant it, and a run does.
    let fresh = tempfile::tempdir().unwrap();
    let on = machine(fresh.path(), fresh.path(), "");
    assert!(tree("kzoo", &on, true).unwrap().starts_with("would plant"));
    assert!(!on.tree.exists());
    assert!(
        tree("kzoo", &on, false)
            .unwrap()
            .starts_with("planted just now, at ")
    );
    assert!(kazoo_wall::tree::git::is_bare(&on.tree).unwrap());
    // Run again, it is there already, dry run or not.
    for dry in [true, false] {
        let said = tree("kzoo", &on, dry).unwrap();
        assert!(said.starts_with("planted already, at "), "{said}");
    }
}

#[test]
fn an_entry_that_cannot_be_put_back_says_so() {
    let home = tempfile::tempdir().unwrap();
    let bin = home.path().join("bin");
    let log = home.path().join("claude.log");
    // Every add fails: the new one, and the old one put back.
    logging(&bin, "claude", &log, "add");
    fs::write(
        home.path().join(".claude.json"),
        r#"{"mcpServers": {"kazoo": {"command": "kazoo", "args": ["mcp"]}}}"#,
    )
    .unwrap();
    let why = claude(
        &machine(home.path(), &bin, ""),
        Path::new("/new/kzoo"),
        false,
    )
    .unwrap_err()
    .to_string();
    assert!(why.contains("cannot be put back"), "{why}");
}

#[test]
fn an_entry_setup_could_not_put_back_as_it_was_is_never_replaced() {
    for (server, has) in [
        (
            r#"{"type": "http", "url": "https://example.com/mcp"}"#,
            "its url, a type other than stdio (http), no command",
        ),
        (
            r#"{"command": "kazoo", "args": ["mcp"], "cwd": "/somewhere"}"#,
            "its cwd",
        ),
        (
            r#"{"command": "kazoo", "env": {"N": 3}}"#,
            "an environment value that is not text",
        ),
        (r#""a string""#, "a shape setup cannot read, no command"),
        (
            r#"{"type": 3, "command": "kazoo"}"#,
            "a type that is not text",
        ),
    ] {
        let home = tempfile::tempdir().unwrap();
        let bin = home.path().join("bin");
        let log = home.path().join("claude.log");
        logging(&bin, "claude", &log, "never");
        fs::write(
            home.path().join(".claude.json"),
            format!(r#"{{"mcpServers": {{"kazoo": {server}}}}}"#),
        )
        .unwrap();
        let refused = claude(
            &machine(home.path(), &bin, ""),
            Path::new("/new/kzoo"),
            false,
        )
        .unwrap_err();
        // Setup run again would leave it again: it is the person's to do.
        assert!(refused.downcast_ref::<LeftForYou>().is_some(), "{server}");
        let why = refused.to_string();
        assert!(
            why.starts_with(&format!(
                "its kazoo entry has {has}, which setup could not put back as it was, so it is left as it is: "
            )),
            "{server}: {why}"
        );
        assert!(calls(&log).is_empty(), "{server}");
    }
}

#[test]
fn an_entry_that_differs_only_in_its_environment_says_so() {
    let entry = Entry::of_claude(&serde_json::json!({
        "command": "/new/kzoo", "args": ["mcp"], "env": {"KAZOO_SEAT": "Tom"}
    }));
    assert_eq!(entry.odd, None);
    assert_ne!(entry, Entry::wanted(Path::new("/new/kzoo")));
    assert_eq!(entry.said(), "KAZOO_SEAT=Tom /new/kzoo mcp");
}

#[test]
fn args_or_env_of_the_wrong_shape_make_an_entry_odd() {
    for (server, odd) in [
        (
            serde_json::json!({"command": "kzoo", "args": "mcp"}),
            "its args, which are not a list",
        ),
        (
            serde_json::json!({"command": "kzoo", "args": ["mcp"], "env": []}),
            "its env, which is not a table",
        ),
    ] {
        assert_eq!(
            Entry::of_claude(&server).odd.as_deref(),
            Some(odd),
            "{server}"
        );
        let codex = serde_json::json!({"name": "kazoo", "enabled": true, "transport": {
            "type": "stdio", "command": server["command"], "args": server["args"],
            "env": server.get("env").cloned().unwrap_or(serde_json::Value::Null)
        }});
        assert_eq!(Entry::of_codex(&codex).odd.as_deref(), Some(odd), "{codex}");
    }
    for server in [
        serde_json::json!({"command": "kzoo", "args": null, "env": null}),
        serde_json::json!({"command": "kzoo"}),
    ] {
        assert_eq!(Entry::of_claude(&server).odd, None, "{server}");
    }
}

/// An entry as `codex mcp get --json` gives it, with `top` and
/// `transport` merged in.
fn codex_json(top: &serde_json::Value, transport: &serde_json::Value) -> serde_json::Value {
    let mut json = serde_json::json!({
        "name": "kazoo",
        "enabled": true,
        "disabled_reason": null,
        "transport": {
            "type": "stdio",
            "command": "/old/kzoo",
            "args": ["mcp"],
            "env": null,
            "env_vars": [],
            "cwd": null
        },
        "startup_timeout_sec": null,
        "tool_timeout_sec": null,
        "auth_status": "unsupported"
    });
    for (key, value) in top.as_object().unwrap() {
        json[key] = value.clone();
    }
    for (key, value) in transport.as_object().unwrap() {
        json["transport"][key] = value.clone();
    }
    json
}

#[test]
fn a_codex_entry_with_more_than_a_command_is_odd() {
    let none = serde_json::json!({});
    assert_eq!(Entry::of_codex(&codex_json(&none, &none)).odd, None);
    for (top, transport, odd) in [
        (
            serde_json::json!({"enabled": false}),
            none.clone(),
            "its enabled set to false",
        ),
        (
            none.clone(),
            serde_json::json!({"cwd": "/somewhere"}),
            "its cwd",
        ),
        (
            none.clone(),
            serde_json::json!({"env_vars": ["HOME"]}),
            "its env_vars",
        ),
        (
            serde_json::json!({"startup_timeout_sec": 30.0}),
            none.clone(),
            "its startup_timeout_sec",
        ),
        (
            serde_json::json!({"tool_timeout_sec": 60.0}),
            none.clone(),
            "its tool_timeout_sec",
        ),
        (
            serde_json::json!({"enabled_tools": ["wall_look"]}),
            none.clone(),
            "its enabled_tools",
        ),
        (
            serde_json::json!({"disabled_tools": ["wall_load"]}),
            none.clone(),
            "its disabled_tools",
        ),
        (
            serde_json::json!({"a_setting_from_later": 1}),
            none.clone(),
            "its a_setting_from_later",
        ),
        (
            none.clone(),
            serde_json::json!({"a_setting_from_later": 1}),
            "its a_setting_from_later",
        ),
        (
            none,
            serde_json::json!({"type": "streamable_http", "url": "http://x"}),
            "a transport other than stdio",
        ),
    ] {
        let json = codex_json(&top, &transport);
        let said = Entry::of_codex(&json).odd.unwrap_or_default();
        assert!(said.contains(odd), "{json}: {said}");
    }
}

#[test]
fn codex_entries_are_replaced_and_put_back() {
    let home = tempfile::tempdir().unwrap();
    let bin = home.path().join("bin");
    let log = home.path().join("codex.log");
    let script = |fail: &str| {
        format!(
            "echo \"$*\" >> '{log}'\n\
             if [ \"$2\" = get ]; then echo '{{\"transport\": {{\"type\": \"stdio\", \"command\": \"kazoo\", \"args\": [\"mcp\"]}}}}'; exit 0; fi\n\
             case \"$*\" in *'{fail}'*) echo 'it went wrong' >&2; exit 1;; esac",
            log = log.display()
        )
    };
    fake(&bin, "codex", &script("never"));
    let on = machine(home.path(), &bin, "");
    let said = codex(&on, Path::new("/new/kzoo"), false).unwrap();
    assert!(said.contains("it was kazoo mcp"), "{said}");
    assert_eq!(
        calls(&log),
        [
            "mcp get kazoo --json",
            "mcp remove kazoo",
            "mcp add kazoo -- /new/kzoo mcp"
        ]
    );
    fs::remove_file(&log).unwrap();
    fake(&bin, "codex", &script("/new/kzoo"));
    let why = codex(&on, Path::new("/new/kzoo"), false)
        .unwrap_err()
        .to_string();
    assert!(why.contains("is put back"), "{why}");
    assert_eq!(calls(&log).last().unwrap(), "mcp add kazoo -- kazoo mcp");
}

#[test]
fn each_package_runner_installs_its_own_way() {
    for (agent, runner, bin_reply, expect) in [
        ("bun/1.2.0", "bun", "{bin}", "add -g kzhz@"),
        ("pnpm/9.0.0", "pnpm", "{bin}", "add -g kzhz@"),
    ] {
        let home = tempfile::tempdir().unwrap();
        let tools = home.path().join("tools");
        let global = home.path().join("global");
        let log = home.path().join("runner.log");
        fake(
            &tools,
            runner,
            &format!(
                "echo \"$*\" >> '{log}'\n\
                 case \"$*\" in *bin*) echo '{reply}'; exit 0;; esac\n\
                 mkdir -p '{global}'\n\
                 printf '#!/bin/sh\\necho kzhz {VERSION}\\n' > '{global}/kzhz'\n\
                 chmod +x '{global}/kzhz'",
                log = log.display(),
                reply = bin_reply.replace("{bin}", &global.display().to_string()),
                global = global.display()
            ),
        );
        let on = machine(home.path(), &tools, agent);
        let found = program("kzoo", &on, false).unwrap();
        assert_eq!(found.path, global.join("kzhz"), "{agent}");
        // The folder is named as the runner gives it, no stray separator.
        assert!(
            found.notes[0].starts_with(&format!("note: {} is not on PATH,", global.display())),
            "{agent}: {:?}",
            found.notes
        );
        assert!(
            calls(&log).iter().any(|call| call.starts_with(expect)),
            "{agent}: {:?}",
            calls(&log)
        );
    }
}

#[test]
fn an_installer_that_cannot_do_it_says_why() {
    let home = tempfile::tempdir().unwrap();
    let tools = home.path().join("tools");
    fs::create_dir_all(&tools).unwrap();
    // Setup does not install with yarn, and a runner not on the path
    // cannot run.
    let why = program("kzoo", &machine(home.path(), &tools, "yarn/4"), false)
        .unwrap_err()
        .to_string();
    assert!(
        why.starts_with("setup does not install with yarn: npm install -g kzhz@"),
        "{why}"
    );
    let why = program("kzoo", &machine(home.path(), &tools, "npm/10"), false)
        .unwrap_err()
        .to_string();
    assert!(why.contains("npm is not where"), "{why}");
    // A runner that cannot say where it puts programs.
    fake(&tools, "npm", "exit 3");
    let why = program("kzoo", &machine(home.path(), &tools, "npm/10"), false)
        .unwrap_err()
        .to_string();
    assert!(why.contains("prefix -g failed"), "{why}");
    // One that installs the wrong version.
    let global = home.path().join("global");
    fake(
        &tools,
        "npm",
        &format!(
            "if [ \"$1\" = prefix ]; then echo '{g}'; exit 0; fi\n\
             mkdir -p '{g}/bin'\n\
             printf '#!/bin/sh\\necho kzhz 0.0.1\\n' > '{g}/bin/kzhz'\n\
             chmod +x '{g}/bin/kzhz'",
            g = global.display()
        ),
    );
    let why = program("kzoo", &machine(home.path(), &tools, "npm/10"), false)
        .unwrap_err()
        .to_string();
    assert!(why.contains("says it is 0.0.1"), "{why}");
}

#[test]
fn setups_at_once_take_turns_with_the_skill() {
    let home = tempfile::tempdir().unwrap();
    let on = machine(home.path(), home.path(), "");
    let runs: Vec<_> = (0..4)
        .map(|_| {
            let on = on.clone();
            std::thread::spawn(move || skill("kzoo", &on, false))
        })
        .collect();
    for run in runs {
        run.join().unwrap().unwrap();
    }
    let dir = skill_dir(home.path());
    for (path, text) in SKILL {
        assert_eq!(fs::read_to_string(dir.join(path)).unwrap(), *text, "{path}");
    }
    assert!(!home.path().join(".claude").join(STAGING).exists());
}

#[test]
fn a_skill_whose_files_were_changed_or_lost_is_put_back_whole() {
    let home = tempfile::tempdir().unwrap();
    let on = machine(home.path(), home.path(), "");
    skill("kzoo", &on, false).unwrap();
    let dir = skill_dir(home.path());
    fs::remove_dir_all(dir.join("references")).unwrap();
    assert!(
        skill("kzoo", &on, true)
            .unwrap()
            .contains("would put it back whole")
    );
    let said = skill("kzoo", &on, false).unwrap();
    assert!(said.contains("put back whole"), "{said}");
    fs::write(dir.join("SKILL.md"), "edited").unwrap();
    assert!(
        skill("kzoo", &on, false)
            .unwrap()
            .contains("put back whole")
    );
    assert_eq!(
        fs::read_to_string(dir.join("SKILL.md")).unwrap(),
        SKILL[0].1
    );
}

#[test]
fn an_old_copy_is_removed_the_way_it_came() {
    let home = tempfile::tempdir().unwrap();
    // Installed by cargo, under another package's name.
    let cargo = home.path().join("cargo");
    let old = fake(&cargo.join("bin"), "kazoo", "echo kazoo-tui 0.1.0");
    fs::write(
        cargo.join(".crates2.json"),
        r#"{"installs": {"kazoo-tui 0.1.0 (path+file:///x)": {"bins": ["kazoo-tui", "kazoo"]}}}"#,
    )
    .unwrap();
    assert_eq!(how_to_remove(&old), "remove it (cargo uninstall kazoo-tui)");
    // A runner's global program: a link into its node_modules.
    let prefix = home.path().join("global");
    let real = fake(
        &prefix
            .join("lib")
            .join("node_modules")
            .join("kzhz")
            .join("bin"),
        "kzoo.js",
        "echo kzoo 0.1.0",
    );
    fs::create_dir_all(prefix.join("bin")).unwrap();
    let link = prefix.join("bin").join("kzhz");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert_eq!(how_to_remove(&link), "remove it (npm uninstall -g kzhz)");
    // A scoped package, as 0.3.0 went out.
    let real = fake(
        &prefix
            .join("lib")
            .join("node_modules")
            .join("@tomwhiting")
            .join("kzoo")
            .join("bin"),
        "kzoo.js",
        "echo kzoo 0.3.0",
    );
    let scoped = prefix.join("bin").join("kzoo");
    std::os::unix::fs::symlink(&real, &scoped).unwrap();
    assert_eq!(
        how_to_remove(&scoped),
        "remove it (npm uninstall -g @tomwhiting/kzoo)"
    );
    // bun's global programs link into its own node_modules.
    let bun = home.path().join(".bun");
    let real = fake(
        &bun.join("install")
            .join("global")
            .join("node_modules")
            .join("kzhz")
            .join("bin"),
        "kzoo.js",
        "echo kzoo 0.1.0",
    );
    fs::create_dir_all(bun.join("bin")).unwrap();
    let link = bun.join("bin").join("kzhz");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert_eq!(how_to_remove(&link), "remove it (bun remove -g kzhz)");
    // A link into a package inside another is not that package's to remove.
    let real = fake(
        &prefix
            .join("lib")
            .join("node_modules")
            .join("kzhz")
            .join("node_modules")
            .join("kzoo-darwin-arm64")
            .join("bin"),
        "kzoo",
        "echo kzoo 0.1.0",
    );
    let nested = home.path().join("nested");
    std::os::unix::fs::symlink(&real, &nested).unwrap();
    assert_eq!(
        how_to_remove(&nested),
        format!("delete {}", nested.display())
    );
    // A program too big to be a runner's script is not read as one.
    let big = home.path().join("big");
    let mut text = b"#!/bin/sh\n# node_modules/kzhz/bin/kzoo.js\n".to_vec();
    text.resize(70 * 1024, b'#');
    fs::write(&big, text).unwrap();
    assert_eq!(how_to_remove(&big), format!("delete {}", big.display()));
    // Anything else is deleted.
    let loose = fake(&home.path().join("loose"), "kzoo", "echo kzoo 0.1.0");
    assert_eq!(how_to_remove(&loose), format!("delete {}", loose.display()));
}

/// Whether the process `pid` (as a file holds it) has gone, waiting a
/// moment for it to be reaped.
fn gone(pid_file: &Path) -> bool {
    let pid = fs::read_to_string(pid_file).unwrap();
    (0..200).any(|_| {
        let alive = std::process::Command::new("kill")
            .args(["-0", pid.trim()])
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success();
        if alive {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        !alive
    })
}

#[test]
fn a_program_that_never_says_its_version_is_stopped() {
    let home = tempfile::tempdir().unwrap();
    // It starts something, then waits on it, never answering.
    let started_pid = home.path().join("started.pid");
    let slow = fake(
        home.path(),
        "kzoo",
        &format!("sleep 30 &\necho $! > '{}'\nwait", started_pid.display()),
    );
    let started = std::time::Instant::now();
    let why = version_within(&slow, std::time::Duration::from_secs(1)).unwrap_err();
    assert!(
        why.starts_with("did not answer --version within 1.0 s"),
        "{why}"
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    assert!(gone(&started_pid), "what it started is stopped with it");
    // One that answers but leaves something holding its output open.
    let left_pid = home.path().join("left.pid");
    let lingering = fake(
        home.path(),
        "kazoo",
        &format!(
            "echo kazoo 9.9.9\nsleep 30 &\necho $! > '{}'",
            left_pid.display()
        ),
    );
    let started = std::time::Instant::now();
    let why = version_within(&lingering, std::time::Duration::from_millis(1500)).unwrap_err();
    assert!(
        why.starts_with("did not finish its answer to --version within 1.5 s"),
        "{why}"
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    assert!(gone(&left_pid), "what it left running is stopped");
    // One that answers and leaves something quietly running.
    let quiet_pid = home.path().join("quiet.pid");
    let quiet = fake(
        home.path(),
        "kzoo-quiet",
        &format!(
            "sleep 30 >/dev/null &\necho $! > '{}'\necho kzoo 9.9.9",
            quiet_pid.display()
        ),
    );
    assert_eq!(
        version_within(&quiet, std::time::Duration::from_secs(5)),
        Ok("9.9.9".to_owned())
    );
    assert!(gone(&quiet_pid), "what it left running is stopped");
    let quick = fake(home.path(), "kzhz", "echo kzhz 9.9.9");
    assert_eq!(
        version_within(&quick, std::time::Duration::from_secs(5)),
        Ok("9.9.9".to_owned())
    );
}

/// pnpm's shim for a global program, as pnpm writes it: `NODE_PATH` lists
/// `node_paths`, and the last line runs the program in `global/5`, which
/// is put there.
fn pnpm_shim(pnpm: &Path, node_paths: &str) -> PathBuf {
    fake(
        &pnpm.join("global/5/node_modules/kzhz/bin"),
        "kzoo.js",
        "echo kzoo 0.1.0",
    );
    fake(
        pnpm,
        "kzhz",
        &format!(
            "basedir=$(dirname \"$(echo \"$0\" | sed -e 's,\\\\,/,g')\")\n\
             if [ -z \"$NODE_PATH\" ]; then\n\
               export NODE_PATH=\"{node_paths}\"\n\
             else\n\
               export NODE_PATH=\"{node_paths}:$NODE_PATH\"\n\
             fi\n\
             exec node  \"$basedir/global/5/node_modules/kzhz/bin/kzoo.js\" \"$@\""
        ),
    )
}

#[test]
fn a_pnpm_program_is_removed_with_pnpm() {
    let home = tempfile::tempdir().unwrap();
    let pnpm = home.path().join("Library").join("pnpm");
    let global = pnpm.join("global").join("5").join("node_modules");
    // As pnpm lays it out (isolated): NODE_PATH in its store.
    let store = format!(
        "{0}/.pnpm/kzhz@0.3.3/node_modules/kzhz/bin/node_modules:{0}/.pnpm/node_modules",
        global.display()
    );
    let shim = pnpm_shim(&pnpm, &store);
    assert_eq!(how_to_remove(&shim), "remove it (pnpm remove -g kzhz)");
    // And hoisted, with no store in NODE_PATH.
    let hoisted = format!("{0}/kzhz/node_modules:{0}/node_modules", global.display());
    let shim = pnpm_shim(&pnpm, &hoisted);
    assert_eq!(how_to_remove(&shim), "remove it (pnpm remove -g kzhz)");
    // A shim left behind when its program was deleted is deleted too.
    fs::remove_dir_all(pnpm.join("global/5/node_modules/kzhz")).unwrap();
    assert_eq!(how_to_remove(&shim), format!("delete {}", shim.display()));
    // A dependency in the store, not installed globally itself, is not
    // pnpm's to remove by that name.
    let real = fake(
        &global
            .join(".pnpm")
            .join("kzoo-darwin-arm64@0.3.3")
            .join("node_modules")
            .join("kzoo-darwin-arm64")
            .join("bin"),
        "kzoo",
        "echo kzoo 0.3.3",
    );
    let link = home.path().join("kzoo");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert_eq!(how_to_remove(&link), format!("delete {}", link.display()));
    // One that is, is.
    let real = fake(
        &global
            .join(".pnpm")
            .join("kzhz@0.3.3")
            .join("node_modules")
            .join("kzhz")
            .join("bin"),
        "kzoo.js",
        "echo kzoo 0.3.3",
    );
    std::os::unix::fs::symlink(
        global
            .join(".pnpm")
            .join("kzhz@0.3.3")
            .join("node_modules")
            .join("kzhz"),
        global.join("kzhz"),
    )
    .unwrap();
    let link = home.path().join("kzhz");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert_eq!(how_to_remove(&link), "remove it (pnpm remove -g kzhz)");
}

#[test]
fn npm_under_a_folder_named_like_pnpm_is_still_npm() {
    let home = tempfile::tempdir().unwrap();
    let prefix = home.path().join("pnpmfan").join("global");
    let real = fake(
        &prefix
            .join("lib")
            .join("node_modules")
            .join("kzhz")
            .join("bin"),
        "kzoo.js",
        "echo kzoo 0.1.0",
    );
    fs::create_dir_all(prefix.join("bin")).unwrap();
    let link = prefix.join("bin").join("kzhz");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert_eq!(how_to_remove(&link), "remove it (npm uninstall -g kzhz)");
}

#[test]
fn a_program_that_cannot_be_stopped_is_never_waited_on() {
    let perm = || Err(std::io::Error::from_raw_os_error(1));
    let mut child = std::process::Command::new("sleep")
        .arg("30")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let started = std::time::Instant::now();
    let why = reap(&mut child, Err(rustix::io::Errno::PERM), perm()).unwrap_err();
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    assert_eq!(why, "Operation not permitted (os error 1)");
    let why = reap(
        &mut child,
        Err(rustix::io::Errno::IO),
        Err(std::io::Error::from_raw_os_error(5)),
    )
    .unwrap_err();
    assert!(why.contains("os error 5"), "{why}");
    // A direct kill that went through is waited for.
    child.kill().unwrap();
    assert!(!reap(&mut child, Ok(()), Ok(())).unwrap().success());
    // One that has exited is reaped, whatever the kills said.
    let mut done = std::process::Command::new("true")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    // Seen to have exited, and left unreaped for `reap`.
    let exited = (0..400).any(|_| {
        let seen = matches!(
            rustix::process::waitid(
                rustix::process::WaitId::Pid(rustix::process::Pid::from_child(&done)),
                rustix::process::WaitIdOptions::EXITED
                    | rustix::process::WaitIdOptions::NOHANG
                    | rustix::process::WaitIdOptions::NOWAIT,
            ),
            Ok(Some(_))
        );
        if !seen {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        seen
    });
    assert!(exited);
    assert!(
        reap(&mut done, Err(rustix::io::Errno::PERM), perm())
            .unwrap()
            .success()
    );
}

#[test]
fn a_program_that_leaves_its_group_is_still_stopped() {
    let home = tempfile::tempdir().unwrap();
    let pid_file = home.path().join("moved.pid");
    // It joins setup's own group, out of the one it was started in.
    let moved = fake(
        home.path(),
        "kzoo",
        // The pid is written only once it has moved, so a pass means it did.
        &format!(
            "exec perl -e 'setpgrp(0, getpgrp(getppid())) or die \"setpgrp: $!\"; open(my $f, \">\", \"{}\") or die; print $f $$; close $f; sleep 30'",
            pid_file.display()
        ),
    );
    let started = std::time::Instant::now();
    let why = version_within(&moved, std::time::Duration::from_secs(2)).unwrap_err();
    assert!(
        why.starts_with("did not answer --version within 2.0 s"),
        "{why}"
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    assert!(gone(&pid_file));
}

#[test]
fn a_hand_written_wrapper_is_deleted_not_uninstalled() {
    let home = tempfile::tempdir().unwrap();
    let wrapper = fake(
        &home.path().join("bin"),
        "kzoo",
        "exec node /usr/local/lib/node_modules/kzhz/bin/kzoo.js \"$@\"",
    );
    assert_eq!(
        how_to_remove(&wrapper),
        format!("delete {}", wrapper.display())
    );
}

#[test]
fn runners_in_their_own_folders_are_still_told_apart() {
    let home = tempfile::tempdir().unwrap();
    // pnpm with PNPM_HOME somewhere not named for it.
    let pnpm_home = home.path().join(".tools");
    let shim = pnpm_shim(
        &pnpm_home,
        &format!(
            "{}/global/5/node_modules/.pnpm/node_modules",
            pnpm_home.display()
        ),
    );
    assert_eq!(how_to_remove(&shim), "remove it (pnpm remove -g kzhz)");
    // bun with BUN_INSTALL somewhere not named for it.
    let bun = home.path().join("opt").join("b");
    let real = fake(
        &bun.join("install")
            .join("global")
            .join("node_modules")
            .join("kzhz")
            .join("bin"),
        "kzoo.js",
        "echo kzoo 0.1.0",
    );
    fs::create_dir_all(bun.join("bin")).unwrap();
    let link = bun.join("bin").join("kzhz");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert_eq!(how_to_remove(&link), "remove it (bun remove -g kzhz)");
    // yarn 1's global add.
    let yarn = home.path().join(".config").join("yarn").join("global");
    let real = fake(
        &yarn.join("node_modules").join("kzhz").join("bin"),
        "kzoo.js",
        "echo kzoo 0.1.0",
    );
    let bin = home.path().join(".yarn").join("bin");
    fs::create_dir_all(&bin).unwrap();
    let link = bin.join("kzhz");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert_eq!(how_to_remove(&link), "remove it (yarn global remove kzhz)");
}

#[test]
fn one_package_owning_every_name_is_one_note_and_goes_first() {
    let home = tempfile::tempdir().unwrap();
    let cargo = home.path().join("cargo");
    let bin = cargo.join("bin");
    for name in ["kzhz", "kzoo", "kazoo"] {
        fake(&bin, name, &format!("echo {name} 0.3.3"));
    }
    fs::write(
        cargo.join(".crates2.json"),
        r#"{"installs": {"kzhz 0.3.3 (registry+https://github.com/rust-lang/crates.io-index)": {"bins": ["kazoo", "kzhz", "kzoo"]}}}"#,
    )
    .unwrap();
    // The kzoo command, owned by the kzhz package, is that package's.
    assert_eq!(
        how_to_remove(&bin.join("kzoo")),
        "remove it (cargo uninstall kzhz)"
    );
    // With none at this version and nothing to install with, one removal
    // comes before the install, naming every copy it takes.
    let on = machine(home.path(), &bin, "");
    let why = program("kzoo", &on, false).unwrap_err().to_string();
    assert_eq!(
        why,
        format!(
            "this is {VERSION}, and what is installed is not: {0}/kzoo (version 0.3.3), {0}/kzhz (version 0.3.3), {0}/kazoo (version 0.3.3): remove it (cargo uninstall kzhz); then npm install -g kzhz@{VERSION} (or cargo install kzoo), then kzoo setup",
            bin.display()
        )
    );
    // Beside a current copy elsewhere, the three old ones are one note.
    let current = home.path().join("current");
    fake(&current, "kzoo", &format!("echo kzoo {VERSION}"));
    let mut on = machine(home.path(), &current, "");
    on.path.push(bin.clone());
    let found = program("kzoo", &on, false).unwrap();
    assert_eq!(found.path, current.join("kzoo"));
    assert_eq!(found.notes.len(), 1, "{:?}", found.notes);
    assert!(
        found.notes[0].starts_with(&format!(
            "note: {}/kzhz (version 0.3.3), {}/kazoo (version 0.3.3) are another kazoo",
            bin.display(),
            bin.display()
        )),
        "{:?}",
        found.notes
    );
    assert!(
        found.notes[0].ends_with(
            "run by their names instead of this one: remove it (cargo uninstall kzhz) to be sure"
        ),
        "{:?}",
        found.notes
    );
}
