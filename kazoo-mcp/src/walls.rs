//! The walls a seat can reach and the ones it has joined (KZ-N02).
//!
//! The server starts on no wall and with no name. `wall_join` puts it on a
//! wall as a name of the model's choosing: that starts the wall's link, its
//! event line ([`link::follow`]) and its notifier ([`channel::notify`]).
//! `wall_leave` stops them. Every other tool asks the wall it names, or the
//! only one joined.
//!
//! Known limits: the name is checked against the seats on the wall by a
//! look just before the hello, so another seat can take it in between; and
//! a join to a wall not yet playing cannot check the name until the wall
//! starts.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use kazoo_wall::protocol::{reserved_name, valid_name};
use rmcp::{Peer, RoleServer};
use tokio::sync::{Mutex, mpsc, watch};
use tokio::task::JoinHandle;

use crate::channel::{self, Feed};
use crate::link::{self, LinkError, Wall};

/// This Mac's wall, as `wall_walls` names it.
pub const HERE: &str = "here";

/// Where [`HERE`] is, as `wall_walls` says it.
const THIS_MAC: &str = "this Mac";

/// A wall this seat can reach, as `wall_walls` lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// Its name, which every tool's `wall` takes.
    pub name: String,
    /// Where it is.
    pub place: &'static str,
    /// Whether it answered; why not, if it did not.
    pub playing: Result<(), String>,
    /// Who is on it, when it answered.
    pub seats: Vec<String>,
    /// This seat's name there, once joined.
    pub joined_as: Option<String>,
}

/// How long a leave waits to see the wall no longer list the seat.
const GONE_WITHIN: Duration = Duration::from_secs(2);

/// How often a leave looks meanwhile.
const LOOK_EVERY: Duration = Duration::from_millis(50);

/// A wall joined: its link, and the event line and notifier running for it.
struct Joined {
    seat: String,
    wall: Arc<Wall>,
    feed: mpsc::UnboundedSender<Feed>,
    follower: Option<JoinHandle<()>>,
    notifier: Option<JoinHandle<()>>,
}

impl Joined {
    /// Stop the event line and the notifier and wait until they have, so
    /// their connection is closed, then close the tools' connections.
    async fn stop(mut self) {
        for task in [self.follower.take(), self.notifier.take()]
            .into_iter()
            .flatten()
        {
            task.abort();
            // An aborted task ends cancelled, and holds nothing now; one
            // that ended otherwise is said.
            if let Err(err) = task.await
                && !err.is_cancelled()
            {
                eprintln!("kazoo-mcp: a task for the wall ended badly: {err}");
            }
        }
        self.wall.close().await;
    }
}

impl Drop for Joined {
    fn drop(&mut self) {
        for task in [self.follower.take(), self.notifier.take()]
            .into_iter()
            .flatten()
        {
            task.abort();
        }
    }
}

/// How a join was asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Asked {
    /// By `wall_join`: a name someone on the wall has is refused, and a
    /// wall that cannot be looked at refuses the join.
    ByTool,
    /// By the going `--seat` at start, which must keep playing as it did:
    /// the name is the seat's own, as ever, and a wall that cannot be
    /// looked at is joined as one not yet playing.
    AtStart,
}

/// What a leave found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Left {
    /// The wall's name.
    pub wall: String,
    /// The name this seat was on it as.
    pub seat: String,
    /// Whether the wall stopped listing the name within [`GONE_WITHIN`]
    /// (or is not playing): when it did not, another connection is on it
    /// as that name.
    pub gone: bool,
}

/// One joined wall, for one tool call: its link, and the line on which the
/// seat tells its notifier of its own changes.
#[derive(Debug, Clone)]
pub struct On {
    /// The wall's link.
    pub wall: Arc<Wall>,
    feed: mpsc::UnboundedSender<Feed>,
}

impl On {
    /// Tell the notifier this seat made change `seq`, so the channel's
    /// `seq` is never behind this seat's own work.
    pub fn own(&self, seq: u64) {
        // A seat that has left has no notifier to tell, and nothing to say.
        if self.feed.send(Feed::Own(seq)).is_err() && !self.wall.is_closed() {
            eprintln!("kazoo-mcp: the notifier has stopped; change #{seq} was not noted");
        }
    }
}

/// The walls of one seat.
pub struct Walls {
    /// This Mac's wall's control socket.
    socket: PathBuf,
    notify_every: Duration,
    peer: watch::Receiver<Option<Peer<RoleServer>>>,
    joined: Mutex<BTreeMap<String, Joined>>,
    /// Held for the whole of a join or a leave, so two never cross.
    joining: Mutex<()>,
}

impl std::fmt::Debug for Walls {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Walls")
            .field("socket", &self.socket)
            .field("notify_every", &self.notify_every)
            .finish_non_exhaustive()
    }
}

impl Walls {
    /// No wall joined yet. This Mac's wall is at `socket`; news comes at
    /// most every `notify_every`, to the session handed over on `peer`.
    #[must_use]
    pub fn new(
        socket: PathBuf,
        notify_every: Duration,
        peer: watch::Receiver<Option<Peer<RoleServer>>>,
    ) -> Self {
        Self {
            socket,
            notify_every,
            peer,
            joined: Mutex::new(BTreeMap::new()),
            joining: Mutex::new(()),
        }
    }

