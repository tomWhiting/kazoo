# KZ-V09a: follow the voice: one shared follower, one key (`i`) and one flag (`--follow`) for every pitched instrument, with the mini first

Line numbers are from the sweep that wrote this brief, at `fa3c8d8`, and may have moved. Find each item by what it does, and never rename anything. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Crates: kazoo-core (`voice/follow.rs`), kazoo-host (`keys.rs`), kazoo-mini, kazoo-cli. Time: about 30 minutes. Depends on:
- KZ-V07b, so the desk forwards voice frames;
- KZ-V08a, so there is a voice to follow for the by-hand proof;
- KZ-A05, because `kazoo-host/src/keys.rs` is on main once its merge lands.

Read by Cassio, 13 Oct evening. Changes: the checks take the lane form, and the by-hand listen is Cassio's and Tom's after landing. In `the_mini_follows_a_voice_legato`, "at most 3 ms behind" means: the measured pitch of each 20 ms window is within 10 cents of the reference slide's pitch over the same window shifted back by some lag of at most 3 ms; print the best lag. Opens only after V07b and V08a have landed.

Why: Tom asked for this: "i'd like voice to able to drive all the engines we have". Each instrument must decide the same things the same way: when to start a note, which pitch, when to retrigger, how loud, and when to let go. That logic is written once, in kazoo-core, and each instrument only maps it onto its own voice. KZ-V09b, V09c and V09d bring the other seven pitched instruments onto it.

## Decided for every instrument (this lane writes it; V09b to V09d follow it)

**How an instrument chooses to follow:**
- The `i` key toggles follow-the-voice. `--follow` on the command line starts the instrument following.
- **Why `i`:**
  - A05 left `c v b n m q r i` and the digits free for commands (`KZ-A05a-keyboard-map.md`, "Why this layout").
  - The sweep found `i` bound by no instrument today; `m` is the only other such letter.
  - `m` already means "hear the wall" on the wall console, so it is not used here.
  - Mnemonic: *i*nput, play from the voice coming in.
- **Why not the desk:** following is the instrument's own choice, like its octave. The desk only shows it (V07b's `follows` badge). A desk command can come later without changing this.

**What following means:**
- **Mono instruments** (mini, 303) play the voice legato with continuous pitch.
- **Poly instruments** (cs80, juno, prophet, dx, string, glass) give one voice to the voice. Keyboard notes keep the other voices.
- **Which pitch:** the frame's `snapped_hz`. The mouth quantises once, so every engine plays the same notes, and with quantise off `snapped_hz == hz` (V05). If `snapped_hz` is 0, `hz` is used.

**Loudness:** `gain = smoothstep(0.35, 0.75, loudness)`, held at or above `GAIN_MIN = 0.05` while the gate is open. Loudness 0.35 is −39 dBFS and 0.75 is −15 dBFS, the range a laptop mic sees from a quiet hum to a full voice.
- `velocity = 0.3 + 0.7 · gain` at each start and retrigger.
- While the gate is open, the followed voice's output is multiplied by `gain`, smoothed per sample with a 5 ms time constant (`GAIN_SMOOTH_MS`), so it swells and fades with the singer.
- When the gate closes, the gain is held at its last value, so the instrument's own release is heard.

**Pitch smoothing:** inside each instrument, the followed voice's Hz moves toward the newest frame's Hz with a one-pole of `FOLLOW_SMOOTH_MS = 2` ms, per sample. That removes the 750 Hz staircase of frame steps without adding audible lag, since 2 ms is under one cycle of any note below 500 Hz.

**Keyboard and voice together on a mono instrument:** the voice wins while it sounds. When it stops, the instrument goes back to whatever its keys hold.

## The design (decided; build exactly this)

### `kazoo-core/src/voice/follow.rs` (`pub mod follow;` in `voice/mod.rs`; tests in `voice/follow/tests.rs`)

