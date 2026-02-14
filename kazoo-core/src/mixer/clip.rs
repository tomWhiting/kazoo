//! Clip data model for timeline-based audio playback.
//!
//! [`AudioClip`] represents a segment of audio placed at a specific position
//! on a track's timeline. Audio data is stored in [`ClipData`], which uses
//! `Arc<Vec<f32>>` for zero-copy sharing when the same file is placed on
//! multiple tracks.
//!
//! The critical hot-path method is [`AudioClip::read_into`], which sums clip
//! samples into an output buffer without allocating.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::{Db, sanitize_sample};

// ---------------------------------------------------------------------------
// ClipId
// ---------------------------------------------------------------------------

/// Unique identifier for an audio clip. Monotonically increasing within
/// an engine lifetime, never reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ClipId(pub u64);

impl std::fmt::Display for ClipId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Clip({})", self.0)
    }
}

// ---------------------------------------------------------------------------
// ClipData
// ---------------------------------------------------------------------------

/// Shared, immutable audio data backing one or more clips.
///
/// All samples are **mono** and **pre-resampled** to the engine's sample rate
/// at load time. This means the processing thread never needs to resample on
/// the fly — clip playback is a simple indexed read with gain.
///
/// Uses [`Arc`] so the same audio can be placed on multiple tracks or
/// duplicated without copying the sample data.
#[derive(Debug, Clone)]
pub struct ClipData {
    /// Mono samples at the engine's sample rate.
    samples: Arc<Vec<f32>>,
    /// Original file path (for display/reload), if loaded from file.
    source_path: Option<PathBuf>,
    /// Original sample rate before resampling (for metadata display).
    original_sample_rate: u32,
    /// Name for UI display (filename stem or "Recording N").
    name: String,
}

impl ClipData {
    /// Create new clip data from pre-resampled mono samples.
    #[must_use]
    pub fn new(
        samples: Vec<f32>,
        name: String,
        source_path: Option<PathBuf>,
        original_sample_rate: u32,
    ) -> Self {
        Self {
            samples: Arc::new(samples),
            source_path,
            original_sample_rate,
            name,
        }
    }

    /// Number of samples (at engine sample rate).
    #[must_use]
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// Whether the clip data is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Access the underlying sample data.
    #[must_use]
    pub fn samples(&self) -> &[f32] {
        &self.samples
    }

    /// Human-readable name for UI display.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Original file path, if this clip was loaded from disk.
    #[must_use]
    pub fn source_path(&self) -> Option<&Path> {
        self.source_path.as_deref()
    }

    /// The sample rate of the original file before resampling.
    #[must_use]
    pub const fn original_sample_rate(&self) -> u32 {
        self.original_sample_rate
    }
}

// ---------------------------------------------------------------------------
// AudioClip
// ---------------------------------------------------------------------------

/// An audio clip placed at a specific position on a track's timeline.
///
/// Clips are defined by:
/// - **Position**: where on the timeline the clip starts (in samples).
/// - **Source range**: `[source_start, source_end)` — the active window into
///   the underlying [`ClipData`], supporting non-destructive trimming.
/// - **Gain**: per-clip volume adjustment.
/// - **Muted**: whether the clip is skipped during playback.
///
/// Overlapping clips on the same track are summed together.
#[derive(Debug, Clone)]
pub struct AudioClip {
    id: ClipId,
    data: ClipData,
    /// Timeline position in samples (engine sample rate).
    position: u64,
    /// Offset into the source data where playback begins (for trim-start).
    source_start: usize,
    /// Offset into the source data where playback ends (for trim-end).
    source_end: usize,
    /// Per-clip gain in dB.
    gain: Db,
    /// Whether this clip is muted (skipped during playback).
    muted: bool,
    /// Cached waveform overview: `(min, max)` pairs for UI rendering.
    /// Computed once on creation, updated on trim.
    waveform_overview: Vec<(f32, f32)>,
}

