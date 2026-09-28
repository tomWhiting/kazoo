//! Desk interaction model: what is focused, how keys and the mouse move
//! controls, and the meter ballistics the UI draws.
//!
//! The desk never owns control values. It reads them from [`SharedState`],
//! changes them, and writes them straight back, so the audio callback always
//! hears exactly what the desk shows. The UI thread is the only writer of
//! controls, so read-modify-write here cannot race.

use std::time::Instant;

use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;

use crate::controls::{ChannelControls, MasterControl, MasterControls, Step, StripControl};
use crate::engine::StereoLevel;
use crate::meters::{PeakMeter, VuMeter};
use crate::shared::{DESK_CHANNELS, MeterTake, SharedState};
use crate::tap::TapTempo;

/// What the keyboard is pointing at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    /// A control on a channel strip.
    Strip {
        /// Strip index (0-based).
        slot: usize,
        /// Focused control.
        control: StripControl,
    },
    /// A control on the master section.
    Master {
        /// Focused control.
        control: MasterControl,
    },
}

/// Something on screen that responds to the mouse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitTarget {
    /// A strip control (knob row or fader).
    Strip {
        /// Strip index.
        slot: usize,
        /// Control under the pointer.
        control: StripControl,
    },
    /// A master-section control.
    Master {
        /// Control under the pointer.
        control: MasterControl,
    },
    /// A strip's mute button.
    Mute {
        /// Strip index.
        slot: usize,
    },
    /// A strip's solo button.
    Solo {
        /// Strip index.
        slot: usize,
    },
    /// Any clip light: clicking clears them all.
    ClipLights,
    /// Show the previous bank of strips.
    BankPrevious,
    /// Show the next bank of strips.
    BankNext,
    /// The transport's play/stop button.
    PlayStop,
}

/// A clickable region recorded while drawing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hit {
    /// Screen area.
    pub area: Rect,
    /// What it controls.
    pub target: HitTarget,
    /// For faders, the rows of the fader throw (clicks and drags set the
    /// fader from the pointer's row within this span).
    pub throw: Option<Throw>,
}

/// Vertical span of a fader's travel on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Throw {
    /// Row of the top of the travel (+10 dB).
    pub top: u16,
    /// Number of rows of travel.
    pub rows: u16,
}

impl Throw {
    /// Fader position (0 bottom, 1 top) for a screen row.
    #[must_use]
    pub fn position_at(self, row: u16) -> f32 {
        if self.rows <= 1 {
            return 1.0;
        }
        let offset = row.saturating_sub(self.top).min(self.rows - 1);
        1.0 - f32::from(offset) / f32::from(self.rows - 1)
    }

    /// Screen row for a fader position.
    #[must_use]
    pub fn row_at(self, position: f32) -> u16 {
        if self.rows <= 1 {
            return self.top;
        }
        let position = if position.is_finite() {
            position.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let offset = ((1.0 - position) * f32::from(self.rows - 1)).round() as u16;
        self.top + offset.min(self.rows - 1)
    }
}

/// Stereo meter ballistics for one strip or the master bus.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct StereoMeters {
    /// Left VU.
    pub vu_left: VuMeter,
    /// Right VU.
    pub vu_right: VuMeter,
    /// Left peak bar.
    pub peak_left: PeakMeter,
    /// Right peak bar.
    pub peak_right: PeakMeter,
    /// Time since the meters last moved, waiting for audio.
    pending_seconds: f32,
    /// No audio has arrived for longer than the stream's rhythm explains.
    stalled: bool,
    /// Recent longest wait between audio arrivals, decaying slowly.
    arrival_gap: f32,
}

impl StereoMeters {
    /// Advance the meters by `dt` seconds with the audio drained this tick.
    ///
    /// A large device buffer arrives less often than the desk redraws, so
    /// many ticks carry no audio. Those ticks hold the meters and the time is
    /// applied with the next audio that arrives, so the needles read the same
    /// at any buffer size.
    ///
    /// When audio stops coming for twice the recent gap between arrivals
    /// (between [`METER_STALL_MIN_SECONDS`] and [`METER_STALL_SECONDS`]),
    /// the stream has stopped: the held time is dropped rather than applied
    /// in one lurch, and the meters fall smoothly, tick by tick, until audio
    /// returns. A stop reads as a brief hold, like the peak marker's, then a
    /// steady fall.
    fn update(&mut self, take: MeterTake, dt: f32) {
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        let (peak, rms, elapsed) = if take.frames > 0 {
            let gap = self.pending_seconds + dt;
            self.arrival_gap = gap.max(self.arrival_gap * ARRIVAL_GAP_DECAY);
            self.stalled = false;
            (take.peak, take.rms, gap)
        } else if self.stalled {
            (StereoLevel::ZERO, StereoLevel::ZERO, dt)
        } else if self.pending_seconds + dt >= self.stall_seconds() {
            self.stalled = true;
            (StereoLevel::ZERO, StereoLevel::ZERO, dt)
        } else {
            self.pending_seconds += dt;
            return;
        };
        self.pending_seconds = 0.0;
        self.vu_left.update(rms.left, elapsed);
        self.vu_right.update(rms.right, elapsed);
        self.peak_left.update(peak.left, elapsed);
        self.peak_right.update(peak.right, elapsed);
    }

    /// Wait without audio after which the stream counts as stopped.
    fn stall_seconds(&self) -> f32 {
        (2.0 * self.arrival_gap).clamp(METER_STALL_MIN_SECONDS, METER_STALL_SECONDS)
    }
}

/// Longest the meters wait for audio before treating the stream as stopped:
/// longer than any device buffer the desk expects.
pub const METER_STALL_SECONDS: f32 = 0.5;

/// Shortest wait before the meters treat the stream as stopped, however
/// often audio has been arriving.
pub const METER_STALL_MIN_SECONDS: f32 = 0.1;

/// Per-arrival decay of the remembered gap between arrivals, so the stall
/// wait follows a device that switches to smaller buffers.
const ARRIVAL_GAP_DECAY: f32 = 0.99;

/// How long a strip shows trouble after its last underrun, slip or resync.
pub const TROUBLE_HOLD_SECONDS: f32 = 3.0;

/// Recent source health for one strip.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct StripHealth {
    underruns: u64,
    slips: u64,
    resyncs: u64,
    trouble_remaining: f32,
}

