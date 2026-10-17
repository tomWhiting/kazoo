use super::{Args, parse_args, parse_quantise};

struct FakeInputHost {
    names: Vec<std::result::Result<&'static str, &'static str>>,
}

impl FakeInputHost {
    fn pick(
        &self,
        notices: &kazoo_core::host::audio::InputNotices,
    ) -> kazoo_core::Result<kazoo_core::host::audio::InputPick> {
        kazoo_core::host::audio::input_for_names(
            None,
            Some(Ok("AirPods".into())),
            "AirPods",
            self.names.iter().map(|name| {
                name.map(str::to_owned)
                    .map_err(|reason| kazoo_core::Error::Device(reason.into()))
            }),
            |message| notices.report(message),
        )
    }
}

#[test]
fn an_unnamable_second_input_is_skipped_and_said_once() {
    let host = FakeInputHost {
        names: vec![
            Ok("AirPods"),
            Err("device disconnected"),
            Ok("MacBook Pro Microphone"),
        ],
    };
    let notices = kazoo_core::host::audio::InputNotices::default();
    let input = host.pick(&notices).unwrap();
    assert_eq!(input.name, "MacBook Pro Microphone");
    let now = std::time::Instant::now();
    let mut app = crate::controls::App::new(super::QuantiseSettings::default(), false);
    super::show_input_notice(&mut app, &notices, now);
    assert!(
        app.notice(now)
            .unwrap()
            .contains("an input device could not be named:")
    );
    assert!(
        app.notice(now)
            .unwrap()
            .contains("device disconnected; left out")
    );
    for _ in 0..5 {
        assert_eq!(host.pick(&notices).unwrap().name, "MacBook Pro Microphone");
        assert!(notices.take().is_none());
    }
    let later = now + std::time::Duration::from_secs(4);
    super::show_input_notice(&mut app, &notices, later);
    assert!(app.notice(later).is_none());
}

#[test]
fn an_only_unnamable_input_says_no_input_device() {
    let host = FakeInputHost {
        names: vec![Err("device disconnected")],
    };
    let notices = kazoo_core::host::audio::InputNotices::default();
    let error = host.pick(&notices).unwrap_err().to_string();
    assert!(error.contains("no input device"), "{error}");
    assert!(
        notices
            .take()
            .unwrap()
            .contains("device disconnected; left out")
    );
    assert!(notices.take().is_none());
}

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

#[test]
fn a_saved_loop_is_a_float_wav_that_loops() -> color_eyre::Result<()> {
    let directory = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../target/kz-logs/mouth-loop-wav-test");
    std::fs::create_dir_all(&directory)?;
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    let samples: Vec<_> = (0..48_000)
        .map(|cell| (std::f32::consts::TAU * 220.0 * cell as f32 / 48_000.0).sin() * 0.25)
        .collect();
    let first = super::save_loop(&directory, &samples, 48_000, seconds)?;
    let second = super::save_loop(&directory, &samples, 48_000, seconds)?;
    assert_eq!(
        first.file_name().unwrap().to_string_lossy(),
        format!("mouth-loop-{seconds}.wav")
    );
    assert_eq!(
        second.file_name().unwrap().to_string_lossy(),
        format!("mouth-loop-{seconds}-2.wav")
    );
    let loaded = kazoo_core::io::read_wav(&first)?;
    assert_eq!(loaded.channels, 1);
    assert_eq!(loaded.sample_rate, 48_000);
    assert_eq!(loaded.samples, samples);
    let spec = hound::WavReader::open(&first)?.spec();
    assert_eq!(spec.bits_per_sample, 32);
    assert_eq!(spec.sample_format, hound::SampleFormat::Float);
    for entry in std::fs::read_dir(&directory)? {
        let name = entry?.file_name();
        let name = name.to_string_lossy();
        assert!(!name.contains(".tmp-") && !name.ends_with(".reserve"));
    }
    std::fs::remove_file(first)?;
    std::fs::remove_file(second)?;
    Ok(())
}

/// The terminal is left once on every way out, and every failure is kept
/// with its cause: the run's, and a leave that failed. Letting go of the
/// mouse is kazoo-host's, tested there (`leave_into`).
#[test]
fn the_terminal_is_left_on_exit_and_every_failure_kept() {
    use super::{Restore, leave};
    use std::cell::Cell;
    use std::rc::Rc;
    #[derive(Debug)]
    struct Guard {
        left: Rc<Cell<usize>>,
        refuses: bool,
    }
    impl Restore for Guard {
        fn leave(self) -> std::io::Result<()> {
            self.left.set(self.left.get() + 1);
            if self.refuses {
                Err(std::io::Error::other("injected restore error"))
            } else {
                Ok(())
            }
        }
    }
    assert!(super::terminal_options().mouse);
    for (failed, refused) in [(false, false), (true, false), (false, true), (true, true)] {
        let left = Rc::new(Cell::new(0));
        let result = if failed {
            Err(color_eyre::eyre::eyre!("injected event error"))
        } else {
            Ok(())
        };
        let guard = Guard {
            left: Rc::clone(&left),
            refuses: refused,
        };
        let result = leave(result, guard, Ok(()));
        assert_eq!(left.get(), 1, "left once");
        assert_eq!(result.is_err(), failed || refused);
        if let Err(error) = result {
            let said = format!("{error:#}");
            assert_eq!(said.contains("injected event error"), failed, "{said}");
            assert_eq!(
                said.contains("restoring the mouth terminal"),
                refused,
                "{said}"
            );
            assert_eq!(said.contains("injected restore error"), refused, "{said}");
        }
    }
}

#[test]
fn desk_refusal_stays_in_the_mouth_status() {
    let mut state = kazoo_core::ipc::link::LinkStatus {
        voice: kazoo_core::ipc::link::VoiceLink::default(),
        normal_priority: false,
        connected: false,
        strip: None,
        blocks_sent: 0,
        blocks_dropped: 0,
        messages_dropped: 0,
        connections: 1,
        last_refusal: Some(
            "the desk refused mouth: it runs at 48000 Hz, the desk at 24000 Hz (use the same device)"
                .into(),
        ),
    };
    assert_eq!(
        super::desk_status(&state),
        "not on the desk: it runs at 48000 Hz, the desk at 24000 Hz (use the same device)"
    );
    state.last_refusal = Some("the desk closed the connection".into());
    assert_eq!(
        super::desk_status(&state),
        "not on the desk: the desk closed the connection"
    );
    state.last_refusal = None;
    assert_eq!(super::desk_status(&state), "playing direct: no desk");
    state.connected = true;
    state.voice.desk_carries = true;
    state.strip = Some(0);
    assert_eq!(super::desk_status(&state), "on the desk, strip 1");
}
