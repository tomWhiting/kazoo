//! Command-path tests for the output callback: every object a command
//! brings in is applied, and everything replaced, removed or refused leaves
//! through the reclaim ring and is freed by the reclaimer, never here.
//!
//! Every piece of callback code the tests run goes through
//! [`on_audio_thread`], which fails the test if it allocates or frees
//! anything. The reclaimer's work is run inline ([`Rig::reclaim`]), so the
//! tests see exactly what the callback leaves in the ring. No audio device
//! is used.

use std::sync::atomic::{AtomicBool, Ordering};

use ringbuf::HeapRb;
use ringbuf::traits::Split;

use super::*;
use crate::engine::display::TimelineSnapshot;
use crate::engine::reclaim::{INBOUND_CAPACITY, OUTBOUND_CAPACITY, Reclaimer, TimelineMailbox};
use crate::mixer::clip::ClipData;

pub(super) const RATE: u32 = 44_100;
pub(super) const BLOCK: usize = 256;
/// Spectrum bins the test state and display frames hold.
const SPECTRUM: usize = 64;
/// Recording chunks in the pool.
const CHUNKS: usize = MAX_TRACKS * CHUNKS_PER_TRACK;

/// Run `f` as audio-thread code, failing the test if it allocates or frees
/// anything.
pub(super) fn on_audio_thread<T>(f: impl FnOnce() -> T) -> T {
    let before = assert_no_alloc::violation_count();
    let result = assert_no_alloc::assert_no_alloc(f);
    assert_eq!(
        assert_no_alloc::violation_count(),
        before,
        "the audio thread allocated or freed memory"
    );
    result
}

// ---------------------------------------------------------------------------
// Probes
// ---------------------------------------------------------------------------

/// A processor that records when it is dropped.
pub(super) struct Probe {
    name: &'static str,
    dropped: Arc<AtomicBool>,
}

impl Probe {
    /// A probe and the flag it sets when dropped.
    pub(super) fn boxed(name: &'static str) -> (Box<dyn Processor>, Arc<AtomicBool>) {
        let dropped = Arc::new(AtomicBool::new(false));
        let probe = Self {
            name,
            dropped: Arc::clone(&dropped),
        };
        (Box::new(probe), dropped)
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}

impl Processor for Probe {
    fn process(&mut self, input: &[f32], output: &mut [f32]) {
        let len = input.len().min(output.len());
        output[..len].copy_from_slice(&input[..len]);
    }

    fn reset(&mut self) {}

    fn name(&self) -> &str {
        self.name
    }

    fn set_sample_rate(&mut self, _sample_rate: f32) {}
}

fn dropped(flag: &AtomicBool) -> bool {
    flag.load(Ordering::SeqCst)
}

// ---------------------------------------------------------------------------
// Rig
// ---------------------------------------------------------------------------

/// The callback's state and IO, the reclaimer, and the far ends of every
/// ring and channel.
pub(super) struct Rig {
    pub(super) state: ProcessingState,
    pub(super) io: ProcessingIO,
    /// The reclaimer, run inline by [`Rig::reclaim`] (or taken to run on
    /// its own thread).
    pub(super) reclaimer: Option<Reclaimer>,
    pub(super) mic: HeapProd<f32>,
    pub(super) display: HeapCons<DisplayState>,
    pub(super) recycle: HeapProd<DisplayState>,
    pub(super) commands: Sender<EngineCommand>,
    pub(super) disk_commands: Receiver<DiskCommand>,
    pub(super) timelines: TimelineMailbox,
    pub(super) pitch: HeapProd<PitchEstimate>,
    pub(super) spectrum: HeapProd<Vec<f32>>,
    pub(super) formants: HeapProd<Option<FormantData>>,
    /// The next track id to give out, as the engine handle would.
    next_track: usize,
    _analysis: HeapCons<f32>,
    _disk: HeapCons<f32>,
}

impl Rig {
    /// A rig at [`RATE`] and [`BLOCK`], standalone.
    pub(super) fn new() -> Self {
        Self::build(RATE, BLOCK, OUTBOUND_CAPACITY, None)
    }

    /// A rig with one unarmed pitch-tracked track, "Test" (id 0).
    fn with_track() -> Self {
        let mut rig = Self::new();
        rig.add_track("Test", SynthesisMode::PitchTracked);
        rig
    }

    /// A rig with one armed pitch-tracked track, "Armed" (id 0).
    fn with_armed_track() -> Self {
        let mut rig = Self::new();
        let id = rig.add_track("Armed", SynthesisMode::PitchTracked);
        rig.apply(EngineCommand::SetTrackArm(id, true));
        rig
    }

    pub(super) fn build(
        rate: u32,
        block: usize,
        outbound_capacity: usize,
        desk: Option<HubLinkAudio>,
    ) -> Self {
        let counters = Arc::new(EngineStats::new());
        let state = ProcessingState::new(rate, block, SPECTRUM, Arc::clone(&counters));
        let (mic, mic_cons) = HeapRb::<f32>::new(block * 16).split();
        let (display_prod, display) = HeapRb::<DisplayState>::new(4).split();
        let (mut recycle, display_recycle) = HeapRb::<DisplayState>::new(8).split();
        for _ in 0..6 {
            assert!(
                recycle
                    .try_push(DisplayState::with_capacity(rate, SPECTRUM))
                    .is_ok()
            );
        }
        let (analysis_prod, analysis) = HeapRb::<f32>::new(block * 64).split();
        let (disk_prod, disk_samples) = HeapRb::<f32>::new(block * 64).split();
        let (pitch, pitch_cons) = HeapRb::<PitchEstimate>::new(4).split();
        let (spectrum, spectrum_cons) = HeapRb::<Vec<f32>>::new(4).split();
        let (formants, formant_cons) = HeapRb::<Option<FormantData>>::new(4).split();
        let (commands, command_rx) = crossbeam_channel::bounded(64);
        let (disk_cmd_tx, disk_commands) = crossbeam_channel::bounded(16);
        let (outbound_prod, outbound_cons) = HeapRb::<Outbound>::new(outbound_capacity).split();
        let (inbound_prod, inbound_cons) = HeapRb::<Inbound>::new(INBOUND_CAPACITY).split();
        let timelines = TimelineMailbox::default();
        let reclaimer = Reclaimer::new(outbound_cons, inbound_prod, timelines.clone());
        let io = ProcessingIO {
            mic_cons,
            display_prod,
            display_recycle,
            analysis_prod,
            disk_prod,
            pitch_cons,
            spectrum_cons,
            formant_cons,
            command_rx,
            disk_cmd_tx,
            reclaim: ReclaimLink::new(outbound_prod, counters),
            inbound: inbound_cons,
            desk,
        };
        Self {
            state,
            io,
            reclaimer: Some(reclaimer),
            mic,
            display,
            recycle,
            commands,
            disk_commands,
            timelines,
            pitch,
            spectrum,
            formants,
            next_track: 0,
            _analysis: analysis,
            _disk: disk_samples,
        }
    }

    /// Apply a command as the callback does, checking it neither
    /// allocates nor frees.
    pub(super) fn apply(&mut self, cmd: EngineCommand) {
        let (state, io) = (&mut self.state, &mut self.io);
        on_audio_thread(|| apply_command(cmd, state, io));
    }

    /// Render one callback of `out`, checking it neither allocates nor
    /// frees.
    pub(super) fn render(&mut self, out: &mut [f32]) {
        let (state, io) = (&mut self.state, &mut self.io);
        on_audio_thread(|| process_block(state, io, out));
    }

    /// Build a track with the next id, as the engine handle would.
    pub(super) fn build_track(
        &mut self,
        name: &str,
        synth: Box<dyn Processor>,
        mode: SynthesisMode,
    ) -> Track {
        let id = TrackId(self.next_track);
        self.next_track += 1;
        Track::new(
            id,
            name,
            synth,
            mode,
            self.state.sample_rate as f32,
            self.state.mic_block.len(),
        )
    }

