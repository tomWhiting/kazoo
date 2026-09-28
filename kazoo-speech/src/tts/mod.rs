//! Text to speech through macOS `say`, rendered off the audio thread.
//!
//! [`Tts::render`] turns a [`RenderRequest`] (text, an optional voice, the
//! engine's sample rate and an optional speaking pace) into a mono
//! [`Phrase`] at exactly that rate. It blocks while `say` works, so it
//! belongs on a control or worker thread; [`TtsWorker`] runs it on a thread
//! of its own and never blocks the caller.
//!
//! # Safety of the command
//!
//! `say` is run directly, never through a shell, with every argument passed
//! separately. The text never appears on the command line at all: it is
//! written to a file that `say` reads with `-f`, so text that starts with
//! `-` (or holds anything else) cannot be taken as an option. Text is held
//! to 500 characters with control characters replaced by spaces; a voice
//! must be one `say` lists. The child is killed if it runs past the
//! timeout.
//!
//! # Where it works
//!
//! Only where `/usr/bin/say` exists, which means macOS. Elsewhere the crate
//! still builds and every render fails with [`SpeechError::SayMissing`],
//! which says so plainly.
//!
//! # Cache
//!
//! Renders are kept in `~/.kazoo/wall/speech/` (made with mode 0700), keyed
//! by text, voice, sample rate and pace, up to a size cap, least recently
//! used first out.

mod cache;
mod voices;
mod worker;

use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

pub use cache::{Cache, CacheKey, Entry};
pub use voices::{Voice, parse_voices};
pub use worker::{Finished, TtsWorker};

use crate::player::Phrase;

/// Longest text a render takes, in characters.
pub const MAX_TEXT_CHARS: usize = 500;

/// Longest voice name accepted, in characters.
pub const MAX_VOICE_CHARS: usize = 64;

/// Lowest sample rate a render can be made at, in Hz.
pub const MIN_SAMPLE_RATE: u32 = 8_000;

/// Highest sample rate a render can be made at, in Hz.
pub const MAX_SAMPLE_RATE: u32 = 192_000;

/// Slowest speaking pace, in words per minute.
pub const MIN_WORDS_PER_MINUTE: u16 = 50;

/// Fastest speaking pace, in words per minute.
pub const MAX_WORDS_PER_MINUTE: u16 = 500;

/// Where macOS keeps `say`.
pub const SAY_PATH: &str = "/usr/bin/say";

/// Default cap on the render cache, in bytes.
pub const DEFAULT_CACHE_CAP: u64 = 256 * 1024 * 1024;

/// Default limit on one run of `say`.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Most output kept from one stream of a child process.
const OUTPUT_LIMIT: u64 = 1024 * 1024;

/// How often a running child is checked on.
const POLL: Duration = Duration::from_millis(5);

/// Temporary files older than this are swept away when a renderer starts.
const STALE_AFTER: Duration = Duration::from_secs(3_600);

/// Everything that can go wrong on the way from text to a phrase.
#[derive(Debug)]
pub enum SpeechError {
    /// Nothing left to say once control characters were taken out.
    EmptyText,
    /// More text than [`MAX_TEXT_CHARS`].
    TextTooLong {
        /// How many characters it had.
        chars: usize,
    },
    /// The voice name cannot be passed to `say`.
    BadVoice(String),
    /// `say` has no voice of that name.
    UnknownVoice(String),
    /// A sample rate outside [`MIN_SAMPLE_RATE`]..=[`MAX_SAMPLE_RATE`].
    BadSampleRate(u32),
    /// A pace outside [`MIN_WORDS_PER_MINUTE`]..=[`MAX_WORDS_PER_MINUTE`].
    BadWordsPerMinute(u16),
    /// `say` is not at the configured path: this is not a Mac.
    SayMissing(PathBuf),
    /// `say` ran and failed.
    SayFailed {
        /// How it exited.
        status: String,
        /// What it said on stderr, trimmed.
        stderr: String,
    },
    /// `say` ran past the timeout and was killed.
    TimedOut(Duration),
    /// `say` wrote something that is not the audio asked for.
    BadRender(String),
    /// `say` rendered no audio at all.
    EmptyRender,
    /// No home directory to keep the cache under.
    NoHome,
    /// The cache directory or `say` path is relative.
    NotAbsolute(PathBuf),
    /// A file or process operation failed.
    Io {
        /// What was being done.
        doing: &'static str,
        /// Why it failed.
        source: io::Error,
    },
    /// The worker's queue is full.
    Busy,
    /// The worker thread has gone.
    WorkerGone,
}