impl StripHealth {
    fn update(&mut self, underruns: u64, slips: u64, resyncs: u64, dt: f32) {
        let worse = underruns > self.underruns || slips > self.slips || resyncs > self.resyncs;
        // Counters reset when a source is (re)attached; follow them down.
        self.underruns = underruns;
        self.slips = slips;
        self.resyncs = resyncs;
        if worse {
            self.trouble_remaining = TROUBLE_HOLD_SECONDS;
        } else if dt.is_finite() {
            self.trouble_remaining = (self.trouble_remaining - dt.max(0.0)).max(0.0);
        }
    }

    /// The source had an underrun, slip or resync in the last few seconds.
    #[must_use]
    pub fn troubled(&self) -> bool {
        self.trouble_remaining > 0.0
    }
}

/// An in-progress fader drag, relative to where the fader was grabbed.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Drag {
    hit: Hit,
    anchor_row: u16,
    anchor_position: f32,
}

impl Drag {
    /// Fader position for the pointer at `row`. Movement is relative to the
    /// grab point, but reaching either end of the drawn throw pins the fader
    /// to that end, so the value always agrees with where the cap is drawn.
    fn position_at(self, throw: Throw, row: u16) -> f32 {
        let bottom = throw.top.saturating_add(throw.rows.saturating_sub(1));
        if row <= throw.top {
            return 1.0;
        }
        if row >= bottom {
            return 0.0;
        }
        let travel = f32::from(throw.rows.saturating_sub(1).max(1));
        let moved = (f32::from(self.anchor_row) - f32::from(row)) / travel;
        (self.anchor_position + moved).clamp(0.0, 1.0)
    }
}

/// Desk UI state.
#[derive(Debug)]
pub struct Desk {
    /// Current keyboard focus.
    pub focus: Focus,
    /// First strip shown when the terminal is too narrow for all of them.
    pub first_visible: usize,
    /// Meter ballistics per strip.
    pub strip_meters: [StereoMeters; DESK_CHANNELS],
    /// Master meter ballistics.
    pub master_meters: StereoMeters,
    /// Recent source health per strip.
    pub strip_health: [StripHealth; DESK_CHANNELS],
    /// Strips that fit on screen at the last draw.
    pub visible_strips: usize,
    /// Key help overlay is open.
    pub help_open: bool,
    /// Clickable regions from the last draw.
    pub hits: Vec<Hit>,
    /// Fader being dragged, if any.
    dragging: Option<Drag>,
    /// The user asked to quit.
    pub should_quit: bool,
    /// Tap tempo state.
    tap: TapTempo,
}

impl Default for Desk {
    fn default() -> Self {
        Self::new()
    }
}

impl Desk {
    /// A desk focused on strip 1's fader.
    #[must_use]
    pub fn new() -> Self {
        Self {
            focus: Focus::Strip {
                slot: 0,
                control: StripControl::Fader,
            },
            first_visible: 0,
            strip_meters: [StereoMeters::default(); DESK_CHANNELS],
            master_meters: StereoMeters::default(),
            strip_health: [StripHealth::default(); DESK_CHANNELS],
            visible_strips: DESK_CHANNELS,
            help_open: false,
            hits: Vec::new(),
            dragging: None,
            should_quit: false,
            tap: TapTempo::default(),
        }
    }

    /// Advance meter ballistics and strip health by `dt_seconds`, draining
    /// everything the callback has accumulated since the last tick.
    pub fn tick(&mut self, shared: &SharedState, dt_seconds: f32) {
        for (slot, (meters, health)) in self
            .strip_meters
            .iter_mut()
            .zip(self.strip_health.iter_mut())
            .enumerate()
        {
            meters.update(shared.take_channel_meters(slot), dt_seconds);
            let readout = shared.channel_readout(slot);
            health.update(
                readout.underruns,
                readout.slips,
                readout.resyncs,
                dt_seconds,
            );
        }
        self.master_meters
            .update(shared.take_master_meters(), dt_seconds);
    }

    /// Keep the focused strip inside the visible bank of `visible` strips.
    pub fn scroll_to_focus(&mut self, visible: usize) {
        let visible = visible.clamp(1, DESK_CHANNELS);
        self.visible_strips = visible;
        let max_first = DESK_CHANNELS - visible;
        if let Focus::Strip { slot, .. } = self.focus {
            if slot < self.first_visible {
                self.first_visible = slot;
            } else if slot >= self.first_visible + visible {
                self.first_visible = slot + 1 - visible;
            }
        }
        self.first_visible = self.first_visible.min(max_first);
    }

    /// Handle a key press.
    pub fn handle_key(&mut self, key: KeyEvent, shared: &SharedState) {
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return;
        }
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        if control && matches!(key.code, KeyCode::Char('c' | 'q' | 'd')) {
            self.should_quit = true;
            return;
        }
        if self.help_open {
            // Any key, Ctrl combinations included, closes help; q also quits.
            self.help_open = false;
            if !control && matches!(key.code, KeyCode::Char('q')) {
                self.should_quit = true;
            }
            return;
        }
        if control {
            return;
        }

        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Char('?') => {
                self.help_open = true;
                self.dragging = None;
            }
            KeyCode::Left | KeyCode::Char('h') => self.move_strip(-1),
            KeyCode::Right | KeyCode::Char('l') => self.move_strip(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_control(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_control(1),
            KeyCode::Char('+' | '=') => self.adjust_focused(shared, 1, Step::Fine),
            KeyCode::Char('-' | '_') => self.adjust_focused(shared, -1, Step::Fine),
            KeyCode::Char('}') | KeyCode::PageUp => self.adjust_focused(shared, 1, Step::Coarse),
            KeyCode::Char('{') | KeyCode::PageDown => {
                self.adjust_focused(shared, -1, Step::Coarse);
            }
            KeyCode::Char('0') => self.reset_focused(shared),
            KeyCode::Char('R') => self.reset_focused_strip(shared),
            KeyCode::Char('m') => self.toggle_focused(shared, Toggle::Mute),
            KeyCode::Char('s') => self.toggle_focused(shared, Toggle::Solo),
            KeyCode::Char('x') | KeyCode::Backspace => shared.clear_clips(),
            KeyCode::Char(' ') => toggle_playing(shared),
            KeyCode::Char('c') => shared.set_click(!shared.click()),
            KeyCode::Char('t') => {
                if let Some(bpm) = self.tap.tap(Instant::now()) {
                    shared.set_tempo(bpm);
                }
            }
            KeyCode::Char('[') => nudge_tempo(shared, -1.0),
            KeyCode::Char(']') => nudge_tempo(shared, 1.0),
            KeyCode::Char(ch) if ch.is_ascii_digit() => self.jump_to_number(ch),
            _ => {}
        }
    }

