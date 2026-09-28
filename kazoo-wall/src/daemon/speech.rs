//! Words for the `speak` modules: rendering them off the audio thread and
//! handing them to the players.
//!
//! A `speak` request is checked at once and queued on a [`Renderer`] (the
//! [`TtsWorker`], which runs macOS `say` on a thread of its own). When the
//! render is ready the phrase goes to the module's player through its
//! [`PhraseFeed`]; the old phrase comes back the same way and is freed
//! here, never on the audio thread. The phrase is kept too, so a rebuilt
//! engine gets it back at once, at whatever rate it now runs (the player
//! resamples).
//!
//! Saved words are *put back* (rendered again with nobody waiting) when a
//! speaker has no phrase: after the daemon starts, or when a removed
//! speaker is brought back. A put-back never overrides words asked for
//! after it, and put-backs that find the render queue full wait their turn
//! here rather than being lost.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::path::PathBuf;

use kazoo_speech::tts::{Finished, MAX_SAMPLE_RATE, MIN_SAMPLE_RATE, check_voice, clean_text};
use kazoo_speech::{Phrase, PhraseFeed, RenderRequest, SpeechError, Tts, TtsConfig, TtsWorker};

use crate::patch::Words;
use crate::protocol::{ErrorCode, WallError};

/// Cache warnings remembered, so each is told once.
const MAX_WARNINGS: usize = 64;

/// A seat's words are refused while this many renders are in flight.
///
/// So they never wait behind more than one: each can take up to 30 s, and a
/// seat waits 75 s for its answer. Saved words go back one at a time, and
/// only while nothing else is rendering.
pub const MAX_RENDERS_AHEAD: usize = 2;

/// What renders words: the speech thread, or a stand-in in tests.
pub trait Renderer: Send + fmt::Debug {
    /// Queue `request`; returns the renderer's own ticket for it. Never
    /// blocks: a full queue is [`SpeechError::Busy`].
    ///
    /// # Errors
    ///
    /// `Busy`, or `WorkerGone` when the renderer has stopped.
    fn submit(&mut self, request: RenderRequest) -> Result<u64, SpeechError>;

    /// The next finished render, if one is ready, in the order asked for.
    ///
    /// # Errors
    ///
    /// `WorkerGone` when the renderer has stopped.
    fn try_finished(&mut self) -> Result<Option<Finished>, SpeechError>;
}

impl Renderer for TtsWorker {
    fn submit(&mut self, request: RenderRequest) -> Result<u64, SpeechError> {
        Self::submit(self, request)
    }

    fn try_finished(&mut self) -> Result<Option<Finished>, SpeechError> {
        Self::try_finished(self)
    }
}

/// A render the daemon is waiting for.
#[derive(Debug)]
struct Pending {
    /// The ticket [`Speech::submit`] gave.
    ticket: u64,
    module: String,
    words: Words,
    /// Who asked, or nobody when the daemon is putting saved words back.
    seat: Option<String>,
}

/// Saved words waiting for room in the render queue.
#[derive(Debug)]
struct PutBack {
    module: String,
    words: Words,
    sample_rate: u32,
}

/// A render that finished, and where its phrase went.
#[derive(Debug)]
pub struct Spoken {
    /// The ticket [`Speech::submit`] gave.
    pub ticket: u64,
    /// The module it was for.
    pub module: String,
    /// The words.
    pub words: Words,
    /// Who asked, or nobody for saved words put back.
    pub seat: Option<String>,
    /// How long the phrase lasts, in seconds, or why there is none.
    pub result: Result<f64, WallError>,
}

/// Starts a renderer, with the cache directory to keep renders in.
pub type Starter = Box<dyn FnMut(PathBuf) -> Result<Box<dyn Renderer>, WallError> + Send>;

