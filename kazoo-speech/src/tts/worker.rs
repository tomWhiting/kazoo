//! A thread that renders speech so its caller never waits on `say`.

use std::fmt;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError, TrySendError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use super::{Render, RenderRequest, SpeechError, Tts};

/// Requests that may wait for the worker at once.
const QUEUE: usize = 16;

/// One render the worker has finished.
#[derive(Debug)]
pub struct Finished {
    /// The ticket [`TtsWorker::submit`] gave for it.
    pub ticket: u64,
    /// What was asked for.
    pub request: RenderRequest,
    /// The phrase, or why there is none.
    pub result: Result<Render, SpeechError>,
}

struct Job {
    ticket: u64,
    request: RenderRequest,
}

/// Runs a [`Tts`] on a thread of its own. Requests go in with
/// [`Self::submit`], which never blocks; finished renders come out of
/// [`Self::try_finished`] in the order they were asked for.
///
/// Dropping the worker lets the thread finish the render it is on and
/// exit; [`Self::shutdown`] waits for it.
pub struct TtsWorker {
    jobs: SyncSender<Job>,
    finished: Receiver<Finished>,
    thread: JoinHandle<()>,
    next_ticket: u64,
}

impl fmt::Debug for TtsWorker {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TtsWorker")
            .field("next_ticket", &self.next_ticket)
            .finish_non_exhaustive()
    }
}

impl TtsWorker {
    /// Start a thread that renders with `tts`.
    pub fn spawn(mut tts: Tts) -> Result<Self, SpeechError> {
        let (jobs, inbox) = mpsc::sync_channel::<Job>(QUEUE);
        let (outbox, finished) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("kazoo-speech".to_string())
            .spawn(move || {
                for Job { ticket, request } in inbox {
                    let result = tts.render(&request);
                    let done = Finished {
                        ticket,
                        request,
                        result,
                    };
                    if outbox.send(done).is_err() {
                        // Nobody is listening any more.
                        break;
                    }
                }
            })
            .map_err(|source| SpeechError::Io {
                doing: "starting the speech thread",
                source,
            })?;
        Ok(Self {
            jobs,
            finished,
            thread,
            next_ticket: 1,
        })
    }

    /// Queue `request`; returns its ticket. Never blocks: a full queue is
    /// [`SpeechError::Busy`].
    pub fn submit(&mut self, request: RenderRequest) -> Result<u64, SpeechError> {
        let ticket = self.next_ticket;
        match self.jobs.try_send(Job { ticket, request }) {
            Ok(()) => {
                self.next_ticket += 1;
                Ok(ticket)
            }
            Err(TrySendError::Full(_)) => Err(SpeechError::Busy),
            Err(TrySendError::Disconnected(_)) => Err(SpeechError::WorkerGone),
        }
    }

    /// The next finished render, if one is ready.
    pub fn try_finished(&self) -> Result<Option<Finished>, SpeechError> {
        match self.finished.try_recv() {
            Ok(done) => Ok(Some(done)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(SpeechError::WorkerGone),
        }
    }

    /// The next finished render, waiting up to `timeout` for one.
    pub fn wait_finished(&self, timeout: Duration) -> Result<Option<Finished>, SpeechError> {
        match self.finished.recv_timeout(timeout) {
            Ok(done) => Ok(Some(done)),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => Err(SpeechError::WorkerGone),
        }
    }

    /// Stop taking requests, let the thread finish what is queued, and wait
    /// for it.
    pub fn shutdown(self) -> Result<(), SpeechError> {
        let Self {
            jobs,
            finished,
            thread,
            ..
        } = self;
        drop(jobs);
        // Drain as it goes, so the thread is never held up sending.
        for done in &finished {
            drop(done);
        }
        thread.join().map_err(|_| SpeechError::WorkerGone)
    }
}
