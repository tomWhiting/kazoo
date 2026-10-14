use std::io::Write;
use std::path::PathBuf;

use color_eyre::Result;
use color_eyre::eyre::eyre;
use kazoo_core::analysis::score::{self, FusedUnderTest, PyinUnderTest, gate};

pub fn print_takes(paths: &[PathBuf], out: &mut impl Write, errors: &mut impl Write) -> Result<()> {
    let mut failed = 0;
    for path in paths {
        match tables(path) {
            Ok((pitch, gates)) => {
                write!(out, "{pitch}{gates}")?;
            }
            Err(error) => {
                writeln!(errors, "kazoo-mouth: {}: {error}", path.display())?;
                failed += 1;
            }
        }
    }
    if failed > 0 {
        Err(eyre!("{failed} take(s) could not be scored"))
    } else {
        Ok(())
    }
}

fn tables(path: &std::path::Path) -> Result<(String, String)> {
    let take = score::wav::read_take(path)?;
    let voice = score::score(&mut FusedUnderTest::new(take.rate)?, &take);
    let pyin = score::score(&mut PyinUnderTest::new(take.rate)?, &take);
    let gates = gate::score_gate(&mut FusedUnderTest::new(take.rate)?, &take);
    Ok((
        score::table(&[("voice", &take.name, voice), ("pyin", &take.name, pyin)]),
        gate::table(&[("voice", &take.name, gates)]),
    ))
}

#[cfg(test)]
mod tests;
