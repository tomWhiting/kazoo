use std::io::Write;

/// What the command line gave.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Args {
    /// Render the six-vignette preview to this WAV and exit: no device, no desk.
    /// When a demo is requested, the phrase is ignored.
    pub demo: Option<std::path::PathBuf>,
    /// A looped `kazoo-play` phrase to play on the transport, as notation.
    pub phrase: Option<String>,
}

/// Help text for both ways to start the synthesizer.
pub const USAGE: &str =
    "kazoo-glass: polyphonic modal bell synth, tuned glass, chimes and metallic keys

usage: kazoo glass [--phrase \"<kazoo-play notation>\"]
       kazoo glass --demo <preview.wav>
       kazoo-glass [--phrase ...] | --demo <preview.wav>

It plugs into the kazoo-mix desk whenever one is running, and plays on its
own otherwise. --demo renders a preview and exits. Press ? inside for keys.
";

/// Read the arguments after the program name. `Ok(None)` means help was
/// asked for, wherever it stands; `Err` says what was wrong (the caller adds
/// the usage).
pub fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Option<Args>, String> {
    let args: Vec<String> = args.into_iter().collect();
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "-h" | "--help"))
    {
        return Ok(None);
    }
    let mut args = args.into_iter();
    let mut parsed = Args::default();
    while let Some(arg) = args.next() {
        if arg == "--demo" {
            let path = args
                .next()
                .ok_or_else(|| "--demo needs a WAV path".to_owned())?;
            parsed.demo = Some(path.into());
        } else if arg == "--phrase" {
            let text = args.next().ok_or_else(|| {
                "--phrase needs notation, e.g. --phrase \"c4/8 e4/8 g4/8 c5/8\"".to_owned()
            })?;
            parsed.phrase = Some(text);
        } else if let Some(text) = arg.strip_prefix("--phrase=") {
            parsed.phrase = Some(text.to_owned());
        } else {
            return Err(format!("unknown argument '{arg}'"));
        }
    }
    Ok(Some(parsed))
}

/// Print help or start the synthesizer from command-line arguments.
pub fn cli(args: impl IntoIterator<Item = String>) -> color_eyre::Result<()> {
    match parse_args(args) {
        Ok(None) => {
            let mut stdout = std::io::stdout().lock();
            stdout.write_all(USAGE.as_bytes())?;
            stdout.flush()?;
            Ok(())
        }
        Ok(Some(args)) => crate::run(args),
        Err(why) => Err(color_eyre::eyre::eyre!("{why}\n\n{USAGE}")),
    }
}

#[cfg(test)]
mod tests {
    use crate::{Args, USAGE, parse_args};

    #[test]
    fn help_returns_instead_of_exiting() -> std::io::Result<()> {
        // An exit with status zero passes nextest; the child must prove it returned.
        if std::env::var_os("KAZOO_GLASS_HELP_RETURN_PROBE").is_some() {
            assert_eq!(parse_args(["--help"].map(str::to_owned)), Ok(None));
            assert_eq!(
                parse_args(["--demo", "/tmp/x.wav", "-h"].map(str::to_owned)),
                Ok(None)
            );
            println!("help parser returned");
            return Ok(());
        }
        let output = std::process::Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "args::tests::help_returns_instead_of_exiting",
                "--nocapture",
            ])
            .env("KAZOO_GLASS_HELP_RETURN_PROBE", "1")
            .output()?;
        assert!(output.status.success());
        assert!(
            output
                .stdout
                .windows(b"help parser returned".len())
                .any(|part| part == b"help parser returned")
        );
        Ok(())
    }

    #[test]
    fn help_wins_wherever_it_stands() {
        for args in [
            vec!["--bogus", "--help"],
            vec!["--demo", "-h"],
            vec!["--phrase", "-h"],
            vec!["-h", "--bogus"],
        ] {
            assert_eq!(parse_args(args.into_iter().map(str::to_owned)), Ok(None));
        }
    }

    #[test]
    fn demo_is_a_path_anywhere() {
        let path = std::path::PathBuf::from("/tmp/p.wav");
        assert_eq!(
            parse_args(["--demo", "/tmp/p.wav"].map(str::to_owned)),
            Ok(Some(Args {
                demo: Some(path.clone()),
                phrase: None
            }))
        );
        assert_eq!(
            parse_args(["--phrase", "c4/4", "--demo", "/tmp/p.wav"].map(str::to_owned)),
            Ok(Some(Args {
                demo: Some(path),
                phrase: Some("c4/4".into())
            }))
        );
        assert!(
            parse_args(["--demo"].map(str::to_owned))
                .is_err_and(|why| why.contains("--demo needs a WAV path"))
        );
    }

    #[test]
    fn phrase_is_taken_both_ways() {
        let expected = Ok(Some(Args {
            demo: None,
            phrase: Some("c4/8 e4/8".into()),
        }));
        assert_eq!(
            parse_args(["--phrase", "c4/8 e4/8"].map(str::to_owned)),
            expected
        );
        assert_eq!(
            parse_args(["--phrase=c4/8 e4/8"].map(str::to_owned)),
            expected
        );
        assert_eq!(parse_args([]), Ok(Some(Args::default())));
    }

    #[test]
    fn a_bare_phrase_flag_says_what_it_needs() {
        assert!(
            parse_args(["--phrase"].map(str::to_owned))
                .is_err_and(|why| why.contains("--phrase needs notation"))
        );
    }

    #[test]
    fn an_unknown_argument_is_refused_by_name() {
        assert_eq!(
            parse_args(["--bogus"].map(str::to_owned)),
            Err("unknown argument '--bogus'".to_owned())
        );
    }

    #[test]
    fn usage_names_both_ways_to_start_it() {
        assert!(USAGE.contains("kazoo glass"));
        assert!(USAGE.contains("kazoo-glass"));
    }

    #[test]
    fn usage_opens_with_the_name_and_description() {
        assert!(USAGE.starts_with(
            "kazoo-glass: polyphonic modal bell synth, tuned glass, chimes and metallic keys\n"
        ));
    }

    #[test]
    fn args_parse() {
        let args = |v: &[&str]| parse_args(v.iter().map(|s| (*s).to_owned()));
        assert_eq!(args(&[]), Ok(Some(Args::default())));
        assert_eq!(
            args(&["--phrase", "c4 e"]),
            Ok(Some(Args {
                demo: None,
                phrase: Some("c4 e".into())
            }))
        );
        assert_eq!(
            args(&["--phrase=c4"]),
            Ok(Some(Args {
                demo: None,
                phrase: Some("c4".into())
            }))
        );
        assert!(args(&["--phrase"]).is_err());
        assert!(args(&["--bogus"]).is_err());
    }
}