impl fmt::Display for SpeechError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyText => write!(formatter, "there is no text to speak"),
            Self::TextTooLong { chars } => write!(
                formatter,
                "the text is {chars} characters; the most is {MAX_TEXT_CHARS}"
            ),
            Self::BadVoice(why) => write!(formatter, "bad voice name: {why}"),
            Self::UnknownVoice(voice) => write!(
                formatter,
                "say has no voice called '{voice}' (list them with `say -v '?'`)"
            ),
            Self::BadSampleRate(rate) => write!(
                formatter,
                "sample rate {rate} Hz is outside {MIN_SAMPLE_RATE}..={MAX_SAMPLE_RATE} Hz"
            ),
            Self::BadWordsPerMinute(pace) => write!(
                formatter,
                "{pace} words a minute is outside \
                 {MIN_WORDS_PER_MINUTE}..={MAX_WORDS_PER_MINUTE}"
            ),
            Self::SayMissing(path) => write!(
                formatter,
                "text-to-speech needs macOS `say`, which is not at {} on this machine",
                path.display()
            ),
            Self::SayFailed { status, stderr } if stderr.is_empty() => {
                write!(formatter, "say failed ({status})")
            }
            Self::SayFailed { status, stderr } => {
                write!(formatter, "say failed ({status}): {stderr}")
            }
            Self::TimedOut(limit) => write!(
                formatter,
                "say took longer than {:.1} s and was stopped",
                limit.as_secs_f64()
            ),
            Self::BadRender(why) => write!(formatter, "say's output was not usable: {why}"),
            Self::EmptyRender => write!(formatter, "say rendered no audio"),
            Self::NoHome => write!(
                formatter,
                "HOME is not set, so there is nowhere to keep the speech cache"
            ),
            Self::NotAbsolute(path) => {
                write!(formatter, "{} must be an absolute path", path.display())
            }
            Self::Io { doing, source } => write!(formatter, "{doing}: {source}"),
            Self::Busy => write!(formatter, "the speech renderer is busy; try again shortly"),
            Self::WorkerGone => write!(formatter, "the speech renderer has stopped"),
        }
    }
}

impl std::error::Error for SpeechError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

fn io_error(doing: &'static str) -> impl FnOnce(io::Error) -> SpeechError {
    move |source| SpeechError::Io { doing, source }
}

/// What to render.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RenderRequest {
    /// The words.
    pub text: String,
    /// A voice `say` lists, or `None` for the system voice.
    pub voice: Option<String>,
    /// The rate to render at: the engine's rate, in Hz.
    pub sample_rate: u32,
    /// Speaking pace in words per minute, or `None` for the voice's own.
    pub words_per_minute: Option<u16>,
}

impl RenderRequest {
    /// `text` in the system voice at its own pace, at `sample_rate`.
    #[must_use]
    pub fn new(text: impl Into<String>, sample_rate: u32) -> Self {
        Self {
            text: text.into(),
            voice: None,
            sample_rate,
            words_per_minute: None,
        }
    }

    /// The same, in `voice`.
    #[must_use]
    pub fn with_voice(mut self, voice: impl Into<String>) -> Self {
        self.voice = Some(voice.into());
        self
    }

    /// The same, at `words_per_minute`.
    #[must_use]
    pub const fn with_words_per_minute(mut self, words_per_minute: u16) -> Self {
        self.words_per_minute = Some(words_per_minute);
        self
    }
}

