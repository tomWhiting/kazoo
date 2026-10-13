mod studio {
    mod tests {
        use std::fs;
        use std::path::PathBuf;
        use std::process::{Command, Output};

        fn launch(label: &str, contents: &str) -> Output {
            let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../target/kz-logs")
                .join(format!("studio-{label}-{}", std::process::id()));
            fs::create_dir_all(directory.join(".kazoo/studio")).unwrap();
            fs::create_dir_all(directory.join("empty-path")).unwrap();
            fs::write(directory.join(".kazoo/studio/demo.toml"), contents).unwrap();
            let output = Command::new(env!("CARGO_BIN_EXE_kazoo"))
                .args(["studio", "up", "demo"])
                .env("HOME", &directory)
                .env("PATH", directory.join("empty-path"))
                .output()
                .unwrap();
            fs::remove_dir_all(directory).unwrap();
            output
        }

        #[test]
        fn file_with_two_instruments_parses() {
            let output = launch(
                "parse",
                "[desk]\nrate = 48000\n[[instrument]]\nname = '808'\n[[instrument]]\nname = 'mini'\n",
            );
            assert_eq!(output.status.code(), Some(2));
            let stdout = String::from_utf8(output.stdout).unwrap();
            assert!(stdout.contains("'808'"), "missing 808 command: {stdout}");
            assert!(stdout.contains("'mini'"), "missing mini command: {stdout}");
        }

        #[test]
        fn unknown_key_is_an_error_naming_it() {
            let output = launch("unknown", "[desk]\nmisspelled_rate = 48000\n");
            let stderr = String::from_utf8(output.stderr).unwrap();
            assert!(stderr.contains("misspelled_rate"), "{stderr}");
            assert!(stderr.contains("line 2"), "{stderr}");
        }

        #[test]
        fn commands_are_built_in_order_desk_first() {
            let output = launch(
                "order",
                "[[instrument]]\nname = 'dx'\nargs = ['--phrase', 'c4/8']\n[[instrument]]\nname = 'mini'\n",
            );
            let program = env!("CARGO_BIN_EXE_kazoo");
            assert_eq!(
                String::from_utf8(output.stdout).unwrap(),
                format!(
                    "'{program}' 'mix'\n'{program}' 'dx' '--phrase' 'c4/8'\n'{program}' 'mini'\n"
                )
            );
        }

        #[test]
        fn no_tmux_prints_the_commands_and_exits_two() {
            let output = launch("no-tmux", "[[instrument]]\nname = '808'\n");
            assert_eq!(output.status.code(), Some(2));
            assert_eq!(
                String::from_utf8(output.stdout).unwrap(),
                format!(
                    "'{}' 'mix'\n'{}' '808'\n",
                    env!("CARGO_BIN_EXE_kazoo"),
                    env!("CARGO_BIN_EXE_kazoo")
                )
            );
            assert!(String::from_utf8(output.stderr).unwrap().contains("tmux"));
        }
    }
}
