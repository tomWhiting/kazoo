//! The console's two connections to the daemon, each on its own thread so
//! blocking socket I/O never holds up drawing.
//!
//! - The **worker** makes requests: the console's [`Job`]s, and a `look`
//!   every [`Config::poll`] for knob motion and meters.
//! - The **feed** holds a subscription and forwards the daemon's events the
//!   moment they arrive, so other seats' changes reach the log at once.
//!
//! Both reconnect on their own when the daemon goes away (see
//! [`Reconnect`]) and report what they are doing as [`Reply`]s. Dropping
//! the [`Connection`] stops and joins both.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use kazoo_wall::daemon;
use kazoo_wall::protocol::client::{ClientError, Subscription, WallClient};
use kazoo_wall::protocol::{ErrorCode, Event, Request, Snapshot};

use super::link::{LinkState, Reconnect};

/// Longest the worker waits for one answer before taking the connection
/// as lost.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(5);

/// How often the feed looks up from waiting for events to see whether it
/// should stop.
const FEED_TICK: Duration = Duration::from_millis(200);

/// How long a start of the daemon may take before it is reported as
/// failed.
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(10);

/// Where and who the console is.
#[derive(Debug, Clone)]
pub struct Config {
    /// The daemon's socket.
    pub socket: PathBuf,
    /// The seat the console speaks as.
    pub seat: String,
    /// The console software, as it says hello.
    pub client: String,
    /// Whether to say hello as a console (which may stop the daemon).
    pub console: bool,
    /// How often to `look`.
    pub poll: Duration,
    /// How to start the daemon: this program, and the file its output goes
    /// to. `None` when the console cannot start one.
    pub launch: Option<(PathBuf, PathBuf)>,
}

/// Something for the worker to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Job {
    /// Send this request and report its answer.
    Call(Request),
    /// Start the daemon, detached, and report how it went.
    Launch,
}

/// Why a request got no result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    /// The wall's error code, when the wall answered with one.
    pub code: Option<ErrorCode>,
    /// A sentence for people.
    pub message: String,
}

impl Failure {
    fn from_client(err: &ClientError) -> Self {
        Self {
            code: err.wall_error().map(|wall| wall.code),
            message: err.to_string(),
        }
    }
}

/// What the connections report.
#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    /// The worker's link changed.
    Link(LinkState),
    /// The wall as it is now.
    Snapshot(Box<Snapshot>),
    /// A request's answer: the result, or why not.
    Answer {
        /// The request.
        request: Request,
        /// Its result.
        outcome: Result<serde_json::Value, Failure>,
    },
    /// How starting the daemon went.
    Launched(Result<(), String>),
    /// The feed's link changed.
    Feed(LinkState),
    /// An event from the daemon.
    Event(Event),
}

/// The running connections.
#[derive(Debug)]
pub struct Connection {
    jobs: Option<Sender<Job>>,
    replies: Receiver<Reply>,
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

impl Connection {
    /// Start the worker and the feed.
    ///
    /// # Errors
    ///
    /// Fails if a thread cannot start.
    pub fn start(config: &Config) -> std::io::Result<Self> {
        let (jobs, job_rx) = mpsc::channel();
        let (reply_tx, replies) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let mut connection = Self {
            jobs: Some(jobs),
            replies,
            stop: Arc::clone(&stop),
            threads: Vec::with_capacity(2),
        };
        let worker = {
            let config = config.clone();
            let replies = reply_tx.clone();
            let stop = Arc::clone(&stop);
            thread::Builder::new()
                .name("kazoo-wall-console".to_string())
                .spawn(move || run_worker(&config, &job_rx, &replies, &stop))?
        };
        connection.threads.push(worker);
        let feed = {
            let config = config.clone();
            thread::Builder::new()
                .name("kazoo-wall-feed".to_string())
                .spawn(move || run_feed(&config, &reply_tx, &stop))?
        };
        connection.threads.push(feed);
        Ok(connection)
    }

    /// Hand the worker a job.
    ///
    /// # Errors
    ///
    /// Fails if the worker has stopped.
    pub fn send(&self, job: Job) -> Result<(), String> {
        self.jobs
            .as_ref()
            .ok_or_else(|| "the console's connection is closed".to_string())?
            .send(job)
            .map_err(|_| "the console's connection thread has stopped".to_string())
    }