    /// The names of the walls this seat can reach.
    const fn names() -> &'static [&'static str] {
        &[HERE]
    }

    /// The walls this seat can reach, each looked at without joining it.
    pub async fn found(&self) -> Vec<Found> {
        let joined = self.joined_as().await;
        let mut found = Vec::new();
        for &name in Self::names() {
            let (playing, seats) = match link::peek(&self.socket).await {
                Ok(hello) => (Ok(()), hello.seats),
                Err(LinkError::NotRunning) => (
                    Err(format!("not playing at {}", self.socket.display())),
                    Vec::new(),
                ),
                Err(err) => (Err(err.to_string()), Vec::new()),
            };
            found.push(Found {
                name: name.to_string(),
                place: THIS_MAC,
                playing,
                seats,
                joined_as: joined.get(name).cloned(),
            });
        }
        found
    }

    /// Each joined wall's name, with this seat's name there.
    async fn joined_as(&self) -> BTreeMap<String, String> {
        self.joined
            .lock()
            .await
            .iter()
            .map(|(wall, joined)| (wall.clone(), joined.seat.clone()))
            .collect()
    }

    /// Join `wall` (the only one this seat can reach, when `None`) as
    /// `seat`.
    ///
    /// # Errors
    ///
    /// A sentence: a name the wall would refuse or someone on it has, a
    /// wall this seat cannot reach, or one it has joined already.
    pub async fn join(
        &self,
        wall: Option<&str>,
        seat: &str,
        asked: Asked,
    ) -> Result<Joining, String> {
        self.join_with(wall, seat, asked, true)
            .await
            .map(|(joining, _)| joining)
    }

    /// [`Self::join`] with no event line or notifier: what the seat tells
    /// its notifier comes out here instead, for a test to read.
    #[cfg(test)]
    pub(crate) async fn join_heard(
        &self,
        wall: Option<&str>,
        seat: &str,
    ) -> Result<mpsc::UnboundedReceiver<Feed>, String> {
        let (_, heard) = self.join_with(wall, seat, Asked::ByTool, false).await?;
        heard.ok_or_else(|| "the join kept its feed".to_string())
    }

    /// Join, with the event line and notifier when `follow`, or else
    /// handing back the feed they would have read.
    async fn join_with(
        &self,
        wall: Option<&str>,
        seat: &str,
        asked: Asked,
        follow: bool,
    ) -> Result<(Joining, Option<mpsc::UnboundedReceiver<Feed>>), String> {
        if !valid_name(seat) {
            return Err(format!(
                "'{}' is not a name the wall takes: use 1 to 24 of A-Z a-z 0-9 space _ . -",
                seat.escape_default()
            ));
        }
        if reserved_name(seat) {
            return Err(format!(
                "'{seat}' is kept for the wall's own use: choose another name"
            ));
        }
        let name = Self::reachable(wall)?;
        // One join or leave at a time; the tools, which take only `joined`,
        // play on meanwhile.
        let turn = self.joining.lock().await;
        self.not_on(name).await?;
        let link = Arc::new(Wall::new(self.socket.clone(), seat.to_string()));
        let joining = match (link::peek(&self.socket).await, asked) {
            (Ok(hello), _) => {
                if asked == Asked::ByTool
                    && let Some(taken) = hello
                        .seats
                        .iter()
                        .find(|other| other.eq_ignore_ascii_case(seat))
                {
                    return Err(format!(
                        "someone is on {name} as {taken} already: choose another name"
                    ));
                }
                match link.hello().await {
                    Ok(hello) => Joining::Playing {
                        wall: name.to_string(),
                        seats: hello.seats,
                        revision: hello.revision,
                    },
                    Err(LinkError::NotRunning) => self.waiting(name),
                    Err(err) => return Err(err.to_string()),
                }
            }
            (Err(LinkError::NotRunning), _) => self.waiting(name),
            (Err(err), Asked::AtStart) => {
                eprintln!(
                    "kazoo-mcp: {name} could not be looked at ({err}); joining it as one not yet playing"
                );
                self.waiting(name)
            }
            (Err(err), Asked::ByTool) => return Err(err.to_string()),
        };
        let mut joined = self.joined.lock().await;
        let (feed, heard) = mpsc::unbounded_channel();
        let (follower, notifier, heard) = if follow {
            let follower = tokio::spawn(link::follow(
                self.socket.clone(),
                seat.to_string(),
                feed.clone(),
            ));
            let notifier = tokio::spawn(channel::notify(
                (seat.to_string(), name.to_string()),
                self.notify_every,
                heard,
                self.peer.clone(),
            ));
            (Some(follower), Some(notifier), None)
        } else {
            (None, None, Some(heard))
        };
        joined.insert(
            name.to_string(),
            Joined {
                seat: seat.to_string(),
                wall: link,
                feed,
                follower,
                notifier,
            },
        );
        drop(joined);
        drop(turn);
        Ok((joining, heard))
    }

    /// Refused when this seat is on `wall` already.
    async fn not_on(&self, wall: &str) -> Result<(), String> {
        self.joined
            .lock()
            .await
            .get(wall)
            .map_or(Ok(()), |there| Err(already(wall, &there.seat)))
    }

    /// A join to `wall`, which is not playing yet.
    fn waiting(&self, wall: &str) -> Joining {
        Joining::Waiting {
            wall: wall.to_string(),
            socket: self.socket.clone(),
        }
    }

    /// Leave `wall` (the only one joined, when `None`). Answers the wall's
    /// name and the name this seat had there.
    ///
    /// # Errors
    ///
    /// A sentence: no wall joined, or not this one.
    pub async fn leave(&self, wall: Option<&str>) -> Result<Left, String> {
        let turn = self.joining.lock().await;
        let mut joined = self.joined.lock().await;
        let name = Self::pick(&joined, wall)?;
        let left = joined
            .remove(&name)
            .ok_or_else(|| format!("this seat is not on {name}"))?;
        drop(joined);
        let seat = left.seat.clone();
        // Every connection under the name closed, then the wall looked at
        // until it lists the name no more.
        left.stop().await;
        let gone = self.gone(&seat).await;
        drop(turn);
        Ok(Left {
            wall: name,
            seat,
            gone,
        })
    }

    /// Whether the wall stops listing `seat` within [`GONE_WITHIN`], or
    /// is not playing.
    async fn gone(&self, seat: &str) -> bool {
        let deadline = tokio::time::Instant::now() + GONE_WITHIN;
        loop {
            match link::peek(&self.socket).await {
                Ok(hello) if hello.seats.iter().any(|other| other == seat) => {}
                Ok(_) | Err(LinkError::NotRunning) => return true,
                Err(err) => {
                    eprintln!("kazoo-mcp: the wall could not be looked at after a leave: {err}");
                    return false;
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(LOOK_EVERY).await;
        }
    }

    /// Leave every wall, as the seat stops.
    pub async fn leave_all(&self) {
        let turn = self.joining.lock().await;
        let left = std::mem::take(&mut *self.joined.lock().await);
        for joined in left.into_values() {
            joined.stop().await;
        }
        drop(turn);
    }

    /// The joined wall a tool asks: `wall`, or the only one joined.
    ///
    /// # Errors
    ///
    /// A sentence: no wall joined, more than one and none named, or one
    /// not joined.
    pub async fn on(&self, wall: Option<&str>) -> Result<On, String> {
        let joined = self.joined.lock().await;
        let name = Self::pick(&joined, wall)?;
        joined
            .get(&name)
            .map(|joined| On {
                wall: Arc::clone(&joined.wall),
                feed: joined.feed.clone(),
            })
            .ok_or_else(|| format!("this seat is not on {name}"))
    }

    /// `wall` among the joined ones, or the only one joined.
    fn pick(joined: &BTreeMap<String, Joined>, wall: Option<&str>) -> Result<String, String> {
        let names = || joined.keys().cloned().collect::<Vec<_>>().join(", ");
        match wall {
            _ if joined.is_empty() => Err(
                "join a wall first: wall_walls lists them, and wall_join {as} joins one"
                    .to_string(),
            ),
            Some(wall) if joined.contains_key(wall) => Ok(wall.to_string()),
            Some(wall) => Err(format!(
                "this seat is not on '{}'; it is on {}",
                wall.escape_default(),
                names()
            )),
            None if joined.len() == 1 => Ok(joined.keys().next().cloned().unwrap_or_default()),
            None => Err(format!(
                "this seat is on more than one wall: say which with wall ({})",
                names()
            )),
        }
    }

    /// `wall` among the walls this seat can reach, or the only one.
    fn reachable(wall: Option<&str>) -> Result<&'static str, String> {
        let names = Self::names();
        let listed = || names.join(", ");
        match (wall, names) {
            (Some(wall), _) => names
                .iter()
                .copied()
                .find(|name| *name == wall)
                .ok_or_else(|| {
                    format!(
                        "there is no wall '{}' to join; wall_walls lists them ({})",
                        wall.escape_default(),
                        listed()
                    )
                }),
            (None, []) => Err("there is no wall this seat can reach".to_string()),
            (None, [only]) => Ok(only),
            (None, _) => Err(format!(
                "there is more than one wall: say which with wall ({})",
                listed()
            )),
        }
    }
}

/// A join refused because this seat is on `wall` as `seat` already.
fn already(wall: &str, seat: &str) -> String {
    format!(
        "this seat is on {wall} already, as {seat}: wall_leave it first to come back as another name"
    )
}

/// What a join found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Joining {
    /// The wall answered.
    Playing {
        /// The wall's name.
        wall: String,
        /// Who is on it now, this seat included.
        seats: Vec<String>,
        /// Its latest change.
        revision: u64,
    },
    /// The wall is not playing yet: the seat is on it as it starts.
    Waiting {
        /// The wall's name.
        wall: String,
        /// Where it is looked for.
        socket: PathBuf,
    },
}

#[cfg(test)]
mod tests;
