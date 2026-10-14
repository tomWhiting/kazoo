//! Print and save the pitch tracker scoreboard.

use std::{
    io::Write,
    path::{Path, PathBuf},
};

use kazoo_core::analysis::score::{
    CycleUnderTest, FusedUnderTest, PyinUnderTest, TrackerUnderTest, gate,
    instrument::instrument_suite, score, signals, table, wav,
};
use kazoo_core::host::live::InputProfile;

const USAGE: &str = "Usage: voice_score [--rate <hz>] [<wav>...]";

fn trackers(rate: u32) -> kazoo_core::Result<Vec<Box<dyn TrackerUnderTest>>> {
    Ok(vec![
        Box::new(PyinUnderTest::new(rate)?),
        Box::new(CycleUnderTest::new(rate)),
        Box::new(FusedUnderTest::new(rate)?),
    ])
}

fn arguments() -> (u32, Vec<PathBuf>) {
    let mut rate = 48_000;
    let mut paths = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--rate" {
            let Some(text) = args.next() else {
                eprintln!("{USAGE}");
                std::process::exit(2);
            };
            let value = match text.parse::<u32>() {
                Ok(value) => value,
                Err(error) => {
                    eprintln!("{error}\n{USAGE}");
                    std::process::exit(2);
                }
            };
            if let Err(error) = PyinUnderTest::new(value) {
                eprintln!("{error}\n{USAGE}");
                std::process::exit(2);
            }
            rate = value;
        } else if arg.starts_with('-') {
            eprintln!("{USAGE}");
            std::process::exit(2);
        } else {
            paths.push(PathBuf::from(arg));
        }
    }
    (rate, paths)
}

fn main() -> kazoo_core::Result<()> {
    let (rate, paths) = arguments();
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut takes = signals::suite(rate);
    takes.extend(wav::fixture_takes(&manifest.join("tests/fixtures/voice"))?);
    for path in paths {
        takes.push(wav::read_take(&path)?);
    }
    let mut rows = Vec::new();
    for take in &takes {
        for mut tracker in trackers(take.rate)? {
            rows.push((
                tracker.name(),
                take.name.as_str(),
                score(tracker.as_mut(), take),
            ));
        }
    }
    let mut gate_rows = Vec::new();
    for take in &takes {
        gate_rows.push((
            "voice",
            take.name.as_str(),
            gate::score_gate(&mut FusedUnderTest::new(take.rate)?, take),
        ));
    }
    let mut instrument_takes = instrument_suite(rate);
    instrument_takes.extend(wav::fixture_takes_with_settings(
        &manifest.join("tests/fixtures/instrument"),
        InputProfile::Line.tracking(),
    )?);
    let mut instrument_rows = Vec::new();
    let mut instrument_gates = Vec::new();
    for (profile, name) in [
        (InputProfile::Guitar, "voice:guitar"),
        (InputProfile::Bass, "voice:bass"),
    ] {
        for take in &instrument_takes {
            instrument_rows.push((
                name,
                take.name.as_str(),
                score(
                    &mut FusedUnderTest::with_settings(take.rate, profile.tracking())?,
                    take,
                ),
            ));
            instrument_gates.push((
                name,
                take.name.as_str(),
                gate::score_gate(
                    &mut FusedUnderTest::with_settings(take.rate, profile.tracking())?,
                    take,
                ),
            ));
        }
    }
    let output = format!(
        "{}\n{}\n## Instruments\n\n{}\n{}",
        table(&rows),
        gate::table(&gate_rows),
        table(&instrument_rows),
        gate::table(&instrument_gates)
    );
    std::io::stdout().lock().write_all(output.as_bytes())?;
    let logs = manifest.join("../target/kz-logs");
    std::fs::create_dir_all(&logs)?;
    std::fs::write(logs.join("voice-score.md"), output)?;
    Ok(())
}