/// Every `speak` module's feed and phrase, and the renders in flight.
pub struct Speech {
    cache_dir: PathBuf,
    /// Starts a renderer when one is first needed (and again after one
    /// stops).
    start: Starter,
    worker: Option<Box<dyn Renderer>>,
    feeds: HashMap<String, PhraseFeed>,
    phrases: HashMap<String, Phrase>,
    /// Renders in flight, by the renderer's ticket.
    pending: HashMap<u64, Pending>,
    /// The last ticket given.
    tickets: u64,
    /// The latest ticket given for each module's words.
    latest: HashMap<String, u64>,
    /// Put-backs that found the render queue full, oldest first.
    backlog: VecDeque<PutBack>,
    /// Renders that failed before reaching the renderer's queue (it had
    /// stopped), to report with the next [`Speech::finished`].
    failed: Vec<Spoken>,
    /// Cache warnings already told.
    warned: HashSet<String>,
}

impl fmt::Debug for Speech {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Speech")
            .field("cache_dir", &self.cache_dir)
            .field("worker", &self.worker)
            .field("feeds", &self.feeds.len())
            .field("pending", &self.pending.len())
            .field("backlog", &self.backlog.len())
            .finish_non_exhaustive()
    }
}

/// The speech thread, running `say` with its cache in `cache_dir`.
fn start_worker(cache_dir: PathBuf) -> Result<Box<dyn Renderer>, WallError> {
    let tts = Tts::new(TtsConfig::with_cache_dir(cache_dir)).map_err(|err| speech_error(&err))?;
    if !tts.available() {
        return Err(WallError::new(
            ErrorCode::Internal,
            format!(
                "this machine cannot speak: there is no {}",
                kazoo_speech::tts::SAY_PATH
            ),
        ));
    }
    let worker = TtsWorker::spawn(tts).map_err(|err| speech_error(&err))?;
    Ok(Box::new(worker))
}

impl Speech {
    /// Speech that keeps its render cache in `cache_dir`. Nothing starts
    /// until the first render.
    #[must_use]
    pub fn new(cache_dir: PathBuf) -> Self {
        Self::with_renderer(cache_dir, Box::new(start_worker))
    }

    /// Speech that renders with what `start` gives.
    #[must_use]
    pub fn with_renderer(cache_dir: PathBuf, start: Starter) -> Self {
        Self {
            cache_dir,
            start,
            worker: None,
            feeds: HashMap::new(),
            phrases: HashMap::new(),
            pending: HashMap::new(),
            tickets: 0,
            latest: HashMap::new(),
            backlog: VecDeque::new(),
            failed: Vec::new(),
            warned: HashSet::new(),
        }
    }

    /// A `speak` module went into the engine with `feed`. Its phrase, if it
    /// has one, goes straight back, whatever rate it was made at (the
    /// player resamples). Returns whether it still needs one (its words
    /// must be rendered again).
    pub fn attach(&mut self, module: &str, mut feed: PhraseFeed) -> bool {
        let phrase = self.phrases.get(module).cloned();
        let needs = phrase.is_none_or(|phrase| feed.load(phrase, false).is_err());
        self.feeds.insert(module.to_string(), feed);
        needs
    }

    /// A module was taken away: forget its feed and phrase, and any saved
    /// words waiting to go back to it.
    pub fn forget(&mut self, module: &str) {
        self.feeds.remove(module);
        self.phrases.remove(module);
        self.backlog.retain(|waiting| waiting.module != module);
    }

