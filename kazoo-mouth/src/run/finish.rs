use std::io;

use color_eyre::Result;
use color_eyre::eyre::WrapErr;

pub(super) trait Restore {
    fn leave(self) -> io::Result<()>;
}

impl Restore for kazoo_host::terminal::Terminal {
    fn leave(self) -> io::Result<()> {
        Self::leave(self)
    }
}

pub(super) fn finish(
    result: Result<()>,
    terminal: impl Restore,
    stopped: Result<()>,
) -> Result<()> {
    let restored = terminal.leave().wrap_err("restoring the mouth terminal");
    super::combine(super::combine(result, restored), stopped)
}