    /// `synth` prepared for this engine.
    pub(super) fn prepared(&self, synth: Box<dyn Processor>) -> Prepared {
        Prepared::new(
            synth,
            self.state.sample_rate as f32,
            self.state.mic_block.len(),
        )
    }

    /// Add a track built as the engine handle builds one, returning its id.
    pub(super) fn add_track(&mut self, name: &str, mode: SynthesisMode) -> TrackId {
        let synth = create_synth(mode, self.state.sample_rate as f32);
        self.add_track_with(name, synth, mode)
    }

    /// Add a track whose primary synth is `synth`.
    pub(super) fn add_track_with(
        &mut self,
        name: &str,
        synth: Box<dyn Processor>,
        mode: SynthesisMode,
    ) -> TrackId {
        let track = self.build_track(name, synth, mode);
        let id = track.id();
        self.apply(EngineCommand::AddTrack {
            track: Parcel::new(track),
        });
        id
    }

    /// Run the reclaimer once, returning how many items it handled.
    pub(super) fn reclaim(&mut self) -> usize {
        self.reclaimer.as_mut().map_or(0, Reclaimer::service)
    }

    /// Run the reclaimer, then let the callback take back what it returned.
    fn settle(&mut self) {
        self.reclaim();
        let (state, io) = (&mut self.state, &mut self.io);
        on_audio_thread(|| {
            io.reclaim.flush();
            accept_returns(io, state);
        });
    }

    /// Put `value` in the first `samples` of the mic block and capture it
    /// into the active takes, as a rendered block would.
    fn capture(&mut self, samples: usize, value: f32) {
        self.state.mic_block[..samples].fill(value);
        let (state, io) = (&mut self.state, &mut self.io);
        on_audio_thread(|| capture_track_recordings(io, state, 0, samples));
    }

    /// The callback's command drain.
    fn drain_commands(&mut self) {
        let (state, io) = (&mut self.state, &mut self.io);
        on_audio_thread(|| drain_commands(io, state));
    }

    /// The callback's analysis drain.
    fn drain_analysis(&mut self) {
        let (state, io) = (&mut self.state, &mut self.io);
        on_audio_thread(|| drain_analysis_results(io, state));
    }

    /// The callback's timeline publishing.
    fn publish_timeline(&mut self) {
        let (state, io) = (&mut self.state, &mut self.io);
        on_audio_thread(|| publish_timeline(io, state));
    }

    /// The callback's display publishing.
    fn push_display(&mut self, input_level_db: f32, cpu_load: f32) {
        let (state, io) = (&mut self.state, &mut self.io);
        on_audio_thread(|| push_display_state(io, state, input_level_db, cpu_load));
    }

    fn stats(&self) -> crate::engine::EngineStatsSnapshot {
        self.state.stats.snapshot()
    }

    fn clips(&self, track: TrackId) -> &[AudioClip] {
        self.state.mixer.track(track).unwrap().clips()
    }

    fn add_clip(&mut self, track_id: TrackId, len: usize, position: u64) {
        let clip_data = test_clip_data(len);
        self.apply(EngineCommand::AddClip {
            track_id,
            clip_data,
            position,
        });
    }

