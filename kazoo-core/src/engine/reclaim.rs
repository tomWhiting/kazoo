//! Moving heap objects across the real-time boundary.
//!
//! The output callback must never allocate or free. Everything it adds to
//! the engine — tracks, synths, effects, clip audio — is built complete on
//! another thread and moved in through a command; everything it takes out,
//! replaces or refuses is moved *out* again, over a lock-free ring, to the
//! reclaim thread (`kazoo-reclaim`), which frees it.
//!
//! The reclaim thread also does the callback's allocating chores:
//!
//! - **Recordings.** Takes are recorded into small chunks from a pool
//!   created (and written to, so its memory is resident) when the engine
//!   starts. Each full chunk goes to the reclaim thread, which appends it to
//!   the take and sends the chunk straight back for reuse. When a take ends,
//!   the reclaim thread makes it a clip and hands it back to be placed on
//!   its track. Nothing on the audio thread depends on a take's length.
//! - **Timeline snapshots.** The callback copies what the timeline shows
//!   (shared references only: no allocation) into a pre-allocated
//!   [`TimelineSource`]; the reclaim thread builds the
//!   [`TimelineSnapshot`] the UI renders (names, sorted clips, waveform
//!   overviews), posts it to the [`super::EngineHandle`], clears the source
//!   (dropping its references off the audio thread) and returns it.
//!
//! The callback reserves room in the outbound ring before taking any
//! command that may retire something, so a retired object always has
//! somewhere to go (see `processing`).

use std::collections::{HashMap, VecDeque};
use std::ops::Range;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use ringbuf::traits::{Consumer, Observer, Producer};
use ringbuf::{HeapCons, HeapProd};

use super::disk::DiskCommand;
use super::display::{ClipSnapshot, TimelineSnapshot, TrackClipSnapshot};
use crate::Processor;
use crate::analysis::FormantData;
use crate::mixer::clip::{AudioClip, ClipData, ClipId, MAX_CLIPS_PER_TRACK};
use crate::mixer::{Mixer, SynthLayer, Track, TrackId};
use crate::{Db, MAX_TRACKS};

/// Capacity of the callback → reclaim ring. Far more than one block can
/// retire; the callback stops taking work that would retire more once it
/// is nearly full, so nothing is ever freed on the audio thread.
pub(super) const OUTBOUND_CAPACITY: usize = 512;

/// Capacity of the reclaim → callback ring. The reclaim thread keeps
/// whatever does not fit and delivers it later.
pub(super) const INBOUND_CAPACITY: usize = 256;

/// Timeline sources in circulation between the callback and the reclaim
/// thread: one can be filled while the other is being turned into a
/// snapshot.
pub(super) const TIMELINE_SOURCES: usize = 2;

/// How long the reclaim thread sleeps when there is nothing to do.
const IDLE_SLEEP: Duration = Duration::from_millis(2);

/// A zeroed buffer of `len` samples whose memory has been written, so the
/// audio thread never takes a page fault on first touching it.
pub(super) fn resident_buffer(len: usize) -> Vec<f32> {
    let mut buffer = Vec::with_capacity(len);
    buffer.resize(len, 0.0);
    buffer
}

// ---------------------------------------------------------------------------
// Parcel
// ---------------------------------------------------------------------------

/// A heap box that carries one object into the output callback and can
/// carry one back out.
///
/// Moving a value out of a `Box` frees the box, so a large object cannot be
/// sent boxed and unboxed on the audio thread. A parcel is instead emptied
/// ([`Parcel::take`]) and the empty parcel kept; when an object must leave
/// the callback, it goes into an empty parcel ([`Parcel::refill`]) and the
/// parcel is sent out whole. Neither step allocates or frees.
pub struct Parcel<T>(Box<Option<T>>);

impl<T> Parcel<T> {
    /// Wrap `value` in a new parcel. Allocates: not for the audio thread.
    #[must_use]
    pub fn new(value: T) -> Self {
        Self(Box::new(Some(value)))
    }

    /// Take the contents out, leaving the parcel empty.
    pub fn take(&mut self) -> Option<T> {
        self.0.take()
    }

    /// The contents, if any.
    #[must_use]
    pub fn get(&self) -> Option<&T> {
        self.0.as_ref().as_ref()
    }