/// Maximum number of clips per track.
pub const MAX_CLIPS_PER_TRACK: usize = 256;

/// Number of min/max pairs in the waveform overview.
const WAVEFORM_OVERVIEW_WIDTH: usize = 128;

impl AudioClip {
    /// Create a new clip at the given timeline position.
    ///
    /// The clip spans the full source data with unity gain and unmuted.
    #[must_use]
    pub fn new(id: ClipId, data: ClipData, position: u64) -> Self {
        let source_end = data.len();
        let overview = compute_waveform_overview(data.samples(), 0, source_end);
        Self {
            id,
            data,
            position,
            source_start: 0,
            source_end,
            gain: Db::UNITY,
            muted: false,
            waveform_overview: overview,
        }
    }

    /// Clip identifier.
    #[must_use]
    pub const fn id(&self) -> ClipId {
        self.id
    }

    /// Access the underlying clip data.
    #[must_use]
    pub const fn data(&self) -> &ClipData {
        &self.data
    }

    /// Timeline position in samples.
    #[must_use]
    pub const fn position(&self) -> u64 {
        self.position
    }

    /// Set the timeline position.
    pub const fn set_position(&mut self, pos: u64) {
        self.position = pos;
    }

    /// Source start offset (trim start).
    #[must_use]
    pub const fn source_start(&self) -> usize {
        self.source_start
    }

    /// Source end offset (trim end).
    #[must_use]
    pub const fn source_end(&self) -> usize {
        self.source_end
    }

    /// Per-clip gain.
    #[must_use]
    pub const fn gain(&self) -> Db {
        self.gain
    }

    /// Set the per-clip gain.
    pub const fn set_gain(&mut self, gain: Db) {
        self.gain = gain;
    }

    /// Whether the clip is muted.
    #[must_use]
    pub const fn is_muted(&self) -> bool {
        self.muted
    }