    /// A reply, if one is waiting.
    ///
    /// # Errors
    ///
    /// Fails if both threads have stopped.
    pub fn try_reply(&self) -> Result<Option<Reply>, String> {
        match self.replies.try_recv() {
            Ok(reply) => Ok(Some(reply)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => {
                Err("the console's connection threads have stopped".to_string())
            }
        }
    }

    /// Wait up to `timeout` for a reply (tests drive the console with
    /// this; the console itself never waits on the connections).
    ///
    /// # Errors
    ///
    /// Fails if both threads have stopped.
    #[cfg(test)]
    pub fn reply_within(&self, timeout: Duration) -> Result<Option<Reply>, String> {
        match self.replies.recv_timeout(timeout) {
            Ok(reply) => Ok(Some(reply)),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => {
                Err("the console's connection threads have stopped".to_string())
            }
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.jobs.take();
        for handle in self.threads.drain(..) {
            let name = handle.thread().name().unwrap_or("console").to_string();
            if handle.join().is_err() {
                eprintln!("kazoo-wall: the {name} thread panicked");
            }
        }
    }
}

/// Whether a failed request left the connection unusable (as opposed to
/// the wall refusing the request).
const fn broken(err: &ClientError) -> bool {
    !matches!(err, ClientError::Wall(_))
}

fn connect(config: &Config) -> Result<WallClient, ClientError> {
    let client = WallClient::connect(&config.socket, &config.seat, &config.client, config.console)?;
    client.set_timeout(Some(ANSWER_TIMEOUT))?;
    Ok(client)
}

/// The worker: requests and the `look` poll, reconnecting as needed.
fn run_worker(config: &Config, jobs: &Receiver<Job>, replies: &Sender<Reply>, stop: &AtomicBool) {
    let mut link = Reconnect::new(Instant::now());
    let mut client: Option<WallClient> = None;
    let mut next_look = Instant::now();
    if replies.send(Reply::Link(link.state().clone())).is_err() {
        return;
    }
    while !stop.load(Ordering::Acquire) {
        let now = Instant::now();
        if client.is_none() && link.due(now) {
            match connect(config) {
                Ok(connected) => {
                    link.connected(connected.hello().daemon.clone());
                    client = Some(connected);
                    next_look = now;
                }
                Err(err) => link.failed(err.to_string(), now),
            }
            if replies.send(Reply::Link(link.state().clone())).is_err() {
                return;
            }
        }
        let wait = if client.is_some() {
            next_look.saturating_duration_since(now)
        } else {
            link.wait(now)
        };
        let reply = match jobs.recv_timeout(wait) {
            Ok(Job::Call(request)) => call(&mut client, &mut link, request),
            Ok(Job::Launch) => launch(config, &mut link),
            Err(RecvTimeoutError::Timeout) => {
                next_look = Instant::now() + config.poll;
                look(&mut client, &mut link)
            }
            Err(RecvTimeoutError::Disconnected) => return,
        };
        for reply in reply {
            if replies.send(reply).is_err() {
                return;
            }
        }
    }
}

/// Answer one request, noting a broken link.
fn call(client: &mut Option<WallClient>, link: &mut Reconnect, request: Request) -> Vec<Reply> {
    let Some(connected) = client.as_mut() else {
        let message = match link.state() {
            LinkState::Down { reason, .. } => {
                format!("the wall is not answering ({reason}); nothing was sent")
            }
            LinkState::Connecting | LinkState::Connected { .. } => {
                "the console is still connecting to the wall; nothing was sent".to_string()
            }
        };
        return vec![Reply::Answer {
            request,
            outcome: Err(Failure {
                code: None,
                message,
            }),
        }];
    };
    match connected.call(request.clone()) {
        Ok(value) => vec![Reply::Answer {
            request,
            outcome: Ok(value),
        }],
        Err(err) => {
            let mut replies = vec![Reply::Answer {
                request,
                outcome: Err(Failure::from_client(&err)),
            }];
            if broken(&err) {
                *client = None;
                link.failed(err.to_string(), Instant::now());
                replies.push(Reply::Link(link.state().clone()));
            }
            replies
        }
    }
}

/// Look at the wall, noting a broken link.
fn look(client: &mut Option<WallClient>, link: &mut Reconnect) -> Vec<Reply> {
    let Some(connected) = client.as_mut() else {
        return Vec::new();
    };
    match connected.look() {
        Ok(snapshot) => vec![Reply::Snapshot(Box::new(snapshot))],
        Err(err) if broken(&err) => {
            *client = None;
            link.failed(err.to_string(), Instant::now());
            vec![Reply::Link(link.state().clone())]
        }
        Err(err) => vec![Reply::Answer {
            request: Request::Look,
            outcome: Err(Failure::from_client(&err)),
        }],
    }
}

/// Start the daemon and try to connect straight after.
fn launch(config: &Config, link: &mut Reconnect) -> Vec<Reply> {
    let Some((exe, log)) = &config.launch else {
        return vec![Reply::Launched(Err(
            "this console cannot start the wall; run `kazoo-wall serve`".to_string(),
        ))];
    };
    let result = daemon::launch_detached(exe, &config.socket, log, LAUNCH_TIMEOUT)
        .map_err(|err| format!("the wall did not start: {err}"));
    if result.is_ok() {
        link.retry_now(Instant::now());
    }
    vec![Reply::Launched(result)]
}

/// The feed: a subscription whose events are forwarded as they arrive.
fn run_feed(config: &Config, replies: &Sender<Reply>, stop: &AtomicBool) {
    let mut link = Reconnect::new(Instant::now());
    while !stop.load(Ordering::Acquire) {
        let now = Instant::now();
        if !link.due(now) {
            thread::sleep(link.wait(now).min(FEED_TICK));
            continue;
        }
        let opened = Subscription::open(&config.socket, &config.seat, &config.client, false)
            .and_then(|(subscription, _)| {
                subscription.set_timeout(Some(FEED_TICK))?;
                Ok(subscription)
            });
        let mut subscription = match opened {
            Ok(subscription) => subscription,
            Err(err) => {
                link.failed(err.to_string(), now);
                if replies.send(Reply::Feed(link.state().clone())).is_err() {
                    return;
                }
                continue;
            }
        };
        link.connected(config.client.clone());
        if replies.send(Reply::Feed(link.state().clone())).is_err() {
            return;
        }
        while !stop.load(Ordering::Acquire) {
            match subscription.next_event() {
                Ok(event) => {
                    if replies.send(Reply::Event(event)).is_err() {
                        return;
                    }
                }
                Err(err) => {
                    if err.is_timeout() {
                        // No event yet: look up to see whether to stop.
                        continue;
                    }
                    link.failed(err.to_string(), Instant::now());
                    if replies.send(Reply::Feed(link.state().clone())).is_err() {
                        return;
                    }
                    break;
                }
            }
        }
    }
}
