# Terminal DAW Best-Case Implementation Checklist

This is not an MVP checklist. It is a construction order for the best-case architecture.

## 0. Design Foundation

- [x] Define top-level terminal DAW architecture.
- [x] Define `kazoo-mix` responsibilities.
- [x] Define `kazoo-tape` responsibilities.
- [x] Define IPC/control/audio transport model.
- [x] Define crate split and future unified binary.
- [ ] Decide whether tape DSP lives in `kazoo-core` or `kazoo-tape` library.
- [ ] Decide whether `kazoo-mix` starts fresh or absorbs pieces of `kazoo-tui`.

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

- [ ] Implement socket-only audio transport for correctness testing.
- [ ] Implement shared memory abstraction.
- [ ] Implement shared audio block ring buffer.
- [ ] Add underrun detection.
- [ ] Add frame-sequence validation.
- [ ] Add reconnect cleanup.
- [ ] Add latency test tool.

## 4. kazoo-mix Engine

- [ ] Create `kazoo-mix` crate.
- [ ] Open and own `cpal` output stream.
- [ ] Implement fixed-size channel storage.
- [ ] Implement channel trim/fader/pan.
- [ ] Implement mute/solo/arm.
- [ ] Implement basic meter state.
- [ ] Implement master bus.
- [ ] Ensure callback has no allocation/locks/socket I/O.
- [ ] Add audio callback stress tests where practical.

## 5. kazoo-mix UI

- [ ] Build terminal console layout.
- [ ] Add channel bank paging.
- [ ] Add transport bar.
- [ ] Add channel faders.
- [ ] Add meters.
- [ ] Add channel health/underrun indicators.
- [ ] Add keyboard control.
- [ ] Add mouse click/drag control.
- [ ] Ensure quit works: Esc, Ctrl-Q, Ctrl-C, Ctrl-D.

## 6. Mixer Server

- [ ] Create session runtime directory.
- [ ] Create Unix control socket.
- [ ] Accept client registration.
- [ ] Assign channels.
- [ ] Broadcast transport snapshots.
- [ ] Track client heartbeat/status.
- [ ] Handle disconnect without audio panic.

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
- [ ] Add studio client mode to `kazoo-808`.
- [ ] Make 808 pattern clock follow mixer BPM/frame.
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