    /// The contents' slot, for handing a value over only when it is
    /// accepted (the slot is left as it is otherwise).
    pub fn slot_mut(&mut self) -> &mut Option<T> {
        &mut self.0
    }

    /// Put `value` in the parcel, returning what it displaced (`None` for an
    /// empty parcel).
    #[must_use = "a displaced value must be disposed of by the caller"]
    pub fn refill(&mut self, value: T) -> Option<T> {
        self.0.replace(value)
    }

    /// Whether the parcel is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_none()
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for Parcel<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Parcel").field(&self.0).finish()
    }
}

// ---------------------------------------------------------------------------
// What crosses the boundary
// ---------------------------------------------------------------------------

/// An object the callback no longer owns, sent out to be freed.
pub(super) enum Retired {
    /// A removed or refused track, in the parcel it arrived in.
    Track(Parcel<Track>),
    /// A replaced, removed or refused synth or effect.
    Processor(Box<dyn Processor>),
    /// A removed or refused synth layer.
    Layer(SynthLayer),
    /// A removed or refused clip.
    Clip(AudioClip),
    /// Clip audio for a track that does not exist.
    ClipData(ClipData),
    /// A spectrum frame from the analysis thread, once copied.
    Samples(Vec<f32>),
    /// Formant data from the analysis thread, once copied.
    Formants(FormantData),
    /// A disk command (with its file path) that could not be delivered.
    Disk(DiskCommand),
    /// A timeline source with no room left in the callback.
    Timeline(TimelineSource),
}

/// Identifies one take across the chunks it is sent in.
pub(super) type TakeId = u64;

/// The end of a take: its last chunk, and where its clip goes.
pub(super) struct FinishedTake {
    /// The take.
    pub take: TakeId,
    /// The track the take was recorded on.
    pub track_id: TrackId,
    /// The id reserved for the clip.
    pub clip_id: ClipId,
    /// Timeline position of the clip.
    pub position: u64,
    /// The take's last chunk; `chunk[..filled]` is recorded audio.
    pub chunk: Vec<f32>,
    /// Recorded samples in `chunk`.
    pub filled: usize,
    /// Number of samples of the whole take the clip keeps.
    pub len: usize,
    /// Engine sample rate (the clip's original rate).
    pub sample_rate: u32,
}

/// From the callback to the reclaim thread.
pub(super) enum Outbound {
    /// Free this.
    Retired(Retired),
    /// A full chunk of a take in progress: `chunk[..filled]` is audio.
    /// Append it to the take and send the chunk back.
    TakeChunk {
        take: TakeId,
        chunk: Vec<f32>,
        filled: usize,
    },
    /// The last chunk of a take: make the take a clip.
    TakeEnd(FinishedTake),
    /// A take whose track was removed: discard it, send the chunk back.
    TakeAbort { take: TakeId, chunk: Vec<f32> },
    /// Build the timeline snapshot this source describes.
    Timeline(TimelineSource),
}

/// From the reclaim thread to the callback.
pub(super) enum Inbound {
    /// A recorded clip to place on its track.
    Clip { track_id: TrackId, clip: AudioClip },
    /// A recording chunk, emptied, for the pool.
    Chunk(Vec<f32>),
    /// A cleared timeline source to fill again.
    Timeline(TimelineSource),
}

// ---------------------------------------------------------------------------
// Timeline mailbox
// ---------------------------------------------------------------------------

/// Where the reclaim thread leaves the newest timeline snapshot for the
/// engine handle. A newer snapshot replaces one the UI has not taken. Both
/// sides are off the audio thread, so a lock is fine.
#[derive(Clone, Default)]
pub(super) struct TimelineMailbox(Arc<Mutex<Option<TimelineSnapshot>>>);

impl TimelineMailbox {
    /// Leave `snapshot`, replacing any the UI has not taken.
    pub(super) fn post(&self, snapshot: TimelineSnapshot) {
        let superseded = self
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .replace(snapshot);
        // Freed after the lock is released.
        drop(superseded);
    }

    /// Take the newest snapshot, if one has arrived since the last take.
    pub(super) fn take(&self) -> Option<TimelineSnapshot> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).take()
    }

    /// Whether anyone besides the reclaim thread still holds the mailbox
    /// (nobody would render a snapshot otherwise).
    fn has_reader(&self) -> bool {
        Arc::strong_count(&self.0) > 1
    }
}

