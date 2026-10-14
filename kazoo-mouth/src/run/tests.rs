use super::{Args, parse_args, parse_quantise};

#[test]
fn standalone_args_match_the_mouth_controls() {
    let args = parse_args(
        [
            "--input",
            "Mic",
            "--input-channel",
            "2",
            "--output",
            "Speakers",
            "--quantise",
            "major:D",
            "--silent",
        ]
        .map(str::to_owned),
    )
    .unwrap()
    .unwrap();
    assert_eq!(args.input.as_deref(), Some("Mic"));
    assert_eq!(args.input_channel, Some(2));
    assert_eq!(args.output.as_deref(), Some("Speakers"));
    assert_eq!(args.quantise.scale, 1);
    assert_eq!(args.quantise.root, 2);
    assert!((args.quantise.blend - 1.0).abs() < f32::EPSILON);
    assert!(args.silent);
    assert_eq!(parse_args(Vec::new()).unwrap(), Some(Args::default()));
}

#[test]
fn standalone_help_wins_and_bad_controls_name_the_choices() {
    assert_eq!(
        parse_args(["--unknown", "--help"].map(str::to_owned)).unwrap(),
        None
    );
    assert!(parse_args(["--score"].map(str::to_owned)).is_err());
    assert!(parse_args(["--input-channel", "0"].map(str::to_owned)).is_err());
    let error = parse_quantise("lydian:H").unwrap_err();
    assert!(error.contains("pentatonic-minor"));
    assert!(error.contains("C#"));
    assert!(parse_quantise("pentatonic-minor:C#").is_ok());
}