    /// Handle a mouse event against the hit map from the last draw.
    pub fn handle_mouse(&mut self, event: MouseEvent, shared: &SharedState) {
        // A drag lives only between one press and its release. A release that
        // was lost (outside the window, or while help was open) must never
        // leave a stale grab that a later drag would act on.
        if matches!(event.kind, MouseEventKind::Down(_) | MouseEventKind::Up(_)) {
            self.dragging = None;
        }
        if self.help_open {
            // The help overlay covers the desk: a click closes it and nothing
            // underneath is touched.
            if matches!(event.kind, MouseEventKind::Down(_)) {
                self.help_open = false;
            }
            return;
        }
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let Some(hit) = self.hit_at(event.column, event.row) else {
                    return;
                };
                match hit.target {
                    HitTarget::Mute { slot } => {
                        self.focus_strip(slot);
                        toggle(shared, slot, Toggle::Mute);
                    }
                    HitTarget::Solo { slot } => {
                        self.focus_strip(slot);
                        toggle(shared, slot, Toggle::Solo);
                    }
                    HitTarget::ClipLights => shared.clear_clips(),
                    HitTarget::BankPrevious => self.shift_bank(-1),
                    HitTarget::BankNext => self.shift_bank(1),
                    HitTarget::PlayStop => toggle_playing(shared),
                    HitTarget::Strip { slot, control } => {
                        self.focus = Focus::Strip { slot, control };
                        if let Some(throw) = hit.throw {
                            let current = shared
                                .channel_controls(slot)
                                .normalized(StripControl::Fader);
                            self.grab_fader(hit, throw, current, event.row, shared);
                        }
                    }
                    HitTarget::Master { control } => {
                        self.focus = Focus::Master { control };
                        if let Some(throw) = hit.throw {
                            let current = shared.master_controls().normalized(MasterControl::Fader);
                            self.grab_fader(hit, throw, current, event.row, shared);
                        }
                    }
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let Some(drag) = self.dragging else {
                    return;
                };
                let Some(throw) = drag.hit.throw else {
                    return;
                };
                apply_fader(shared, drag.hit.target, drag.position_at(throw, event.row));
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let steps = if event.kind == MouseEventKind::ScrollUp {
                    1
                } else {
                    -1
                };
                let Some(hit) = self.hit_at(event.column, event.row) else {
                    return;
                };
                match hit.target {
                    HitTarget::Strip { slot, control } => {
                        self.focus = Focus::Strip { slot, control };
                        self.adjust_focused(shared, steps, Step::Fine);
                    }
                    HitTarget::Master { control } => {
                        self.focus = Focus::Master { control };
                        self.adjust_focused(shared, steps, Step::Fine);
                    }
                    HitTarget::Mute { .. }
                    | HitTarget::Solo { .. }
                    | HitTarget::ClipLights
                    | HitTarget::BankPrevious
                    | HitTarget::BankNext
                    | HitTarget::PlayStop => {}
                }
            }
            _ => {}
        }
    }

    /// Drop a fader drag whose fader is no longer drawn where it was grabbed.
    ///
    /// Called after every draw rebuilds the hit map: if the terminal was
    /// resized or the bank moved mid-drag, the pointer rows no longer map
    /// onto the throw the drag started with, so the grab is released rather
    /// than moving a fader that is elsewhere or off screen.
    pub fn release_drag_if_moved(&mut self) {
        let Some(drag) = self.dragging else {
            return;
        };
        // Same control, same place, same throw: the target alone is not
        // enough, since a bank move slides a strip sideways unchanged.
        let still_drawn = self.hits.contains(&drag.hit);
        if !still_drawn {
            self.dragging = None;
        }
    }

    /// Start a fader drag. Grabbing the cap leaves the value exactly where it
    /// is; clicking elsewhere on the throw jumps the fader there first. Either
    /// way the drag then moves relative to the grab point.
    fn grab_fader(&mut self, hit: Hit, throw: Throw, current: f32, row: u16, shared: &SharedState) {
        let anchor_position = if throw.row_at(current) == row {
            current
        } else {
            let position = throw.position_at(row);
            apply_fader(shared, hit.target, position);
            position
        };
        self.dragging = Some(Drag {
            hit,
            anchor_row: row,
            anchor_position,
        });
    }

    /// Page the visible bank of strips, moving focus onto the new bank.
    fn shift_bank(&mut self, direction: i32) {
        let visible = self.visible_strips.clamp(1, DESK_CHANNELS);
        let max_first = DESK_CHANNELS - visible;
        let first = if direction < 0 {
            self.first_visible.saturating_sub(visible)
        } else {
            (self.first_visible + visible).min(max_first)
        };
        self.first_visible = first;
        let control = match self.focus {
            Focus::Strip { control, .. } => control,
            Focus::Master { .. } => StripControl::Fader,
        };
        self.focus = Focus::Strip {
            slot: first,
            control,
        };
    }

    /// Number keys: 1‥N pick a strip, N+1 picks the master section.
    fn jump_to_number(&mut self, ch: char) {
        let Some(number) = ch.to_digit(10).map(|d| d as usize) else {
            return;
        };
        if (1..=DESK_CHANNELS).contains(&number) {
            let slot = number - 1;
            self.focus = match self.focus {
                Focus::Strip { control, .. } => Focus::Strip { slot, control },
                Focus::Master { .. } => Focus::Strip {
                    slot,
                    control: StripControl::Fader,
                },
            };
        } else if number == DESK_CHANNELS + 1 {
            self.focus = Focus::Master {
                control: MasterControl::Fader,
            };
        }
    }

    fn hit_at(&self, column: u16, row: u16) -> Option<Hit> {
        // Later hits are drawn on top, so search from the end.
        self.hits.iter().rev().copied().find(|hit| {
            column >= hit.area.x
                && column < hit.area.x.saturating_add(hit.area.width)
                && row >= hit.area.y
                && row < hit.area.y.saturating_add(hit.area.height)
        })
    }

    const fn focus_strip(&mut self, slot: usize) {
        if let Focus::Strip { slot: current, .. } = &mut self.focus {
            *current = slot;
        } else {
            self.focus = Focus::Strip {
                slot,
                control: StripControl::Fader,
            };
        }
    }

    fn move_strip(&mut self, delta: i32) {
        self.focus = match (self.focus, delta.signum()) {
            (Focus::Strip { slot, control }, 1) if slot + 1 >= DESK_CHANNELS => Focus::Master {
                control: if control == StripControl::Aux {
                    MasterControl::AuxReturn
                } else {
                    MasterControl::Fader
                },
            },
            (Focus::Strip { slot, control }, 1) => Focus::Strip {
                slot: slot + 1,
                control,
            },
            (Focus::Strip { slot, control }, -1) => Focus::Strip {
                slot: slot.saturating_sub(1),
                control,
            },
            (Focus::Master { control }, -1) => Focus::Strip {
                slot: DESK_CHANNELS - 1,
                control: match control {
                    MasterControl::AuxReturn => StripControl::Aux,
                    MasterControl::Fader => StripControl::Fader,
                },
            },
            (focus, _) => focus,
        };
    }

    fn move_control(&mut self, delta: i32) {
        self.focus = match self.focus {
            Focus::Strip { slot, control } => Focus::Strip {
                slot,
                control: if delta > 0 {
                    control.next()
                } else {
                    control.previous()
                },
            },
            Focus::Master { control } => Focus::Master {
                control: control.toggled(),
            },
        };
    }

    fn adjust_focused(&self, shared: &SharedState, steps: i32, step: Step) {
        match self.focus {
            Focus::Strip { slot, control } => {
                let mut controls = shared.channel_controls(slot);
                controls.adjust(control, steps, step);
                shared.store_channel_controls(slot, controls);
            }
            Focus::Master { control } => {
                let mut master = shared.master_controls();
                master.adjust(control, steps, step);
                shared.store_master_controls(master);
            }
        }
    }

    fn reset_focused(&self, shared: &SharedState) {
        match self.focus {
            Focus::Strip { slot, control } => {
                let mut controls = shared.channel_controls(slot);
                controls.reset(control);
                shared.store_channel_controls(slot, controls);
            }
            Focus::Master { control } => {
                let mut master = shared.master_controls();
                master.reset(control);
                shared.store_master_controls(master);
            }
        }
    }

    fn reset_focused_strip(&self, shared: &SharedState) {
        match self.focus {
            Focus::Strip { slot, .. } => {
                shared.store_channel_controls(slot, ChannelControls::DEFAULT);
            }
            Focus::Master { .. } => shared.store_master_controls(MasterControls::DEFAULT),
        }
    }

    fn toggle_focused(&self, shared: &SharedState, which: Toggle) {
        if let Focus::Strip { slot, .. } = self.focus {
            toggle(shared, slot, which);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Toggle {
    Mute,
    Solo,
}

fn toggle(shared: &SharedState, slot: usize, which: Toggle) {
    let mut controls = shared.channel_controls(slot);
    match which {
        Toggle::Mute => controls.muted = !controls.muted,
        Toggle::Solo => controls.soloed = !controls.soloed,
    }
    shared.store_channel_controls(slot, controls);
}

fn apply_fader(shared: &SharedState, target: HitTarget, position: f32) {
    match target {
        HitTarget::Strip { slot, .. } => set_strip_fader(shared, slot, position),
        HitTarget::Master { .. } => set_master_fader(shared, position),
        HitTarget::Mute { .. }
        | HitTarget::Solo { .. }
        | HitTarget::ClipLights
        | HitTarget::BankPrevious
        | HitTarget::BankNext
        | HitTarget::PlayStop => {}
    }
}

/// Start the studio if stopped, stop it if playing.
fn toggle_playing(shared: &SharedState) {
    shared.set_playing(!shared.transport().playing);
}

/// Move the tempo by `bpm` (clamped to the transport's range).
fn nudge_tempo(shared: &SharedState, bpm: f32) {
    shared.set_tempo(shared.transport().bpm + bpm);
}

fn set_strip_fader(shared: &SharedState, slot: usize, position: f32) {
    let mut controls = shared.channel_controls(slot);
    controls.set_fader_position(position);
    shared.store_channel_controls(slot, controls);
}

fn set_master_fader(shared: &SharedState, position: f32) {
    let mut master = shared.master_controls();
    master.set_fader_position(position);
    shared.store_master_controls(master);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controls::{FADER_MAX_DB, FADER_OFF_DB};
    use crate::test_support::{assert_float_eq, assert_float_ne};
    use crossterm::event::KeyEventState;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn arrows_walk_strips_into_master_and_back() {
        let shared = SharedState::new();
        let mut desk = Desk::new();
        for _ in 0..DESK_CHANNELS {
            desk.handle_key(key(KeyCode::Right), &shared);
        }
        assert_eq!(
            desk.focus,
            Focus::Master {
                control: MasterControl::Fader
            }
        );
        desk.handle_key(key(KeyCode::Right), &shared);
        assert!(matches!(desk.focus, Focus::Master { .. }));
        desk.handle_key(key(KeyCode::Left), &shared);
        assert_eq!(
            desk.focus,
            Focus::Strip {
                slot: DESK_CHANNELS - 1,
                control: StripControl::Fader
            }
        );
        for _ in 0..20 {
            desk.handle_key(key(KeyCode::Char('h')), &shared);
        }
        assert!(matches!(desk.focus, Focus::Strip { slot: 0, .. }));
    }

    #[test]
    fn vertical_keys_cycle_controls() {
        let shared = SharedState::new();
        let mut desk = Desk::new();
        desk.handle_key(key(KeyCode::Down), &shared);
        assert_eq!(
            desk.focus,
            Focus::Strip {
                slot: 0,
                control: StripControl::Trim
            }
        );
        desk.handle_key(key(KeyCode::Char('k')), &shared);
        assert_eq!(
            desk.focus,
            Focus::Strip {
                slot: 0,
                control: StripControl::Fader
            }
        );
    }

    #[test]
    fn plus_minus_and_reset_edit_the_focused_control() {
        let shared = SharedState::new();
        let mut desk = Desk::new();
        desk.handle_key(key(KeyCode::Char('-')), &shared);
        assert!(shared.channel_controls(0).fader_db < 0.0);
        desk.handle_key(key(KeyCode::Char('0')), &shared);
        assert_float_eq(shared.channel_controls(0).fader_db, 0.0);
        desk.handle_key(key(KeyCode::PageUp), &shared);
        assert!(shared.channel_controls(0).fader_db > 0.0);
        // Other strips untouched.
        assert_eq!(shared.channel_controls(1), ChannelControls::DEFAULT);
    }

    #[test]
    fn mute_solo_and_strip_reset() {
        let shared = SharedState::new();
        let mut desk = Desk::new();
        desk.handle_key(key(KeyCode::Char('3')), &shared);
        desk.handle_key(key(KeyCode::Char('m')), &shared);
        desk.handle_key(key(KeyCode::Char('s')), &shared);
        let controls = shared.channel_controls(2);
        assert!(controls.muted && controls.soloed);
        desk.handle_key(key(KeyCode::Char('R')), &shared);
        assert_eq!(shared.channel_controls(2), ChannelControls::DEFAULT);
    }

    #[test]
    fn master_section_is_editable() {
        let shared = SharedState::new();
        let mut desk = Desk::new();
        desk.handle_key(key(KeyCode::Char('9')), &shared);
        desk.handle_key(key(KeyCode::Char('{')), &shared);
        assert!(shared.master_controls().fader_db < 0.0);
        desk.handle_key(key(KeyCode::Up), &shared);
        desk.handle_key(key(KeyCode::Char('+')), &shared);
        assert!(shared.master_controls().aux_return > MasterControls::DEFAULT.aux_return);
    }

    #[test]
    fn quit_keys_and_help() {
        let shared = SharedState::new();
        let mut desk = Desk::new();
        desk.handle_key(key(KeyCode::Char('?')), &shared);
        assert!(desk.help_open);
        desk.handle_key(key(KeyCode::Esc), &shared);
        assert!(!desk.help_open && !desk.should_quit);
        desk.handle_key(key(KeyCode::Char('q')), &shared);
        assert!(desk.should_quit);

        let mut desk = Desk::new();
        desk.handle_key(
            KeyEvent {
                modifiers: KeyModifiers::CONTROL,
                ..key(KeyCode::Char('c'))
            },
            &shared,
        );
        assert!(desk.should_quit);
    }

    #[test]
    fn key_release_events_are_ignored() {
        let shared = SharedState::new();
        let mut desk = Desk::new();
        desk.handle_key(
            KeyEvent {
                kind: KeyEventKind::Release,
                ..key(KeyCode::Char('q'))
            },
            &shared,
        );
        assert!(!desk.should_quit);
    }

    #[test]
    fn bank_follows_focus() {
        let mut desk = Desk::new();
        desk.focus = Focus::Strip {
            slot: 6,
            control: StripControl::Fader,
        };
        desk.scroll_to_focus(3);
        assert_eq!(desk.first_visible, 4);
        desk.focus = Focus::Strip {
            slot: 1,
            control: StripControl::Fader,
        };
        desk.scroll_to_focus(3);
        assert_eq!(desk.first_visible, 1);
        desk.scroll_to_focus(DESK_CHANNELS + 5);
        assert_eq!(desk.first_visible, 0);
    }

    #[test]
    fn throw_maps_rows_to_positions_both_ways() {
        let throw = Throw { top: 10, rows: 11 };
        assert_float_eq(throw.position_at(10), 1.0);
        assert_float_eq(throw.position_at(20), 0.0);
        assert_float_eq(throw.position_at(99), 0.0);
        assert_float_eq(throw.position_at(0), 1.0);
        assert_eq!(throw.row_at(0.5), 15);
        assert_eq!(throw.row_at(f32::NAN), 20);
        let flat = Throw { top: 3, rows: 1 };
        assert_float_eq(flat.position_at(3), 1.0);
        assert_eq!(flat.row_at(0.2), 3);
    }

    #[test]
    fn mouse_click_and_drag_move_a_fader() {
        let shared = SharedState::new();
        let mut desk = Desk::new();
        let throw = Throw { top: 5, rows: 11 };
        desk.hits.push(Hit {
            area: Rect::new(2, 5, 3, 11),
            target: HitTarget::Strip {
                slot: 4,
                control: StripControl::Fader,
            },
            throw: Some(throw),
        });
        // Clicking the throw away from the cap jumps the fader there.
        desk.handle_mouse(
            mouse(MouseEventKind::Down(MouseButton::Left), 3, 5),
            &shared,
        );
        assert_float_eq(shared.channel_controls(4).fader_db, FADER_MAX_DB);
        // Dragging is relative and keeps working off the fader.
        desk.handle_mouse(
            mouse(MouseEventKind::Drag(MouseButton::Left), 40, 30),
            &shared,
        );
        assert_float_eq(shared.channel_controls(4).fader_db, FADER_OFF_DB);
        desk.handle_mouse(
            mouse(MouseEventKind::Up(MouseButton::Left), 40, 30),
            &shared,
        );
        desk.handle_mouse(
            mouse(MouseEventKind::Drag(MouseButton::Left), 3, 5),
            &shared,
        );
        assert_float_eq(shared.channel_controls(4).fader_db, FADER_OFF_DB);
        assert_eq!(
            desk.focus,
            Focus::Strip {
                slot: 4,
                control: StripControl::Fader
            }
        );
    }

    #[test]
    fn mouse_scroll_turns_knobs_and_buttons_toggle() {
        let shared = SharedState::new();
        let mut desk = Desk::new();
        desk.hits.push(Hit {
            area: Rect::new(0, 0, 9, 1),
            target: HitTarget::Strip {
                slot: 1,
                control: StripControl::EqHigh,
            },
            throw: None,
        });
        desk.hits.push(Hit {
            area: Rect::new(0, 1, 3, 1),
            target: HitTarget::Mute { slot: 1 },
            throw: None,
        });
        desk.handle_mouse(mouse(MouseEventKind::ScrollUp, 4, 0), &shared);
        assert!(shared.channel_controls(1).eq.high_db > 0.0);
        desk.handle_mouse(
            mouse(MouseEventKind::Down(MouseButton::Left), 1, 1),
            &shared,
        );
        assert!(shared.channel_controls(1).muted);
        // Clicking empty space does nothing.
        desk.handle_mouse(
            mouse(MouseEventKind::Down(MouseButton::Left), 50, 50),
            &shared,
        );
        assert!(shared.channel_controls(1).muted);
    }

    #[test]
    fn clip_lights_clear_from_key_and_click() {
        let shared = SharedState::new();
        let mut desk = Desk::new();
        let clipped = crate::engine::ChannelSnapshot {
            clipped: true,
            ..crate::engine::ChannelSnapshot::EMPTY
        };
        shared.publish_channel(0, &clipped, 64);
        desk.handle_key(key(KeyCode::Char('x')), &shared);
        assert!(!shared.channel_readout(0).clip);

        shared.publish_channel(0, &clipped, 64);
        desk.hits.push(Hit {
            area: Rect::new(8, 2, 1, 1),
            target: HitTarget::ClipLights,
            throw: None,
        });
        desk.handle_mouse(
            mouse(MouseEventKind::Down(MouseButton::Left), 8, 2),
            &shared,
        );
        assert!(!shared.channel_readout(0).clip);
    }

    #[test]
    fn tick_drives_meters_from_shared_readouts() {
        let shared = SharedState::new();
        let mut desk = Desk::new();
        let loud = crate::engine::ChannelSnapshot {
            peak: crate::engine::StereoLevel {
                left: 0.5,
                right: 0.5,
            },
            rms: crate::engine::StereoLevel {
                left: 0.3,
                right: 0.3,
            },
            ..crate::engine::ChannelSnapshot::EMPTY
        };
        for _ in 0..30 {
            // The callback keeps publishing between desk frames.
            shared.publish_channel(2, &loud, 1_600);
            desk.tick(&shared, 0.033);
        }
        assert!(desk.strip_meters[2].peak_left.level_db() > -7.0);
        assert!(desk.strip_meters[2].vu_left.vu() > 5.0);
        assert_float_eq(desk.strip_meters[0].peak_left.level_db(), f32::NEG_INFINITY);
    }

    #[test]
    fn grabbing_the_cap_does_not_move_the_fader() {
        let shared = SharedState::new();
        let mut desk = Desk::new();
        let throw = Throw { top: 5, rows: 19 };
        desk.hits.push(Hit {
            area: Rect::new(0, 5, 10, 19),
            target: HitTarget::Strip {
                slot: 0,
                control: StripControl::Fader,
            },
            throw: Some(throw),
        });
        // Unity sits between rows; the cap is drawn on the rounded row.
        let cap_row = throw.row_at(shared.channel_controls(0).normalized(StripControl::Fader));
        assert_float_ne(
            throw.position_at(cap_row),
            shared.channel_controls(0).normalized(StripControl::Fader),
        );
        desk.handle_mouse(
            mouse(MouseEventKind::Down(MouseButton::Left), 4, cap_row),
            &shared,
        );
        assert_float_eq(shared.channel_controls(0).fader_db, 0.0);
        // Moving away and back returns exactly to unity.
        desk.handle_mouse(
            mouse(MouseEventKind::Drag(MouseButton::Left), 4, cap_row + 3),
            &shared,
        );
        assert!(shared.channel_controls(0).fader_db < 0.0);
        desk.handle_mouse(
            mouse(MouseEventKind::Drag(MouseButton::Left), 4, cap_row),
            &shared,
        );
        assert!(shared.channel_controls(0).fader_db.abs() < 1e-4);
    }

    #[test]
    fn clicks_and_scrolls_over_help_never_reach_the_desk() {
        let shared = SharedState::new();
        let mut desk = Desk::new();
        desk.hits.push(Hit {
            area: Rect::new(0, 0, 9, 1),
            target: HitTarget::Mute { slot: 0 },
            throw: None,
        });
        desk.hits.push(Hit {
            area: Rect::new(0, 1, 9, 1),
            target: HitTarget::Strip {
                slot: 0,
                control: StripControl::Trim,
            },
            throw: None,
        });
        desk.help_open = true;
        desk.handle_mouse(mouse(MouseEventKind::ScrollUp, 2, 1), &shared);
        assert!(desk.help_open);
        desk.handle_mouse(
            mouse(MouseEventKind::Down(MouseButton::Left), 2, 0),
            &shared,
        );
        assert!(!desk.help_open);
        assert_eq!(shared.channel_controls(0), ChannelControls::DEFAULT);
    }

    #[test]
    fn number_keys_follow_the_channel_count() {
        let shared = SharedState::new();
        let mut desk = Desk::new();
        desk.handle_key(key(KeyCode::Char('1')), &shared);
        assert!(matches!(desk.focus, Focus::Strip { slot: 0, .. }));
        let last = char::from_digit(u32::try_from(DESK_CHANNELS).unwrap(), 10).unwrap();
        desk.handle_key(key(KeyCode::Char(last)), &shared);
        assert!(matches!(desk.focus, Focus::Strip { slot, .. } if slot == DESK_CHANNELS - 1));
        let master = char::from_digit(u32::try_from(DESK_CHANNELS + 1).unwrap(), 10).unwrap();
        desk.handle_key(key(KeyCode::Char(master)), &shared);
        assert!(matches!(desk.focus, Focus::Master { .. }));
        desk.handle_key(key(KeyCode::Char('0')), &shared);
        assert!(
            matches!(desk.focus, Focus::Master { .. }),
            "0 resets, it does not jump"
        );
    }

    #[test]
    fn bank_arrows_page_and_move_focus() {
        let shared = SharedState::new();
        let mut desk = Desk::new();
        desk.scroll_to_focus(3);
        desk.hits.push(Hit {
            area: Rect::new(0, 0, 1, 1),
            target: HitTarget::BankNext,
            throw: None,
        });
        desk.hits.push(Hit {
            area: Rect::new(2, 0, 1, 1),
            target: HitTarget::BankPrevious,
            throw: None,
        });
        desk.handle_mouse(
            mouse(MouseEventKind::Down(MouseButton::Left), 0, 0),
            &shared,
        );
        assert_eq!(desk.first_visible, 3);
        assert!(matches!(desk.focus, Focus::Strip { slot: 3, .. }));
        for _ in 0..5 {
            desk.handle_mouse(
                mouse(MouseEventKind::Down(MouseButton::Left), 0, 0),
                &shared,
            );
        }
        assert_eq!(desk.first_visible, DESK_CHANNELS - 3);
        desk.handle_mouse(
            mouse(MouseEventKind::Down(MouseButton::Left), 2, 0),
            &shared,
        );
        assert_eq!(desk.first_visible, DESK_CHANNELS - 6);
    }

    #[test]
    fn strip_health_shows_recent_trouble_then_clears() {
        let shared = SharedState::new();
        let mut desk = Desk::new();
        let troubled = crate::engine::ChannelSnapshot {
            connected: true,
            underruns: 1,
            ..crate::engine::ChannelSnapshot::EMPTY
        };
        shared.publish_channel(0, &troubled, 64);
        desk.tick(&shared, 0.03);
        assert!(desk.strip_health[0].troubled());
        for _ in 0..200 {
            desk.tick(&shared, 0.03);
        }
        assert!(!desk.strip_health[0].troubled());
        // A new underrun lights it again.
        shared.publish_channel(
            0,
            &crate::engine::ChannelSnapshot {
                underruns: 2,
                ..troubled
            },
            64,
        );
        desk.tick(&shared, 0.03);
        assert!(desk.strip_health[0].troubled());
    }

    #[test]
    fn tick_drains_the_shared_meters() {
        let shared = SharedState::new();
        let mut desk = Desk::new();
        let loud = crate::engine::ChannelSnapshot {
            peak: crate::engine::StereoLevel {
                left: 0.5,
                right: 0.5,
            },
            ..crate::engine::ChannelSnapshot::EMPTY
        };
        shared.publish_channel(1, &loud, 64);
        shared.publish_master(loud.peak, loud.rms, 64, false);
        desk.tick(&shared, 0.03);
        assert!(desk.strip_meters[1].peak_left.level_db() > -7.0);
        assert!(desk.master_meters.peak_left.level_db() > -7.0);
        // Everything was taken: nothing is left for a second look.
        assert_eq!(shared.take_channel_meters(1).frames, 0);
        assert_eq!(shared.take_master_meters().frames, 0);
    }

    fn fader_hit(throw: Throw) -> Hit {
        Hit {
            area: Rect::new(0, throw.top, 10, throw.rows),
            target: HitTarget::Strip {
                slot: 0,
                control: StripControl::Fader,
            },
            throw: Some(throw),
        }
    }

    #[test]
    fn a_lost_release_never_leaves_a_stale_drag() {
        let shared = SharedState::new();
        let mut desk = Desk::new();
        let throw = Throw { top: 5, rows: 19 };
        desk.hits.push(fader_hit(throw));
        desk.handle_mouse(
            mouse(MouseEventKind::Down(MouseButton::Left), 4, 10),
            &shared,
        );
        // Help opens mid-drag and the release lands on the overlay.
        desk.handle_key(key(KeyCode::Char('?')), &shared);
        desk.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 4, 10), &shared);
        desk.handle_key(key(KeyCode::Char('?')), &shared);
        let before = shared.channel_controls(0);
        desk.handle_mouse(
            mouse(MouseEventKind::Drag(MouseButton::Left), 4, 20),
            &shared,
        );
        assert_eq!(shared.channel_controls(0), before);

        // A press that misses every control also ends any earlier grab.
        desk.handle_mouse(
            mouse(MouseEventKind::Down(MouseButton::Left), 4, 10),
            &shared,
        );
        desk.handle_mouse(
            mouse(MouseEventKind::Down(MouseButton::Left), 60, 60),
            &shared,
        );
        let before = shared.channel_controls(0);
        desk.handle_mouse(
            mouse(MouseEventKind::Drag(MouseButton::Left), 4, 20),
            &shared,
        );
        assert_eq!(shared.channel_controls(0), before);

        // A right-button release ends a grab too.
        desk.handle_mouse(
            mouse(MouseEventKind::Down(MouseButton::Left), 4, 10),
            &shared,
        );
        desk.handle_mouse(
            mouse(MouseEventKind::Up(MouseButton::Right), 4, 10),
            &shared,
        );
        let before = shared.channel_controls(0);
        desk.handle_mouse(
            mouse(MouseEventKind::Drag(MouseButton::Left), 4, 20),
            &shared,
        );
        assert_eq!(shared.channel_controls(0), before);
    }

    #[test]
    fn dragging_to_either_end_of_the_throw_pins_the_fader_there() {
        let shared = SharedState::new();
        let mut desk = Desk::new();
        let throw = Throw { top: 5, rows: 19 };
        desk.hits.push(fader_hit(throw));
        // Grab the cap at unity, then drag past the top of the throw.
        let cap_row = throw.row_at(shared.channel_controls(0).normalized(StripControl::Fader));
        desk.handle_mouse(
            mouse(MouseEventKind::Down(MouseButton::Left), 4, cap_row),
            &shared,
        );
        desk.handle_mouse(
            mouse(MouseEventKind::Drag(MouseButton::Left), 4, 5),
            &shared,
        );
        assert_float_eq(
            shared.channel_controls(0).normalized(StripControl::Fader),
            1.0,
        );
        desk.handle_mouse(
            mouse(MouseEventKind::Drag(MouseButton::Left), 4, 0),
            &shared,
        );
        assert_float_eq(
            shared.channel_controls(0).normalized(StripControl::Fader),
            1.0,
        );
        // And to the bottom row: fully off, as drawn.
        desk.handle_mouse(
            mouse(MouseEventKind::Drag(MouseButton::Left), 4, 23),
            &shared,
        );
        assert_float_eq(
            shared.channel_controls(0).normalized(StripControl::Fader),
            0.0,
        );
        desk.handle_mouse(
            mouse(MouseEventKind::Drag(MouseButton::Left), 4, 40),
            &shared,
        );
        assert_float_eq(
            shared.channel_controls(0).normalized(StripControl::Fader),
            0.0,
        );
        // In between, movement stays relative to the grab.
        desk.handle_mouse(
            mouse(MouseEventKind::Drag(MouseButton::Left), 4, cap_row),
            &shared,
        );
        assert!(shared.channel_controls(0).fader_db.abs() < 1e-4);
    }

    /// A 4096-frame device buffer at 48 kHz arrives about every third desk
    /// tick. The needles must read the same as with a callback every tick.
    #[test]
    fn sparse_callbacks_read_the_same_as_dense_ones() {
        let zero_vu = 10.0_f32.powf(crate::meters::VU_REFERENCE_DBFS / 20.0);
        let steady = crate::engine::ChannelSnapshot {
            peak: StereoLevel {
                left: zero_vu * 1.4,
                right: zero_vu * 1.4,
            },
            rms: StereoLevel {
                left: zero_vu,
                right: zero_vu,
            },
            ..crate::engine::ChannelSnapshot::EMPTY
        };
        for every in [1, 3] {
            let shared = SharedState::new();
            let mut desk = Desk::new();
            let mut lowest = f32::INFINITY;
            let mut highest = f32::NEG_INFINITY;
            for tick in 0..300 {
                if tick % every == 0 {
                    shared.publish_channel(0, &steady, 1_600 * every);
                }
                desk.tick(&shared, 0.033);
                if tick >= 100 {
                    let vu = desk.strip_meters[0].vu_left.vu();
                    lowest = lowest.min(vu);
                    highest = highest.max(vu);
                }
            }
            assert!(
                lowest > -0.5 && highest < 0.5,
                "every {every}: {lowest}..{highest}"
            );
        }
    }

    #[test]
    fn meters_fall_when_the_stream_stops() {
        let shared = SharedState::new();
        let mut desk = Desk::new();
        let loud = crate::engine::ChannelSnapshot {
            peak: StereoLevel {
                left: 0.5,
                right: 0.5,
            },
            rms: StereoLevel {
                left: 0.3,
                right: 0.3,
            },
            ..crate::engine::ChannelSnapshot::EMPTY
        };
        for _ in 0..30 {
            shared.publish_channel(0, &loud, 1_600);
            desk.tick(&shared, 0.033);
        }
        let playing = desk.strip_meters[0].vu_left.vu();
        let peak_playing = desk.strip_meters[0].peak_left.level_db();
        // Nothing arrives: the meters hold while a buffer may still be due,
        // then fall smoothly every tick at the peak fall rate, starting with
        // the very first falling tick.
        let mut ticks = 0;
        loop {
            desk.tick(&shared, 0.033);
            ticks += 1;
            if desk.strip_meters[0].peak_left.level_db() < peak_playing {
                break;
            }
            assert!(ticks < 100, "meters never fell");
        }
        // Audio came every tick, so the stop is noticed after the minimum
        // wait, not the maximum.
        let stall_ticks = (METER_STALL_MIN_SECONDS / 0.033).ceil() as i32;
        assert_eq!(ticks, stall_ticks);
        let mut previous = peak_playing;
        for _ in 0..30 {
            let now = desk.strip_meters[0].peak_left.level_db();
            let fell = previous - now;
            assert!(
                24.0_f32.mul_add(-0.033, fell).abs() < 1e-3,
                "fell {fell} dB in a tick"
            );
            previous = now;
            desk.tick(&shared, 0.033);
        }
        assert!(desk.strip_meters[0].vu_left.vu() < playing - 20.0);
        // Audio returns: the meters follow it at once.
        shared.publish_channel(0, &loud, 1_600);
        desk.tick(&shared, 0.033);
        assert!((desk.strip_meters[0].peak_left.level_db() - peak_playing).abs() < 1e-4);
    }

    #[test]
    fn a_drag_is_released_when_its_fader_moves_on_screen() {
        let shared = SharedState::new();
        let mut desk = Desk::new();
        let throw = Throw { top: 5, rows: 19 };
        desk.hits.push(fader_hit(throw));
        desk.handle_mouse(
            mouse(MouseEventKind::Down(MouseButton::Left), 4, 10),
            &shared,
        );
        // Redrawn in the same place: the drag survives.
        desk.release_drag_if_moved();
        let grabbed = shared.channel_controls(0);
        desk.handle_mouse(
            mouse(MouseEventKind::Drag(MouseButton::Left), 4, 12),
            &shared,
        );
        assert_ne!(shared.channel_controls(0), grabbed);

        // The terminal shrinks: the throw is shorter now, so the grab ends.
        desk.hits.clear();
        desk.hits.push(fader_hit(Throw { top: 5, rows: 9 }));
        desk.release_drag_if_moved();
        let before = shared.channel_controls(0);
        desk.handle_mouse(
            mouse(MouseEventKind::Drag(MouseButton::Left), 4, 6),
            &shared,
        );
        assert_eq!(shared.channel_controls(0), before);

        // The bank moves by one: the strip slides sideways, same throw.
        desk.hits.clear();
        desk.hits.push(fader_hit(throw));
        desk.handle_mouse(
            mouse(MouseEventKind::Down(MouseButton::Left), 4, 8),
            &shared,
        );
        desk.hits.clear();
        desk.hits.push(Hit {
            area: Rect::new(12, throw.top, 10, throw.rows),
            ..fader_hit(throw)
        });
        desk.release_drag_if_moved();
        let before = shared.channel_controls(0);
        desk.handle_mouse(
            mouse(MouseEventKind::Drag(MouseButton::Left), 4, 6),
            &shared,
        );
        assert_eq!(shared.channel_controls(0), before);

        // The bank moves further: the dragged strip is no longer drawn.
        desk.handle_mouse(
            mouse(MouseEventKind::Down(MouseButton::Left), 4, 8),
            &shared,
        );
        desk.hits.clear();
        desk.release_drag_if_moved();
        let before = shared.channel_controls(0);
        desk.handle_mouse(
            mouse(MouseEventKind::Drag(MouseButton::Left), 4, 6),
            &shared,
        );
        assert_eq!(shared.channel_controls(0), before);
    }

    #[test]
    fn any_key_closes_help_including_ctrl_combinations() {
        let shared = SharedState::new();
        let mut desk = Desk::new();
        desk.help_open = true;
        desk.handle_key(
            KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL),
            &shared,
        );
        assert!(!desk.help_open && !desk.should_quit);
        // Without help, an unbound Ctrl key does nothing at all.
        let focus = desk.focus;
        desk.handle_key(
            KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL),
            &shared,
        );
        assert_eq!(desk.focus, focus);
        assert!(!desk.help_open && !desk.should_quit);
        // Ctrl-Q quits even from help.
        desk.help_open = true;
        desk.handle_key(
            KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL),
            &shared,
        );
        assert!(desk.should_quit);
    }

    #[test]
    fn transport_keys_drive_the_studio() {
        let shared = SharedState::new();
        let mut desk = Desk::new();
        desk.handle_key(key(KeyCode::Char(' ')), &shared);
        assert!(shared.transport().playing);
        desk.handle_key(key(KeyCode::Char(' ')), &shared);
        assert!(!shared.transport().playing);

        let bpm = shared.transport().bpm;
        desk.handle_key(key(KeyCode::Char(']')), &shared);
        assert_float_eq(shared.transport().bpm, bpm + 1.0);
        desk.handle_key(key(KeyCode::Char('[')), &shared);
        desk.handle_key(key(KeyCode::Char('[')), &shared);
        assert_float_eq(shared.transport().bpm, bpm - 1.0);

        assert!(!shared.click());
        desk.handle_key(key(KeyCode::Char('c')), &shared);
        assert!(shared.click());
    }
}