/// `text` made safe to speak: every control character (newlines and tabs
/// included) becomes a space, and the ends are trimmed. Refuses text that
/// is empty afterwards or longer than [`MAX_TEXT_CHARS`].
pub fn clean_text(text: &str) -> Result<String, SpeechError> {
    let cleaned: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let cleaned = cleaned.trim();
    let chars = cleaned.chars().count();
    if chars == 0 {
        return Err(SpeechError::EmptyText);
    }
    if chars > MAX_TEXT_CHARS {
        return Err(SpeechError::TextTooLong { chars });
    }
    Ok(cleaned.to_string())
}

/// Check a voice name is safe to pass to `say` as an argument: not empty,
/// not too long, no control characters, and not starting with `-`.
pub fn check_voice(voice: &str) -> Result<(), SpeechError> {
    let why = if voice.trim().is_empty() {
        "it is empty"
    } else if voice.chars().count() > MAX_VOICE_CHARS {
        "it is too long"
    } else if voice.chars().any(char::is_control) {
        "it holds control characters"
    } else if voice.starts_with('-') {
        "it starts with '-'"
    } else {
        return Ok(());
    };
    Err(SpeechError::BadVoice(why.to_string()))
}

/// The arguments for one render.
///
/// `say` reads the text from `text_file` and writes 32-bit float
/// little-endian WAVE at `sample_rate` to `out_file`, optionally in `voice`
/// at `words_per_minute`. The text itself is never an argument. Both paths
/// must be absolute, so neither can look like an option.
#[must_use]
pub fn render_arguments(
    text_file: &Path,
    out_file: &Path,
    voice: Option<&str>,
    sample_rate: u32,
    words_per_minute: Option<u16>,
) -> Vec<OsString> {
    let mut arguments: Vec<OsString> = vec![
        "-o".into(),
        out_file.into(),
        "--file-format=WAVE".into(),
        format!("--data-format=LEF32@{sample_rate}").into(),
        "-f".into(),
        text_file.into(),
    ];
    if let Some(voice) = voice {
        arguments.push("-v".into());
        arguments.push(voice.into());
    }
    if let Some(pace) = words_per_minute {
        arguments.push("-r".into());
        arguments.push(pace.to_string().into());
    }
    arguments
}

/// How a render was made.
#[derive(Debug)]
pub struct Render {
    /// The words, as audio.
    pub phrase: Phrase,
    /// Whether it came from the cache rather than a fresh run of `say`.
    pub from_cache: bool,
    /// A cache problem that did not stop the render (the cache could not
    /// be read, written or trimmed), for the log.
    pub cache_warning: Option<SpeechError>,
}

/// Where and how a [`Tts`] works.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TtsConfig {
    /// The `say` binary; absolute.
    pub say_path: PathBuf,
    /// The render cache; absolute. Made with mode 0700 if missing.
    pub cache_dir: PathBuf,
    /// Most bytes the cache may hold.
    pub cache_cap_bytes: u64,
    /// Longest one run of `say` may take before it is killed.
    pub timeout: Duration,
}

impl TtsConfig {
    /// `/usr/bin/say`, a cache in `~/.kazoo/wall/speech/` of 256 MiB, a
    /// 30 s timeout.
    pub fn standard() -> Result<Self, SpeechError> {
        let home = std::env::var_os("HOME")
            .filter(|home| !home.is_empty())
            .ok_or(SpeechError::NoHome)?;
        Ok(Self::with_cache_dir(
            PathBuf::from(home)
                .join(".kazoo")
                .join("wall")
                .join("speech"),
        ))
    }

    /// The standard settings with the cache in `cache_dir`.
    #[must_use]
    pub fn with_cache_dir(cache_dir: PathBuf) -> Self {
        Self {
            say_path: PathBuf::from(SAY_PATH),
            cache_dir,
            cache_cap_bytes: DEFAULT_CACHE_CAP,
            timeout: DEFAULT_TIMEOUT,
        }
    }
}

/// Renders text to phrases: see the [module documentation](self).
#[derive(Debug)]
pub struct Tts {
    config: TtsConfig,
    cache: Cache,
    voices: Option<Vec<Voice>>,
    renders: u64,
}