impl std::fmt::Debug for TimelineMailbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TimelineMailbox").finish_non_exhaustive()
    }
}

// ---------------------------------------------------------------------------
// Timeline source
// ---------------------------------------------------------------------------

/// One track as the timeline shows it.
struct TimelineTrack {
    track_id: TrackId,
    name: Arc<str>,
    /// This track's entries in [`TimelineSource::clips`].
    clips: Range<usize>,
}

/// One clip as the timeline shows it.
struct TimelineClip {
    id: ClipId,
    data: ClipData,
    position: u64,
    source_start: usize,
    source_end: usize,
    gain: Db,
    muted: bool,
}

/// What the timeline shows, copied out of the mixer by the callback.
///
/// Holds only shared references (track names and clip audio), with room
/// for [`MAX_TRACKS`] tracks of [`MAX_CLIPS_PER_TRACK`] clips reserved, so
/// filling it never allocates. It is only filled when empty, and only the
/// reclaim thread empties it, so the callback never drops a reference.
pub(super) struct TimelineSource {
    tracks: Vec<TimelineTrack>,
    clips: Vec<TimelineClip>,
}

impl TimelineSource {
    /// An empty source with room for every track and clip. Allocates.
    pub(super) fn new() -> Self {
        Self {
            tracks: Vec::with_capacity(MAX_TRACKS),
            clips: Vec::with_capacity(MAX_TRACKS * MAX_CLIPS_PER_TRACK),
        }
    }

    /// Whether the source holds nothing (and may be filled).
    pub(super) fn is_empty(&self) -> bool {
        self.tracks.is_empty() && self.clips.is_empty()
    }

    /// Copy the mixer's tracks and clips in. Real-time safe: only clones
    /// shared references, within the reserved capacity. Returns `false`
    /// (leaving the source untouched) if it was not empty.
    ///
    /// A mixer holding more than its limits (only possible when built off
    /// the audio thread with [`Mixer::add_track`]) is shown up to them.
    pub(super) fn fill(&mut self, mixer: &Mixer) -> bool {
        if !self.is_empty() {
            return false;
        }
        for track in mixer.tracks() {
            if self.tracks.len() == self.tracks.capacity() {
                break;
            }
            let start = self.clips.len();
            for clip in track.clips() {
                if self.clips.len() == self.clips.capacity() {
                    break;
                }
                self.clips.push(TimelineClip {
                    id: clip.id(),
                    data: clip.data().clone(),
                    position: clip.position(),
                    source_start: clip.source_start(),
                    source_end: clip.source_end(),
                    gain: clip.gain(),
                    muted: clip.is_muted(),
                });
            }
            self.tracks.push(TimelineTrack {
                track_id: track.id(),
                name: Arc::clone(track.shared_name()),
                clips: start..self.clips.len(),
            });
        }
        true
    }

    /// Drop everything held, keeping the capacity. Not for the audio thread.
    fn clear(&mut self) {
        self.tracks.clear();
        self.clips.clear();
    }
}

/// Free a retired object. Every variant is spelled out so that adding one
/// is a deliberate decision about how it is freed.
fn free(retired: Retired) {
    match retired {
        Retired::Track(parcel) => drop(parcel),
        Retired::Processor(processor) => drop(processor),
        Retired::Layer(layer) => drop(layer),
        Retired::Clip(clip) => drop(clip),
        Retired::ClipData(data) => drop(data),
        Retired::Samples(samples) => drop(samples),
        Retired::Formants(formants) => drop(formants),
        Retired::Disk(command) => drop(command),
        Retired::Timeline(source) => drop(source),
    }
}

// ---------------------------------------------------------------------------
// Reclaimer
// ---------------------------------------------------------------------------

/// A clip's cached waveform overview, valid for one audio range.
struct CachedOverview {
    data: ClipData,
    source: Range<usize>,
    overview: Vec<(f32, f32)>,
}