    /// Publish the timeline and build it, returning the snapshot.
    fn timeline(&mut self) -> TimelineSnapshot {
        let (state, io) = (&mut self.state, &mut self.io);
        on_audio_thread(|| publish_timeline(io, state));
        self.settle();
        self.timelines.take().expect("a timeline snapshot")
    }
}

impl Drop for Rig {
    /// Every test ends by proving the callback never fell back to freeing
    /// an object itself.
    fn drop(&mut self) {
        if !std::thread::panicking() {
            assert_eq!(
                self.state.stats.snapshot().callback_frees,
                0,
                "the callback freed an object on the audio thread"
            );
        }
    }
}

/// Create test clip data of a given length.
fn test_clip_data(len: usize) -> ClipData {
    let samples: Vec<f32> = (0..len).map(|i| (i as f32) / len as f32).collect();
    ClipData::new(samples, "TestClip".into(), None, RATE)
}

// ---------------------------------------------------------------------------
// Synth factory and initial state
// ---------------------------------------------------------------------------

#[test]
fn create_synth_names_every_mode() {
    for (mode, name) in [
        (SynthesisMode::Passthrough, "Passthrough"),
        (SynthesisMode::PitchTracked, "Pitch Tracked Synth"),
        (SynthesisMode::Wavetable, "Wavetable Oscillator"),
        (SynthesisMode::Granular, "Granular Synth"),
        (SynthesisMode::Vocoder, "Vocoder"),
        (SynthesisMode::PhaseVocoder, "Phase Vocoder"),
    ] {
        assert_eq!(create_synth(mode, 44_100.0).name(), name);
        let prepared = prepared_synth(mode, 48_000.0, 512);
        assert_eq!(prepared.processor().name(), name);
        assert!(prepared.fits(48_000.0, 512));
        assert!(!prepared.fits(48_000.0, 1024));
        assert!(!prepared.fits(44_100.0, 512));
    }
}

#[test]
fn processing_state_initializes_correctly() {
    let state = ProcessingState::new(44_100, 256, SPECTRUM, Arc::default());
    assert_eq!(state.sample_rate, 44_100);
    assert_eq!(state.mic_block.len(), 256);
    assert!(!state.is_recording);
    assert!(state.latest_pitch.frequency.is_none());
    assert!(state.spectrum.is_empty());
    assert_eq!(state.formants.num_formants, 0);
    assert_eq!(state.next_clip_id, 0);
    assert!(state.takes.active.is_empty());
    assert!(!state.timeline_dirty);
    // Every collection the callback grows is created at its maximum.
    assert!(state.spectrum.capacity() >= SPECTRUM);
    assert!(state.takes.active.capacity() >= MAX_TRACKS);
    assert!(state.parcels.capacity() >= MAX_TRACKS);
    assert_eq!(state.timelines.len(), TIMELINE_SOURCES);
    // The recording chunks exist from the start, a quarter second each.
    assert_eq!(state.takes.pool.len(), CHUNKS);
    assert!(state.takes.pool.iter().all(|chunk| chunk.len() == 11_025));
}

#[test]
fn processing_state_initializes_at_48k_512() {
    let state = ProcessingState::new(48_000, 512, SPECTRUM, Arc::default());
    assert_eq!(state.sample_rate, 48_000);
    assert_eq!(state.mic_block.len(), 512);
    assert!(state.takes.pool.iter().all(|chunk| chunk.len() == 12_000));
}

// ---------------------------------------------------------------------------
// Tracks
// ---------------------------------------------------------------------------

#[test]
fn add_track_places_the_built_track() {
    let mut rig = Rig::new();
    let (synth, synth_dropped) = Probe::boxed("Probe");
    let id = rig.add_track_with("Lead", synth, SynthesisMode::Passthrough);

    let track = rig.state.mixer.track(id).unwrap();
    assert_eq!(track.id(), TrackId(0));
    assert_eq!(track.name(), "Lead");
    assert_eq!(track.synth().name(), "Probe");
    assert_eq!(rig.state.parcels.len(), 1);
    assert!(rig.state.parcels[0].is_empty());
    assert!(rig.state.timeline_dirty);

    // Nothing was retired.
    assert_eq!(rig.reclaim(), 0);
    assert!(!dropped(&synth_dropped));
}

#[test]
fn add_track_beyond_the_limit_is_refused_and_freed_off_the_callback() {
    let mut rig = Rig::new();
    for i in 0..MAX_TRACKS {
        rig.add_track(&format!("{i}"), SynthesisMode::Passthrough);
    }
    let (synth, synth_dropped) = Probe::boxed("Extra");
    let track = rig.build_track("Extra", synth, SynthesisMode::Passthrough);
    rig.apply(EngineCommand::AddTrack {
        track: Parcel::new(track),
    });

    assert_eq!(rig.state.mixer.track_count(), MAX_TRACKS);
    assert_eq!(rig.stats().tracks_rejected, 1);
    // The refused track waits in the ring, still alive.
    assert!(!dropped(&synth_dropped));
    assert_eq!(rig.reclaim(), 1);
    assert!(dropped(&synth_dropped));
}

#[test]
fn add_track_built_for_smaller_blocks_or_another_rate_is_refused() {
    let mut rig = Rig::new();
    for (rate, block) in [(RATE as f32, BLOCK / 2), (48_000.0, BLOCK)] {
        let track = Track::new(
            TrackId(0),
            "Wrong",
            create_synth(SynthesisMode::Passthrough, rate),
            SynthesisMode::Passthrough,
            rate,
            block,
        );
        rig.apply(EngineCommand::AddTrack {
            track: Parcel::new(track),
        });
    }
    assert_eq!(rig.state.mixer.track_count(), 0);
    assert_eq!(rig.stats().tracks_rejected, 2);
    assert_eq!(rig.reclaim(), 2);
}

#[test]
fn remove_track_sends_the_track_out_in_its_parcel() {
    let mut rig = Rig::new();
    let (synth, synth_dropped) = Probe::boxed("Doomed");
    let id = rig.add_track_with("Doomed", synth, SynthesisMode::Passthrough);
    let keep = rig.add_track("Keep", SynthesisMode::Passthrough);

    rig.apply(EngineCommand::RemoveTrack(id));
    assert!(rig.state.mixer.track(id).is_none());
    assert!(rig.state.mixer.track(keep).is_some());
    assert_eq!(rig.state.parcels.len(), 1);

    assert!(!dropped(&synth_dropped), "freed on the callback");
    assert_eq!(rig.reclaim(), 1);
    assert!(dropped(&synth_dropped));
}

#[test]
fn remove_missing_track_retires_nothing() {
    let mut rig = Rig::with_track();
    rig.apply(EngineCommand::RemoveTrack(TrackId(42)));
    assert_eq!(rig.state.mixer.track_count(), 1);
    assert_eq!(rig.reclaim(), 0);
}

#[test]
fn set_synthesis_mode_applies_the_new_synth_and_returns_the_old() {
    let mut rig = Rig::new();
    let (old, old_dropped) = Probe::boxed("Old");
    let id = rig.add_track_with("T", old, SynthesisMode::Passthrough);
    let (new, new_dropped) = Probe::boxed("New");
    let synth = rig.prepared(new);

    rig.apply(EngineCommand::SetTrackSynthesisMode {
        track_id: id,
        synth,
        mode: SynthesisMode::Granular,
    });
    let track = rig.state.mixer.track(id).unwrap();
    assert_eq!(track.synth().name(), "New");
    assert_eq!(track.layer(0).unwrap().mode(), SynthesisMode::Granular);

    assert!(!dropped(&old_dropped), "old synth freed on the callback");
    assert_eq!(rig.reclaim(), 1);
    assert!(dropped(&old_dropped));
    assert!(!dropped(&new_dropped));
}

#[test]
fn set_synthesis_mode_for_a_missing_track_retires_the_new_synth() {
    let mut rig = Rig::new();
    let (new, new_dropped) = Probe::boxed("New");
    let synth = rig.prepared(new);
    rig.apply(EngineCommand::SetTrackSynthesisMode {
        track_id: TrackId(3),
        synth,
        mode: SynthesisMode::Granular,
    });
    assert!(!dropped(&new_dropped));
    assert_eq!(rig.reclaim(), 1);
    assert!(dropped(&new_dropped));
    assert_eq!(
        rig.stats().processors_rejected,
        0,
        "no track is not a refusal"
    );
}

#[test]
fn an_under_prepared_synth_is_refused_not_run() {
    let mut rig = Rig::with_track();
    // A vocoder prepared for smaller blocks than the engine renders.
    let small = Prepared::new(
        create_synth(SynthesisMode::Vocoder, RATE as f32),
        RATE as f32,
        BLOCK / 4,
    );
    rig.apply(EngineCommand::SetTrackSynthesisMode {
        track_id: TrackId(0),
        synth: small,
        mode: SynthesisMode::Vocoder,
    });
    let track = rig.state.mixer.track(TrackId(0)).unwrap();
    assert_eq!(track.layer(0).unwrap().mode(), SynthesisMode::PitchTracked);
    assert_eq!(rig.stats().processors_rejected, 1);
    assert_eq!(rig.reclaim(), 1);
}

// ---------------------------------------------------------------------------
// Synth layers and effects
// ---------------------------------------------------------------------------

#[test]
fn synth_layers_are_added_and_removed_without_freeing_on_the_callback() {
    let mut rig = Rig::with_track();
    let (synth, layer_dropped) = Probe::boxed("Layer");
    let layer = SynthLayer::new(rig.prepared(synth), SynthesisMode::Wavetable, "Pad".into());
    rig.apply(EngineCommand::AddSynthLayer {
        track_id: TrackId(0),
        layer,
    });
    let track = rig.state.mixer.track(TrackId(0)).unwrap();
    assert_eq!(track.layer_count(), 2);
    assert_eq!(track.layer(1).unwrap().label(), "Pad");
    assert_eq!(rig.reclaim(), 0);

    rig.apply(EngineCommand::RemoveSynthLayer {
        track_id: TrackId(0),
        layer_index: 1,
    });
    assert_eq!(rig.state.mixer.track(TrackId(0)).unwrap().layer_count(), 1);
    assert!(!dropped(&layer_dropped));
    assert_eq!(rig.reclaim(), 1);
    assert!(dropped(&layer_dropped));
}

#[test]
fn refused_synth_layers_are_retired() {
    let mut rig = Rig::with_track();
    for _ in 1..crate::MAX_SYNTH_LAYERS {
        let (synth, _) = Probe::boxed("Fill");
        let layer = SynthLayer::new(rig.prepared(synth), SynthesisMode::Wavetable, "Fill".into());
        rig.apply(EngineCommand::AddSynthLayer {
            track_id: TrackId(0),
            layer,
        });
    }
    let (extra, extra_dropped) = Probe::boxed("Extra");
    let layer = SynthLayer::new(
        rig.prepared(extra),
        SynthesisMode::Wavetable,
        "Extra".into(),
    );
    rig.apply(EngineCommand::AddSynthLayer {
        track_id: TrackId(0),
        layer,
    });
    let (orphan, orphan_dropped) = Probe::boxed("Orphan");
    let layer = SynthLayer::new(
        rig.prepared(orphan),
        SynthesisMode::Wavetable,
        "Orphan".into(),
    );
    rig.apply(EngineCommand::AddSynthLayer {
        track_id: TrackId(9),
        layer,
    });
    assert_eq!(
        rig.state.mixer.track(TrackId(0)).unwrap().layer_count(),
        crate::MAX_SYNTH_LAYERS
    );
    assert_eq!(rig.stats().processors_rejected, 1);
    assert!(!dropped(&extra_dropped) && !dropped(&orphan_dropped));
    assert_eq!(rig.reclaim(), 2);
    assert!(dropped(&extra_dropped) && dropped(&orphan_dropped));
}

#[test]
fn effects_are_added_removed_and_refused_without_freeing_on_the_callback() {
    let mut rig = Rig::with_track();
    let (effect, effect_dropped) = Probe::boxed("Effect");
    let effect = rig.prepared(effect);
    rig.apply(EngineCommand::AddEffect {
        track_id: TrackId(0),
        effect,
    });
    assert_eq!(
        rig.state.mixer.track(TrackId(0)).unwrap().effects().len(),
        1
    );
    assert_eq!(rig.reclaim(), 0);

    rig.apply(EngineCommand::RemoveEffect {
        track_id: TrackId(0),
        effect_index: 0,
    });
    assert!(
        rig.state
            .mixer
            .track(TrackId(0))
            .unwrap()
            .effects()
            .is_empty()
    );
    assert!(!dropped(&effect_dropped));
    assert_eq!(rig.reclaim(), 1);
    assert!(dropped(&effect_dropped));

    for _ in 0..crate::MAX_EFFECTS_PER_TRACK {
        let (fill, _) = Probe::boxed("Fill");
        let effect = rig.prepared(fill);
        rig.apply(EngineCommand::AddEffect {
            track_id: TrackId(0),
            effect,
        });
    }
    let (extra, extra_dropped) = Probe::boxed("Extra");
    let effect = rig.prepared(extra);
    rig.apply(EngineCommand::AddEffect {
        track_id: TrackId(0),
        effect,
    });
    let (orphan, orphan_dropped) = Probe::boxed("Orphan");
    let effect = rig.prepared(orphan);
    rig.apply(EngineCommand::AddEffect {
        track_id: TrackId(7),
        effect,
    });
    assert_eq!(
        rig.state.mixer.track(TrackId(0)).unwrap().effects().len(),
        crate::MAX_EFFECTS_PER_TRACK
    );
    assert_eq!(rig.stats().processors_rejected, 1);
    assert!(!dropped(&extra_dropped) && !dropped(&orphan_dropped));
    assert_eq!(rig.reclaim(), 2);
    assert!(dropped(&extra_dropped) && dropped(&orphan_dropped));
}

#[test]
fn valid_effect_parameter_is_applied() {
    let mut rig = Rig::with_track();
    let track_id = TrackId(0);
    let effect = rig.prepared(Box::new(crate::effects::Delay::new(44_100.0)));
    rig.apply(EngineCommand::AddEffect { track_id, effect });
    rig.apply(EngineCommand::SetEffectParameter {
        track_id,
        effect_index: 0,
        // Delay parameter 2 is its wet/dry mix.
        param_index: 2,
        value: 0.25,
    });
    assert_eq!(rig.stats().params_rejected, 0);
}

// ---------------------------------------------------------------------------
// Clips
// ---------------------------------------------------------------------------

#[test]
fn apply_add_clip_assigns_unique_ids() {
    let mut rig = Rig::with_track();
    rig.add_clip(TrackId(0), 100, 0);
    rig.add_clip(TrackId(0), 200, 1000);

    assert_eq!(rig.state.next_clip_id, 2);
    let clips = rig.clips(TrackId(0));
    assert_eq!(clips.len(), 2);
    assert_eq!(clips[0].id(), ClipId(0));
    assert_eq!(clips[1].id(), ClipId(1));
}

#[test]
fn apply_add_clip_nonexistent_track_retires_the_audio() {
    let mut rig = Rig::with_track();
    let data = test_clip_data(100);
    rig.apply(EngineCommand::AddClip {
        track_id: TrackId(999),
        clip_data: data.clone(),
        position: 0,
    });
    // next_clip_id should not have been incremented.
    assert_eq!(rig.state.next_clip_id, 0);
    assert_eq!(rig.stats().clips_rejected, 1);
    // The refused audio is alive in the ring until the reclaimer frees it.
    assert_eq!(data.holders(), 2);
    assert_eq!(rig.reclaim(), 1);
    assert_eq!(data.holders(), 1);
}

#[test]
fn apply_remove_clip_removes_correct_clip_and_retires_it() {
    let mut rig = Rig::with_track();
    let data = test_clip_data(100);
    rig.apply(EngineCommand::AddClip {
        track_id: TrackId(0),
        clip_data: data.clone(),
        position: 0,
    });
    rig.add_clip(TrackId(0), 50, 500);
    assert_eq!(rig.clips(TrackId(0)).len(), 2);

    rig.apply(EngineCommand::RemoveClip {
        track_id: TrackId(0),
        clip_id: ClipId(0),
    });
    let clips = rig.clips(TrackId(0));
    assert_eq!(clips.len(), 1);
    assert_eq!(clips[0].id(), ClipId(1));

    // The last reference outside this test is dropped by the reclaimer.
    assert_eq!(data.holders(), 2);
    assert_eq!(rig.reclaim(), 1);
    assert_eq!(data.holders(), 1);
}

#[test]
fn apply_remove_clip_nonexistent_is_noop() {
    let mut rig = Rig::with_track();
    rig.apply(EngineCommand::RemoveClip {
        track_id: TrackId(0),
        clip_id: ClipId(99),
    });
    assert!(rig.clips(TrackId(0)).is_empty());
    assert_eq!(rig.reclaim(), 0);
}

#[test]
fn apply_move_clip_changes_position() {
    let mut rig = Rig::with_track();
    rig.add_clip(TrackId(0), 100, 0);
    rig.state.timeline_dirty = false;
    rig.apply(EngineCommand::MoveClip {
        track_id: TrackId(0),
        clip_id: ClipId(0),
        new_position: 5000,
    });
    assert_eq!(rig.clips(TrackId(0))[0].position(), 5000);
    assert!(rig.state.timeline_dirty);
}

#[test]
fn apply_move_clip_nonexistent_clip_is_noop() {
    let mut rig = Rig::with_track();
    rig.state.timeline_dirty = false;
    rig.apply(EngineCommand::MoveClip {
        track_id: TrackId(0),
        clip_id: ClipId(99),
        new_position: 500,
    });
    assert!(rig.clips(TrackId(0)).is_empty());
    assert!(!rig.state.timeline_dirty);
}

#[test]
fn apply_trim_clip_start_and_end() {
    let mut rig = Rig::with_track();
    rig.add_clip(TrackId(0), 100, 0);
    rig.state.timeline_dirty = false;
    rig.apply(EngineCommand::TrimClipStart {
        track_id: TrackId(0),
        clip_id: ClipId(0),
        samples: 10,
    });
    assert!(rig.state.timeline_dirty);
    rig.state.timeline_dirty = false;
    rig.apply(EngineCommand::TrimClipEnd {
        track_id: TrackId(0),
        clip_id: ClipId(0),
        samples: 20,
    });
    assert!(rig.state.timeline_dirty);
    let clip = &rig.clips(TrackId(0))[0];
    assert_eq!(clip.source_start(), 10);
    assert_eq!(clip.source_end(), 80);
    assert_eq!(clip.effective_length(), 70);
}

#[test]
fn apply_split_clip_creates_two_clips_sharing_audio() {
    let mut rig = Rig::with_track();
    rig.add_clip(TrackId(0), 100, 1000);
    rig.state.timeline_dirty = false;
    rig.apply(EngineCommand::SplitClip {
        track_id: TrackId(0),
        clip_id: ClipId(0),
        split_position: 1040,
    });
    let clips = rig.clips(TrackId(0));
    assert_eq!(clips.len(), 2);
    assert_eq!(clips[0].effective_length(), 40);
    assert_eq!(clips[1].id(), ClipId(1));
    assert_eq!(clips[1].position(), 1040);
    assert_eq!(clips[1].effective_length(), 60);
    assert!(clips[0].data().shares_audio_with(clips[1].data()));
    assert_eq!(rig.state.next_clip_id, 2);
    assert!(rig.state.timeline_dirty);
}

#[test]
fn apply_split_clip_at_boundary_is_noop() {
    let mut rig = Rig::with_track();
    rig.add_clip(TrackId(0), 100, 1000);
    rig.apply(EngineCommand::SplitClip {
        track_id: TrackId(0),
        clip_id: ClipId(0),
        split_position: 1000,
    });
    assert_eq!(rig.clips(TrackId(0)).len(), 1);
    assert_eq!(rig.state.next_clip_id, 1);
}

#[test]
fn apply_set_clip_gain_and_mute() {
    let mut rig = Rig::with_track();
    rig.add_clip(TrackId(0), 100, 0);
    rig.state.timeline_dirty = false;
    rig.apply(EngineCommand::SetClipGain {
        track_id: TrackId(0),
        clip_id: ClipId(0),
        gain: Db::new(-6.0),
    });
    assert!(rig.state.timeline_dirty);
    rig.state.timeline_dirty = false;
    rig.apply(EngineCommand::SetClipMute {
        track_id: TrackId(0),
        clip_id: ClipId(0),
        muted: true,
    });
    assert!(rig.state.timeline_dirty);
    let clip = &rig.clips(TrackId(0))[0];
    assert!((clip.gain().value() + 6.0).abs() < f32::EPSILON);
    assert!(clip.is_muted());
}

#[test]
fn apply_duplicate_clip_shares_the_audio() {
    let mut rig = Rig::with_track();
    rig.add_clip(TrackId(0), 100, 0);
    rig.state.timeline_dirty = false;
    rig.apply(EngineCommand::DuplicateClip {
        track_id: TrackId(0),
        clip_id: ClipId(0),
        new_position: 500,
    });
    let clips = rig.clips(TrackId(0));
    assert_eq!(clips.len(), 2);
    assert_eq!(clips[1].id(), ClipId(1));
    assert_eq!(clips[1].position(), 500);
    assert!(clips[0].data().shares_audio_with(clips[1].data()));
    assert!(rig.state.timeline_dirty);
}

#[test]
fn apply_duplicate_clip_nonexistent_is_noop() {
    let mut rig = Rig::with_track();
    rig.apply(EngineCommand::DuplicateClip {
        track_id: TrackId(0),
        clip_id: ClipId(99),
        new_position: 500,
    });
    assert!(rig.clips(TrackId(0)).is_empty());
    assert_eq!(rig.state.next_clip_id, 0);
}

#[test]
fn add_clip_to_missing_or_full_track_is_counted() {
    let mut rig = Rig::with_track();
    rig.add_clip(TrackId(999), 10, 0);
    assert_eq!(rig.stats().clips_rejected, 1);

    for i in 0..MAX_CLIPS_PER_TRACK {
        rig.add_clip(TrackId(0), 10, (i as u64) * 100);
    }
    assert_eq!(rig.stats().clips_rejected, 1);

    rig.add_clip(TrackId(0), 10, 1_000_000);
    assert_eq!(rig.stats().clips_rejected, 2);
    assert_eq!(rig.clips(TrackId(0)).len(), MAX_CLIPS_PER_TRACK);
    // Both refusals left through the ring.
    assert_eq!(rig.reclaim(), 2);
}

#[test]
fn split_on_full_track_leaves_clip_intact() {
    let mut rig = Rig::with_track();
    for i in 0..MAX_CLIPS_PER_TRACK {
        rig.add_clip(TrackId(0), 100, (i as u64) * 1000);
    }
    rig.apply(EngineCommand::SplitClip {
        track_id: TrackId(0),
        clip_id: ClipId(0),
        split_position: 50,
    });
    let clip = rig
        .state
        .mixer
        .track(TrackId(0))
        .unwrap()
        .find_clip(ClipId(0))
        .unwrap();
    assert_eq!(clip.effective_length(), 100, "no audio may be cut off");
    assert_eq!(rig.stats().clips_rejected, 1);
}

#[test]
fn clip_capacity_is_reserved_so_filling_a_track_never_reallocates() {
    let mut rig = Rig::with_track();
    rig.add_clip(TrackId(0), 10, 0);
    let slots = rig.clips(TrackId(0)).as_ptr();
    for i in 1..MAX_CLIPS_PER_TRACK {
        rig.add_clip(TrackId(0), 10, (i as u64) * 100);
    }
    assert_eq!(rig.clips(TrackId(0)).as_ptr(), slots);
}

// ---------------------------------------------------------------------------
// Recording
// ---------------------------------------------------------------------------

#[test]
fn start_recording_takes_a_chunk_from_the_pool() {
    let mut rig = Rig::with_armed_track();
    rig.apply(EngineCommand::Transport(TransportCommand::Record));

    assert_eq!(rig.state.takes.active.len(), 1);
    let take = &rig.state.takes.active[0];
    assert_eq!(take.track_id, TrackId(0));
    assert_eq!(take.recorded, 0);
    assert_eq!(take.chunk.len(), chunk_len(RATE));
    assert_eq!(rig.state.takes.pool.len(), CHUNKS - 1);
}

#[test]
fn start_recording_unarmed_track_not_recorded() {
    let mut rig = Rig::with_track();
    rig.apply(EngineCommand::Transport(TransportCommand::Record));
    assert!(rig.state.takes.active.is_empty());
    assert_eq!(rig.state.takes.pool.len(), CHUNKS);
}

#[test]
fn capture_track_recordings_fills_the_chunk() {
    let mut rig = Rig::with_armed_track();
    rig.apply(EngineCommand::Transport(TransportCommand::Record));
    rig.capture(64, 0.5);
    assert_eq!(rig.state.takes.active[0].recorded, 64);
    assert_eq!(rig.state.takes.active[0].filled, 64);
}

#[test]
fn stop_recording_sends_the_take_out_and_places_the_returned_clip() {
    let mut rig = Rig::with_armed_track();
    rig.apply(EngineCommand::Transport(TransportCommand::Record));
    for _ in 0..4 {
        rig.capture(64, 0.5);
    }
    assert_eq!(rig.state.takes.active[0].recorded, 256);

    rig.apply(EngineCommand::Transport(TransportCommand::Stop));
    // The take has left the callback; its clip is not placed yet.
    assert!(rig.state.takes.active.is_empty());
    assert_eq!(rig.state.takes.pool.len(), CHUNKS - 1);
    assert!(rig.clips(TrackId(0)).is_empty());
    assert_eq!(rig.state.next_clip_id, 1);

    rig.settle();
    let clips = rig.clips(TrackId(0));
    assert_eq!(clips.len(), 1);
    assert_eq!(clips[0].id(), ClipId(0));
    assert_eq!(clips[0].effective_length(), 256);
    assert_eq!(clips[0].name(), "Recording 0");
    assert_eq!(clips[0].data().len(), 256);
    assert!(
        clips[0]
            .data()
            .samples()
            .iter()
            .all(|&s| (s - 0.5).abs() < 1e-6)
    );
    assert!(rig.state.timeline_dirty);
    // The chunk came back to the pool.
    assert_eq!(rig.state.takes.pool.len(), CHUNKS);
}

#[test]
fn a_long_take_spans_many_chunks_and_comes_back_whole() {
    let mut rig = Rig::with_armed_track();
    rig.apply(EngineCommand::Transport(TransportCommand::Record));
    // Just over three chunks, each block a different level.
    let blocks = chunk_len(RATE) * 3 / BLOCK + 2;
    for block in 0..blocks {
        rig.capture(BLOCK, block as f32 / 1000.0);
        // The reclaimer returns full chunks while recording goes on.
        rig.settle();
    }
    assert_eq!(rig.state.takes.pool.len(), CHUNKS - 1);
    rig.apply(EngineCommand::Transport(TransportCommand::Stop));
    rig.settle();

    let clip = &rig.clips(TrackId(0))[0];
    assert_eq!(clip.effective_length(), blocks * BLOCK);
    for (block, samples) in clip.data().samples().chunks(BLOCK).enumerate() {
        let level = block as f32 / 1000.0;
        assert!(
            samples.iter().all(|&s| (s - level).abs() < 1e-6),
            "block {block}"
        );
    }
    assert_eq!(rig.state.takes.pool.len(), CHUNKS);
    assert_eq!(rig.stats().take_samples_dropped, 0);
}

#[test]
fn recording_with_no_free_chunk_is_counted_not_allocated() {
    let mut rig = Rig::with_armed_track();
    rig.apply(EngineCommand::Transport(TransportCommand::Record));
    // The reclaim thread never returns a chunk: the pool runs dry.
    let mut captured = 0;
    while rig.stats().take_samples_dropped == 0 {
        rig.capture(BLOCK, 0.25);
        captured += BLOCK;
        assert!(captured <= (CHUNKS + 1) * chunk_len(RATE), "never ran dry");
    }
    assert!(rig.state.takes.pool.is_empty());
    let dropped_samples = rig.stats().take_samples_dropped;
    assert_eq!(
        rig.state.takes.active[0].recorded + dropped_samples as usize,
        captured
    );
}

#[test]
fn pause_recording_creates_clip() {
    let mut rig = Rig::with_armed_track();
    rig.apply(EngineCommand::Transport(TransportCommand::Record));
    rig.capture(64, 0.5);
    // Pause also finalizes recordings.
    rig.apply(EngineCommand::Transport(TransportCommand::Pause));
    assert!(rig.state.takes.active.is_empty());
    rig.settle();
    let clips = rig.clips(TrackId(0));
    assert_eq!(clips.len(), 1);
    assert_eq!(clips[0].effective_length(), 64);
}

#[test]
fn recording_empty_produces_no_clip_and_keeps_the_chunk() {
    let mut rig = Rig::with_armed_track();
    rig.apply(EngineCommand::Transport(TransportCommand::Record));
    rig.apply(EngineCommand::Transport(TransportCommand::Stop));
    assert!(rig.state.takes.active.is_empty());
    assert_eq!(rig.state.takes.pool.len(), CHUNKS, "unused chunk kept");
    assert_eq!(rig.reclaim(), 0);
    assert!(rig.clips(TrackId(0)).is_empty());
    assert_eq!(rig.state.next_clip_id, 0);
}

#[test]
fn multiple_armed_tracks_record_independently() {
    let mut rig = Rig::new();
    let a = rig.add_track("A", SynthesisMode::PitchTracked);
    let b = rig.add_track("B", SynthesisMode::PitchTracked);
    rig.apply(EngineCommand::SetTrackArm(a, true));
    rig.apply(EngineCommand::SetTrackArm(b, true));

    rig.apply(EngineCommand::Transport(TransportCommand::Record));
    assert_eq!(rig.state.takes.active.len(), 2);
    rig.capture(32, 0.5);
    rig.apply(EngineCommand::Transport(TransportCommand::Stop));
    rig.settle();

    for track in [a, b] {
        let clips = rig.clips(track);
        assert_eq!(clips.len(), 1);
        assert_eq!(clips[0].effective_length(), 32);
    }
    assert_eq!(rig.state.takes.pool.len(), CHUNKS);
}

#[test]
fn double_record_command_does_not_start_a_second_take() {
    let mut rig = Rig::with_armed_track();
    rig.apply(EngineCommand::Transport(TransportCommand::Record));
    assert_eq!(rig.state.takes.active.len(), 1);
    // Second Record should be a no-op (the track is already recording).
    rig.apply(EngineCommand::Transport(TransportCommand::Record));
    assert_eq!(rig.state.takes.active.len(), 1);
    assert_eq!(rig.stats().takes_unavailable, 0);
}

#[test]
fn recording_again_right_after_stop_works() {
    let mut rig = Rig::with_armed_track();
    rig.apply(EngineCommand::Transport(TransportCommand::Record));
    rig.capture(64, 0.5);
    rig.apply(EngineCommand::Transport(TransportCommand::Stop));
    // The reclaimer has not caught up, and it does not matter.
    rig.apply(EngineCommand::Transport(TransportCommand::Record));
    assert_eq!(rig.state.takes.active.len(), 1);
    rig.capture(32, 0.25);
    rig.apply(EngineCommand::Transport(TransportCommand::Stop));
    rig.settle();
    let lengths: Vec<usize> = rig
        .clips(TrackId(0))
        .iter()
        .map(AudioClip::effective_length)
        .collect();
    assert_eq!(lengths, vec![64, 32]);
    assert_eq!(rig.stats().takes_unavailable, 0);
}

#[test]
fn removing_a_track_mid_take_abandons_the_take() {
    let mut rig = Rig::with_armed_track();
    rig.apply(EngineCommand::Transport(TransportCommand::Record));
    rig.capture(64, 0.5);
    rig.apply(EngineCommand::RemoveTrack(TrackId(0)));
    assert!(rig.state.takes.active.is_empty());
    rig.apply(EngineCommand::Transport(TransportCommand::Stop));
    rig.settle();
    assert_eq!(rig.stats().clips_rejected, 0, "nothing was refused");
    assert_eq!(rig.state.next_clip_id, 0);
    assert_eq!(rig.state.takes.pool.len(), CHUNKS, "the chunk came back");
}

#[test]
fn disarming_mid_take_ends_the_take_and_arming_starts_one() {
    let mut rig = Rig::new();
    let a = rig.add_track("A", SynthesisMode::Passthrough);
    let b = rig.add_track("B", SynthesisMode::Passthrough);
    rig.apply(EngineCommand::SetTrackArm(a, true));
    rig.apply(EngineCommand::Transport(TransportCommand::Record));
    rig.capture(64, 0.5);

    rig.apply(EngineCommand::SetTrackArm(a, false));
    rig.apply(EngineCommand::SetTrackArm(b, true));
    assert_eq!(rig.state.takes.active.len(), 1);
    assert_eq!(rig.state.takes.active[0].track_id, b);
    rig.capture(32, 0.25);
    rig.apply(EngineCommand::Transport(TransportCommand::Stop));
    rig.settle();
    assert_eq!(rig.clips(a)[0].effective_length(), 64);
    assert_eq!(rig.clips(b)[0].effective_length(), 32);
}

#[test]
fn seeking_mid_take_starts_a_new_take_where_the_transport_lands() {
    let mut rig = Rig::with_armed_track();
    rig.apply(EngineCommand::Transport(TransportCommand::Record));
    rig.capture(64, 0.5);
    rig.apply(EngineCommand::Transport(TransportCommand::Seek(10_000)));
    rig.capture(32, 0.25);
    rig.apply(EngineCommand::Transport(TransportCommand::Stop));
    rig.settle();
    let clips: Vec<(u64, usize)> = rig
        .clips(TrackId(0))
        .iter()
        .map(|clip| (clip.position(), clip.effective_length()))
        .collect();
    assert_eq!(clips, vec![(0, 64), (10_000, 32)]);
}

#[test]
fn take_ends_overflow_into_the_callback_while_the_ring_is_full() {
    // Room for exactly two items.
    let mut rig = Rig::build(RATE, BLOCK, 2, None);
    let ids: Vec<TrackId> = ["A", "B", "C"]
        .into_iter()
        .map(|name| rig.add_track(name, SynthesisMode::Passthrough))
        .collect();
    for &id in &ids {
        rig.apply(EngineCommand::SetTrackArm(id, true));
    }
    rig.apply(EngineCommand::Transport(TransportCommand::Record));
    rig.capture(16, 0.25);
    rig.apply(EngineCommand::Transport(TransportCommand::Stop));
    assert_eq!(rig.io.reclaim.overflow.len(), 1, "one take end waits");
    // Commands wait while anything is overflowing.
    assert_eq!(rig.io.reclaim.room(), 0);

    for _ in 0..4 {
        rig.settle();
    }
    assert!(rig.io.reclaim.overflow.is_empty());
    for id in ids {
        assert_eq!(rig.clips(id).len(), 1, "take on {id} lost");
    }
    assert_eq!(rig.state.takes.pool.len(), CHUNKS);
}

#[test]
fn timeline_dirty_on_placed_recording() {
    let mut rig = Rig::with_armed_track();
    rig.apply(EngineCommand::Transport(TransportCommand::Record));
    rig.capture(64, 0.5);
    rig.apply(EngineCommand::Transport(TransportCommand::Stop));
    rig.state.timeline_dirty = false;
    rig.settle();
    assert!(rig.state.timeline_dirty);
}

// ---------------------------------------------------------------------------
// Disk recording
// ---------------------------------------------------------------------------

#[test]
fn start_recording_only_feeds_disk_when_command_delivered() {
    let mut rig = Rig::new();
    // Fill the disk channel: Start cannot land.
    for _ in 0..16 {
        rig.io.disk_cmd_tx.try_send(DiskCommand::Shutdown).unwrap();
    }
    rig.apply(EngineCommand::StartRecording {
        path: "/tmp/never.wav".into(),
    });
    assert!(!rig.state.is_recording, "must not record into a void");
    assert_eq!(rig.stats().disk_commands_dropped, 1);
    // The undelivered command (and its path) left through the ring.
    assert_eq!(rig.reclaim(), 1);

    // A live, non-full channel records normally.
    while rig.disk_commands.try_recv().is_ok() {}
    rig.apply(EngineCommand::StartRecording {
        path: "/tmp/ok.wav".into(),
    });
    assert!(rig.state.is_recording);
    assert!(matches!(
        rig.disk_commands.try_recv(),
        Ok(DiskCommand::Start { from_sample: 0, .. })
    ));

    // Stop with a full channel: stop feeding, count the lost command.
    for _ in 0..16 {
        rig.io.disk_cmd_tx.try_send(DiskCommand::Shutdown).unwrap();
    }
    rig.apply(EngineCommand::StopRecording);
    assert!(!rig.state.is_recording);
    assert_eq!(rig.stats().disk_commands_dropped, 2);
}

// ---------------------------------------------------------------------------
// Parameters and MIDI
// ---------------------------------------------------------------------------

#[test]
fn rejected_parameters_are_counted() {
    let mut rig = Rig::with_track();
    let track_id = TrackId(0);
    rig.apply(EngineCommand::SetSynthParameter {
        track_id,
        param_index: 999,
        value: 0.5,
    });
    rig.apply(EngineCommand::SetSynthLayerParameter {
        track_id,
        layer_index: 0,
        param_index: 999,
        value: 0.5,
    });
    rig.apply(EngineCommand::SetEffectParameter {
        track_id,
        effect_index: 42,
        param_index: 0,
        value: 0.5,
    });
    assert_eq!(rig.stats().params_rejected, 3);

    // A valid parameter is not counted.
    rig.apply(EngineCommand::SetSynthParameter {
        track_id,
        // Index 1 is the pitch-tracked synth's detune parameter.
        param_index: 1,
        value: 5.0,
    });
    assert_eq!(rig.stats().params_rejected, 3);
}

#[test]
fn parameters_for_missing_targets_are_counted() {
    let mut rig = Rig::with_track();
    let missing = TrackId(77);
    for command in [
        EngineCommand::SetSynthParameter {
            track_id: missing,
            param_index: 0,
            value: 0.5,
        },
        EngineCommand::SetSynthLayerParameter {
            track_id: missing,
            layer_index: 0,
            param_index: 0,
            value: 0.5,
        },
        EngineCommand::SetSynthLayerParameter {
            track_id: TrackId(0),
            layer_index: 9,
            param_index: 0,
            value: 0.5,
        },
        EngineCommand::SetEffectParameter {
            track_id: missing,
            effect_index: 0,
            param_index: 0,
            value: 0.5,
        },
        // A non-finite value on a real parameter is refused too.
        EngineCommand::SetSynthParameter {
            track_id: TrackId(0),
            param_index: 1,
            value: f32::NAN,
        },
    ] {
        rig.apply(command);
    }
    assert_eq!(rig.stats().params_rejected, 5);
}

#[test]
fn layer_gain_and_enable_are_applied() {
    let mut rig = Rig::with_track();
    rig.apply(EngineCommand::SetSynthLayerGain {
        track_id: TrackId(0),
        layer_index: 0,
        gain: Db::new(-3.0),
    });
    rig.apply(EngineCommand::SetSynthLayerEnabled {
        track_id: TrackId(0),
        layer_index: 0,
        enabled: false,
    });
    let layer = rig.state.mixer.track(TrackId(0)).unwrap().layer(0).unwrap();
    assert!((layer.gain().value() + 3.0).abs() < f32::EPSILON);
    assert!(!layer.is_enabled());
}

#[test]
fn midi_note_on_does_not_clobber_synth_parameter_zero() {
    let mut rig = Rig::with_armed_track();
    let before = rig
        .state
        .mixer
        .track(TrackId(0))
        .unwrap()
        .synth()
        .param_value(0)
        .unwrap();
    rig.apply(EngineCommand::MidiNoteOn {
        note: 69,
        velocity: 1,
        channel: 0,
    });
    let after = rig
        .state
        .mixer
        .track(TrackId(0))
        .unwrap()
        .synth()
        .param_value(0)
        .unwrap();
    assert!((before - after).abs() < f32::EPSILON);
    assert_eq!(rig.state.midi_last_note, Some(69));
}

#[test]
fn detected_pitch_reaches_armed_tracks_only() {
    let mut rig = Rig::new();
    let armed = rig.add_track("Armed", SynthesisMode::PitchTracked);
    rig.add_track("Idle", SynthesisMode::PitchTracked);
    rig.apply(EngineCommand::SetTrackArm(armed, true));
    let estimate = PitchEstimate {
        frequency: Some(220.0),
        voiced_probability: 0.9,
        midi_note: Some(57),
    };
    assert!(rig.pitch.try_push(estimate).is_ok());
    rig.drain_analysis();
    assert_eq!(rig.state.latest_pitch.frequency, Some(220.0));
    assert_eq!(rig.reclaim(), 0, "a pitch estimate holds nothing to free");
}

// ---------------------------------------------------------------------------
// Back-pressure: nothing is taken that could not be retired
// ---------------------------------------------------------------------------

#[test]
fn commands_wait_while_the_reclaim_ring_lacks_room() {
    // Room for three items: one command's worth plus one.
    let mut rig = Rig::build(RATE, BLOCK, 3, None);
    let id = rig.add_track("T", SynthesisMode::Passthrough);
    for _ in 0..2 {
        let (effect, _) = Probe::boxed("Effect");
        let effect = rig.prepared(effect);
        rig.apply(EngineCommand::AddEffect {
            track_id: id,
            effect,
        });
    }
    // Two removals queued; the first leaves one slot, too few for another.
    for _ in 0..2 {
        rig.commands
            .send(EngineCommand::RemoveEffect {
                track_id: id,
                effect_index: 0,
            })
            .unwrap();
    }
    // Fill one slot so only two remain.
    rig.io.reclaim.retire(Retired::Samples(Vec::new()));
    rig.drain_commands();
    assert_eq!(rig.state.mixer.track(id).unwrap().effects().len(), 1);
    assert_eq!(rig.commands.len(), 1, "the second command waits");

    rig.reclaim();
    rig.drain_commands();
    assert!(rig.state.mixer.track(id).unwrap().effects().is_empty());
    assert!(rig.commands.is_empty());
}

#[test]
fn analysis_frames_wait_while_the_reclaim_ring_is_full() {
    let mut rig = Rig::build(RATE, BLOCK, 1, None);
    assert!(rig.spectrum.try_push(vec![1.0; 8]).is_ok());
    rig.io.reclaim.retire(Retired::Samples(Vec::new()));
    rig.drain_analysis();
    assert!(
        rig.state.spectrum.is_empty(),
        "taken with nowhere to send it"
    );

    rig.reclaim();
    rig.drain_analysis();
    assert_eq!(rig.state.spectrum, vec![1.0; 8]);
}

// ---------------------------------------------------------------------------
// Analysis results
// ---------------------------------------------------------------------------

#[test]
fn spectrum_and_formants_are_copied_and_their_buffers_retired() {
    let mut rig = Rig::new();
    let spectrum_buffer = rig.state.spectrum.as_ptr();
    let formant_buffer = rig.state.formants.frequencies.as_ptr();

    assert!(rig.spectrum.try_push(vec![-6.0; 16]).is_ok());
    assert!(rig.spectrum.try_push(vec![-3.0; SPECTRUM * 2]).is_ok());
    assert!(
        rig.formants
            .try_push(Some(FormantData {
                frequencies: vec![500.0, 1500.0],
                bandwidths: vec![80.0, 120.0],
                num_formants: 2,
            }))
            .is_ok()
    );
    rig.drain_analysis();

    // The newest spectrum, cut to the pre-allocated capacity.
    assert_eq!(rig.state.spectrum.len(), SPECTRUM);
    assert!(
        rig.state
            .spectrum
            .iter()
            .all(|&db| (db + 3.0).abs() < f32::EPSILON)
    );
    assert_eq!(rig.state.spectrum.as_ptr(), spectrum_buffer, "reallocated");
    assert_eq!(rig.state.formants.frequencies, vec![500.0, 1500.0]);
    assert_eq!(rig.state.formants.num_formants, 2);
    assert_eq!(rig.state.formants.frequencies.as_ptr(), formant_buffer);
    // The analysis thread's three buffers go to the reclaimer.
    assert_eq!(rig.reclaim(), 3);
}

// ---------------------------------------------------------------------------
// Timeline
// ---------------------------------------------------------------------------

#[test]
fn timeline_of_an_empty_mixer_is_empty() {
    let mut rig = Rig::new();
    rig.state.timeline_dirty = true;
    let timeline = rig.timeline();
    assert!(timeline.tracks.is_empty());
    assert_eq!(timeline.total_length, 0);
}

#[test]
fn timeline_describes_tracks_and_sorted_clips() {
    let mut rig = Rig::with_track();
    rig.add_clip(TrackId(0), 50, 1000);
    rig.add_clip(TrackId(0), 200, 500);
    rig.add_clip(TrackId(0), 1000, 0);
    rig.apply(EngineCommand::SetClipGain {
        track_id: TrackId(0),
        clip_id: ClipId(2),
        gain: Db::new(-12.0),
    });
    rig.apply(EngineCommand::SetClipMute {
        track_id: TrackId(0),
        clip_id: ClipId(2),
        muted: true,
    });

    let timeline = rig.timeline();
    assert!(!rig.state.timeline_dirty);
    assert_eq!(timeline.tracks.len(), 1);
    let track = &timeline.tracks[0];
    assert_eq!(track.track_id, 0);
    assert_eq!(track.track_name, "Test");
    assert_eq!(track.recording, None);
    let positions: Vec<u64> = track.clips.iter().map(|c| c.position).collect();
    assert_eq!(positions, vec![0, 500, 1000]);
    assert_eq!(track.clips[1].length, 200);
    assert_eq!(track.clips[0].name, "TestClip");
    assert!((track.clips[0].gain_db + 12.0).abs() < f32::EPSILON);
    assert!(track.clips[0].muted);
    assert!(!track.clips[0].waveform_overview.is_empty());
    // End of the last clip: 1000 + 50.
    assert_eq!(timeline.total_length, 1050);

    // The source came back, cleared, for the next change.
    assert_eq!(rig.state.timelines.len(), TIMELINE_SOURCES);
    assert!(rig.state.timelines.iter().all(TimelineSource::is_empty));
}

#[test]
fn timeline_lists_tracks_without_clips() {
    let mut rig = Rig::new();
    let id = rig.add_track("Flagged", SynthesisMode::PitchTracked);
    rig.apply(EngineCommand::SetTrackArm(id, true));
    rig.apply(EngineCommand::SetTrackMute(id, true));
    let timeline = rig.timeline();
    assert_eq!(timeline.tracks.len(), 1);
    assert_eq!(timeline.tracks[0].track_name, "Flagged");
    assert_eq!(timeline.total_length, 0);
}

#[test]
fn timeline_is_not_republished_until_something_changes() {
    let mut rig = Rig::with_track();
    rig.timeline();
    rig.publish_timeline();
    assert_eq!(rig.reclaim(), 0);
    rig.add_clip(TrackId(0), 10, 0);
    assert_eq!(rig.timeline().tracks[0].clips.len(), 1);
}

#[test]
fn timeline_waits_for_a_free_source() {
    let mut rig = Rig::with_track();
    // Both sources in flight at the reclaimer.
    for _ in 0..TIMELINE_SOURCES {
        rig.state.timeline_dirty = true;
        rig.publish_timeline();
    }
    assert!(rig.state.timelines.is_empty());
    rig.state.timeline_dirty = true;
    rig.publish_timeline();
    assert!(rig.state.timeline_dirty, "published with no source");

    rig.settle();
    assert_eq!(rig.state.timelines.len(), TIMELINE_SOURCES);
    rig.publish_timeline();
    assert!(!rig.state.timeline_dirty);
}

// ---------------------------------------------------------------------------
// Display frames
// ---------------------------------------------------------------------------

#[test]
fn display_frames_are_recycled_not_allocated() {
    let mut rig = Rig::with_armed_track();
    rig.apply(EngineCommand::Transport(TransportCommand::Record));
    rig.capture(64, 0.5);
    rig.state.spectrum.extend_from_slice(&[1.0; 8]);

    // Take every frame in circulation, as a UI that never hands them back.
    let mut frames = Vec::new();
    loop {
        rig.push_display(-12.0, 0.5);
        match rig.display.try_pop() {
            Some(frame) => frames.push(frame),
            None => break,
        }
    }
    assert_eq!(frames.len(), 6);
    // With none left, the snapshot was skipped and counted.
    assert_eq!(rig.stats().display_frames_dropped, 1);

    let frame = &frames[0];
    assert_eq!(frame.spectrum_magnitudes, vec![1.0; 8]);
    assert_eq!(frame.mixer.track_meters.len(), 1);
    assert!((frame.input_level_db + 12.0).abs() < f32::EPSILON);
    assert_eq!(
        frame.recording_on(0),
        Some(RecordingSpan {
            start: 0,
            length: 64
        })
    );

    // Handed back, the same frames (and their buffers) are written again.
    let buffers: Vec<(*const f32, *const crate::mixer::TrackMeter)> = frames
        .iter()
        .map(|f| {
            (
                f.spectrum_magnitudes.as_ptr(),
                f.mixer.track_meters.as_ptr(),
            )
        })
        .collect();
    for frame in frames {
        assert!(rig.recycle.try_push(frame).is_ok());
    }
    for _ in 0..20 {
        rig.push_display(0.0, 0.0);
        let frame = rig.display.try_pop().expect("a recycled frame");
        let pointers = (
            frame.spectrum_magnitudes.as_ptr(),
            frame.mixer.track_meters.as_ptr(),
        );
        assert!(buffers.contains(&pointers), "a frame was reallocated");
        assert_eq!(frame.spectrum_magnitudes, vec![1.0; 8]);
        assert!(rig.recycle.try_push(frame).is_ok());
    }
    assert_eq!(rig.stats().display_frames_dropped, 1);
}
