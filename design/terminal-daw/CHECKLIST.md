# Terminal DAW Best-Case Implementation Checklist

This is not an MVP checklist. It is a construction order for the best-case architecture.

## 0. Design Foundation

- [x] Define top-level terminal DAW architecture.
- [x] Define `kazoo-mix` responsibilities.
- [x] Define `kazoo-tape` responsibilities.
- [x] Define IPC/control/audio transport model.
- [x] Define crate split and future unified binary.
- [ ] Decide whether tape DSP lives in `kazoo-core` or `kazoo-tape` library.
- [x] Decide whether `kazoo-mix` starts fresh or absorbs pieces of `kazoo-tui`. Decided 2026-09-25: `kazoo-mix` is the studio hub, built fresh; `kazoo-tui` is the voice-driven instrument.

## 1. Core Protocol Types

- [x] Add `kazoo-core::protocol` module.
- [x] Define `ClientHello` / `ServerWelcome`.
- [x] Define `TransportSnapshot`.
- [x] Define `RenderRequest` / `RenderComplete`.
- [x] Define `NoteEvent` / `ParameterEvent`.
- [x] Define `ChannelId`, `ClientId`, `BufferId`.
- [x] Add version negotiation.
- [ ] Add serialization tests.

Note: these are currently domain types only. The existing `kazoo-core::ipc` hub protocol remains in place until `kazoo-mix` grows the new control-plane/server implementation.

## 2. Transport Math

- [ ] Add sample frame to bar/beat/tick conversion.
- [ ] Add BPM change model.
- [ ] Add loop region frame math.
- [ ] Add render-request splitting at loop boundaries.
- [ ] Add swing state.
- [ ] Add deterministic groove template type.
- [ ] Add tests for BPM, loop, and swing timing.

## 3. Shared Audio Transport

- [x] Implement socket-only audio transport for correctness testing. Instruments stream over the hub socket; the hub stamps each block onto the studio clock into a frame-indexed ring per strip.
- [ ] Implement shared memory abstraction.
- [ ] Implement shared audio block ring buffer.
- [x] Add underrun detection. Per-strip underruns, slips and resyncs show on the desk.
- [x] Add frame-sequence validation. Stale blocks are discarded, early ones leave a gap, and far-off streams re-anchor.
- [x] Add reconnect cleanup. The hub unplugs a strip when its instrument leaves; `kazoo_core::ipc::link` reconnects instruments whenever the desk returns.
- [ ] Add latency test tool.

## 4. kazoo-mix Engine

- [x] Create `kazoo-mix` crate.
- [x] Open and own `cpal` output stream.
- [x] Implement fixed-size channel storage.
- [x] Implement channel trim/fader/pan (plus 3-band EQ and a post-fader aux send into a reverb return).
- [ ] Implement mute/solo/arm. Mute and solo are done; arm waits on recording.
- [x] Implement basic meter state (peak with hold, VU-style RMS, clip latches).
- [x] Implement master bus.
- [x] Ensure callback has no allocation/locks/socket I/O.
- [ ] Add audio callback stress tests where practical.

## 5. kazoo-mix UI

- [x] Build terminal console layout.
- [x] Add channel bank paging.
- [x] Add transport bar. Play/stop button, tempo, tap tempo (`t`), tempo nudge (`[` `]`), with every instrument following.
- [x] Add channel faders.
- [x] Add meters.
- [x] Add channel health/underrun indicators.
- [x] Add keyboard control.
- [x] Add mouse click/drag control.
- [x] Ensure quit works: Esc, Ctrl-Q, Ctrl-C, Ctrl-D.

## 6. Mixer Server

- [x] Create session runtime directory. Uses the kazoo-core discovery directory and PID file that instruments already know.
- [x] Create Unix control socket.
- [x] Accept client registration. Sample rate and channel count are checked; refusals are shown on the desk.
- [x] Assign channels.
- [x] Broadcast transport snapshots. Tempo and play state go out on change and on join; instruments can request changes.
- [ ] Track client heartbeat/status. Disconnects and stalled readers are detected; a hung instrument that keeps its socket open but stops sending is not yet flagged.
- [x] Handle disconnect without audio panic. Rings leave the callback through the patchbay and are freed on the hub thread.