impl Tts {
    /// A renderer working as `config` says. Makes the cache directory (mode
    /// 0700) and sweeps out temporary files an interrupted render left
    /// behind. Works on any system; renders fail with
    /// [`SpeechError::SayMissing`] where there is no `say`.
    pub fn new(config: TtsConfig) -> Result<Self, SpeechError> {
        for path in [&config.say_path, &config.cache_dir] {
            if !path.is_absolute() {
                return Err(SpeechError::NotAbsolute(path.clone()));
            }
        }
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&config.cache_dir)
            .map_err(io_error("making the speech cache directory"))?;
        fs::set_permissions(&config.cache_dir, fs::Permissions::from_mode(0o700))
            .map_err(io_error("making the speech cache directory private"))?;
        sweep(&config.cache_dir).map_err(io_error("clearing old temporary speech files"))?;
        let cache = Cache::new(config.cache_dir.clone(), config.cache_cap_bytes);
        Ok(Self {
            config,
            cache,
            voices: None,
            renders: 0,
        })
    }

    /// How it is set up.
    #[must_use]
    pub const fn config(&self) -> &TtsConfig {
        &self.config
    }

    /// Whether `say` is there to run.
    #[must_use]
    pub fn available(&self) -> bool {
        self.config.say_path.is_file()
    }

    /// The voices `say` offers, asked for once and remembered.
    pub fn voices(&mut self) -> Result<&[Voice], SpeechError> {
        if self.voices.is_none() {
            let listed = list_voices(&self.config.say_path, self.config.timeout)?;
            self.voices = Some(listed);
        }
        Ok(self.voices.as_deref().unwrap_or_default())
    }

    /// Render `request` to a phrase at its sample rate, from the cache if it
    /// has it. Blocks while `say` runs, at most for the timeout.
    pub fn render(&mut self, request: &RenderRequest) -> Result<Render, SpeechError> {
        let text = clean_text(&request.text)?;
        if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&request.sample_rate) {
            return Err(SpeechError::BadSampleRate(request.sample_rate));
        }
        if let Some(pace) = request.words_per_minute {
            if !(MIN_WORDS_PER_MINUTE..=MAX_WORDS_PER_MINUTE).contains(&pace) {
                return Err(SpeechError::BadWordsPerMinute(pace));
            }
        }
        let voice = match &request.voice {
            Some(voice) => {
                check_voice(voice)?;
                Some(voice.clone())
            }
            None => None,
        };
        let key_for = |voice: Option<&str>| {
            CacheKey::new(
                &text,
                voice.unwrap_or_default(),
                request.sample_rate,
                request.words_per_minute.unwrap_or(0),
            )
        };
        let mut cache_warning = None;
        let mut key = key_for(voice.as_deref());
        if let Some(render) = self.cached(&key, request.sample_rate, &mut cache_warning) {
            return Ok(render);
        }
        if !self.available() {
            return Err(SpeechError::SayMissing(self.config.say_path.clone()));
        }
        // A voice asked for in another case is filed under its listed name.
        let resolved = match &voice {
            Some(voice) => Some(self.resolve_voice(voice)?),
            None => None,
        };
        if resolved != voice {
            key = key_for(resolved.as_deref());
            if let Some(render) = self.cached(&key, request.sample_rate, &mut cache_warning) {
                return Ok(render);
            }
        }
        let samples = self.run_say(&text, resolved.as_deref(), request)?;
        if let Err(source) = self.cache.put(&key, request.sample_rate, &samples) {
            cache_warning = Some(SpeechError::Io {
                doing: "writing the speech cache",
                source,
            });
        }
        Ok(Render {
            phrase: Phrase::new(samples, request.sample_rate),
            from_cache: false,
            cache_warning,
        })
    }

    /// The cached render under `key`, if there is one. A cache that cannot
    /// be read is noted in `warning` and counts as a miss.
    fn cached(
        &self,
        key: &CacheKey,
        sample_rate: u32,
        warning: &mut Option<SpeechError>,
    ) -> Option<Render> {
        match self.cache.get(key) {
            Ok(Some(entry)) if entry.sample_rate == sample_rate => Some(Render {
                phrase: Phrase::new(entry.samples, entry.sample_rate),
                from_cache: true,
                cache_warning: None,
            }),
            Ok(_) => None,
            Err(source) => {
                *warning = Some(SpeechError::Io {
                    doing: "reading the speech cache",
                    source,
                });
                None
            }
        }
    }

    /// The listed name for `voice`: an exact match, else the only match
    /// ignoring case.
    fn resolve_voice(&mut self, voice: &str) -> Result<String, SpeechError> {
        let voices = self.voices()?;
        if let Some(found) = voices.iter().find(|known| known.name == voice) {
            return Ok(found.name.clone());
        }
        let lower = voice.to_lowercase();
        let mut matches = voices
            .iter()
            .filter(|known| known.name.to_lowercase() == lower);
        match (matches.next(), matches.next()) {
            (Some(found), None) => Ok(found.name.clone()),
            _ => Err(SpeechError::UnknownVoice(voice.to_string())),
        }
    }

    fn run_say(
        &mut self,
        text: &str,
        voice: Option<&str>,
        request: &RenderRequest,
    ) -> Result<Vec<f32>, SpeechError> {
        self.renders += 1;
        let stem = format!("tmp-{}-{}", std::process::id(), self.renders);
        let files = TempFiles {
            text: self.config.cache_dir.join(format!("{stem}.txt")),
            audio: self.config.cache_dir.join(format!("{stem}.wav")),
        };
        let result = self.run_say_with(&files, text, voice, request);
        let cleanup = files.remove();
        let samples = result?;
        cleanup.map_err(io_error("removing temporary speech files"))?;
        Ok(samples)
    }

    fn run_say_with(
        &self,
        files: &TempFiles,
        text: &str,
        voice: Option<&str>,
        request: &RenderRequest,
    ) -> Result<Vec<f32>, SpeechError> {
        fs::write(&files.text, text).map_err(io_error("writing the text for say"))?;
        let arguments = render_arguments(
            &files.text,
            &files.audio,
            voice,
            request.sample_rate,
            request.words_per_minute,
        );
        let mut command = Command::new(&self.config.say_path);
        command.args(arguments);
        let ran = run(&mut command, self.config.timeout)?;
        if !ran.status.success() {
            return Err(SpeechError::SayFailed {
                status: ran.status.to_string(),
                stderr: tidy_stderr(&ran.stderr),
            });
        }
        load_wav(&files.audio, request.sample_rate)
    }
}

