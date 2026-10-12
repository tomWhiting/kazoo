use kazoo_wall::{Args, ServeArgs, USAGE, parse_args};

fn parse(args: &[&str]) -> Result<Option<Args>, String> {
    parse_args(args.iter().map(|arg| (*arg).to_owned()))
}

#[test]
fn no_arguments_is_the_console() {
    assert_eq!(parse(&[]), Ok(Some(Args::Console)));
}

#[test]
fn serve_flags_are_parsed_with_their_defaults() {
    assert_eq!(
        parse(&["serve"]),
        Ok(Some(Args::Serve(ServeArgs::default())))
    );
    assert_eq!(
        ServeArgs::default(),
        ServeArgs {
            audio: true,
            desk: true,
            rate: None
        }
    );
    assert_eq!(
        parse(&["serve", "--no-audio", "--no-desk", "--rate", "44100"]),
        Ok(Some(Args::Serve(ServeArgs {
            audio: false,
            desk: false,
            rate: Some(44100)
        })))
    );
    assert_eq!(
        parse(&["serve", "--rate", "8000", "--rate", "384000"]),
        Ok(Some(Args::Serve(ServeArgs {
            rate: Some(384_000),
            ..ServeArgs::default()
        })))
    );
    for value in [
        None,
        Some("7999"),
        Some("384001"),
        Some("0"),
        Some("NaN"),
        Some("inf"),
        Some("-1"),
        Some("4294967296"),
    ] {
        let mut args = vec!["serve", "--rate"];
        if let Some(value) = value {
            args.push(value);
        }
        assert_eq!(
            parse(&args),
            Err("--rate takes a sample rate from 8000 to 384000".to_owned())
        );
    }
}

#[test]
fn stop_is_stop() {
    assert_eq!(parse(&["stop"]), Ok(Some(Args::Stop)));
}

#[test]
fn an_unknown_argument_is_refused_by_name() {
    for args in [
        vec!["--bogus"],
        vec!["console"],
        vec!["stop", "--bogus"],
        vec!["stop", "serve"],
    ] {
        let unknown = args.last().unwrap();
        assert_eq!(parse(&args), Err(format!("unknown argument '{unknown}'")));
    }
    assert_eq!(
        parse(&["serve", "--bogus"]),
        Err("unknown option '--bogus'".to_owned())
    );
    assert_eq!(
        parse(&["serve", "--rate=44100"]),
        Err("unknown option '--rate=44100'".to_owned())
    );
}

#[test]
fn help_asks_for_the_usage() {
    for args in [
        vec!["help"],
        vec!["--help"],
        vec!["-h"],
        vec!["--bogus", "--help"],
        vec!["serve", "--rate", "-h"],
        vec!["stop", "--help"],
        vec!["serve", "--rate", "NaN", "--help"],
    ] {
        assert_eq!(parse(&args), Ok(None));
    }
    assert!(USAGE.starts_with("kazoo-wall: the shared modular synth that never stops\n"));
    assert!(USAGE.contains("kazoo wall serve"));
    assert!(USAGE.contains("kazoo-wall serve"));
}
