//! A cache of rendered phrases on disk.
//!
//! Each render is one file named by a hash of its key (text, voice, sample
//! rate, speaking rate). The file carries the whole key, so a hash
//! collision reads as a miss rather than the wrong words. Files are written
//! to a temporary name and renamed into place, so a reader never sees half
//! a file. When the files together pass the size cap, the least recently
//! used go first.
//!
//! File layout, little-endian: the magic `KZSPEECH1\n`, the key's length
//! (`u32`) and UTF-8 bytes, the sample rate (`u32`), the frame count
//! (`u64`), then the samples (`f32`).

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const MAGIC: &[u8; 10] = b"KZSPEECH1\n";

/// Extension of every cache entry.
const EXTENSION: &str = "kzs";

/// What a render is filed under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheKey(String);

impl CacheKey {
    /// The key for `text` in `voice` (empty for the system voice) at
    /// `sample_rate`, spoken at `words_per_minute` (0 for the voice's own
    /// pace).
    #[must_use]
    pub fn new(text: &str, voice: &str, sample_rate: u32, words_per_minute: u16) -> Self {
        Self(format!(
            "{text}\u{0}{voice}\u{0}{sample_rate}\u{0}{words_per_minute}"
        ))
    }

    /// The file name this key is stored under.
    fn file_name(&self) -> String {
        // 64-bit FNV-1a: stable across builds, unlike the std hasher.
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in self.0.bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
        format!("{hash:016x}.{EXTENSION}")
    }
}

/// A rendered phrase as the cache holds it.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// Its rate, in Hz.
    pub sample_rate: u32,
    /// Its mono samples.
    pub samples: Vec<f32>,
}

/// The render cache in one directory.
#[derive(Debug, Clone)]
pub struct Cache {
    dir: PathBuf,
    cap_bytes: u64,
    writes: u64,
}

impl Cache {
    /// A cache in `dir` (which must already exist) holding at most
    /// `cap_bytes` of renders.
    #[must_use]
    pub const fn new(dir: PathBuf, cap_bytes: u64) -> Self {
        Self {
            dir,
            cap_bytes,
            writes: 0,
        }
    }

    /// The directory it lives in.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The render filed under `key`, if the cache has a sound copy. A hit
    /// marks the entry as just used; a damaged or colliding file is a miss.
    pub fn get(&self, key: &CacheKey) -> io::Result<Option<Entry>> {
        let path = self.dir.join(key.file_name());
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let Some(entry) = decode(&bytes, key) else {
            return Ok(None);
        };
        fs::File::options()
            .write(true)
            .open(&path)?
            .set_modified(SystemTime::now())?;
        Ok(Some(entry))
    }

    /// File `samples` at `sample_rate` under `key`, then trim the cache to
    /// its cap, oldest first. A render bigger than the whole cap is not
    /// kept. Returns whether it was kept.
    pub fn put(&mut self, key: &CacheKey, sample_rate: u32, samples: &[f32]) -> io::Result<bool> {
        let bytes = encode(key, sample_rate, samples);
        if bytes.len() as u64 > self.cap_bytes {
            return Ok(false);
        }
        let name = key.file_name();
        self.writes += 1;
        let temporary = self.dir.join(format!(
            ".{name}.{}-{}.part",
            std::process::id(),
            self.writes
        ));
        let written = write_synced(&temporary, &bytes)
            .and_then(|()| fs::rename(&temporary, self.dir.join(&name)));
        if let Err(error) = written {
            return Err(match fs::remove_file(&temporary) {
                Ok(()) => error,
                Err(cleanup) if cleanup.kind() == io::ErrorKind::NotFound => error,
                Err(cleanup) => io::Error::new(
                    error.kind(),
                    format!("{error}; the partial file could not be removed either: {cleanup}"),
                ),
            });
        }
        self.trim(&name)?;
        Ok(true)
    }

    /// How many bytes the cache's entries take.
    pub fn size(&self) -> io::Result<u64> {
        Ok(self.entries()?.iter().map(|(_, len, _)| len).sum())
    }

