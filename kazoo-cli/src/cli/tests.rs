use std::path::PathBuf;
use std::time::Duration;

use clap::{CommandFactory, Parser};
use kazoo_wall::daemon::{AudioMode, DaemonConfig, DeskMode};

use super::{Cli, Command, WallCommand};

const COMMANDS: [&str; 17] = [
    "808", "303", "cs80", "mini", "juno", "prophet", "dx", "string", "glass", "arp", "play",
    "mouth", "tui", "mix", "wall", "mcp", "tree",
];

#[test]
fn every_instrument_is_a_subcommand() {
    for name in COMMANDS {
        let mut args = vec!["kazoo", name];
        if name == "mcp" {
            args.extend(["--seat", "Tom"]);
        }
        let command = Cli::try_parse_from(args).unwrap().command;
        let matched = match command {
            Command::EightOhEight => name == "808",
            Command::ThreeOhThree => name == "303",
            Command::Cs80 => name == "cs80",
            Command::Mini => name == "mini",
            Command::Juno => name == "juno",
            Command::Prophet => name == "prophet",
            Command::Dx(_) => name == "dx",
            Command::String(_) => name == "string",
            Command::Glass(_) => name == "glass",
            Command::Arp(_) => name == "arp",
            Command::Play(_) => name == "play",
            Command::Mouth(_) => name == "mouth",
            Command::Tui => name == "tui",
            Command::Tree(args) => name == "tree" && !args.print,
            Command::Mix(args) => name == "mix" && !args.demo,
            Command::Wall { command } => name == "wall" && command.is_none(),
            Command::Mcp(args) => name == "mcp" && args.seat == "Tom",
            Command::Studio { .. } => false,
        };
        assert!(matched, "wrong command for {name}");
    }
}

#[test]
fn wall_without_a_subcommand_is_the_console() {
    assert_eq!(
        Cli::try_parse_from(["kazoo", "wall"]).unwrap().command,
        Command::Wall { command: None }
    );
    let Command::Wall {
        command: Some(WallCommand::Serve(args)),
    } = Cli::try_parse_from([
        "kazoo",
        "wall",
        "serve",
        "--no-audio",
        "--no-desk",
        "--rate",
        "44100",
    ])
    .unwrap()
    .command
    else {
        panic!("expected serve");
    };
    let config = DaemonConfig::try_from(args).unwrap();
    assert!(matches!(
        config.audio,
        AudioMode::Headless {
            sample_rate: 44_100
        }
    ));
    assert!(matches!(config.desk, DeskMode::Off));
    assert!(Cli::try_parse_from(["kazoo", "wall", "serve", "--rate", "100"]).is_err());
    for name in ["stop", "console"] {
        assert!(Cli::try_parse_from(["kazoo", "wall", name]).is_ok());
    }
    let Command::Wall {
        command: Some(WallCommand::Serve(args)),
    } = Cli::try_parse_from(["kazoo", "wall", "serve"])
        .unwrap()
        .command
    else {
        panic!("expected serve");
    };
    let config = DaemonConfig::try_from(args).unwrap();
    assert!(matches!(
        config.audio,
        AudioMode::Device { sample_rate: None }
    ));
    assert!(matches!(config.desk, DeskMode::Discover));
}

#[test]
fn play_flags_become_play_args() {
    let flags = [
        "--jam",
        "--drive",
        "--name",
        "bass",
        "--velocity",
        "90",
        "--bpm",
        "96",
        "--rate",
        "44100",
        "-c",
        "2",
        "--format",
        "tsv",
        "c4/4",
        "e4/4",
    ];
    let Command::Play(args) = Cli::try_parse_from(["kazoo", "play"].into_iter().chain(flags))
        .unwrap()
        .command
    else {
        panic!("expected play");
    };
    let expected = kazoo_play::parse_args(flags.map(str::to_owned))
        .unwrap()
        .unwrap();
    assert_eq!(kazoo_play::Args::from(args), expected);
    assert!(Cli::try_parse_from(["kazoo", "play", "--play", "--jam"]).is_err());
    for bpm in ["0", "NaN", "inf", "-inf", "-1"] {
        assert!(Cli::try_parse_from(["kazoo", "play", "--bpm", bpm]).is_err());
    }
    for (flag, value) in [("--velocity", "128"), ("--channel", "16"), ("--rate", "0")] {
        assert!(Cli::try_parse_from(["kazoo", "play", flag, value]).is_err());
    }
    let Command::Play(args) = Cli::try_parse_from(["kazoo", "play"]).unwrap().command else {
        panic!("expected play");
    };
    assert_eq!(kazoo_play::Args::from(args), kazoo_play::Args::default());
}