```rust
/// What an instrument does with the voice. Called on its audio thread.
pub trait Follows {
    /// The voice began a note: sound one voice at `hz`.
    fn follow_start(&mut self, hz: f32, velocity: f32);
    /// The followed voice's pitch now (every frame while the gate is open).
    fn follow_pitch(&mut self, hz: f32);
    /// A re-sung syllable: retrigger the followed voice's envelopes at `hz`.
    fn follow_retrigger(&mut self, hz: f32, velocity: f32);
    /// The voice stopped: release the followed voice.
    fn follow_stop(&mut self);
    /// The followed voice's gain target, 0..=1 (the instrument smooths it).
    fn follow_gain(&mut self, gain: f32);
}

pub const STALE_MS: f32 = 100.0;   // no frame for 100 ms while sounding → stop: the mouth went away
pub const LIVE_MS: f32 = 250.0;    // a frame in the last 250 ms → the voice is live (for the status line)
pub const GAIN_MIN: f32 = 0.05;
pub const GAIN_SMOOTH_MS: f32 = 5.0;
pub const FOLLOW_SMOOTH_MS: f32 = 2.0;
#[must_use] pub fn gain_from_loudness(loudness: f32) -> f32;
#[must_use] pub fn velocity_from_gain(gain: f32) -> f32;

/// Lock-free flags the audio thread writes for the UI.
#[derive(Debug, Default)]
pub struct FollowShared { pub live: AtomicBool, pub sounding: AtomicBool }

pub struct Follower { /* rate, on, sounding, samples since the last frame, Arc<FollowShared> */ }
impl Follower {
    pub fn new(rate: u32, shared: Arc<FollowShared>) -> Self;
    /// Turning off while sounding calls follow_stop, then follow_gain(1.0).
    pub fn set_on(&mut self, on: bool, synth: &mut impl Follows);
    pub fn frame(&mut self, frame: &VoiceFrame, synth: &mut impl Follows);
    /// Once per callback, after the frames: advances the stale and live clocks.
    pub fn elapsed(&mut self, frames: usize, synth: &mut impl Follows);
    pub const fn is_on(&self) -> bool;
    pub const fn is_sounding(&self) -> bool;
}

/// The words every instrument shows, from the UI side.
pub enum FollowStatus { Off, NoDesk, DeskTooOld, Waiting, Singing }
impl FollowStatus {
    pub fn of(on: bool, connected: bool, desk_carries_voice: bool, shared: &FollowShared) -> Self;
}
// Display:
//   Off        → "i: follow the voice"
//   NoDesk     → "following the voice: no desk yet"
//   DeskTooOld → "following the voice: this desk cannot carry it"
//   Waiting    → "following the voice: waiting for a voice"
//   Singing    → "following the voice"
```

`frame()` does nothing while off. While on:
- gate open and not sounding: `follow_start(hz, velocity)` and `follow_gain(gain)`, then sounding;
- gate open and sounding: `follow_retrigger` on `onset`, else `follow_pitch`; then `follow_gain`;
- gate closed and sounding: `follow_stop`, then not sounding.

Every frame resets the stale clock and sets `live`. `elapsed()` stops a sounding voice after `STALE_MS` without frames, and clears `live` after `LIVE_MS`.

### `kazoo-host/src/keys.rs`

Add `pub const FOLLOW_VOICE: char = 'i';`, with the reason as its doc. Add one test: `semitone(FOLLOW_VOICE)` and `piano_key(FOLLOW_VOICE, _)` are both `None`.

### The pattern each instrument follows (the mini here; V09b to V09d repeat it)

1. **Audio thread.** The synth implements `Follows`, in a new `follow.rs` in the instrument's crate. In the place where it drains hub events every callback (mini: `audio.rs:193-195`), it also calls `kazoo_core::host::link::drain_voice(&mut hub.voice, |f| follower.frame(&f, &mut synth))`, then `follower.elapsed(frames, &mut synth)`. Frames are applied at the start of the callback, in order.
2. **Command.** `AudioCommand::Follow(bool)` (mini: `command.rs:26`) calls `follower.set_on(on, &mut synth)`.
3. **UI.** `i` toggles: the UI flips its `following` flag, sends `AudioCommand::Follow`, and calls `HubLink::follow_voice(on)`.
   - `Err(NotConnected)` is fine: the link sends the wish when it connects (V07a).
   - `Err(Unsupported)` shows `DeskTooOld`.
   - `Err(Full)` shows "busy: press i again", and `following` stays as it was.
4. **Status line.** `FollowStatus::of(..)` is shown in the instrument's footer or status line, and its help lists `i`.
5. **Flag.** `--follow` in the instrument's `Args` and in its kazoo-cli subcommand: at start, the same as pressing `i` once.

### The mini (kazoo-mini)