/// The reclaim thread's state: frees what the callback retires, assembles
/// takes into clips, and turns timeline sources into snapshots.
pub(super) struct Reclaimer {
    outbound: HeapCons<Outbound>,
    inbound: HeapProd<Inbound>,
    /// Items the inbound ring had no room for, delivered in order later.
    backlog: VecDeque<Inbound>,
    /// The audio of each take in progress, by take.
    takes: HashMap<TakeId, Vec<f32>>,
    /// Where timeline snapshots are left for the engine handle.
    mailbox: TimelineMailbox,
    /// Waveform overviews by clip, so an unchanged clip is not rescanned.
    overviews: HashMap<ClipId, CachedOverview>,
}

impl Reclaimer {
    /// A reclaimer serving the callback's end of `outbound` and `inbound`,
    /// leaving timeline snapshots in `mailbox`.
    pub(super) fn new(
        outbound: HeapCons<Outbound>,
        inbound: HeapProd<Inbound>,
        mailbox: TimelineMailbox,
    ) -> Self {
        Self {
            outbound,
            inbound,
            backlog: VecDeque::new(),
            takes: HashMap::new(),
            mailbox,
            overviews: HashMap::new(),
        }
    }

    /// Run until the callback's end of the outbound ring is gone and
    /// everything it sent has been handled.
    pub(super) fn run(mut self) {
        loop {
            if self.service() == 0 {
                if !self.outbound.write_is_held() && self.outbound.is_empty() {
                    return;
                }
                std::thread::sleep(IDLE_SLEEP);
            }
        }
    }

    /// Handle everything waiting, returning how many items were handled.
    ///
    /// Frees, chunk returns and finished takes are handled in arrival
    /// order, so the audio thread gets its chunks back promptly; timeline
    /// sources are dealt with last, and only the newest is built (older
    /// ones are superseded and returned cleared).
    pub(super) fn service(&mut self) -> usize {
        let mut handled = self.flush_backlog();
        let mut newest: Option<TimelineSource> = None;
        while let Some(item) = self.outbound.try_pop() {
            handled += 1;
            match item {
                Outbound::Retired(retired) => free(retired),
                Outbound::TakeChunk {
                    take,
                    chunk,
                    filled,
                } => {
                    self.append(take, &chunk, filled);
                    self.deliver(Inbound::Chunk(chunk));
                }
                Outbound::TakeEnd(finished) => self.finish_take(finished),
                Outbound::TakeAbort { take, chunk } => {
                    drop(self.takes.remove(&take));
                    self.deliver(Inbound::Chunk(chunk));
                }
                Outbound::Timeline(source) => {
                    if let Some(superseded) = newest.replace(source) {
                        self.return_source(superseded);
                    }
                }
            }
        }
        if let Some(source) = newest {
            self.build_timeline(source);
        }
        handled
    }

    /// Append `chunk[..filled]` to the take's audio.
    fn append(&mut self, take: TakeId, chunk: &[f32], filled: usize) {
        let audio = &chunk[..filled.min(chunk.len())];
        self.takes.entry(take).or_default().extend_from_slice(audio);
    }

    /// Deliver backlogged items while the inbound ring has room.
    fn flush_backlog(&mut self) -> usize {
        let mut delivered = 0;
        while let Some(item) = self.backlog.pop_front() {
            match self.inbound.try_push(item) {
                Ok(()) => delivered += 1,
                Err(item) => {
                    self.backlog.push_front(item);
                    break;
                }
            }
        }
        delivered
    }

    /// Send `item` to the callback, keeping it for later if there is no room
    /// (or if older items are still waiting, to keep the order).
    fn deliver(&mut self, item: Inbound) {
        if !self.backlog.is_empty() {
            self.backlog.push_back(item);
            return;
        }
        if let Err(item) = self.inbound.try_push(item) {
            self.backlog.push_back(item);
        }
    }

    /// Complete a take: return its last chunk, then make the take a clip
    /// (trimmed to the clip's length) and send it to be placed.
    fn finish_take(&mut self, finished: FinishedTake) {
        let FinishedTake {
            take,
            track_id,
            clip_id,
            position,
            chunk,
            filled,
            len,
            sample_rate,
        } = finished;
        self.append(take, &chunk, filled);
        self.deliver(Inbound::Chunk(chunk));

        let mut samples = self.takes.remove(&take).unwrap_or_default();
        samples.truncate(len);
        samples.shrink_to_fit();
        let data = ClipData::new(
            samples,
            format!("Recording {}", clip_id.0),
            None,
            sample_rate,
        );
        let clip = AudioClip::new(clip_id, data, position);
        self.deliver(Inbound::Clip { track_id, clip });
    }