    /// Queue a render of `words` for `module` at `sample_rate`, for `seat`.
    /// Returns the ticket its [`Spoken`] will carry.
    ///
    /// # Errors
    ///
    /// `bad_request` for words or a voice `say` cannot take; `slow_down`
    /// when renders are queued up; `internal` when this machine cannot
    /// speak.
    pub fn submit(
        &mut self,
        module: &str,
        words: Words,
        sample_rate: u32,
        seat: Option<String>,
    ) -> Result<u64, WallError> {
        clean_text(&words.text).map_err(|err| speech_error(&err))?;
        if let Some(voice) = &words.voice {
            check_voice(voice).map_err(|err| speech_error(&err))?;
        }
        if seat.is_some() && self.pending.len() >= MAX_RENDERS_AHEAD {
            return Err(WallError::new(
                ErrorCode::SlowDown,
                format!(
                    "{} renders are ahead of these words; try again in a moment",
                    self.pending.len()
                ),
            ));
        }
        // `say` renders at 8 to 192 kHz; the player resamples to the
        // engine's rate from there.
        let mut request = RenderRequest::new(
            words.text.clone(),
            sample_rate.clamp(MIN_SAMPLE_RATE, MAX_SAMPLE_RATE),
        );
        if let Some(voice) = &words.voice {
            request = request.with_voice(voice.clone());
        }
        if self.worker.is_none() {
            self.worker = Some((self.start)(self.cache_dir.clone())?);
        }
        let submitted = self
            .worker
            .as_mut()
            .map_or(Err(SpeechError::WorkerGone), |worker| {
                worker.submit(request)
            });
        let queued = match submitted {
            Ok(queued) => queued,
            Err(SpeechError::WorkerGone) => {
                // Nothing queued on it will come back: start afresh next
                // time.
                self.lose_worker(&speech_error(&SpeechError::WorkerGone));
                return Err(speech_error(&SpeechError::WorkerGone));
            }
            Err(err) => return Err(speech_error(&err)),
        };
        self.tickets += 1;
        let ticket = self.tickets;
        self.latest.insert(module.to_string(), ticket);
        self.pending.insert(
            queued,
            Pending {
                ticket,
                module: module.to_string(),
                words,
                seat,
            },
        );
        Ok(ticket)
    }

    /// Put `module`'s saved `words` back at `sample_rate`, with nobody
    /// waiting: now if the render queue has room, or as soon as it does.
    /// Nothing is queued while words for the module are already on their
    /// way (they are at least as new).
    pub fn put_back(&mut self, module: &str, words: Words, sample_rate: u32) {
        if self
            .pending
            .values()
            .any(|pending| pending.module == module)
        {
            return;
        }
        self.backlog.retain(|waiting| waiting.module != module);
        self.backlog.push_back(PutBack {
            module: module.to_string(),
            words,
            sample_rate,
        });
        self.feed_backlog();
    }

    /// Move the next waiting put-back into the render queue, if nothing is
    /// rendering (a seat's words never wait behind more than one render).
    fn feed_backlog(&mut self) {
        while self.pending.is_empty() {
            let Some(waiting) = self.backlog.pop_front() else {
                return;
            };
            if !self.feeds.contains_key(&waiting.module) {
                // Taken away while it waited.
                continue;
            }
            match self.submit(
                &waiting.module,
                waiting.words.clone(),
                waiting.sample_rate,
                None,
            ) {
                Ok(_) => {}
                Err(err) if err.code == ErrorCode::SlowDown => {
                    self.backlog.push_front(waiting);
                    return;
                }
                Err(err) => eprintln!(
                    "kazoo-wall: {}'s words cannot be put back: {}",
                    waiting.module, err.message
                ),
            }
        }
    }

    /// Every render in flight fails with `why`, and the renderer is let go.
    fn lose_worker(&mut self, why: &WallError) {
        self.worker = None;
        let mut lost: Vec<Pending> = self.pending.drain().map(|(_, pending)| pending).collect();
        lost.sort_by_key(|pending| pending.ticket);
        for pending in lost {
            self.failed.push(Spoken {
                ticket: pending.ticket,
                module: pending.module,
                words: pending.words,
                seat: pending.seat,
                result: Err(why.clone()),
            });
        }
    }