    /// Set the mute state.
    pub const fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
    }

    /// Human-readable clip name (from the underlying data).
    #[must_use]
    pub fn name(&self) -> &str {
        self.data.name()
    }

    /// The effective length of this clip in samples (after trimming).
    #[must_use]
    pub const fn effective_length(&self) -> usize {
        self.source_end.saturating_sub(self.source_start)
    }

    /// The timeline sample position where this clip ends.
    #[must_use]
    pub const fn end_position(&self) -> u64 {
        self.position.saturating_add(self.effective_length() as u64)
    }

    /// Cached waveform overview for UI timeline rendering.
    #[must_use]
    pub fn waveform_overview(&self) -> &[(f32, f32)] {
        &self.waveform_overview
    }

    /// Trim the start of the clip (move `source_start` forward by `samples`).
    ///
    /// The timeline position is **not** adjusted — the clip starts later in
    /// its source data but at the same timeline position. Clamped so that
    /// `source_start` never exceeds `source_end`.
    pub fn trim_start(&mut self, samples: usize) {
        self.source_start = self
            .source_start
            .saturating_add(samples)
            .min(self.source_end);
        self.recompute_overview();
    }

    /// Trim the end of the clip (move `source_end` backward by `samples`).
    ///
    /// Clamped so that `source_end` never falls below `source_start`.
    pub fn trim_end(&mut self, samples: usize) {
        self.source_end = self
            .source_end
            .saturating_sub(samples)
            .max(self.source_start);
        self.recompute_overview();
    }

    /// Split this clip at the given timeline position.
    ///
    /// `self` becomes the left half (trimmed to end at `split_pos`), and a
    /// new right-half clip is returned. Returns `None` if `split_pos` is
    /// outside the clip's active range.
    ///
    /// The right half receives `new_id` as its identifier. Both halves share
    /// the same underlying [`ClipData`] via `Arc`.
    #[must_use]
    pub fn split_at(&mut self, split_pos: u64, new_id: ClipId) -> Option<Self> {
        if split_pos <= self.position || split_pos >= self.end_position() {
            return None;
        }

        let offset_in_clip = (split_pos - self.position) as usize;
        let absolute_split = self.source_start + offset_in_clip;

        // Build the right half.
        let right_overview =
            compute_waveform_overview(self.data.samples(), absolute_split, self.source_end);
        let right = Self {
            id: new_id,
            data: self.data.clone(),
            position: split_pos,
            source_start: absolute_split,
            source_end: self.source_end,
            gain: self.gain,
            muted: self.muted,
            waveform_overview: right_overview,
        };

        // Trim the left half (self).
        self.source_end = absolute_split;
        self.recompute_overview();

        Some(right)
    }

    /// Read samples from this clip for a given timeline range, **summing**
    /// into `output`.
    ///
    /// This is the critical hot-path method called every audio block on the
    /// processing thread. It reads directly from the `Arc<Vec<f32>>` backing
    /// store with no allocation.
    ///
    /// Returns the number of samples contributed.
    pub fn read_into(&self, timeline_start: u64, output: &mut [f32]) -> usize {
        if self.muted || output.is_empty() || self.effective_length() == 0 {
            return 0;
        }

        let clip_end = self.end_position();
        let block_end = timeline_start.saturating_add(output.len() as u64);

        // No overlap between this clip and the requested range.
        if timeline_start >= clip_end || block_end <= self.position {
            return 0;
        }

        let gain = self.gain.to_linear();
        let samples = self.data.samples();

        // Calculate the overlap region.
        let read_start = timeline_start.max(self.position);
        let read_end = block_end.min(clip_end);
        let count = (read_end - read_start) as usize;

        let output_offset = (read_start - timeline_start) as usize;
        let source_offset = self.source_start + (read_start - self.position) as usize;

        for i in 0..count {
            let src_idx = source_offset + i;
            let dst_idx = output_offset + i;
            // Bounds checks are optimized away when the compiler can prove
            // the indices are in range, but we guard defensively for safety.
            if src_idx < samples.len() && dst_idx < output.len() {
                output[dst_idx] += sanitize_sample(samples[src_idx]) * gain;
            }
        }

        count
    }

    /// Recompute the cached waveform overview after a trim or split.
    ///
    /// Reuses the existing `Vec` capacity to avoid allocation when the number
    /// of overview bins has not increased.
    fn recompute_overview(&mut self) {
        compute_waveform_overview_into(
            self.data.samples(),
            self.source_start,
            self.source_end,
            &mut self.waveform_overview,
        );
    }
}

// ---------------------------------------------------------------------------
// Waveform overview
// ---------------------------------------------------------------------------

/// Compute a downsampled waveform overview as `(min, max)` pairs, writing
/// into the provided `Vec` to reuse existing capacity.
///
/// The overview is used by the TUI timeline widget to draw a mini waveform
/// inside each clip rectangle. Pre-computed to avoid per-frame work.
fn compute_waveform_overview_into(
    samples: &[f32],
    start: usize,
    end: usize,
    out: &mut Vec<(f32, f32)>,
) {
    out.clear();

    let len = end.saturating_sub(start);
    if len == 0 || start >= samples.len() {
        return;
    }
    let effective_end = end.min(samples.len());
    let effective_len = effective_end.saturating_sub(start);
    if effective_len == 0 {
        return;
    }

    let num_bins = WAVEFORM_OVERVIEW_WIDTH.min(effective_len);
    let step = effective_len as f64 / num_bins as f64;

    for i in 0..num_bins {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let bin_start = start + (i as f64 * step) as usize;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let bin_end = (start + ((i + 1) as f64 * step) as usize).min(effective_end);

        if bin_start >= effective_end {
            break;
        }
        let bin_end = bin_end.max(bin_start + 1).min(effective_end);

        let mut min_val = f32::INFINITY;
        let mut max_val = f32::NEG_INFINITY;
        for &s in &samples[bin_start..bin_end] {
            let s = sanitize_sample(s);
            if s < min_val {
                min_val = s;
            }
            if s > max_val {
                max_val = s;
            }
        }

        out.push((min_val.clamp(-1.0, 1.0), max_val.clamp(-1.0, 1.0)));
    }
}

