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

use kazoo_wall::protocol::valid_name;
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
        if self.feed.send(Feed::Own(seq)).is_err() {
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
    const fn names() -> [&'static str; 1] {
        [HERE]
    }

    /// The walls this seat can reach, each looked at without joining it.
    pub async fn found(&self) -> Vec<Found> {
        let joined = self.joined_as().await;
        let mut found = Vec::new();
        for name in Self::names() {
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
    /// `seat`. Answers the wall's name.
    ///
    /// # Errors
    ///
    /// A sentence: a name the wall would refuse, a wall this seat cannot
    /// reach, or one it has joined already.
    pub async fn join(&self, wall: Option<&str>, seat: &str) -> Result<String, String> {
        self.join_with(wall, seat, true).await.map(|(name, _)| name)
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
    ) -> Result<(String, Option<mpsc::UnboundedReceiver<Feed>>), String> {
        if !valid_name(seat) {
            return Err(format!(
                "'{}' is not a name the wall takes: use 1 to 24 of A-Z a-z 0-9 space _ . -",
                seat.escape_default()
            ));
        }
        let name = Self::reachable(wall)?;
        let mut joined = self.joined.lock().await;
        if let Some(there) = joined.get(name) {
            return Err(format!(
                "this seat is on {name} already, as {}: wall_leave it first to come back as another name",
                there.seat
            ));
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
                wall: Arc::new(Wall::new(self.socket.clone(), seat.to_string())),
                feed,
                follower,
                notifier,
            },
        );
        drop(joined);
        Ok((name.to_string(), heard))
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
        joined
            .remove(&name)
            .map(|left| (name.clone(), left.seat.clone()))
            .ok_or_else(|| format!("this seat is not on {name}"))
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
        let [only] = names;
        wall.map_or(Ok(only), |wall| {
            names.into_iter().find(|name| *name == wall).ok_or_else(|| {
                format!(
                    "there is no wall '{}' to join; wall_walls lists them ({})",
                    wall.escape_default(),
                    names.join(", ")
                )
            })
        })
    }
}

#[cfg(test)]
mod tests;
