//! The walls a seat can reach and the ones it has joined (KZ-N02).
//!
//! The server starts on no wall and with no name. `wall_join` puts it on a
//! wall as a name of the model's choosing: that starts the wall's link, its
//! event line ([`link::follow`]) and its notifier ([`channel::notify`]).
//! `wall_leave` stops them. Every other tool asks the wall it names, or the
//! only one joined.

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

/// A wall joined: its link, and the event line and notifier running for it.
struct Joined {
    seat: String,
    wall: Arc<Wall>,
    feed: mpsc::UnboundedSender<Feed>,
    follower: JoinHandle<()>,
    notifier: JoinHandle<()>,
}

impl Drop for Joined {
    fn drop(&mut self) {
        self.follower.abort();
        self.notifier.abort();
    }
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
    pub async fn join(&self, wall: Option<&str>, seat: &str) -> Result<Joining, String> {
        self.join_with(wall, seat, true)
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
        let (_, heard) = self.join_with(wall, seat, false).await?;
        heard.ok_or_else(|| "the join kept its feed".to_string())
    }

    /// Join, with the event line and notifier when `follow`, or else
    /// handing back the feed they would have read.
    async fn join_with(
        &self,
        wall: Option<&str>,
        seat: &str,
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
        self.not_on(name).await?;
        // The wall is asked with the lock let go, so the other tools play on.
        let link = Arc::new(Wall::new(self.socket.clone(), seat.to_string()));
        let joining = match link::peek(&self.socket).await {
            Ok(hello) => {
                if let Some(taken) = hello
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
            Err(LinkError::NotRunning) => self.waiting(name),
            Err(err) => return Err(err.to_string()),
        };
        let mut joined = self.joined.lock().await;
        // Joined meanwhile, by a call alongside this one.
        if let Some(there) = joined.get(name) {
            let refused = already(name, &there.seat);
            drop(joined);
            link.close().await;
            return Err(refused);
        }
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
            (follower, notifier, None)
        } else {
            (tokio::spawn(async {}), tokio::spawn(async {}), Some(heard))
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
    pub async fn leave(&self, wall: Option<&str>) -> Result<(String, String), String> {
        let mut joined = self.joined.lock().await;
        let name = Self::pick(&joined, wall)?;
        let left = joined
            .remove(&name)
            .ok_or_else(|| format!("this seat is not on {name}"))?;
        drop(joined);
        // Its tasks stop as it drops; its connections close here, so the
        // wall has seen the seat go by the time this answers.
        left.wall.close().await;
        Ok((name, left.seat.clone()))
    }

    /// Leave every wall, as the seat stops.
    pub async fn leave_all(&self) {
        let left = std::mem::take(&mut *self.joined.lock().await);
        for joined in left.values() {
            joined.wall.close().await;
        }
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