    /// Clear `source` and send it back to the callback.
    fn return_source(&mut self, mut source: TimelineSource) {
        source.clear();
        self.deliver(Inbound::Timeline(source));
    }

    /// Build the timeline snapshot `source` describes, post it for the UI
    /// (if anyone is still reading), and send the cleared source back.
    fn build_timeline(&mut self, source: TimelineSource) {
        if self.mailbox.has_reader() {
            let snapshot = self.snapshot(&source);
            self.mailbox.post(snapshot);
        }
        self.return_source(source);
    }

    /// The timeline snapshot for `source`, with each track's clips sorted
    /// by position. Refreshes the overview cache, forgetting clips that are
    /// gone.
    fn snapshot(&mut self, source: &TimelineSource) -> TimelineSnapshot {
        let mut total_length = 0_u64;
        let mut tracks = Vec::with_capacity(source.tracks.len());
        let mut cache = HashMap::with_capacity(source.clips.len());
        for track in &source.tracks {
            let mut clips: Vec<ClipSnapshot> = source.clips[track.clips.clone()]
                .iter()
                .map(|clip| {
                    let length = clip.source_end.saturating_sub(clip.source_start) as u64;
                    total_length = total_length.max(clip.position.saturating_add(length));
                    let overview = self.overview(clip);
                    let snapshot = ClipSnapshot {
                        id: clip.id.0,
                        name: clip.data.name().to_owned(),
                        position: clip.position,
                        length,
                        gain_db: clip.gain.value(),
                        muted: clip.muted,
                        waveform_overview: overview.overview.clone(),
                    };
                    cache.insert(clip.id, overview);
                    snapshot
                })
                .collect();
            clips.sort_by_key(|clip| clip.position);
            tracks.push(TrackClipSnapshot {
                track_id: track.track_id.0,
                track_name: track.name.to_string(),
                clips,
                recording: None,
            });
        }
        self.overviews = cache;
        TimelineSnapshot {
            tracks,
            total_length,
        }
    }

    /// The overview for `clip`, from the cache when its audio and range are
    /// unchanged.
    fn overview(&mut self, clip: &TimelineClip) -> CachedOverview {
        let source = clip.source_start..clip.source_end;
        match self.overviews.remove(&clip.id) {
            Some(cached)
                if cached.data.shares_audio_with(&clip.data) && cached.source == source =>
            {
                cached
            }
            _ => CachedOverview {
                overview: crate::mixer::clip::waveform_overview(
                    clip.data.samples(),
                    source.start,
                    source.end,
                ),
                data: clip.data.clone(),
                source,
            },
        }
    }
}

