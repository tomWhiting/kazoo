//! The engine's per-slot state, and how it grows.
//!
//! A [`Room`] is a table of pages, each holding [`SLOTS_PER_PAGE`] slots:
//! for every slot, the module, its tag and kind, its knobs and their
//! glides, its output buffers, and the atomics its knob values are
//! published through. The engine starts with room for a modest wall. When
//! the control side needs a slot past the end, it builds a larger page
//! table with fresh pages past the engine's own (every allocation happens
//! there) and sends it over the command ring. The engine moves its pages
//! into it ([`Room::take_from`]): one pointer per page, so the audio
//! thread's share of growing stays a few hundred pointer moves however
//! large the wall, and nothing is allocated, freed or copied. The emptied
//! old table goes back over the retire ring to be freed off the audio
//! thread. The control side grows the table by doubling.
//!
//! A page's knob cells are made with the page and never move, so the
//! control side reads a slot's knobs from the same cells before and after
//! any growth.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use super::lines::Line;
use super::{Glide, slice_of};
use crate::catalogue::Kind;
use crate::dsp::{Block, Module, Sweep};
use crate::{MAX_JACKS, MAX_KNOBS, MAX_OUTPUTS, SUB_BLOCK};

/// Slots in a page: the unit the room grows by.
pub const SLOTS_PER_PAGE: usize = 64;

/// Pages holding `slots` slots, rounded up.
#[must_use]
pub const fn pages_for(slots: usize) -> usize {
    slots.div_ceil(SLOTS_PER_PAGE)
}

/// One page's knob values, published for the control side: by slot within
/// the page, then knob. A cell holds NaN until the engine publishes to it.
pub type KnobCells = Box<[[AtomicU32; MAX_KNOBS]]>;

/// Per-slot render nanoseconds, NaN until the slot has been timed.
pub type CostCells = Box<[AtomicU32]>;

/// [`SLOTS_PER_PAGE`] slots' state, with one optional line per jack.
pub struct Page {
    pub(super) lines: Box<[Option<Box<Line>>]>,
    pub(super) modules: Box<[Option<Box<dyn Module>>]>,
    pub(super) tags: Box<[u32]>,
    pub(super) kinds: Box<[Option<Kind>]>,
    pub(super) glides: Box<[[Glide; MAX_KNOBS]]>,
    pub(super) gliding: Box<[u32]>,
    pub(super) knobs: Box<[[f32; MAX_KNOBS]]>,
    pub(super) outputs: Box<[[Block; MAX_OUTPUTS]]>,
    /// Shared with the control side, which reads the values the engine
    /// publishes here.
    pub(super) published: Arc<KnobCells>,
    pub(super) costs: Arc<CostCells>,
}

impl std::fmt::Debug for Page {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Page")
            .field(
                "modules",
                &self
                    .modules
                    .iter()
                    .filter(|module| module.is_some())
                    .count(),
            )
            .finish_non_exhaustive()
    }
}

impl Page {
    /// An empty page, or `None` when there is no memory for it (control
    /// side only).
    fn new() -> Option<Self> {
        let cells: KnobCells = slice_of(SLOTS_PER_PAGE, || {
            std::array::from_fn(|_| AtomicU32::new(f32::NAN.to_bits()))
        })?;
        Some(Self {
            modules: slice_of(SLOTS_PER_PAGE, || None)?,
            lines: slice_of(SLOTS_PER_PAGE * MAX_JACKS, || None)?,
            tags: slice_of(SLOTS_PER_PAGE, || 0)?,
            kinds: slice_of(SLOTS_PER_PAGE, || None)?,
            glides: slice_of(SLOTS_PER_PAGE, || [Glide::at(0.0, 0.0); MAX_KNOBS])?,
            gliding: slice_of(SLOTS_PER_PAGE, || 0)?,
            knobs: slice_of(SLOTS_PER_PAGE, || [0.0; MAX_KNOBS])?,
            outputs: slice_of(SLOTS_PER_PAGE, || [[0.0; SUB_BLOCK]; MAX_OUTPUTS])?,
            published: Arc::new(cells),
            costs: Arc::new(slice_of(SLOTS_PER_PAGE, || {
                AtomicU32::new(f32::NAN.to_bits())
            })?),
        })
    }

    /// Publish the knob `knob` of the slot `at` in this page as `value`.
    pub(super) fn publish(&self, at: usize, knob: usize, value: f32) {
        if let Some(cell) = self.published.get(at).and_then(|knobs| knobs.get(knob)) {
            cell.store(value.to_bits(), Ordering::Relaxed);
        }
    }

    /// Follow real cost increases slowly and discard preempted timings quickly.
    /// A timing of nothing (both clock reads in one tick of the clock, about
    /// 42 ns on Apple silicon) measured nothing and is ignored: held, it
    /// would pin the cost at zero for good, a rise being clamped to
    /// [`super::COST_CLAMP`] times what is held.
    pub(super) fn note_cost(&self, at: usize, ns: f32) {
        if !ns.is_finite() || ns <= 0.0 {
            return;
        }
        let cell = &self.costs[at];
        let held = f32::from_bits(cell.load(Ordering::Relaxed));
        let value = if held.is_nan() {
            ns
        } else if ns < held {
            super::COST_FALL.mul_add(ns - held, held)
        } else {
            super::COST_RISE.mul_add(ns.min(super::COST_CLAMP * held) - held, held)
        };
        cell.store(value.to_bits(), Ordering::Relaxed);
    }