/// The temporary files of one render.
struct TempFiles {
    text: PathBuf,
    audio: PathBuf,
}

impl TempFiles {
    /// Remove both; a file that was never made is fine.
    fn remove(&self) -> io::Result<()> {
        let text = remove_if_present(&self.text);
        remove_if_present(&self.audio)?;
        text
    }
}

/// Remove the file at `path`; one that is not there is fine.
fn remove_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// Remove temporary render files older than an hour from `dir`.
fn sweep(dir: &Path) -> io::Result<()> {
    let now = SystemTime::now();
    for item in fs::read_dir(dir)? {
        let item = item?;
        let name = item.file_name();
        let part = Path::new(&name)
            .extension()
            .is_some_and(|extension| extension == "part");
        let temporary = name
            .to_str()
            .is_some_and(|name| name.starts_with("tmp-") || (name.starts_with('.') && part));
        if !temporary {
            continue;
        }
        let modified = match item.metadata().and_then(|meta| meta.modified()) {
            Ok(modified) => modified,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        let stale = now
            .duration_since(modified)
            .is_ok_and(|age| age > STALE_AFTER);
        if stale {
            remove_if_present(&item.path())?;
        }
    }
    Ok(())
}

/// The first few lines of a child's stderr, for an error message.
fn tidy_stderr(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let tidy: String = text
        .trim()
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(300)
        .collect();
    tidy
}

/// What a finished child left behind.
#[derive(Debug)]
struct Ran {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

/// Run `command` with no stdin, collecting stdout and stderr (up to 1 MiB
/// each) on threads of their own so a chatty child never stalls on a full
/// pipe. Kills it if it runs past `timeout`.
fn run(command: &mut Command, timeout: Duration) -> Result<Ran, SpeechError> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(io_error("starting say"))?;
    let stdout = child.stdout.take().map(drain);
    let stderr = child.stderr.take().map(drain);
    let waited = wait(&mut child, timeout);
    let stdout = collect(stdout)?;
    let stderr = collect(stderr)?;
    Ok(Ran {
        status: waited?,
        stdout,
        stderr,
    })
}

fn wait(child: &mut Child, timeout: Duration) -> Result<ExitStatus, SpeechError> {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().map_err(io_error("waiting for say"))? {
            return Ok(status);
        }
        if started.elapsed() >= timeout {
            let killed = child.kill();
            let reaped = child.wait();
            killed.map_err(io_error("stopping say after it timed out"))?;
            reaped.map_err(io_error("reaping say after it timed out"))?;
            return Err(SpeechError::TimedOut(timeout));
        }
        thread::sleep(POLL);
    }
}