#[test]
fn mcp_seat_comes_from_the_flag_or_the_environment_and_is_checked() {
    let Command::Mcp(args) = Cli::try_parse_from(["kazoo", "mcp", "--seat", "Waffles"])
        .unwrap()
        .command
    else {
        panic!("expected mcp");
    };
    let options = kazoo_mcp::Args::from(args);
    assert_eq!(options.seat, "Waffles");
    assert_eq!(options.notify_every, Some(Duration::from_secs(20)));
    assert_eq!(options.socket, Some(kazoo_wall::paths::socket_path()));
    let err = Cli::try_parse_from(["kazoo", "mcp", "--seat", "bad/name"]).unwrap_err();
    assert!(err.to_string().contains("valid seat name"));
    assert!(Cli::try_parse_from(["kazoo", "mcp", "--seat", "Tom", "--notify-every", "0"]).is_err());
    if std::env::var_os("KAZOO_CLI_ENV_PROBE").is_some() {
        let Command::Mcp(args) = Cli::try_parse_from(["kazoo", "mcp"]).unwrap().command else {
            panic!("expected mcp from environment");
        };
        assert_eq!(args.seat, "Vesper");
        println!("environment seat verified");
        return;
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "cli::tests::mcp_seat_comes_from_the_flag_or_the_environment_and_is_checked",
            "--nocapture",
        ])
        .env("KAZOO_CLI_ENV_PROBE", "1")
        .env("KAZOO_SEAT", "Vesper")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("environment seat verified"));
}

#[test]
fn dx_and_glass_take_their_phrases() {
    let Command::Dx(args) = Cli::try_parse_from(["kazoo", "dx", "--phrase", "c4/8"])
        .unwrap()
        .command
    else {
        panic!("expected dx");
    };
    assert_eq!(
        kazoo_dx::Args::from(args),
        kazoo_dx::Args {
            phrase: Some("c4/8".into())
        }
    );
    let Command::Glass(args) = Cli::try_parse_from(["kazoo", "glass", "--demo", "/tmp/p.wav"])
        .unwrap()
        .command
    else {
        panic!("expected glass");
    };
    assert_eq!(
        kazoo_glass::Args::from(args),
        kazoo_glass::Args {
            demo: Some(PathBuf::from("/tmp/p.wav")),
            phrase: None
        }
    );
    let Command::String(args) = Cli::try_parse_from(["kazoo", "string", "--phrase", "e4/8"])
        .unwrap()
        .command
    else {
        panic!("expected string");
    };
    assert_eq!(
        kazoo_string::Args::from(args),
        kazoo_string::Args {
            phrase: Some("e4/8".into())
        }
    );
}

#[test]
fn help_names_every_command() {
    let mut command = Cli::command();
    let help = command.render_long_help().to_string();
    for name in COMMANDS {
        let count = help
            .lines()
            .filter(|line| line.split_whitespace().next() == Some(name))
            .count();
        assert_eq!(count, 1, "missing or repeated {name}: {help}");
    }
}

#[test]
fn the_console_restarts_the_wall_as_kazoo_wall_serve() {
    let program = PathBuf::from("/a/program/kazoo");
    let serve = super::console_serve(program.clone());
    assert_eq!(serve.program, program);
    assert_eq!(serve.args, ["wall", "serve"]);
}

#[test]
fn arp_keeps_its_target_and_silent_flags() {
    let Command::Arp(args) = Cli::try_parse_from(["kazoo", "arp", "--silent"])
        .unwrap()
        .command
    else {
        panic!("expected arp");
    };
    assert_eq!(
        kazoo_arp::Args::from(args),
        kazoo_arp::Args {
            target: [0; 16],
            silent: true
        }
    );
    let uuid = "0123456789abcdef0123456789abcdef";
    let Command::Arp(args) = Cli::try_parse_from(["kazoo", "arp", "--target", uuid])
        .unwrap()
        .command
    else {
        panic!("expected arp");
    };
    let expected = kazoo_arp::parse_args(["--target".to_owned(), uuid.to_owned()])
        .unwrap()
        .unwrap();
    assert_eq!(kazoo_arp::Args::from(args), expected);
    assert!(Cli::try_parse_from(["kazoo", "arp", "--target", "bogus"]).is_err());
}

#[test]
fn mouth_flags_become_microphone_arguments() {
    let Command::Mouth(args) = Cli::try_parse_from([
        "kazoo",
        "mouth",
        "--input",
        "X",
        "--input-channel",
        "2",
        "--list-inputs",
    ])
    .unwrap()
    .command
    else {
        panic!("expected mouth");
    };
    assert_eq!(args.input.as_deref(), Some("X"));
    assert_eq!(args.input_channel, Some(2));
    assert!(args.list_inputs);
    assert_eq!(
        kazoo_mouth::Args::from(args),
        kazoo_mouth::Args {
            input: Some("X".into()),
            input_channel: Some(2),
            list_inputs: true,
            ..kazoo_mouth::Args::default()
        }
    );
}

