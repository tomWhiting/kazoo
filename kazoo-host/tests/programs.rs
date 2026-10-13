use std::fs;
use std::path::Path;

#[test]
fn programs_use_one_terminal_and_report_host() -> std::io::Result<()> {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut violations = Vec::new();
    for entry in fs::read_dir(workspace)? {
        let path = entry?.path();
        let name = path.file_name().unwrap().to_string_lossy();
        if name.starts_with("kazoo-") && name != "kazoo-host" && path.is_dir() {
            audit(&path.join("src"), &mut violations)?;
        }
    }
    assert!(violations.is_empty(), "{}", violations.join("\n"));
    Ok(())
}

fn audit(directory: &Path, violations: &mut Vec<String>) -> std::io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            audit(&path, violations)?;
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            if path.file_name().is_some_and(|name| name == "terminal.rs") {
                violations.push(format!("{}: private terminal module", path.display()));
            }
            let source = fs::read_to_string(&path)?;
            for (line, text) in source.lines().enumerate() {
                for setup in [
                    "enable_raw_mode",
                    "disable_raw_mode",
                    "EnterAlternateScreen",
                    "LeaveAlternateScreen",
                    "PushKeyboardEnhancementFlags",
                    "PopKeyboardEnhancementFlags",
                    "EnableMouseCapture",
                    "DisableMouseCapture",
                    "color_eyre::install",
                    "ratatui::init(",
                    "ratatui::try_init(",
                    "ratatui::restore(",
                    "ratatui::try_restore(",
                ] {
                    if text.contains(setup) {
                        violations.push(format!("{}:{}: {setup}", path.display(), line + 1));
                    }
                }
            }
        }
    }
    Ok(())
}
