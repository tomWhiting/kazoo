//! A loop's record in the tree: its tape as a `[loop.tape]` table in the
//! loop's own file, and removed loops as `[[retired_loops]]` in
//! `wall.toml`. A bar never written is an empty table in `bars`, since a
//! TOML array holds no nulls.
use serde::Deserialize;

use super::{TreeError, float, line, string};
use crate::patch::{LoopRecord, RetiredLoop, TapeRecord};

/// The body of a loop's table (the caller has written its header,
/// `[loop]` or `[[retired_loops]]`), then `[<table>.tape]` if it has tape.
pub(super) fn write(
    out: &mut String,
    table: &str,
    record: &LoopRecord,
    path: &str,
) -> Result<(), TreeError> {
    if let Some(from) = &record.from {
        line(out, "from", &string(from));
    }
    if let Some(tape) = &record.tape {
        out.push_str("\n[");
        out.push_str(table);
        out.push_str(".tape]\n");
        write_tape(out, tape, path)?;
    }
    Ok(())
}

fn write_tape(out: &mut String, tape: &TapeRecord, path: &str) -> Result<(), TreeError> {
    for (what, number) in [
        ("a loop's frames", tape.frames),
        ("a loop's tail", tape.tail),
    ] {
        if i64::try_from(number).is_err() {
            return Err(TreeError::TooLarge { what });
        }
    }
    line(out, "reel", &string(&tape.reel));
    line(out, "rate", &tape.rate.to_string());
    line(out, "frames", &tape.frames.to_string());
    line(out, "tail", &tape.tail.to_string());
    line(out, "beats", &float(tape.beats, path, "beats")?);
    line(out, "anchor", &float(tape.anchor, path, "anchor")?);
    let bars: Vec<String> = tape
        .bars
        .iter()
        .map(|bar| bar.map_or_else(|| "{}".to_string(), |pass| format!("{{ pass = {pass} }}")))
        .collect();
    line(out, "bars", &format!("[{}]", bars.join(", ")));
    Ok(())
}

/// A module file's `[loop]` table.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LoopToml {
    from: Option<String>,
    tape: Option<TapeToml>,
}

/// One entry of `wall.toml`'s `[[retired_loops]]`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RetiredLoopToml {
    module: String,
    from: Option<String>,
    tape: Option<TapeToml>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TapeToml {
    reel: String,
    rate: u32,
    frames: u64,
    tail: u64,
    beats: f64,
    anchor: f64,
    bars: Vec<BarToml>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BarToml {
    pass: Option<i64>,
}

impl From<TapeToml> for TapeRecord {
    fn from(tape: TapeToml) -> Self {
        Self {
            reel: tape.reel,
            rate: tape.rate,
            frames: tape.frames,
            tail: tape.tail,
            beats: tape.beats,
            anchor: tape.anchor,
            bars: tape.bars.into_iter().map(|bar| bar.pass).collect(),
        }
    }
}

impl From<LoopToml> for LoopRecord {
    fn from(record: LoopToml) -> Self {
        Self {
            tape: record.tape.map(TapeRecord::from),
            from: record.from,
        }
    }
}

/// `wall.toml`'s removed loops, oldest removal first.
pub(super) fn retired(list: Vec<RetiredLoopToml>) -> Vec<RetiredLoop> {
    list.into_iter().map(RetiredLoop::from).collect()
}

impl From<RetiredLoopToml> for RetiredLoop {
    fn from(retired: RetiredLoopToml) -> Self {
        Self {
            module: retired.module,
            record: LoopRecord {
                tape: retired.tape.map(TapeRecord::from),
                from: retired.from,
            },
        }
    }
}