    pub(super) fn advance_glides(
        &mut self,
        at: usize,
        kind: Kind,
        sweeps: &mut [Sweep; MAX_KNOBS],
    ) -> u32 {
        let moving = self.gliding[at];
        let mut mask = moving;
        while mask != 0 {
            let knob = mask.trailing_zeros() as usize;
            let glide = &mut self.glides[at][knob];
            sweeps[knob] = glide.advance(kind.spec().knobs.get(knob));
            if sweeps[knob].frames > 0 {
                self.knobs[at][knob] = glide.value;
                self.published[at][knob].store(glide.value.to_bits(), Ordering::Relaxed);
            }
            if glide.remaining == 0 {
                self.gliding[at] &= !(1 << knob);
            }
            mask &= mask - 1;
        }
        moving
    }
}

pub(super) const fn rest_sweeps(mut mask: u32, sweeps: &mut [Sweep; MAX_KNOBS]) {
    while mask != 0 {
        sweeps[mask.trailing_zeros() as usize] = Sweep::REST;
        mask &= mask - 1;
    }
}

/// Every slot's state, in pages.
#[derive(Debug, Default)]
pub struct Room {
    pages: Box<[Option<Box<Page>>]>,
}

impl Room {
    /// A page table with room for at least `want` slots, in whole pages,
    /// for an engine holding `have`: the places of the engine's own pages
    /// are left empty for them, and every place past them gets a fresh
    /// page. `None` when there is no memory for it (this allocates: build
    /// rooms on the control side).
    #[must_use]
    pub fn new(have: usize, want: usize) -> Option<Self> {
        let mut pages = slice_of(pages_for(want), || None)?;
        for place in pages.iter_mut().skip(pages_for(have)) {
            *place = Some(Box::new(Page::new()?));
        }
        Some(Self { pages })
    }

    /// Slots the room has places for.
    #[must_use]
    pub fn len(&self) -> usize {
        self.pages.len() * SLOTS_PER_PAGE
    }

    /// Whether the room has no places.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pages.is_empty()
    }

    /// Whether every page place is empty (as an outgrown room is, once the
    /// engine has moved its pages out).
    #[must_use]
    pub fn is_emptied(&self) -> bool {
        self.pages.iter().all(Option::is_none)
    }

    /// The knob cells of the pages this room holds, by page number (this
    /// allocates: control side only).
    #[must_use]
    pub fn published(&self) -> Vec<(usize, Arc<KnobCells>)> {
        self.pages
            .iter()
            .enumerate()
            .filter_map(|(number, page)| {
                page.as_ref()
                    .map(|page| (number, Arc::clone(&page.published)))
            })
            .collect()
    }

    /// Cost cells of this room's pages, by page number; control side only.
    #[must_use]
    pub fn costs(&self) -> Vec<(usize, Arc<CostCells>)> {
        self.pages
            .iter()
            .enumerate()
            .filter_map(|(number, page)| {
                page.as_ref().map(|page| (number, Arc::clone(&page.costs)))
            })
            .collect()
    }

    /// Move every page in `old` into the same place here; `old` is left
    /// with none. Anything this room held in those places goes to `old`,
    /// to be freed with it. Real-time safe: one pointer moves per page, and
    /// nothing is allocated, freed or copied.
    pub fn take_from(&mut self, old: &mut Self) {
        for (mine, theirs) in self.pages.iter_mut().zip(old.pages.iter_mut()) {
            std::mem::swap(mine, theirs);
        }
    }

    /// The page holding `slot`, and the slot's place in it.
    pub(super) fn page(&self, slot: usize) -> Option<(&Page, usize)> {
        let page = self.pages.get(slot / SLOTS_PER_PAGE)?.as_deref()?;
        Some((page, slot % SLOTS_PER_PAGE))
    }

    /// The page holding `slot`, to change, and the slot's place in it.
    pub(super) fn page_mut(&mut self, slot: usize) -> Option<(&mut Page, usize)> {
        let page = self.pages.get_mut(slot / SLOTS_PER_PAGE)?.as_deref_mut()?;
        Some((page, slot % SLOTS_PER_PAGE))
    }

    /// Whether `slot` holds a module.
    #[cfg(test)]
    pub(super) fn holds(&self, slot: usize) -> bool {
        self.page(slot)
            .is_some_and(|(page, at)| page.modules.get(at).is_some_and(Option::is_some))
    }

    /// The output `port` of `slot`.
    pub(super) fn output(&self, slot: usize, port: usize) -> Option<&Block> {
        let (page, at) = self.page(slot)?;
        page.outputs.get(at)?.get(port)
    }
}
