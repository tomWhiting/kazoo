//! Everything that can go wrong, in words a person can act on.

use std::fmt;
use std::io;

/// The raw OS error for "no space left on device" on macOS and Linux.
const ENOSPC: i32 = 28;

/// Why a store, recorder or sample operation failed.
#[derive(Debug)]
pub enum Error {
    /// A sample name broke the naming rules. `name` is the refused text,
    /// cut to 64 characters; show it with `{:?}` so control characters stay
    /// visible.
    BadName {
        /// The refused text.
        name: String,
        /// Which rule it broke.
        reason: &'static str,
    },
    /// No sample of that name.
    NotFound {
        /// The sample asked for.
        name: String,
    },
    /// A sample of that name already exists and the caller asked not to
    /// replace it.
    Exists {
        /// The sample's name.
        name: String,
    },
    /// The file under a sample's name is not a regular file (a symbolic
    /// link, a directory). The store never follows one.
    NotAFile {
        /// The sample's name.
        name: String,
    },
    /// A sample is longer than the store allows.
    TooLong {
        /// The sample's name.
        name: String,
        /// How long it is.
        seconds: f64,
        /// The store's cap.
        max_seconds: f64,
    },
    /// Loading a sample would take the loaded samples over the memory cap.
    MemoryFull {
        /// The sample's name.
        name: String,
        /// Bytes it needs.
        needed: usize,
        /// Bytes left under the cap.
        available: usize,
    },
    /// The WAV file is damaged or not a WAV file at all.
    Corrupt {
        /// The sample's name.
        name: String,
        /// What is wrong with it.
        reason: String,
    },
    /// The WAV file is valid but holds a format this crate cannot read.
    Unsupported {
        /// The sample's name.
        name: String,
        /// What it holds.
        reason: String,
    },
    /// Audio handed in was unusable: no frames, a rate out of range, or
    /// channels of different lengths.
    BadAudio {
        /// What is wrong with it.
        reason: String,
    },
    /// The disk is full. Nothing was left half-written.
    DiskFull {
        /// What was being written.
        context: String,
    },
    /// Any other I/O failure.
    Io {
        /// What was being done.
        context: String,
        /// The failure.
        source: io::Error,
    },
    /// `$HOME` is not set, so the default store directory is unknown.
    NoHome,
    /// A take is already being recorded or finished.
    Busy,
    /// There is no take to stop.
    NotRecording,
    /// The take ended with no audio in it.
    EmptyTake {
        /// The take's name.
        name: String,
    },
    /// Trimming was asked for and the take never rose above the threshold.
    SilentTake {
        /// The take's name.
        name: String,
        /// The trim threshold, in dBFS.
        threshold_db: f32,
    },
    /// The recorder's writer thread is gone; no more takes can be made.
    WriterStopped,
}

impl Error {
    /// An I/O failure while doing `context`, with a full disk told apart.
    pub(crate) fn io(context: impl Into<String>, source: io::Error) -> Self {
        let context = context.into();
        if source.kind() == io::ErrorKind::StorageFull || source.raw_os_error() == Some(ENOSPC) {
            Self::DiskFull { context }
        } else {
            Self::Io { context, source }
        }
    }

    /// A refused name, cut short for safe display.
    pub(crate) fn bad_name(name: &str, reason: &'static str) -> Self {
        Self::BadName {
            name: name.chars().take(64).collect(),
            reason,
        }
    }

    /// Whether this is [`Error::DiskFull`].
    #[must_use]
    pub const fn is_disk_full(&self) -> bool {
        matches!(self, Self::DiskFull { .. })
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadName { name, reason } => write!(f, "sample name {name:?} refused: {reason}"),
            Self::NotFound { name } => write!(f, "no sample called '{name}'"),
            Self::Exists { name } => write!(f, "a sample called '{name}' already exists"),
            Self::NotAFile { name } => {
                write!(f, "'{name}' is not a regular file in the sample store")
            }
            Self::TooLong {
                name,
                seconds,
                max_seconds,
            } => write!(
                f,
                "'{name}' is {seconds:.1} s long; samples may be at most {max_seconds:.1} s"
            ),
            Self::MemoryFull {
                name,
                needed,
                available,
            } => write!(
                f,
                "loading '{name}' needs {} MiB but only {} MiB is left for loaded samples",
                needed.div_ceil(1 << 20),
                available / (1 << 20)
            ),
            Self::Corrupt { name, reason } => write!(f, "'{name}' is not a readable WAV: {reason}"),
            Self::Unsupported { name, reason } => write!(f, "'{name}' cannot be read: {reason}"),
            Self::BadAudio { reason } => write!(f, "unusable audio: {reason}"),
            Self::DiskFull { context } => write!(f, "the disk is full ({context})"),
            Self::Io { context, source } => write!(f, "{context}: {source}"),
            Self::NoHome => write!(f, "HOME is not set, so ~/.kazoo cannot be found"),
            Self::Busy => write!(f, "a take is already being recorded"),
            Self::NotRecording => write!(f, "nothing is being recorded"),
            Self::EmptyTake { name } => write!(f, "the take '{name}' has no audio in it"),
            Self::SilentTake { name, threshold_db } => write!(
                f,
                "the take '{name}' never rose above the trim threshold of {threshold_db:.1} dBFS"
            ),
            Self::WriterStopped => write!(f, "the recorder's writer thread has stopped"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_disk_is_told_apart() {
        let full = Error::io("saving", io::Error::from_raw_os_error(ENOSPC));
        assert!(full.is_disk_full());
        let other = Error::io("saving", io::Error::from(io::ErrorKind::PermissionDenied));
        assert!(!other.is_disk_full());
        assert!(std::error::Error::source(&other).is_some());
    }

    #[test]
    fn refused_names_are_cut_and_escaped() {
        let long = "\u{7}".repeat(200);
        let error = Error::bad_name(&long, "too long");
        let shown = error.to_string();
        assert!(shown.contains("\\u{7}"));
        assert!(shown.len() < 600);
    }
}
