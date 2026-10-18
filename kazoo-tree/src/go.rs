//! `g`: putting the wall at a moment of its tree.
//!
//! The tree only reads, but for this: it says `go` to the wall over its
//! control socket, as Tom's console (the tree is Tom's own view, and a
//! console's go is never held to a seat's allowance or refused for the
//! wall's guess at its cost: it is put up, with a note when heavy), and the wall puts
//! itself at that moment while it plays (keeping the wall as it was first,
//! so the go can be undone). The go is sent off the screen's thread, so a
//! wall that is slow to answer never stops the screen.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use kazoo_wall::protocol::client::{ClientError, WallClient};
use kazoo_wall::protocol::{LoadResult, Request};

use crate::app::short;

/// Who the go is logged under: the tree is Tom's own view.
pub const SEAT: &str = "Tom";
/// What the wall is told the go came from.
const CLIENT: &str = concat!("kazoo tree ", env!("CARGO_PKG_VERSION"));
/// How long to wait for the wall to answer.
const WAIT: Duration = Duration::from_secs(15);

/// Goes sent to the wall, and what came back from each.
#[derive(Debug)]
pub struct Goes {
    socket: PathBuf,
    tell: mpsc::Sender<String>,
    told: mpsc::Receiver<String>,
}

impl Goes {
    /// Goes to the wall listening on `socket`.
    #[must_use]
    pub fn to(socket: PathBuf) -> Self {
        let (tell, told) = mpsc::channel();
        Self { socket, tell, told }
    }

    /// Put the wall at `moment` (a whole commit). What happened is
    /// [`Goes::heard`] once the wall has answered.
    pub fn send(&self, moment: &str) {
        let socket = self.socket.clone();
        let moment = moment.to_owned();
        let tell = self.tell.clone();
        let spawned = std::thread::Builder::new()
            .name("kazoo-tree-go".to_owned())
            .spawn(move || {
                let words = answer(&moment, || go(&socket, &moment));
                // Nobody is left to tell once the screen has gone.
                if tell.send(words).is_err() {
                    eprintln!(
                        "kazoo tree: the go to {} ended after the tree closed",
                        short(&moment)
                    );
                }
            });
        if let Err(err) = spawned
            && self
                .tell
                .send(format!("the go could not be started: {err}"))
                .is_err()
        {
            eprintln!("kazoo tree: the go could not be started: {err}");
        }
    }

    /// What the wall said to a go since the last ask, if it has answered.
    #[must_use]
    pub fn heard(&self) -> Option<String> {
        self.told.try_iter().next()
    }
}

/// What a go to `moment` comes to: the wall's answer, why it failed, or, if
/// the go panicked, that it did and why. A panic is caught here because the
/// terminal's hook leaves this thread's panics to the tree.
fn answer(
    moment: &str,
    go: impl FnOnce() -> Result<String, String> + std::panic::UnwindSafe,
) -> String {
    std::panic::catch_unwind(go)
        .unwrap_or_else(|panic| {
            Err(format!(
                "the go to {} panicked: {}",
                short(moment),
                crate::panic_words(panic.as_ref())
            ))
        })
        .unwrap_or_else(|why| why)
}

/// Say `go` to the wall on `socket` for `moment`, and answer in words what
/// happened, for the footer.
///
/// # Errors
///
/// In words, when the wall cannot be reached, refuses, or does not answer.
pub fn go(socket: &Path, moment: &str) -> Result<String, String> {
    let shown = short(moment);
    let mut wall = WallClient::connect(socket, SEAT, CLIENT, true).map_err(|err| {
        format!("the wall could not be reached to go to {shown}: {err}; is it playing?")
    })?;
    wall.set_timeout(Some(WAIT))
        .map_err(|err| format!("the wall's socket could not be set up: {err}"))?;
    let value = wall
        .call(Request::Go {
            moment: moment.to_owned(),
        })
        .map_err(|err| match err {
            ClientError::Wall(_) => format!("the wall would not go to {shown}: {err}"),
            _ if err.is_timeout() => format!(
                "the wall did not answer the go to {shown} within {} s",
                WAIT.as_secs()
            ),
            _ => format!("the go to {shown} failed: {err}"),
        })?;
    let done: LoadResult = serde_json::from_value(value).map_err(|err| {
        format!("the wall went to {shown}, but its answer was not understood: {err}")
    })?;
    let mut words = format!(
        "the wall is at {shown} (change {}; the wall as it was is saved as {})",
        done.change.seq, done.backup
    );
    for note in &done.notes {
        words.push_str("; ");
        words.push_str(note);
    }
    Ok(words)
}

#[cfg(test)]
mod tests;
