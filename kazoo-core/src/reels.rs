//! Shared, content-addressed stereo recordings and durable holder pin lists.
//!
//! This store performs allocation and disk I/O. Call it on a tape or deck
//! thread, never an audio callback. Samples retain their exact floating-point
//! bits; their hash, rather than the WAV header, names each recording.

#[cfg(test)]
mod tests;

use std::collections::BTreeSet;
use std::fmt;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

/// Overrides the shared recordings directory.
pub const REELS_DIR_ENV: &str = "KAZOO_REELS_DIR";
/// Maximum loop length, including the seam tail.
pub const MAX_REEL_FRAMES: u64 = 4_612_096;
/// Grace between writing an unpinned recording and reclaiming it.
pub const SWEEP_GRACE: Duration = Duration::from_secs(24 * 60 * 60);
/// Disk budget enforced by the wall's pruning policy.
pub const REELS_BUDGET_BYTES: u64 = 4 * 1024 * 1024 * 1024;

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

/// The shared directory, from the override or `$HOME/.kazoo/reels`.
///
/// # Errors
/// Returns `NotFound` if neither nonempty environment variable is available.
pub fn reels_dir() -> io::Result<PathBuf> {
    if let Some(dir) = std::env::var_os(REELS_DIR_ENV).filter(|dir| !dir.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(|home| PathBuf::from(home).join(".kazoo").join("reels"))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("neither ${REELS_DIR_ENV} nor $HOME is set"),
            )
        })
}

/// The SHA-256 of a recording's format and exact sample bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReelId([u8; 32]);

impl ReelId {
    /// The first eight hexadecimal digits, for display to people.
    #[must_use]
    pub fn short(&self) -> String {
        self.to_string()[..8].to_owned()
    }
}

impl fmt::Display for ReelId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl FromStr for ReelId {
    type Err = ReelError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        id_from_hex(text).ok_or(ReelError::BadId)
    }
}

fn id_from_hex(text: &str) -> Option<ReelId> {
    if text.len() != 64 {
        return None;
    }
    let mut bytes = [0_u8; 32];
    for (output, pair) in bytes.iter_mut().zip(text.as_bytes().chunks_exact(2)) {
        let digit = |byte| match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            _ => None,
        };
        *output = digit(pair[0])? * 16 + digit(pair[1])?;
    }
    Some(ReelId(bytes))
}

/// Serde field helpers using the canonical lowercase string form.
///
/// ```
/// use kazoo_core::reels::{ReelId, reel_id_serde};
/// #[derive(serde::Serialize, serde::Deserialize)]
/// struct SavedLoop {
///     #[serde(with = "reel_id_serde")]
///     reel: ReelId,
/// }
/// ```
pub mod reel_id_serde {
    use super::ReelId;
    use serde::{Deserialize, Deserializer, Serializer};

    /// Serialize a reel identifier as a string.
    pub fn serialize<S: Serializer>(id: &ReelId, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(id)
    }

    /// Deserialize a canonical reel identifier, refusing all other strings.
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<ReelId, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

/// A stereo recording whose samples alternate left and right.
#[derive(Debug)]
pub struct Reel {
    /// Samples per second on each channel.
    pub rate: u32,
    /// Interleaved, sample-exact floating-point audio.
    pub samples: Box<[f32]>,
}

impl Reel {
    /// Number of stereo frames.
    #[must_use]
    pub fn frames(&self) -> usize {
        self.samples.len() / 2
    }
}

/// Hash the versioned format prefix and each sample's little-endian bits.
/// Validation belongs to [`ReelStore::put`]; this function only hashes.
#[must_use]
pub fn reel_id(rate: u32, samples: &[f32]) -> ReelId {
    let mut hash = Sha256::new();
    hash.update(b"KZREEL1\0");
    hash.update(rate.to_le_bytes());
    hash.update(2_u16.to_le_bytes());
    hash.update((samples.len() as u64 / 2).to_le_bytes());
    for sample in samples {
        hash.update(sample.to_bits().to_le_bytes());
    }
    ReelId(hash.finalize().into())
}

/// Why a recording or pin list could not be used.
#[derive(Debug)]
pub enum ReelError {
    /// An identifier was not 64 lowercase hexadecimal digits.
    BadId,
    /// Interleaved audio had an unmatched channel sample.
    NotStereo,
    /// The recording exceeds the loop's bounded capacity.
    TooLong { frames: u64 },
    /// Sample rate is outside the supported range.
    BadRate(u32),
    /// A sample cannot be safely played.
    NotFinite { at: usize },
    /// No file names this recording.
    Missing(ReelId),
    /// The file is not the required stereo float WAV format.
    WrongFormat(String),
    /// The bounded sample reservation failed.
    NoMemory,
    /// Samples differed from the file's name and were moved aside.
    Corrupt(ReelId),
    /// A holder's complete pin list could not be read.
    PinUnreadable { holder: String },
    /// A filesystem operation, including a durability sync, failed.
    Io(io::Error),
}

impl fmt::Display for ReelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadId => f.write_str("a reel name must be 64 lowercase hexadecimal digits"),
            Self::NotStereo => f.write_str("a reel must have an even number of stereo samples"),
            Self::TooLong { frames } => write!(
                f,
                "reel has {frames} frames; the limit is {MAX_REEL_FRAMES}"
            ),
            Self::BadRate(rate) => {
                write!(f, "reel sample rate {rate} is outside 8000 to 384000 Hz")
            }
            Self::NotFinite { at } => write!(f, "reel sample {at} is not finite"),
            Self::Missing(id) => write!(f, "reel {} is missing", id.short()),
            Self::WrongFormat(reason) => {
                write!(f, "reel is not a stereo 32-bit float WAV: {reason}")
            }
            Self::NoMemory => f.write_str("there is not enough memory to read the reel"),
            Self::Corrupt(id) => write!(
                f,
                "reel {} is damaged: its samples do not match its name; moved aside as {id}.corrupt-<unix>.wav",
                id.short()
            ),
            Self::PinUnreadable { holder } => write!(
                f,
                "reel pin list for {holder} is unreadable; nothing was swept"
            ),
            Self::Io(error) => write!(f, "reel storage failed: {error}"),
        }
    }
}

