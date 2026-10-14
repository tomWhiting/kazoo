use std::process::Command;

use color_eyre::Result;
use kazoo_core::io::file::{AudioBuffer, write_wav};

#[test]
fn mouth_and_tui_help_exit_successfully_without_audio() {
    for command in ["mouth", "tui"] {
        let output = Command::new(env!("CARGO_BIN_EXE_kazoo"))
            .args([command, "--help"])
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(0),
            "{command}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains(&format!("kazoo {command}")));
    }
}

#[test]
fn mouth_scores_a_take_without_opening_audio() -> Result<()> {
    let directory = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../target/kz-logs/mouth-score-test")
        .join(format!("cli-{}", std::process::id()));
    std::fs::create_dir_all(&directory)?;
    let path = directory.join("cli-sine-220.wav");
    let samples = (0..48_000)
        .map(|sample| (std::f32::consts::TAU * 220.0 * sample as f32 / 48_000.0).sin() * 0.5)
        .collect();
    write_wav(&path, &AudioBuffer::new(samples, 1, 48_000)?)?;
    let output = Command::new(env!("CARGO_BIN_EXE_kazoo"))
        .args([
            "mouth",
            "--input",
            "__no_mouth_mic__",
            "--output",
            "__no_mouth_speakers__",
            "--score",
        ])
        .arg(&path)
        .output()?;
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout)?;
    assert!(text.contains("| voice | cli-sine-220.wav |"));
    assert!(text.contains("| pyin | cli-sine-220.wav |"));
    assert!(text.contains("rise max ms"));
    let missing = directory.join(format!("missing-cli-{}.wav", std::process::id()));
    let output = Command::new(env!("CARGO_BIN_EXE_kazoo"))
        .args(["mouth", "--score"])
        .arg(&missing)
        .arg(&path)
        .output()?;
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8(output.stderr)?.contains("missing-cli-"));
    assert!(String::from_utf8(output.stdout)?.contains("| pyin | cli-sine-220.wav |"));
    Ok(())
}

#[test]
fn studio_accepts_the_dedicated_mouth_flags() {
    let session = kazoo::studio::StudioFile::parse(
        "[[instrument]]\nname = 'mouth'\nargs = ['--quantise', 'major:D', '--silent']\n",
    )
    .unwrap();
    assert_eq!(session.instrument[0].name, "mouth");
    assert_eq!(
        session.instrument[0].args,
        ["--quantise", "major:D", "--silent"]
    );
}