- **Voice.** The mono voice and its note stack are at `synth/mod.rs:27-90`, and the glide at `synth/glide.rs:23-100`, in semitones per second.
- **`follow_start(hz, v)`:** the glide jumps to `hz` with a new `Glide::jump_to(hz)` (current = target = hz). The envelopes gate on as for a non-legato key note, with velocity `v`. The voice's pitch then comes from a follow smoother, a one-pole at `FOLLOW_SMOOTH_MS` per sample, instead of the glide, until `follow_stop`. Why bypass the glide: at its default of 60 semitones per second (`glide.rs`), a vibrato or a slide would be rounded off, and Hz-for-Hz is the point.
- **`follow_pitch(hz)`:** the smoother's target.
- **`follow_retrigger`:** the envelopes retrigger at the current pitch.
- **`follow_stop`:** if the note stack holds keys, the newest held key sounds as if just pressed, with legato from the voice's pitch through the glide. Otherwise the envelopes release.
- **`follow_gain`:** a smoothed multiplier on the voice's output (`GAIN_SMOOTH_MS`).
- While following and sounding, key presses still go into the note stack but do not sound until the voice stops.

## Not this lane

The other instruments (V09b: 303, cs80, dx; V09c: juno, prophet; V09d: string, glass). The wall (V10).

## Red tests first

Red for all: none of this exists.
- `voice/follow/tests.rs`: a recording `Follows` fake.
  - `a_note_starts_glides_and_stops`: frames gate-open at 220 then 230 Hz, then gate-closed give start(220), pitch(230), stop, in that order, with the right velocities.
  - `a_re_sung_syllable_retriggers`: an `onset` with the gate held open gives `retrigger`.
  - `off_ignores_the_voice`.
  - `turning_off_while_sounding_releases_and_restores_gain`: gives stop, then gain(1.0).
  - `a_voice_that_goes_away_is_released`: no frames for 100 ms while sounding gives stop, and after 250 ms `live` is false.
  - `snapped_pitch_is_followed_and_zero_falls_back_to_hz`.
  - `gain_and_velocity_curves_are_pinned`: loudness 0.35 → `GAIN_MIN`, 0.55 → 0.5, 0.75 → 1.0, NaN → `GAIN_MIN`.
  - `following_never_allocates`: 10,000 frames inside `assert_no_alloc`.
  - `the_status_words`: every `FollowStatus` case from `of(..)`.
- `kazoo-host` `keys::tests::the_follow_key_is_never_a_note`.
- kazoo-mini:
  - `the_mini_follows_a_voice_legato`: drive the audio render with frames at 220 Hz, then a slide to 330 Hz over 200 ms. The rendered output's pitch, from zero crossings over 20 ms windows, tracks the slide within 10 cents, at most 3 ms behind it.
  - `the_mini_gives_the_keys_back`: hold a key while the voice sounds; when the voice stops, the key's note sounds.
  - `i_toggles_following_and_says_so`: the key handler sends `Follow(true)` and the footer shows the status words.
- kazoo-cli: `mini_follow_flag_parses`.

## Checks

Every cargo command runs under the kazoo cargo lock, niced, with four jobs. Read `df -h /System/Volumes/Data` before each and put it in START and END.

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-core -p kazoo-host -p kazoo-mini -p kazoo-cli --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-core -p kazoo-host -p kazoo-mini -p kazoo-cli --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-core/src/voice kazoo-host kazoo-mini kazoo-cli
```

Doc tests: `nice lockf -k /Users/tom/.kazoo-cargo.lock cargo test --doc -p kazoo-core -p kazoo-host -j 4`.

## Definition of done

- [ ] All red tests are red first (paste the output), then green. Every existing test is green.
- [ ] Not the builder's: after landing, Cassio starts the desk, the mouth and the mini, checks that `i` on the mini says "following the voice: waiting for a voice", and asks Tom to sing: the mini should play his line legato, say "following the voice", and release when he stops. The builder never opens Tom's mic or speakers, lists this under NOT CONFIRMED and does not claim it.

## Code quality

As PLAN.md's STACK RULES. `Follower`, `drain_voice` and every `Follows` method run on the audio thread, with no allocation, lock, I/O, format or panic. `FollowShared` holds atomics only. The UI never waits on the audio side. No `as` casts that clippy pedantic flags. Files stay under 1000 lines.

## END

```
START KZ-V09a base=<sha> branch=lane/kz-V09a df=<GB> at <time>
END KZ-V09a head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: each crate's nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: none asked
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