impl std::error::Error for ReelError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for ReelError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

fn wav_error(error: hound::Error) -> ReelError {
    match error {
        hound::Error::IoError(error) => ReelError::Io(error),
        error => ReelError::WrongFormat(error.to_string()),
    }
}

// The float writer uses WAVEFORMATEXTENSIBLE, with a 68-byte header.
const fn wav_size(frames: u64) -> u64 {
    68 + frames * 8
}

fn check_rate(rate: u32) -> Result<(), ReelError> {
    if !(8_000..=384_000).contains(&rate) {
        return Err(ReelError::BadRate(rate));
    }
    Ok(())
}

fn check_finite(samples: &[f32]) -> Result<(), ReelError> {
    if let Some(at) = samples.iter().position(|sample| !sample.is_finite()) {
        return Err(ReelError::NotFinite { at });
    }
    Ok(())
}

fn check_holder(holder: &str) -> io::Result<()> {
    if holder.is_empty()
        || holder.len() > 48
        || !holder
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a reel holder must match [a-z0-9-]{1,48}",
        ));
    }
    Ok(())
}

fn atomic_write<E, F>(path: &Path, write: F) -> Result<(), E>
where
    E: From<io::Error> + fmt::Display,
    F: FnOnce(&mut File) -> Result<(), E>,
{
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "reel path has no directory"))?;
    let filename = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "reel path has no filename"))?;
    let mut temporary_name = filename.to_os_string();
    temporary_name.push(format!(
        ".tmp-{}-{}",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    let temporary = parent.join(temporary_name);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)?;
    let result = (|| {
        write(&mut file)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if let Err(error) = result {
        match fs::remove_file(&temporary) {
            Ok(()) => {
                File::open(parent)?.sync_all()?;
                return Err(error);
            }
            Err(cleanup) if cleanup.kind() == io::ErrorKind::NotFound => return Err(error),
            Err(cleanup) => {
                return Err(io::Error::new(
                    cleanup.kind(),
                    format!("{error}; temporary file cleanup failed: {cleanup}"),
                )
                .into());
            }
        }
    }
    Ok(())
}

/// Counts of canonical reel files, excluding sidecars and damaged files.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    /// Number of recordings.
    pub reels: u64,
    /// WAV bytes on disk.
    pub bytes: u64,
}

/// Results of a sweep; temporary files and sidecars are not reel counts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Swept {
    /// Number of recordings deleted.
    pub deleted: u64,
    /// WAV bytes reclaimed.
    pub bytes: u64,
    /// Number of recordings retained.
    pub kept: u64,
}

/// A shared recording directory, independent of any program's save files.
#[derive(Debug, Clone)]
pub struct ReelStore {
    dir: PathBuf,
}

impl ReelStore {
    /// Create the store and its pin directory with private directory modes.
    /// A directory that already exists is made private too, as the wall's
    /// state directory is.
    pub fn open(dir: &Path) -> io::Result<Self> {
        DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        let pins = dir.join("pins");
        DirBuilder::new()
            .mode(0o700)
            .create(&pins)
            .or_else(|error| {
                if error.kind() == io::ErrorKind::AlreadyExists && pins.is_dir() {
                    Ok(())
                } else {
                    Err(error)
                }
            })?;
        fs::set_permissions(&pins, fs::Permissions::from_mode(0o700))?;
        Ok(Self {
            dir: dir.to_owned(),
        })
    }