impl std::fmt::Debug for Reclaimer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reclaimer")
            .field("backlog", &self.backlog.len())
            .field("takes", &self.takes.len())
            .field("cached_overviews", &self.overviews.len())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ringbuf::HeapRb;
    use ringbuf::traits::Split;

    /// A reclaimer and the callback's ends of its rings.
    struct Ends {
        reclaimer: Reclaimer,
        outbound: HeapProd<Outbound>,
        inbound: HeapCons<Inbound>,
        mailbox: TimelineMailbox,
    }

    fn ends(inbound_capacity: usize) -> Ends {
        let (outbound, outbound_cons) = HeapRb::<Outbound>::new(16).split();
        let (inbound_prod, inbound) = HeapRb::<Inbound>::new(inbound_capacity).split();
        let mailbox = TimelineMailbox::default();
        let reclaimer = Reclaimer::new(outbound_cons, inbound_prod, mailbox.clone());
        Ends {
            reclaimer,
            outbound,
            inbound,
            mailbox,
        }
    }

    fn chunk(level: f32, filled: usize) -> Vec<f32> {
        let mut chunk = vec![0.0_f32; 100];
        chunk[..filled].fill(level);
        chunk
    }

    fn end(take: TakeId, clip: u64, filled: usize, len: usize) -> Outbound {
        Outbound::TakeEnd(FinishedTake {
            take,
            track_id: TrackId(3),
            clip_id: ClipId(clip),
            position: 48,
            chunk: chunk(0.75, filled),
            filled,
            len,
            sample_rate: 48_000,
        })
    }

    fn next(ends: &mut Ends) -> Inbound {
        ends.inbound
            .try_pop()
            .expect("an item back from the reclaimer")
    }

    #[test]
    fn parcels_carry_one_value_in_and_out() {
        let mut parcel = Parcel::new(7_u32);
        assert_eq!(parcel.get(), Some(&7));
        assert_eq!(parcel.take(), Some(7));
        assert!(parcel.is_empty());
        assert_eq!(parcel.take(), None);
        assert_eq!(parcel.refill(9), None);
        assert_eq!(parcel.refill(11), Some(9));
        assert_eq!(parcel.slot_mut().take(), Some(11));
        assert!(parcel.is_empty());
        assert!(format!("{parcel:?}").contains("Parcel"));
    }

    #[test]
    fn resident_buffers_are_zeroed_to_length() {
        let buffer = resident_buffer(1000);
        assert_eq!(buffer.len(), 1000);
        assert!(buffer.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn chunks_are_assembled_into_a_clip_and_returned() {
        let mut ends = ends(8);
        let first = Outbound::TakeChunk {
            take: 5,
            chunk: chunk(0.25, 100),
            filled: 100,
        };
        assert!(ends.outbound.try_push(first).is_ok());
        assert!(ends.outbound.try_push(end(5, 12, 30, 120)).is_ok());
        assert_eq!(ends.reclaimer.service(), 2);

        // Both chunks come back for reuse before the clip.
        assert!(matches!(next(&mut ends), Inbound::Chunk(c) if c.len() == 100));
        assert!(matches!(next(&mut ends), Inbound::Chunk(c) if c.len() == 100));
        let Inbound::Clip { track_id, clip } = next(&mut ends) else {
            panic!("expected the clip");
        };
        assert_eq!(track_id, TrackId(3));
        assert_eq!(clip.id(), ClipId(12));
        assert_eq!(clip.position(), 48);
        assert_eq!(clip.name(), "Recording 12");
        assert_eq!(clip.data().original_sample_rate(), 48_000);
        // 100 samples of the first chunk, then 20 of the last (the clip
        // keeps 120 of the 130 recorded).
        let samples = clip.data().samples();
        assert_eq!(samples.len(), 120);
        assert!(samples[..100].iter().all(|&s| (s - 0.25).abs() < 1e-6));
        assert!(samples[100..].iter().all(|&s| (s - 0.75).abs() < 1e-6));
        assert!(ends.reclaimer.takes.is_empty());
    }

    #[test]
    fn an_aborted_take_is_discarded_and_its_chunk_returned() {
        let mut ends = ends(8);
        let first = Outbound::TakeChunk {
            take: 1,
            chunk: chunk(0.5, 100),
            filled: 100,
        };
        assert!(ends.outbound.try_push(first).is_ok());
        let abort = Outbound::TakeAbort {
            take: 1,
            chunk: chunk(0.5, 10),
        };
        assert!(ends.outbound.try_push(abort).is_ok());
        ends.reclaimer.service();
        assert!(matches!(next(&mut ends), Inbound::Chunk(_)));
        assert!(matches!(next(&mut ends), Inbound::Chunk(_)));
        assert!(ends.inbound.try_pop().is_none());
        assert!(ends.reclaimer.takes.is_empty());
    }

    #[test]
    fn returns_wait_in_order_when_the_callback_is_not_taking_them() {
        let mut ends = ends(1);
        for clip in 0..3 {
            assert!(ends.outbound.try_push(end(clip, clip, 10, 10)).is_ok());
        }
        ends.reclaimer.service();
        // Each end returns a chunk then a clip: six items, one delivered.
        assert_eq!(ends.reclaimer.backlog.len(), 5);

        let mut clips = Vec::new();
        for _ in 0..6 {
            match next(&mut ends) {
                Inbound::Clip { clip, .. } => clips.push(clip.id()),
                Inbound::Chunk(_) => {}
                Inbound::Timeline(_) => panic!("no timeline was sent"),
            }
            ends.reclaimer.service();
        }
        assert_eq!(clips, vec![ClipId(0), ClipId(1), ClipId(2)]);
        assert!(ends.reclaimer.backlog.is_empty());
    }

    #[test]
    fn timelines_are_built_sorted_and_the_source_returned_empty() {
        let mut mixer = Mixer::new();
        let a = mixer.add_track(
            "A".into(),
            crate::engine::create_synth(crate::synthesis::SynthesisMode::Passthrough, 44_100.0),
            crate::synthesis::SynthesisMode::Passthrough,
        );
        let data = ClipData::new(vec![0.25; 400], "Loop".into(), None, 44_100);
        let track = mixer.track_mut(a).unwrap();
        assert!(
            track
                .add_clip(AudioClip::new(ClipId(1), data.clone(), 900))
                .is_ok()
        );
        assert!(track.add_clip(AudioClip::new(ClipId(2), data, 100)).is_ok());

        let mut ends = ends(4);
        let mut source = TimelineSource::new();
        assert!(source.fill(&mixer));
        assert!(!source.fill(&mixer), "a full source must not be refilled");
        assert!(ends.outbound.try_push(Outbound::Timeline(source)).is_ok());
        ends.reclaimer.service();

        let timeline = ends.mailbox.take().unwrap();
        assert_eq!(timeline.tracks.len(), 1);
        assert_eq!(timeline.tracks[0].track_name, "A");
        let ids: Vec<u64> = timeline.tracks[0].clips.iter().map(|c| c.id).collect();
        assert_eq!(ids, vec![2, 1]);
        assert_eq!(timeline.tracks[0].clips[0].name, "Loop");
        assert!(!timeline.tracks[0].clips[0].waveform_overview.is_empty());
        assert_eq!(timeline.total_length, 1300);
        assert!(ends.mailbox.take().is_none(), "taken once");

        assert!(matches!(next(&mut ends), Inbound::Timeline(s) if s.is_empty()));
        assert_eq!(ends.reclaimer.overviews.len(), 2);
    }

    #[test]
    fn only_the_newest_timeline_is_built() {
        let mut ends = ends(8);
        let mixer = Mixer::new();
        for _ in 0..3 {
            let mut source = TimelineSource::new();
            assert!(source.fill(&mixer));
            assert!(ends.outbound.try_push(Outbound::Timeline(source)).is_ok());
        }
        ends.reclaimer.service();
        assert!(ends.mailbox.take().is_some());
        // All three sources come back, cleared.
        for _ in 0..3 {
            assert!(matches!(next(&mut ends), Inbound::Timeline(s) if s.is_empty()));
        }
    }

    #[test]
    fn no_timeline_is_built_once_nobody_reads_them() {
        let mut ends = ends(4);
        let Ends {
            reclaimer,
            outbound,
            inbound,
            mailbox,
        } = &mut ends;
        let mut source = TimelineSource::new();
        assert!(source.fill(&Mixer::new()));
        assert!(outbound.try_push(Outbound::Timeline(source)).is_ok());
        let reader = std::mem::take(mailbox);
        drop(reader);
        reclaimer.service();
        assert!(!reclaimer.mailbox.has_reader());
        assert!(reclaimer.mailbox.take().is_none());
        assert!(matches!(inbound.try_pop(), Some(Inbound::Timeline(_))));
    }

    #[test]
    fn retired_objects_are_freed_by_the_reclaimer() {
        let mut ends = ends(4);
        let data = ClipData::new(vec![0.0; 8], "Gone".into(), None, 44_100);
        assert!(
            ends.outbound
                .try_push(Outbound::Retired(Retired::ClipData(data.clone())))
                .is_ok()
        );
        assert_eq!(data.holders(), 2);
        assert_eq!(ends.reclaimer.service(), 1);
        assert_eq!(data.holders(), 1);
    }

    #[test]
    fn the_thread_exits_once_the_callback_is_gone() {
        let Ends {
            reclaimer,
            mut outbound,
            ..
        } = ends(4);
        assert!(outbound.try_push(end(0, 0, 4, 4)).is_ok());
        let worker = std::thread::spawn(move || reclaimer.run());
        drop(outbound);
        worker.join().unwrap();
    }
}
