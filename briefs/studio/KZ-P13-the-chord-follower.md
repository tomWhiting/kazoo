# KZ-P13: the chord follower: a fine chroma built from the input's spectral peaks names the chord being played and gives each of its tones' exact, continuous pitch, so a slid chord glides instead of stepping

Line numbers are from the sweep that wrote this brief, at `efef1b6`, and may have moved. Find each item by what it does, and never rename anything. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Place in the track: Track P (audio inputs), lane 13, the analysis half of the fourth chord lane. P14 puts its frames on the wire, P15 sends an input's chord from the desk, P16 lets the arp play it, and P17 puts it on the wall.

Written by the Track P drafting seat, 13 Oct 2026 (night). Read by Cassio, 13 Oct night, line by line. Checked: kazoo-fx depends on nothing in the workspace (only rustfft after P12), so kazoo-core depending on it makes no cycle; the 0.5 and 0.577 flat-chroma scores; the 64 ms naming time. Change: the tuning learner runs on the unit circle and learns only from steady hops, because a slid chord's offsets sweep through every value once a semitone and would otherwise drag the tuning with the slide.

Time: about 40 minutes. Crates: kazoo-core (which gains a path dependency on kazoo-fx, for P12's peak picker).

Depends: each of these must have landed before this lane opens:
- KZ-P12 (`kazoo_fx::shape::peaks`, the one peak picker in the workspace);
- KZ-V04 (`voice/judge.rs`'s feed and thread, which this lane factors so both use one);
- KZ-P01 (`InputProfile::range_hz`).

Files (8): `kazoo-core/Cargo.toml` (`kazoo-fx = { path = "../kazoo-fx" }`; kazoo-fx depends on nothing, so there is no cycle), `analysis/feed.rs` (new: V04's feed made generic), `voice/judge.rs` (uses it), `analysis/chord.rs` (new), `analysis/chord/templates.rs` (new), `analysis/chord/tests.rs` (new), `analysis/score/chords.rs` (new: synthetic chord takes), `analysis/mod.rs`.

## What is known about how it is done (research, with sources)

- **Chroma and templates.**
  - T. Fujishima, "Realtime chord recognition of musical sound: a system using Common Lisp Music", ICMC 1999, pp. 464-467: a twelve-bin pitch-class profile matched against chord-type templates, in real time.
  - M. Müller's FMP notebook C5S2, "Template-based chord recognition": a 4096-sample window, log compression `log(1 + γ|X|²)`, chroma and templates both ℓ2-normalised, cosine similarity, the best template per frame; 24 binary major and minor templates, with smoothing left to later methods.
  - L. Oudre, Y. Grenier and C. Févotte, "Template-based chord recognition: influence of the chord types", ISMIR 2009, pp. 153-158: more chord types and harmonic-weighted templates.
- **Finer than a semitone.** librosa's `chroma_cqt` and the constant-Q C++ library's chromagram default to 36 bins an octave, three per semitone, so a tuning offset or a note between semitones lands between bins instead of splitting.
- **Exact frequencies.** P12's peak picker: the phase-vocoder frequency (Laroche and Dolson 1999), with the parabola (PARSHL) across transients.
- **What was weighed and not taken.**
  - Klapuri's multiple-F0 estimation (iterative estimation and cancellation, IEEE TSAP 2003) finds the notes, which is more than asked and costs more.
  - NMF with calibrated templates (PLAN's note (e)) is the later "proper" route.
  - Tom asked for the chord's name and notes, not the voicing, so chroma matching is the right size.

Decided where the sources differ:
- **Templates.** Binary, not harmonic-weighted. A string's third and fifth harmonics land on the chord's fifth and major third, so they reinforce a major triad rather than hide it, and binary templates are what FMP measures as the baseline.
- **Chroma.** Built from peaks, not from every bin. Peaks carry the exact frequencies the slides need, and they are already computed (P12).

## Outcome

`kazoo_core::analysis::chord::ChordFollower` runs on its own thread, fed from an audio callback. Every hop (10.7 ms at 48 kHz) it gives a `ChordFrame`: whether a chord is sounding, its root and kind (ten kinds), and up to four tones. Each tone is the exact, continuous pitch of the strongest peak of that pitch class (as V/oct, 0 = C4, V04's scale), with its detune in cents from equal temperament at A440. A chord slid on the strings glides its tones, and its name changes once as each semitone is crossed, never flickering. A tuning offset is learnt over two seconds, so a guitar 20 cents flat still reads right.

## The design (decided; build exactly this)

### The feed, once (`analysis/feed.rs`)

V04's judge feed (`voice/judge.rs`: an SPSC sample ring, `Thread::unpark` from the callback, a `park_timeout(20 ms)` loop that exits when the producer goes) becomes `pub struct Feed`, `pub struct FedThread`, and `pub fn spawn_fed<R: Copy + Send>(name, ring_len, results_len, worker: impl FnMut(&[f32], &mut dyn FnMut(R)) + Send + 'static) -> Result<(Feed, ResultsRx<R>, FedThread)>`. V04's judge is rebuilt on it with no change in behaviour: every V04 test passes unchanged. That is the proof.

### Types (`analysis/chord.rs`)

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChordKind { Major, Minor, Dominant7, Major7, Minor7, Sus2, Sus4, Diminished, Augmented, Power }
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChordTone { pub pitch_class: u8, pub volts: f32, pub detune_cents: f32, pub level: f32 }
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChordFrame {
    pub at: u64,              // samples fed so far at the end of the analysed frame
    pub sounding: bool,
    pub changed: bool,        // the name changed in this frame
    pub root: u8,             // 0 = C … 11 = B
    pub kind: ChordKind,
    pub count: u8,            // tones used, 2..=4
    pub tones: [ChordTone; 4],
    pub confidence: f32,      // the winning cosine similarity, 0..=1
}
pub struct ChordFollower { /* picker, chroma, tuning, hysteresis */ }
impl ChordFollower {
    pub fn new(rate: u32, profile: InputProfile) -> Self;              // allocates
    pub fn hop(&mut self, newest: &[f32]) -> ChordFrame;               // allocates nothing
}
pub fn spawn_chord(rate: u32, profile: InputProfile, block: usize) -> crate::Result<(Feed, ResultsRx<ChordFrame>, FedThread)>;
```

### Per hop (`N = 4096`, `H = 512` at 48 kHz, both scaled with the rate)

1. **Peaks.** `PeakPicker::analyse` over the newest `N`, between the profile's `low` (from `range_hz`; 60 Hz for guitar, 29 for bass) and 5 kHz, within 50 dB of the strongest and above −70 dBFS, at most 24.
2. **Tuning.** Each hop, the magnitude-weighted circular mean of every peak's offset from the nearest A440 semitone, as a unit vector `e^(i·2π·offset/100)`. Only a steady hop feeds the learner: one whose mean offset moved less than 3 cents since the last hop. The learner is a one-pole of 2 s on that vector, and `tuning_cents` is its angle × 100/2π. Why steady hops only: a slide's offsets sweep through −50..50 once per semitone and would drag the tuning with them; a vibrato's moving hops are left out too, and its turning points fall either side of the centre. Why 2 s: long against vibrato, and short against retuning between songs.
3. **Fine chroma (36 bins).** For each peak, `c = 36 × frac(log2(hz / f_C) − tuning_cents/1200)`, where `f_C` is C at A440. Add `log(1 + 100 × amp)` (log compression, FMP's γ idea on amplitude) to the bins either side of `c`, split linearly.
4. **Chroma (12).** Each semitone's centre bin plus half of each neighbour, then ℓ2-normalised.
5. **Match.**
   - The 120 templates (`templates.rs`: ten kinds × twelve roots, binary, ℓ2-normalised; intervals major {0,4,7}, minor {0,3,7}, 7 {0,4,7,10}, maj7 {0,4,7,11}, m7 {0,3,7,10}, sus2 {0,2,7}, sus4 {0,5,7}, dim {0,3,6}, aug {0,4,8}, power {0,7}) are scored by cosine similarity.
   - It is not sounding when the strongest peak is under −50 dBFS or the best similarity is under 0.65. Why 0.65: a perfectly flat chroma (noise at its most even) scores exactly 0.5 against any triad template and 0.577 against any four-note one (`k/(√12·√k)` = √(k/12)), and a played triad scores well over 0.85.
   - Ties within 0.01 go to the template with fewer notes. Why: a power chord matches its triads' templates nearly as well.
6. **Hysteresis.** The current chord is replaced only when another has beaten it by at least 0.03 for two hops in a row (21 ms). `changed` marks the hop it is replaced.
7. **Tones.** For each of the chord's pitch classes, the strongest peak within ±75 cents (tuning-corrected) of that class, in any octave:
   - `volts = log2(hz / C4_HZ)` (V04's `volts_from_hz`), continuous;
   - `detune_cents` is its offset from the nearest equal-tempered semitone at A440, in −50..50;
   - `level` is the peak's amplitude.

   A class with no peak (an omitted fifth) is placed at its interval above the root tone, with the root's detune and level 0. Tones are ordered from the root up.

### Nothing sticks to notes

The name is discrete by nature. The tones are not: every hop gives each tone the measured frequency of its peak. A slide moves every peak, so every tone's `volts` glides, hop by hop, with the hop's interpolation left to the consumer (P17 interpolates at the wall's rate). The name changes once per semitone crossed, by the hysteresis.

### Timing and cost, stated

A strummed chord is named within about `N/2 + 2H` of the strum: 43 ms for the frame to fill half way, plus up to 21 ms of hysteresis, about 64 ms. Tones then track every 10.7 ms, 43 ms behind. Per hop: one 4096-point FFT (about 20 µs), peaks (about 5 µs), the chroma and 120 cosines (about 2 µs), on the chord thread: 0.3% of a core. The callback only pushes samples into the feed.

## Not this lane

Carrying frames across the desk (P14, P15). Playing them (P16, P17). Note-by-note following of a sweep: that is the voice (P08), and P16's arp follows the chord while the voice follows the notes.

## Red tests first

`analysis/chord/tests.rs` on `analysis/score/chords.rs`'s takes:
- harmonic tones with 6 harmonics at 1/h;
- Karplus-Strong strums of standard open voicings, strings 15 ms apart: E, A, D, G, C, Am, Em, Dm, E7, A7.

Red for all: none of this exists.

1. `the_v04_judge_is_unchanged_on_the_shared_feed`: every V04 test passes after the factoring (name them in END).
2. `every_kind_is_named`: each of the ten kinds at roots C, F♯ and A, in two registers (around C3 and C4): the right root and kind in at least 95% of hops after the first 100 ms.
3. `open_chords_are_named_after_a_strum`: each strum is named correctly within 120 ms of its last string, and stays correct in at least 90% of hops while it rings above −50 dBFS.
4. `a_flat_guitar_still_reads_right`: the strums 20 cents flat: correct as in 3 after 2 s, with each tone's `detune_cents` within 3 cents of −20.
5. `a_slid_chord_glides`: an E major of harmonic tones gliding up a perfect fourth over 2 s:
   - every tone's `volts` is within 10 cents of the truth for its partial at that hop, and no hop-to-hop change exceeds the truth's change plus 5 cents;
   - the name passes E, F, F♯, G, G♯, A, each exactly once, with no flicker.
6. `a_vibrato_chord_keeps_its_name`: A minor with ±30 cents at 6 Hz on every note: the name never changes, and each tone's detune follows the vibrato within 8 cents.
7. `noise_and_silence_are_not_chords`: white noise at −20 dBFS: `sounding` in at most 5% of hops; silence: never.
8. `a_power_chord_is_a_power_chord`: E5 (E2, B2, E3) is named `Power`, not E major or E minor.
9. `nonsense_never_leaves_the_follower`: NaN and ±Inf bursts: every frame finite, and not sounding through them.
10. `the_feed_never_allocates_and_the_thread_comes_and_goes`: `Feed::push` inside `assert_no_alloc`; `spawn_chord`, 1 s of a chord, at least 80 frames, and a join within 100 ms of dropping the feed.
11. Bench `chord/hop/48k`: at most 100 µs; print it with the load average.

## Checks

Every cargo command runs under the kazoo cargo lock, niced, with four jobs. Read `df -h /System/Volumes/Data` before each and put it in START and END.

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-core --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-core --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-core/src/analysis kazoo-core/src/voice
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo bench -p kazoo-core --bench voice -j 4 -- 'chord'
```

Doc tests: `nice lockf -k /Users/tom/.kazoo-cargo.lock cargo test --doc -p kazoo-core -j 4`.

## Definition of done

- [ ] All red tests fail first (paste the failures), then pass. Every existing kazoo-core test passes, V04's unchanged.
- [ ] `cargo tree -p kazoo-fx` shows kazoo-fx still depends on nothing but rustfft.
- [ ] The module doc cites Fujishima, FMP C5S2, Oudre et al., the 36-bin defaults, and Laroche and Dolson, and says why binary templates and why peaks.
- [ ] BENCH.md `## KZ-P13 chord follower`: the bench row and the per-kind accuracy table, with the load average, date and sha.

## Code quality

As PLAN.md's STACK RULES. `ChordFollower::hop` allocates nothing (the picker, chroma and templates are built in `new`). `Feed::push` is callback-safe (V04's rules: one `Thread::unpark`, no lock). Every float in a frame is checked finite before it leaves. No `as` casts clippy pedantic flags. Files under 1000 lines.

## END

```
START KZ-P13 base=<sha> branch=lane/kz-P13 df=<GB> at <time>
END KZ-P13 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: each crate's nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: chord/hop/48k; the accuracy table
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
