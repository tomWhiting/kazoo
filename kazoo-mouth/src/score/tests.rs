use std::path::PathBuf;
use std::sync::OnceLock;

use color_eyre::Result;

fn take_path() -> Result<&'static PathBuf> {
    static TAKE: OnceLock<std::result::Result<PathBuf, String>> = OnceLock::new();
    TAKE.get_or_init(|| {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../target/kz-logs/mouth-score-test")
            .join(format!("unit-{}", std::process::id()));
        std::fs::create_dir_all(&root).map_err(|error| error.to_string())?;
        let path = root.join("sine-220.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer =
            hound::WavWriter::create(&path, spec).map_err(|error| error.to_string())?;
        for sample in 0..48_000 {
            writer
                .write_sample(
                    (std::f32::consts::TAU * 220.0 * sample as f32 / 48_000.0).sin() * 0.5,
                )
                .map_err(|error| error.to_string())?;
        }
        writer.finalize().map_err(|error| error.to_string())?;
        Ok(path)
    })
    .as_ref()
    .map_err(|error| color_eyre::eyre::eyre!("{error}"))
}

#[test]
fn score_reads_a_take() -> Result<()> {
    let path = take_path()?;
    let args = crate::parse_args(["--score".into(), path.display().to_string()])
        .map_err(|error| color_eyre::eyre::eyre!(error))?
        .ok_or_else(|| color_eyre::eyre::eyre!("score requested help"))?;
    let mut output = Vec::new();
    let mut errors = Vec::new();
    super::print_takes(&args.score, &mut output, &mut errors)?;
    let text = String::from_utf8(output)?;
    assert!(text.contains("| voice | sine-220.wav |"));
    assert!(text.contains("| pyin | sine-220.wav |"));
    assert!(text.contains("rise max ms"));
    assert!(errors.is_empty());
    Ok(())
}

#[test]
fn unreadable_takes_report_an_error_and_scoring_continues() -> Result<()> {
    let valid = take_path()?.clone();
    let missing = valid.with_file_name("missing.wav");
    let mut output = Vec::new();
    let mut errors = Vec::new();
    let result = super::print_takes(&[missing, valid], &mut output, &mut errors);
    assert!(result.is_err());
    assert!(String::from_utf8(errors)?.contains("missing.wav"));
    assert!(String::from_utf8(output)?.contains("| pyin | sine-220.wav |"));
    Ok(())
}
