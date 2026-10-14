use std::sync::atomic::Ordering;

use std::time::Duration;

use super::{Engine, Shared, thread_cpu};

#[derive(Debug, Default)]
pub(super) struct ModuleSpan {
    pub(super) started: Option<Duration>,
    pub(super) left: usize,
    #[cfg(test)]
    pub(super) reads: usize,
}

impl Engine {
    pub(super) fn begin_module_span(&mut self, modules: usize) {
        if modules > 0 && self.module_span.started.is_none() {
            self.module_span.started = Some(thread_cpu());
            #[cfg(test)]
            {
                self.module_span.reads += 1;
            }
        }
    }

    pub(super) fn end_module_span(&mut self) {
        // The last sub-block can end early for a wall swap. Count frames
        // still owed by the callback, rather than assuming full blocks.
        if self.module_span.left <= self.carry_len {
            if let Some(started) = self.module_span.started.take() {
                self.module_cpu = thread_cpu().saturating_sub(started);
                #[cfg(test)]
                {
                    self.module_span.reads += 1;
                }
            }
        }
    }

    pub(super) fn measure_fixed(&mut self, elapsed: Duration, lasted: f32, weight: f32) {
        // One span encloses the callback's first through last module loops.
        // Subtract it from the same CPU clock's total to count fixed work once.
        let fixed = elapsed.saturating_sub(self.module_cpu).as_secs_f32() / lasted;
        let averaged = weight.mul_add(fixed - self.fixed_load, self.fixed_load);
        if averaged.is_finite() {
            self.fixed_load = averaged;
            self.shared
                .fixed
                .store(averaged.to_bits(), Ordering::Relaxed);
        }
    }
}

impl Shared {
    pub(crate) fn fixed_load(&self) -> f32 {
        let fixed = f32::from_bits(self.fixed.load(Ordering::Relaxed));
        if fixed.is_finite() {
            fixed.max(0.0)
        } else {
            0.0
        }
    }

    #[cfg(test)]
    pub(crate) fn set_fixed_load(&self, fixed: f32) {
        self.fixed.store(fixed.to_bits(), Ordering::Relaxed);
    }
}