/// Compute a downsampled waveform overview as `(min, max)` pairs, returning
/// a new `Vec`.
///
/// Convenience wrapper around [`compute_waveform_overview_into`] for use in
/// constructors where no existing `Vec` is available.
#[must_use]
fn compute_waveform_overview(samples: &[f32], start: usize, end: usize) -> Vec<(f32, f32)> {
    let mut out = Vec::new();
    compute_waveform_overview_into(samples, start, end, &mut out);
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Create test clip data with a known signal.
    fn test_clip_data(len: usize) -> ClipData {
        let samples: Vec<f32> = (0..len).map(|i| (i as f32) / len as f32).collect();
        ClipData::new(samples, "TestClip".into(), None, 44_100)
    }

    /// Create test clip data with a constant value.
    fn constant_clip_data(len: usize, value: f32) -> ClipData {
        ClipData::new(vec![value; len], "Constant".into(), None, 44_100)
    }

    // -- ClipId --------------------------------------------------------------

    #[test]
    fn clip_id_display() {
        let id = ClipId(42);
        assert_eq!(format!("{id}"), "Clip(42)");
    }

    #[test]
    fn clip_id_equality() {
        assert_eq!(ClipId(1), ClipId(1));
        assert_ne!(ClipId(1), ClipId(2));
    }

    #[test]
    fn clip_id_hash() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(ClipId(1));
        set.insert(ClipId(2));
        set.insert(ClipId(1));
        assert_eq!(set.len(), 2);
    }

    // -- ClipData ------------------------------------------------------------

    #[test]
    fn clip_data_construction() {
        let data = test_clip_data(100);
        assert_eq!(data.len(), 100);
        assert!(!data.is_empty());
        assert_eq!(data.name(), "TestClip");
        assert!(data.source_path().is_none());
        assert_eq!(data.original_sample_rate(), 44_100);
    }

    #[test]
    fn clip_data_empty() {
        let data = ClipData::new(vec![], "Empty".into(), None, 44_100);
        assert_eq!(data.len(), 0);
        assert!(data.is_empty());
    }

    #[test]
    fn clip_data_with_source_path() {
        let path = PathBuf::from("/tmp/test.wav");
        let data = ClipData::new(vec![0.0; 10], "Test".into(), Some(path.clone()), 48_000);
        assert_eq!(data.source_path(), Some(path.as_path()));
        assert_eq!(data.original_sample_rate(), 48_000);
    }

    #[test]
    fn clip_data_clone_shares_arc() {
        let data = test_clip_data(1000);
        let cloned = data.clone();
        // Both should point to the same underlying allocation.
        assert_eq!(
            data.samples().as_ptr(),
            cloned.samples().as_ptr(),
            "clone should share Arc"
        );
    }

    // -- AudioClip construction ----------------------------------------------

    #[test]
    fn audio_clip_new() {
        let data = test_clip_data(100);
        let clip = AudioClip::new(ClipId(1), data, 500);

        assert_eq!(clip.id(), ClipId(1));
        assert_eq!(clip.position(), 500);
        assert_eq!(clip.source_start(), 0);
        assert_eq!(clip.source_end(), 100);
        assert_eq!(clip.effective_length(), 100);
        assert_eq!(clip.end_position(), 600);
        assert_eq!(clip.gain(), Db::UNITY);
        assert!(!clip.is_muted());
        assert_eq!(clip.name(), "TestClip");
    }

    #[test]
    fn audio_clip_setters() {
        let data = test_clip_data(100);
        let mut clip = AudioClip::new(ClipId(1), data, 0);

        clip.set_position(1000);
        assert_eq!(clip.position(), 1000);

        clip.set_gain(Db::new(-6.0));
        assert!((clip.gain().value() - (-6.0)).abs() < f32::EPSILON);

        clip.set_muted(true);
        assert!(clip.is_muted());
    }

    // -- read_into: no overlap -----------------------------------------------

    #[test]
    fn read_into_before_clip_returns_zero() {
        let data = constant_clip_data(100, 0.5);
        let clip = AudioClip::new(ClipId(1), data, 1000);

        let mut output = [0.0_f32; 64];
        let written = clip.read_into(0, &mut output);
        assert_eq!(written, 0);
        assert!(output.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn read_into_after_clip_returns_zero() {
        let data = constant_clip_data(100, 0.5);
        let clip = AudioClip::new(ClipId(1), data, 1000);

        let mut output = [0.0_f32; 64];
        let written = clip.read_into(1100, &mut output);
        assert_eq!(written, 0);
    }

    #[test]
    fn read_into_muted_returns_zero() {
        let data = constant_clip_data(100, 0.5);
        let mut clip = AudioClip::new(ClipId(1), data, 0);
        clip.set_muted(true);

        let mut output = [0.0_f32; 64];
        let written = clip.read_into(0, &mut output);
        assert_eq!(written, 0);
        assert!(output.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn read_into_empty_output() {
        let data = constant_clip_data(100, 0.5);
        let clip = AudioClip::new(ClipId(1), data, 0);

        let mut output: [f32; 0] = [];
        let written = clip.read_into(0, &mut output);
        assert_eq!(written, 0);
    }

    // -- read_into: full containment -----------------------------------------

    #[test]
    fn read_into_full_containment() {
        let data = constant_clip_data(100, 0.5);
        let clip = AudioClip::new(ClipId(1), data, 0);

        let mut output = [0.0_f32; 100];
        let written = clip.read_into(0, &mut output);
        assert_eq!(written, 100);
        for &s in &output {
            assert!((s - 0.5).abs() < f32::EPSILON, "expected 0.5, got {s}");
        }
    }

    #[test]
    fn read_into_block_larger_than_clip() {
        let data = constant_clip_data(50, 0.8);
        let clip = AudioClip::new(ClipId(1), data, 10);

        // Block [0, 100) overlaps clip [10, 60).
        let mut output = [0.0_f32; 100];
        let written = clip.read_into(0, &mut output);
        assert_eq!(written, 50);

        // First 10 samples should be untouched.
        for &s in &output[..10] {
            assert!(s.abs() < f32::EPSILON, "before clip should be 0.0");
        }
        // Clip region [10, 60) should have the signal.
        for &s in &output[10..60] {
            assert!(
                (s - 0.8).abs() < f32::EPSILON,
                "clip region expected 0.8, got {s}"
            );
        }
        // After clip should be untouched.
        for &s in &output[60..] {
            assert!(s.abs() < f32::EPSILON, "after clip should be 0.0");
        }
    }

    // -- read_into: partial overlap ------------------------------------------

    #[test]
    fn read_into_partial_overlap_start() {
        // Clip at position 50, length 100 → [50, 150).
        // Block at position 0, length 80 → [0, 80).
        // Overlap: [50, 80) = 30 samples.
        let data = constant_clip_data(100, 0.7);
        let clip = AudioClip::new(ClipId(1), data, 50);

        let mut output = [0.0_f32; 80];
        let written = clip.read_into(0, &mut output);
        assert_eq!(written, 30);

        // First 50 samples untouched.
        for &s in &output[..50] {
            assert!(s.abs() < f32::EPSILON);
        }
        // Overlap region [50, 80).
        for &s in &output[50..80] {
            assert!(
                (s - 0.7).abs() < f32::EPSILON,
                "overlap expected 0.7, got {s}"
            );
        }
    }

    #[test]
    fn read_into_partial_overlap_end() {
        // Clip at position 0, length 100 → [0, 100).
        // Block at position 80, length 64 → [80, 144).
        // Overlap: [80, 100) = 20 samples.
        let data = constant_clip_data(100, 0.6);
        let clip = AudioClip::new(ClipId(1), data, 0);

        let mut output = [0.0_f32; 64];
        let written = clip.read_into(80, &mut output);
        assert_eq!(written, 20);

        // First 20 samples of output have clip data (from source [80, 100)).
        for &s in &output[..20] {
            assert!(
                (s - 0.6).abs() < f32::EPSILON,
                "overlap expected 0.6, got {s}"
            );
        }
        // Rest untouched.
        for &s in &output[20..] {
            assert!(s.abs() < f32::EPSILON);
        }
    }

    // -- read_into: gain application -----------------------------------------

    #[test]
    fn read_into_applies_gain() {
        let data = constant_clip_data(100, 1.0);
        let mut clip = AudioClip::new(ClipId(1), data, 0);
        clip.set_gain(Db::new(-6.0)); // ≈ 0.501

        let mut output = [0.0_f32; 10];
        clip.read_into(0, &mut output);

        let expected = Db::new(-6.0).to_linear();
        for &s in &output {
            assert!(
                (s - expected).abs() < 1e-3,
                "-6dB gain: expected ~{expected}, got {s}"
            );
        }
    }

    // -- read_into: summing behaviour ----------------------------------------

    #[test]
    fn read_into_sums_onto_existing() {
        let data = constant_clip_data(50, 0.3);
        let clip = AudioClip::new(ClipId(1), data, 0);

        // Pre-fill output with 0.2.
        let mut output = [0.2_f32; 50];
        clip.read_into(0, &mut output);

        // Each sample should be 0.2 + 0.3 = 0.5.
        for &s in &output {
            assert!((s - 0.5).abs() < f32::EPSILON, "sum expected 0.5, got {s}");
        }
    }

    // -- read_into: NaN/Inf defense ------------------------------------------

    #[test]
    fn read_into_sanitizes_nan() {
        let data = ClipData::new(vec![f32::NAN; 10], "NaN".into(), None, 44_100);
        let clip = AudioClip::new(ClipId(1), data, 0);

        let mut output = [0.0_f32; 10];
        clip.read_into(0, &mut output);

        for &s in &output {
            assert!(s.is_finite(), "NaN should be sanitized, got {s}");
        }
    }

    #[test]
    fn read_into_sanitizes_inf() {
        let data = ClipData::new(vec![f32::INFINITY; 10], "Inf".into(), None, 44_100);
        let clip = AudioClip::new(ClipId(1), data, 0);

        let mut output = [0.0_f32; 10];
        clip.read_into(0, &mut output);

        for &s in &output {
            assert!(s.is_finite(), "Inf should be sanitized, got {s}");
        }
    }

    // -- read_into: empty clip data ------------------------------------------

    #[test]
    fn read_into_empty_clip_returns_zero() {
        let data = ClipData::new(vec![], "Empty".into(), None, 44_100);
        let clip = AudioClip::new(ClipId(1), data, 0);

        let mut output = [0.0_f32; 10];
        let written = clip.read_into(0, &mut output);
        assert_eq!(written, 0);
    }

    // -- trim ----------------------------------------------------------------

    #[test]
    fn trim_start_reduces_effective_length() {
        let data = test_clip_data(100);
        let mut clip = AudioClip::new(ClipId(1), data, 0);

        clip.trim_start(20);
        assert_eq!(clip.source_start(), 20);
        assert_eq!(clip.source_end(), 100);
        assert_eq!(clip.effective_length(), 80);
    }

    #[test]
    fn trim_end_reduces_effective_length() {
        let data = test_clip_data(100);
        let mut clip = AudioClip::new(ClipId(1), data, 0);

        clip.trim_end(30);
        assert_eq!(clip.source_start(), 0);
        assert_eq!(clip.source_end(), 70);
        assert_eq!(clip.effective_length(), 70);
    }

    #[test]
    fn trim_start_clamped_to_source_end() {
        let data = test_clip_data(100);
        let mut clip = AudioClip::new(ClipId(1), data, 0);

        clip.trim_start(200);
        assert_eq!(clip.source_start(), 100);
        assert_eq!(clip.effective_length(), 0);
    }

    #[test]
    fn trim_end_clamped_to_source_start() {
        let data = test_clip_data(100);
        let mut clip = AudioClip::new(ClipId(1), data, 0);

        clip.trim_start(50);
        clip.trim_end(200);
        assert_eq!(clip.source_end(), 50);
        assert_eq!(clip.effective_length(), 0);
    }

    #[test]
    fn trim_affects_read_into() {
        // Clip data: [0.0, 0.1, 0.2, ..., 0.9] (10 samples).
        let samples: Vec<f32> = (0..10).map(|i| i as f32 * 0.1).collect();
        let data = ClipData::new(samples, "Trim".into(), None, 44_100);
        let mut clip = AudioClip::new(ClipId(1), data, 0);

        clip.trim_start(3); // skip first 3 samples
        clip.trim_end(2); // skip last 2 samples
        // Effective range: source [3, 8) → values [0.3, 0.4, 0.5, 0.6, 0.7]

        assert_eq!(clip.effective_length(), 5);

        let mut output = [0.0_f32; 5];
        let written = clip.read_into(0, &mut output);
        assert_eq!(written, 5);

        let expected = [0.3, 0.4, 0.5, 0.6, 0.7];
        for (i, (&got, &exp)) in output.iter().zip(expected.iter()).enumerate() {
            assert!(
                (got - exp).abs() < 1e-6,
                "trim read[{i}]: expected {exp}, got {got}"
            );
        }
    }

    // -- split ---------------------------------------------------------------

    #[test]
    fn split_at_mid() {
        let data = constant_clip_data(100, 0.5);
        let mut left = AudioClip::new(ClipId(1), data, 1000);

        let right = left.split_at(1040, ClipId(2));
        assert!(right.is_some());
        let right = right.unwrap();

        // Left half: [1000, 1040), source [0, 40).
        assert_eq!(left.position(), 1000);
        assert_eq!(left.source_start(), 0);
        assert_eq!(left.source_end(), 40);
        assert_eq!(left.effective_length(), 40);
        assert_eq!(left.end_position(), 1040);

        // Right half: [1040, 1100), source [40, 100).
        assert_eq!(right.position(), 1040);
        assert_eq!(right.source_start(), 40);
        assert_eq!(right.source_end(), 100);
        assert_eq!(right.effective_length(), 60);
        assert_eq!(right.end_position(), 1100);
        assert_eq!(right.id(), ClipId(2));

        // Both share the same Arc.
        assert_eq!(
            left.data().samples().as_ptr(),
            right.data().samples().as_ptr(),
        );
    }

    #[test]
    fn split_at_clip_start_returns_none() {
        let data = constant_clip_data(100, 0.5);
        let mut clip = AudioClip::new(ClipId(1), data, 1000);

        assert!(clip.split_at(1000, ClipId(2)).is_none());
    }

    #[test]
    fn split_at_clip_end_returns_none() {
        let data = constant_clip_data(100, 0.5);
        let mut clip = AudioClip::new(ClipId(1), data, 1000);

        assert!(clip.split_at(1100, ClipId(2)).is_none());
    }

    #[test]
    fn split_at_before_clip_returns_none() {
        let data = constant_clip_data(100, 0.5);
        let mut clip = AudioClip::new(ClipId(1), data, 1000);

        assert!(clip.split_at(500, ClipId(2)).is_none());
    }

    #[test]
    fn split_at_after_clip_returns_none() {
        let data = constant_clip_data(100, 0.5);
        let mut clip = AudioClip::new(ClipId(1), data, 1000);

        assert!(clip.split_at(2000, ClipId(2)).is_none());
    }

    #[test]
    fn split_preserves_gain_and_mute() {
        let data = constant_clip_data(100, 0.5);
        let mut clip = AudioClip::new(ClipId(1), data, 0);
        clip.set_gain(Db::new(-12.0));
        clip.set_muted(true);

        let right = clip.split_at(50, ClipId(2)).unwrap();
        assert_eq!(right.gain(), Db::new(-12.0));
        assert!(right.is_muted());
    }

    // -- waveform overview ---------------------------------------------------

    #[test]
    fn overview_non_empty_clip() {
        let data = constant_clip_data(1000, 0.5);
        let clip = AudioClip::new(ClipId(1), data, 0);

        let overview = clip.waveform_overview();
        assert!(!overview.is_empty());
        assert!(overview.len() <= WAVEFORM_OVERVIEW_WIDTH);

        for &(min, max) in overview {
            assert!((min - 0.5).abs() < f32::EPSILON);
            assert!((max - 0.5).abs() < f32::EPSILON);
        }
    }

    #[test]
    fn overview_empty_clip() {
        let data = ClipData::new(vec![], "Empty".into(), None, 44_100);
        let clip = AudioClip::new(ClipId(1), data, 0);
        assert!(clip.waveform_overview().is_empty());
    }

    #[test]
    fn overview_short_clip() {
        // Fewer samples than WAVEFORM_OVERVIEW_WIDTH.
        let data = constant_clip_data(10, 0.3);
        let clip = AudioClip::new(ClipId(1), data, 0);

        let overview = clip.waveform_overview();
        assert_eq!(overview.len(), 10);
    }

    #[test]
    fn overview_updates_after_trim() {
        let data = constant_clip_data(1000, 0.5);
        let mut clip = AudioClip::new(ClipId(1), data, 0);

        let len_before = clip.waveform_overview().len();
        clip.trim_start(500);
        let len_after = clip.waveform_overview().len();

        // Overview should still be populated (just from the trimmed range).
        assert!(len_after > 0);
        // Both should be full overview width since 500 > 128.
        assert_eq!(len_before, WAVEFORM_OVERVIEW_WIDTH);
        assert_eq!(len_after, WAVEFORM_OVERVIEW_WIDTH);
    }

    // -- read_into with trimmed clip at non-zero position --------------------

    #[test]
    fn read_into_trimmed_clip_at_offset() {
        // 10 samples: [0.0, 0.1, 0.2, ..., 0.9].
        let samples: Vec<f32> = (0..10).map(|i| i as f32 * 0.1).collect();
        let data = ClipData::new(samples, "T".into(), None, 44_100);
        let mut clip = AudioClip::new(ClipId(1), data, 100);

        // Trim to source [2, 7) → effective at timeline [100, 105).
        clip.trim_start(2);
        clip.trim_end(3);
        assert_eq!(clip.effective_length(), 5);
        assert_eq!(clip.position(), 100);
        assert_eq!(clip.end_position(), 105);

        // Read block [98, 108) → overlap is [100, 105).
        let mut output = [0.0_f32; 10];
        let written = clip.read_into(98, &mut output);
        assert_eq!(written, 5);

        // output[0..2] = 0.0 (before clip)
        assert!(output[0].abs() < f32::EPSILON);
        assert!(output[1].abs() < f32::EPSILON);
        // output[2..7] = source[2..7] = [0.2, 0.3, 0.4, 0.5, 0.6]
        let expected = [0.2, 0.3, 0.4, 0.5, 0.6];
        for (i, &exp) in expected.iter().enumerate() {
            assert!(
                (output[i + 2] - exp).abs() < 1e-6,
                "offset read[{i}]: expected {exp}, got {}",
                output[i + 2]
            );
        }
        // output[7..10] = 0.0 (after clip)
        for &s in &output[7..] {
            assert!(s.abs() < f32::EPSILON);
        }
    }
}