    /// Every entry: path, size, last use.
    fn entries(&self) -> io::Result<Vec<(PathBuf, u64, SystemTime)>> {
        let mut entries = Vec::new();
        for item in fs::read_dir(&self.dir)? {
            let item = item?;
            let path = item.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some(EXTENSION) {
                continue;
            }
            let meta = match item.metadata() {
                Ok(meta) => meta,
                // Removed by someone else between listing and looking.
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            if meta.is_file() {
                entries.push((path, meta.len(), meta.modified()?));
            }
        }
        Ok(entries)
    }

    /// Remove the least recently used entries until the cache fits its cap,
    /// never removing `keep`.
    fn trim(&self, keep: &str) -> io::Result<()> {
        let mut entries = self.entries()?;
        let mut total: u64 = entries.iter().map(|(_, len, _)| len).sum();
        entries.sort_by_key(|(_, _, used)| *used);
        for (path, len, _) in entries {
            if total <= self.cap_bytes {
                break;
            }
            if path.file_name().and_then(|name| name.to_str()) == Some(keep) {
                continue;
            }
            match fs::remove_file(&path) {
                Ok(()) => total = total.saturating_sub(len),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    total = total.saturating_sub(len);
                }
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
}

fn write_synced(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = fs::File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn encode(key: &CacheKey, sample_rate: u32, samples: &[f32]) -> Vec<u8> {
    let key = key.0.as_bytes();
    let mut bytes = Vec::with_capacity(MAGIC.len() + 4 + key.len() + 4 + 8 + samples.len() * 4);
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&(key.len() as u32).to_le_bytes());
    bytes.extend_from_slice(key);
    bytes.extend_from_slice(&sample_rate.to_le_bytes());
    bytes.extend_from_slice(&(samples.len() as u64).to_le_bytes());
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

fn decode(bytes: &[u8], key: &CacheKey) -> Option<Entry> {
    let (magic, mut reader) = bytes.split_first_chunk::<10>()?;
    if magic != MAGIC {
        return None;
    }
    let key_len = read_u32(&mut reader)? as usize;
    let stored_key = reader.get(..key_len)?;
    if stored_key != key.0.as_bytes() {
        return None;
    }
    reader = &reader[key_len..];
    let sample_rate = read_u32(&mut reader)?;
    let frames = read_u64(&mut reader)?;
    if reader.len() as u64 != frames.checked_mul(4)? {
        return None;
    }
    let samples = reader
        .chunks_exact(4)
        .map(|chunk| {
            let value = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            if value.is_finite() { value } else { 0.0 }
        })
        .collect();
    Some(Entry {
        sample_rate,
        samples,
    })
}

fn read_u32(reader: &mut &[u8]) -> Option<u32> {
    let (head, rest) = reader.split_first_chunk::<4>()?;
    *reader = rest;
    Some(u32::from_le_bytes(*head))
}

fn read_u64(reader: &mut &[u8]) -> Option<u64> {
    let (head, rest) = reader.split_first_chunk::<8>()?;
    *reader = rest;
    Some(u64::from_le_bytes(*head))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Scratch;

    fn samples(len: usize) -> Vec<f32> {
        (0..len).map(|n| (n as f32 * 0.01).sin()).collect()
    }

    #[test]
    fn a_put_comes_back() {
        let scratch = Scratch::new("cache-roundtrip");
        let mut cache = Cache::new(scratch.0.clone(), 1 << 20);
        let key = CacheKey::new("hello", "Daniel", 48_000, 0);
        assert_eq!(cache.get(&key).expect("get"), None);
        assert!(cache.put(&key, 48_000, &samples(1_000)).expect("put"));
        let entry = cache.get(&key).expect("get").expect("hit");
        assert_eq!(entry.sample_rate, 48_000);
        assert_eq!(entry.samples, samples(1_000));
        // Any part of the key differing is a miss.
        for other in [
            CacheKey::new("hello!", "Daniel", 48_000, 0),
            CacheKey::new("hello", "Moira", 48_000, 0),
            CacheKey::new("hello", "Daniel", 44_100, 0),
            CacheKey::new("hello", "Daniel", 48_000, 200),
        ] {
            assert_eq!(cache.get(&other).expect("get"), None);
        }
    }

    #[test]
    fn a_colliding_or_damaged_file_is_a_miss() {
        let scratch = Scratch::new("cache-damage");
        let mut cache = Cache::new(scratch.0.clone(), 1 << 20);
        let key = CacheKey::new("hello", "", 48_000, 0);
        assert!(cache.put(&key, 48_000, &samples(100)).expect("put"));
        let path = scratch.0.join(key.file_name());
        // Another key's contents under this key's name.
        let other = encode(
            &CacheKey::new("goodbye", "", 48_000, 0),
            48_000,
            &samples(100),
        );
        fs::write(&path, other).expect("write");
        assert_eq!(cache.get(&key).expect("get"), None);
        // Truncated.
        let whole = encode(&key, 48_000, &samples(100));
        fs::write(&path, &whole[..whole.len() - 3]).expect("write");
        assert_eq!(cache.get(&key).expect("get"), None);
        // Garbage.
        fs::write(&path, b"not a cache file").expect("write");
        assert_eq!(cache.get(&key).expect("get"), None);
    }

    #[test]
    fn the_cap_evicts_the_least_recently_used() {
        let scratch = Scratch::new("cache-cap");
        let one = encode(&CacheKey::new("a", "", 48_000, 0), 48_000, &samples(1_000)).len() as u64;
        let mut cache = Cache::new(scratch.0.clone(), one * 2 + one / 2);
        let keys: Vec<CacheKey> = ["a", "b", "c"]
            .iter()
            .map(|text| CacheKey::new(text, "", 48_000, 0))
            .collect();
        assert!(cache.put(&keys[0], 48_000, &samples(1_000)).expect("put"));
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert!(cache.put(&keys[1], 48_000, &samples(1_000)).expect("put"));
        std::thread::sleep(std::time::Duration::from_millis(20));
        // Using "a" makes "b" the oldest.
        assert!(cache.get(&keys[0]).expect("get").is_some());
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert!(cache.put(&keys[2], 48_000, &samples(1_000)).expect("put"));
        assert!(cache.get(&keys[0]).expect("get").is_some());
        assert!(cache.get(&keys[1]).expect("get").is_none());
        assert!(cache.get(&keys[2]).expect("get").is_some());
        assert!(cache.size().expect("size") <= one * 2 + one / 2);
        // Too big for the whole cache: not kept, nothing else lost.
        assert!(!cache.put(&keys[1], 48_000, &samples(10_000)).expect("put"));
        assert!(cache.get(&keys[0]).expect("get").is_some());
    }

    #[test]
    fn file_names_are_stable() {
        assert_eq!(
            CacheKey::new("hello", "", 48_000, 0).file_name(),
            CacheKey::new("hello", "", 48_000, 0).file_name()
        );
        assert_ne!(
            CacheKey::new("hello", "", 48_000, 0).file_name(),
            CacheKey::new("hello", "", 44_100, 0).file_name()
        );
        // FNV-1a of the empty key parts, pinned so a change is noticed.
        assert_eq!(CacheKey(String::new()).file_name(), "cbf29ce484222325.kzs");
    }
}