    /// Renders that have finished since the last call: each phrase is
    /// handed to its module (if it is still on the wall). Also frees the
    /// phrases the players are done with, and queues put-backs that were
    /// waiting for room.
    pub fn finished(&mut self) -> Vec<Spoken> {
        for feed in self.feeds.values_mut() {
            feed.collect();
        }
        let mut done = std::mem::take(&mut self.failed);
        loop {
            let next = match self.worker.as_mut().map(|worker| worker.try_finished()) {
                None | Some(Ok(None)) => break,
                Some(Ok(Some(next))) => next,
                Some(Err(err)) => {
                    // The thread has gone: every render waiting on it fails.
                    self.lose_worker(&speech_error(&err));
                    done.append(&mut self.failed);
                    break;
                }
            };
            let Some(pending) = self.pending.remove(&next.ticket) else {
                continue;
            };
            let superseded = self.latest.get(&pending.module) != Some(&pending.ticket);
            let result = match next.result {
                // Words (saved words put back, or a seat's) never replace
                // words asked for after them.
                Ok(_) if superseded => Err(WallError::new(
                    ErrorCode::NotAllowed,
                    format!(
                        "{} was given newer words before these were ready",
                        pending.module
                    ),
                )),
                Ok(render) => {
                    if let Some(warning) = render.cache_warning {
                        self.warn(&warning);
                    }
                    self.hand_over(&pending.module, render.phrase)
                }
                Err(err) => Err(speech_error(&err)),
            };
            if !superseded {
                self.latest.remove(&pending.module);
            }
            done.push(Spoken {
                ticket: pending.ticket,
                module: pending.module,
                words: pending.words,
                seat: pending.seat,
                result,
            });
        }
        self.feed_backlog();
        done.append(&mut self.failed);
        done
    }

    /// Tell a cache warning, once for each different one.
    fn warn(&mut self, warning: &SpeechError) {
        let said = warning.to_string();
        if self.warned.len() < MAX_WARNINGS && self.warned.insert(said.clone()) {
            eprintln!("kazoo-wall: the speech cache: {said}");
        }
    }

    /// Give `phrase` to `module`'s player; returns its length in seconds.
    fn hand_over(&mut self, module: &str, phrase: Phrase) -> Result<f64, WallError> {
        let Some(feed) = self.feeds.get_mut(module) else {
            return Err(WallError::new(
                ErrorCode::UnknownModule,
                format!("{module} was taken away before its words were ready"),
            ));
        };
        let seconds = phrase.seconds();
        if feed.load(phrase.clone(), false).is_err() {
            return Err(WallError::new(
                ErrorCode::SlowDown,
                format!("{module} is not taking new words yet; try again in a moment"),
            ));
        }
        self.phrases.insert(module.to_string(), phrase);
        Ok(seconds)
    }

    /// Renders still in flight.
    #[must_use]
    pub fn in_flight(&self) -> usize {
        self.pending.len()
    }

    /// Put-backs waiting for room in the render queue.
    #[must_use]
    pub fn waiting(&self) -> usize {
        self.backlog.len()
    }

    /// The phrase `module`'s player was last given, if any.
    #[must_use]
    pub fn phrase(&self, module: &str) -> Option<&Phrase> {
        self.phrases.get(module)
    }
}

/// A speech error as the protocol says it.
fn speech_error(err: &SpeechError) -> WallError {
    let code = match err {
        SpeechError::EmptyText
        | SpeechError::TextTooLong { .. }
        | SpeechError::BadVoice(_)
        | SpeechError::UnknownVoice(_)
        | SpeechError::BadSampleRate(_)
        | SpeechError::BadWordsPerMinute(_) => ErrorCode::BadRequest,
        SpeechError::Busy => ErrorCode::SlowDown,
        SpeechError::SayMissing(_)
        | SpeechError::SayFailed { .. }
        | SpeechError::TimedOut(_)
        | SpeechError::BadRender(_)
        | SpeechError::EmptyRender
        | SpeechError::NoHome
        | SpeechError::NotAbsolute(_)
        | SpeechError::Io { .. }
        | SpeechError::WorkerGone => ErrorCode::Internal,
    };
    WallError::new(code, err.to_string())
}

#[cfg(test)]
pub mod tests;