    /// Validate and durably store sample-exact stereo audio, deduplicated by hash.
    pub fn put(&self, rate: u32, samples: &[f32]) -> Result<ReelId, ReelError> {
        if samples.len() % 2 != 0 {
            return Err(ReelError::NotStereo);
        }
        let frames = samples.len() as u64 / 2;
        if frames > MAX_REEL_FRAMES {
            return Err(ReelError::TooLong { frames });
        }
        check_rate(rate)?;
        check_finite(samples)?;
        let id = reel_id(rate, samples);
        let path = self.path(&id);
        let existing = match File::open(&path) {
            Ok(file) => Some(file),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        if let Some(file) = existing {
            if file.metadata()?.len() == wav_size(frames) {
                // Refresh grace before the caller can publish its new pin list.
                file.set_modified(SystemTime::now())?;
                file.sync_all()?;
                return Ok(id);
            }
        }
        atomic_write::<ReelError, _>(&path, |file| {
            let spec = hound::WavSpec {
                channels: 2,
                sample_rate: rate,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            };
            let mut writer =
                hound::WavWriter::new(BufWriter::new(file), spec).map_err(wav_error)?;
            for sample in samples {
                writer.write_sample(*sample).map_err(wav_error)?;
            }
            writer.finalize().map_err(wav_error)
        })?;
        Ok(id)
    }

    /// Read bounded stereo audio and verify its name before returning samples.
    pub fn get(&self, id: &ReelId) -> Result<Reel, ReelError> {
        let path = self.path(id);
        let file = File::open(&path).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                ReelError::Missing(*id)
            } else {
                ReelError::Io(error)
            }
        })?;
        let mut reader = hound::WavReader::new(BufReader::new(file)).map_err(wav_error)?;
        let spec = reader.spec();
        if spec.channels != 2
            || spec.bits_per_sample != 32
            || spec.sample_format != hound::SampleFormat::Float
        {
            return Err(ReelError::WrongFormat(format!("found {spec:?}")));
        }
        let frames = u64::from(reader.duration());
        if frames > MAX_REEL_FRAMES {
            return Err(ReelError::TooLong { frames });
        }
        check_rate(spec.sample_rate)?;
        let count = usize::try_from(reader.len()).map_err(|_| ReelError::NoMemory)?;
        let mut samples = Vec::new();
        samples
            .try_reserve_exact(count)
            .map_err(|_| ReelError::NoMemory)?;
        for sample in reader.samples::<f32>() {
            samples.push(sample.map_err(wav_error)?);
        }
        if reel_id(spec.sample_rate, &samples) != *id {
            let unix = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(io::Error::other)?
                .as_nanos();
            fs::rename(&path, self.dir.join(format!("{id}.corrupt-{unix}.wav")))?;
            File::open(&self.dir)?.sync_all()?;
            return Err(ReelError::Corrupt(*id));
        }
        check_finite(&samples)?;
        Ok(Reel {
            rate: spec.sample_rate,
            samples: samples.into_boxed_slice(),
        })
    }

    /// Whether the canonical recording path currently exists as a file.
    #[must_use]
    pub fn contains(&self, id: &ReelId) -> bool {
        self.path(id).is_file()
    }

    /// Canonical WAV path for a peak builder.
    #[must_use]
    pub fn path(&self, id: &ReelId) -> PathBuf {
        self.dir.join(format!("{id}.wav"))
    }

    /// Durably replace a holder's complete, sorted pin list.
    pub fn pin(&self, holder: &str, ids: &BTreeSet<ReelId>) -> io::Result<()> {
        check_holder(holder)?;
        let path = self.dir.join("pins").join(format!("{holder}.txt"));
        atomic_write(&path, |file| {
            let mut writer = BufWriter::new(file);
            for id in ids {
                writeln!(writer, "{id}")?;
            }
            writeln!(writer, "end")?;
            writer.flush()
        })
    }

    /// Remove a holder's pin list; absence already satisfies the request.
    pub fn unpin(&self, holder: &str) -> io::Result<()> {
        check_holder(holder)?;
        let pins = self.dir.join("pins");
        match fs::remove_file(pins.join(format!("{holder}.txt"))) {
            Ok(()) => File::open(pins)?.sync_all(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// Count only files with canonical reel names.
    pub fn usage(&self) -> io::Result<Usage> {
        let mut usage = Usage::default();
        for entry in fs::read_dir(&self.dir)? {
            let entry = entry?;
            if entry.file_type()?.is_file() && canonical_id(&entry.path()).is_some() {
                usage.reels += 1;
                usage.bytes += entry.metadata()?.len();
            }
        }
        Ok(usage)
    }

    /// Delete one reel and its sidecars, after the caller checks other pins.
    pub fn remove(&self, id: &ReelId) -> io::Result<bool> {
        let removed = self.remove_files(id)?;
        File::open(&self.dir)?.sync_all()?;
        Ok(removed)
    }

    fn remove_files(&self, id: &ReelId) -> io::Result<bool> {
        let removed = match fs::remove_file(self.path(id)) {
            Ok(()) => true,
            Err(error) if error.kind() == io::ErrorKind::NotFound => false,
            Err(error) => return Err(error),
        };
        let stem = id.to_string();
        for entry in fs::read_dir(&self.dir)? {
            let entry = entry?;
            if entry.file_type()?.is_file()
                && entry
                    .path()
                    .file_stem()
                    .is_some_and(|name| name == stem.as_str())
            {
                remove_if_present(&entry.path())?;
            }
        }
        Ok(removed)
    }

    /// All pins except one holder's, failing closed on any unreadable list.
    pub fn pinned_elsewhere(&self, holder: &str) -> Result<BTreeSet<ReelId>, ReelError> {
        check_holder(holder)?;
        self.read_pins(Some(holder))
    }

    fn read_pins(&self, except: Option<&str>) -> Result<BTreeSet<ReelId>, ReelError> {
        let mut ids = BTreeSet::new();
        for entry in fs::read_dir(self.dir.join("pins"))? {
            let path = entry?.path();
            if path.extension().is_none_or(|extension| extension != "txt") {
                continue;
            }
            let holder = path
                .file_stem()
                .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
            if except == Some(holder.as_str()) {
                continue;
            }
            let list = read_pin_list(&path).map_err(|_| ReelError::PinUnreadable { holder })?;
            ids.extend(list);
        }
        Ok(ids)
    }

    /// Reclaim old unpinned reels only after every pin list has been validated.
    pub fn sweep(&self, grace: Duration, now: SystemTime) -> Result<Swept, ReelError> {
        let pinned = self.read_pins(None)?;
        let mut swept = Swept::default();
        for entry in fs::read_dir(&self.dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let path = entry.path();
            if let Some(id) = canonical_id(&path) {
                let metadata = entry.metadata()?;
                if !pinned.contains(&id) && old_enough(&metadata, grace, now)? {
                    if self.remove_files(&id)? {
                        swept.deleted += 1;
                        swept.bytes += metadata.len();
                    }
                } else {
                    swept.kept += 1;
                }
            } else if is_temporary(&path) && old_enough(&entry.metadata()?, grace, now)? {
                remove_if_present(&path)?;
            }
        }
        let pins = self.dir.join("pins");
        let mut pins_changed = false;
        for entry in fs::read_dir(&pins)? {
            let entry = entry?;
            if entry.file_type()?.is_file()
                && is_temporary(&entry.path())
                && old_enough(&entry.metadata()?, grace, now)?
            {
                remove_if_present(&entry.path())?;
                pins_changed = true;
            }
        }
        if pins_changed {
            File::open(pins)?.sync_all()?;
        }
        File::open(&self.dir)?.sync_all()?;
        Ok(swept)
    }
}

fn read_pin_list(path: &Path) -> io::Result<BTreeSet<ReelId>> {
    let mut reader = BufReader::new(File::open(path)?);
    let mut ids = BTreeSet::new();
    let mut line = Vec::with_capacity(66);
    loop {
        line.clear();
        // A bounded line keeps malformed pin files from allocating without limit.
        Read::by_ref(&mut reader)
            .take(66)
            .read_until(b'\n', &mut line)?;
        if line.last() == Some(&b'\n') {
            line.truncate(line.len() - 1);
        }
        if line == b"end" {
            let mut extra = [0_u8; 1];
            if reader.read(&mut extra)? == 0 {
                return Ok(ids);
            }
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "data follows the pin list end marker",
            ));
        }
        let text = std::str::from_utf8(&line)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let id = text
            .parse::<ReelId>()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        ids.insert(id);
    }
}

fn canonical_id(path: &Path) -> Option<ReelId> {
    if path.extension()? != "wav" {
        return None;
    }
    id_from_hex(path.file_stem()?.to_str()?)
}

fn is_temporary(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name.to_string_lossy().contains(".tmp-"))
}

fn old_enough(metadata: &fs::Metadata, grace: Duration, now: SystemTime) -> io::Result<bool> {
    // Future mtimes and an overflowing deadline retain the writer's grace.
    Ok(metadata
        .modified()?
        .checked_add(grace)
        .is_some_and(|deadline| deadline < now))
}

fn remove_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}
