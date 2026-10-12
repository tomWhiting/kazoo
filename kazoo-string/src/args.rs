use std::io::Write;

/// What the command line gave.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Args {
    /// A looped `kazoo-play` phrase to play on the transport, as notation.
    pub phrase: Option<String>,
}

/// Help text for both ways to start the synthesizer.
pub const USAGE: &str = "kazoo-string: plucked-string physical-model synth, Karplus-Strong waveguides with stiffness, pick position and a wooden body

usage: kazoo string [--phrase \"<kazoo-play notation>\"]
       kazoo-string [--phrase \"<kazoo-play notation>\"]

It plugs into the kazoo-mix desk whenever one is running, and plays on its
own otherwise. A phrase loops on the transport. Press ? inside for the keys.
";

/// Read the arguments after the program name. `Ok(None)` means help was
/// asked for, wherever it stands; `Err` is one sentence saying what was wrong
/// (the caller adds the usage).
pub fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Option<Args>, String> {
    let args: Vec<String> = args.into_iter().collect();
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "-h" | "--help"))
    {
        return Ok(None);
    }
    let mut args = args.into_iter();
    let mut phrase = None;
    while let Some(arg) = args.next() {
        if arg == "--phrase" {
            let text = args.next().ok_or_else(|| {
                "--phrase needs notation, e.g. --phrase \"c4/8 e4/8 g4/8 c5/8\"".to_owned()
            })?;
            phrase = Some(text);
        } else if let Some(text) = arg.strip_prefix("--phrase=") {
            phrase = Some(text.to_owned());
        } else {
            return Err(format!("unknown argument '{arg}'"));
        }
    }
    Ok(Some(Args { phrase }))
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
    use super::{Args, USAGE, parse_args};

    #[test]
    fn help_returns_instead_of_exiting() {
        assert_eq!(parse_args(["--help"].map(str::to_owned)), Ok(None));
        assert_eq!(
            parse_args(["--phrase", "c4/4", "-h"].map(str::to_owned)),
            Ok(None)
        );
    }

    #[test]
    fn help_wins_wherever_it_stands() {
        assert_eq!(
            parse_args(["--bogus", "--help"].map(str::to_owned)),
            Ok(None)
        );
        assert_eq!(parse_args(["--phrase", "-h"].map(str::to_owned)), Ok(None));
        assert_eq!(parse_args(["-h", "--phrase"].map(str::to_owned)), Ok(None));
    }

    #[test]
    fn phrase_is_taken_both_ways() {
        let expected = Ok(Some(Args {
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
        assert_eq!(parse_args([]), Ok(Some(Args { phrase: None })));
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
        assert!(
            parse_args(["--bogus"].map(str::to_owned)).is_err_and(|why| why.contains("--bogus"))
        );
    }

    #[test]
    fn usage_names_both_ways_to_start_it() {
        assert!(USAGE.contains("kazoo string"));
        assert!(USAGE.contains("kazoo-string"));
    }

    #[test]
    fn usage_opens_with_the_name_and_description() {
        assert!(USAGE.starts_with(
            "kazoo-string: plucked-string physical-model synth, Karplus-Strong waveguides with stiffness, pick position and a wooden body\n"
        ));
    }

    #[test]
    fn args_parse() {
        let args = |v: &[&str]| parse_args(v.iter().map(|s| (*s).to_owned()));
        assert_eq!(args(&[]), Ok(Some(Args { phrase: None })));
        assert_eq!(
            args(&["--phrase", "c4 e"]),
            Ok(Some(Args {
                phrase: Some("c4 e".into())
            }))
        );
        assert_eq!(
            args(&["--phrase=c4"]),
            Ok(Some(Args {
                phrase: Some("c4".into())
            }))
        );
        assert!(args(&["--phrase"]).is_err());
        assert!(args(&["--bogus"]).is_err());
        assert_eq!(args(&["-h"]), Ok(None));
        assert_eq!(
            args(&["--phrase", "c4", "--phrase=e4"]),
            Ok(Some(Args {
                phrase: Some("e4".into())
            }))
        );
        assert_eq!(
            args(&["--phrase=c4", "--phrase", "e4"]),
            Ok(Some(Args {
                phrase: Some("e4".into())
            }))
        );
    }
}