type Drain = thread::JoinHandle<io::Result<Vec<u8>>>;

fn drain(mut pipe: impl Read + Send + 'static) -> Drain {
    thread::spawn(move || {
        let mut kept = Vec::new();
        (&mut pipe).take(OUTPUT_LIMIT).read_to_end(&mut kept)?;
        io::copy(&mut pipe, &mut io::sink())?;
        Ok(kept)
    })
}

fn collect(drain: Option<Drain>) -> Result<Vec<u8>, SpeechError> {
    let Some(drain) = drain else {
        return Ok(Vec::new());
    };
    drain
        .join()
        .unwrap_or_else(|_| Err(io::Error::other("the reader thread panicked")))
        .map_err(io_error("reading say's output"))
}

/// Ask `say` (at `say_path`) for its voices.
pub fn list_voices(say_path: &Path, timeout: Duration) -> Result<Vec<Voice>, SpeechError> {
    if !say_path.is_file() {
        return Err(SpeechError::SayMissing(say_path.to_path_buf()));
    }
    let mut command = Command::new(say_path);
    command.args(["-v", "?"]);
    let ran = run(&mut command, timeout)?;
    if !ran.status.success() {
        return Err(SpeechError::SayFailed {
            status: ran.status.to_string(),
            stderr: tidy_stderr(&ran.stderr),
        });
    }
    Ok(parse_voices(&String::from_utf8_lossy(&ran.stdout)))
}

/// Read a mono (or mixed-down) 32-bit float WAVE at exactly `sample_rate`.
fn load_wav(path: &Path, sample_rate: u32) -> Result<Vec<f32>, SpeechError> {
    let reader = hound::WavReader::open(path)
        .map_err(|error| SpeechError::BadRender(format!("cannot read the WAVE: {error}")))?;
    let spec = reader.spec();
    if spec.sample_format != hound::SampleFormat::Float || spec.bits_per_sample != 32 {
        return Err(SpeechError::BadRender(format!(
            "expected 32-bit float samples, got {}-bit {:?}",
            spec.bits_per_sample, spec.sample_format
        )));
    }
    if spec.sample_rate != sample_rate {
        return Err(SpeechError::BadRender(format!(
            "expected {sample_rate} Hz, got {} Hz",
            spec.sample_rate
        )));
    }
    if spec.channels == 0 {
        return Err(SpeechError::BadRender(
            "the WAVE has no channels".to_string(),
        ));
    }
    let channels = usize::from(spec.channels);
    let interleaved = reader
        .into_samples::<f32>()
        .collect::<Result<Vec<f32>, hound::Error>>()
        .map_err(|error| SpeechError::BadRender(format!("cannot read the samples: {error}")))?;
    let scale = 1.0 / channels as f32;
    let samples: Vec<f32> = interleaved
        .chunks_exact(channels)
        .map(|frame| {
            let sum: f32 = frame.iter().filter(|x| x.is_finite()).sum();
            sum * scale
        })
        .collect();
    if samples.is_empty() {
        return Err(SpeechError::EmptyRender);
    }
    Ok(samples)
}

#[cfg(test)]
mod tests;