#[test]
fn mouth_without_flags_uses_defaults() {
    let Command::Mouth(args) = Cli::try_parse_from(["kazoo", "mouth"]).unwrap().command else {
        panic!("expected mouth");
    };
    assert_eq!(args, super::MouthArgs::default());
    assert_eq!(kazoo_mouth::Args::from(args), kazoo_mouth::Args::default());
}

#[test]
fn mouth_refuses_channels_outside_the_interface_range() {
    for channel in ["0", "65", "x"] {
        let error =
            Cli::try_parse_from(["kazoo", "mouth", "--input-channel", channel]).unwrap_err();
        assert!(error.to_string().contains("--input-channel"));
    }
}

#[test]
fn mouth_help_lists_the_input_flags() {
    let error = Cli::try_parse_from(["kazoo", "mouth", "--help"]).unwrap_err();
    assert_eq!(error.kind(), clap::error::ErrorKind::DisplayHelp);
    let help = error.to_string();
    for flag in ["--input", "--input-channel", "--list-inputs"] {
        assert!(help.contains(flag), "{help}");
    }
}

#[test]
fn args_parse() {
    let Command::Mouth(args) =
        Cli::try_parse_from(["kazoo", "mouth", "--quantise", "major:D", "--silent"])
            .unwrap()
            .command
    else {
        panic!("expected mouth");
    };
    let args = kazoo_mouth::Args::from(args);
    assert_eq!(args.quantise.scale, 1);
    assert_eq!(args.quantise.root, 2);
    assert!((args.quantise.blend - 1.0).abs() < f32::EPSILON);
    assert!(args.silent);
    let error = Cli::try_parse_from(["kazoo", "mouth", "--quantise", "lydian:H"]).unwrap_err();
    assert_eq!(error.exit_code(), 2);
    let error = error.to_string();
    for choice in [
        "chromatic",
        "major",
        "lydian",
        "pentatonic-minor",
        "C#",
        "B",
    ] {
        assert!(error.contains(choice), "{error}");
    }
    assert_eq!(
        Cli::try_parse_from(["kazoo", "tui"]).unwrap().command,
        Command::Tui
    );
}

#[test]
fn mouth_output_score_and_sharp_root_parse() {
    let Command::Mouth(args) = Cli::try_parse_from([
        "kazoo",
        "mouth",
        "--output",
        "Speakers",
        "--quantise",
        "pentatonic-minor:C#",
        "--score",
        "a.wav",
        "b.wav",
    ])
    .unwrap()
    .command
    else {
        panic!("expected mouth");
    };
    let args = kazoo_mouth::Args::from(args);
    assert_eq!(args.output.as_deref(), Some("Speakers"));
    assert_eq!(args.quantise.scale, 8);
    assert_eq!(args.quantise.root, 1);
    assert_eq!(args.score, [PathBuf::from("a.wav"), PathBuf::from("b.wav")]);
    assert!(Cli::try_parse_from(["kazoo", "mouth", "--score"]).is_err());
    for command in ["mouth", "tui"] {
        assert_eq!(
            Cli::try_parse_from(["kazoo", command, "--help"])
                .unwrap_err()
                .exit_code(),
            0
        );
    }
}

#[test]
fn wall_tree_import_parses() {
    let parsed = Cli::try_parse_from(["kazoo", "wall", "tree", "import", "--dry-run"])
        .unwrap()
        .command;
    assert_eq!(
        parsed,
        Command::Wall {
            command: Some(WallCommand::Tree {
                command: super::TreeCommand::Import(super::ImportArgs { dry_run: true })
            })
        }
    );
    assert!(Cli::try_parse_from(["kazoo", "wall", "tree", "import", "--unknown"]).is_err());
}

#[test]
fn tree_prints_at_a_size_or_walks() {
    assert_eq!(
        Cli::try_parse_from(["kazoo", "tree", "--print", "--cols", "80", "--rows", "24"])
            .unwrap()
            .command,
        Command::Tree(super::TreeArgs {
            print: true,
            cols: 80,
            rows: 24
        })
    );
    assert!(Cli::try_parse_from(["kazoo", "tree", "--cols", "5"]).is_err());
    let err = Cli::try_parse_from(["kazoo", "tree", "--cols", "80"]).expect_err("needs --print");
    assert!(err.to_string().contains("--print"), "{err}");
    assert_eq!(
        Cli::try_parse_from(["kazoo", "tree"]).unwrap().command,
        Command::Tree(super::TreeArgs {
            print: false,
            cols: 100,
            rows: 40
        })
    );
}