## 7. Juno Studio Client

- [ ] Add `--standalone` / `--connect` flags.
- [ ] Auto-detect mixer socket.
- [ ] In connected mode, do not open output device.
- [ ] Register as instrument client.
- [ ] Render requested blocks.
- [ ] Send audio to assigned shared buffer.
- [ ] Receive BPM/transport.
- [ ] Display assigned channel/status in UI.

## 8. 303 / 808 / Arp Sync

- [ ] Add studio client mode to `kazoo-303`.
- [ ] Make 303 sequencer follow mixer BPM/frame.
- [x] Add studio client mode to `kazoo-808` (shared `kazoo_core::ipc::link`; local output goes quiet while the desk plays it). Also `kazoo-mini`, `kazoo-cs80`, `kazoo-dx` and `kazoo-arp`.
- [x] Make 808 pattern clock follow mixer BPM/frame. Sample-accurate:
  - The desk schedules each transport change ahead (`kazoo-mix/src/song.rs`).
  - The hub tells each instrument the frame of its own stream the change lands on, and the song position there.
  - The 808 applies it on that frame with `TransportFollower` and `SequencerClock::seek`.
  - Proven end to end by `hub::tests::an_instrument_starts_on_the_very_frame_the_desk_does`.
  - Also fixed two 808 clock bugs: the downbeat fired a sixteenth late, and swing was inverted.
  - The same work is in progress for the 303, arp and DX phrase.
- [ ] Add controller mode to `kazoo-arp`.
- [ ] Send timestamped note events via mixer.
- [ ] Add swing/groove support.

## 9. kazoo-tape DSP

- [ ] Create tape DSP library.
- [ ] Implement saturation.
- [ ] Implement head bump.
- [ ] Implement HF rolloff.
- [ ] Implement wow/flutter delay modulation.
- [ ] Implement procedural hiss.
- [ ] Implement crosstalk/stereo glue.
- [ ] Add finite-output tests.
- [ ] Add bypass/click-management tests.

## 10. kazoo-tape Recorder

- [ ] Implement record block queue.
- [ ] Implement disk writer thread.
- [ ] Write 32-bit float WAV.
- [ ] Record master pre/post tape.
- [ ] Record stems.
- [ ] Implement loop take naming.
- [ ] Implement punch in/out.
- [ ] Ensure no disk I/O in audio callback.

## 11. kazoo-tape UI

- [ ] Build reel-to-reel terminal UI.
- [ ] Show tape speed, reels, meters.
- [ ] Control record/play/stop/loop/punch.
- [ ] Edit tape parameters.
- [ ] Show current take.
- [ ] Connect as tape UI to mixer.

## 12. kazoo-mouth Split

- [ ] Decide current `kazoo-tui` pieces to keep.
- [ ] Create/rename `kazoo-mouth`.
- [ ] Keep mic input and voice analysis.
- [ ] Keep pitch/formant/onset modes.
- [ ] Remove central mixer responsibilities.
- [ ] Add studio client mode.
- [ ] Send generated mouth-noise audio to mixer.

## 13. Session Files

- [ ] Define session directory format.
- [ ] Save mixer state.
- [ ] Save transport state.
- [ ] Save routing.
- [ ] Save instrument instance ids.
- [ ] Save tape takes metadata.
- [ ] Restore session.

## 14. Unified Binary

- [ ] Create top-level `kazoo` package/binary.
- [ ] Add subcommands.
- [ ] Add common CLI flags.
- [ ] Add `kazoo studio` launcher.
- [ ] Add tmux/wezterm/iTerm layout backends.
- [ ] Keep individual crates runnable during development.

## 15. Quality Gates

- [ ] Per-crate tests pass.
- [ ] Clippy clean for touched crates.
- [ ] No callback allocation in mixer/tape hot path.
- [ ] No callback locks/socket/file I/O.
- [ ] Instrument disconnect does not crash mixer.
- [ ] Missing audio blocks become silence with visible underrun.
- [ ] Quit shortcuts work everywhere.
- [ ] BPM sync verified against 303/808/arp.
- [ ] Loop recording aligns to sample frame.
