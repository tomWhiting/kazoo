mod controls;
pub mod engine;
mod health;
mod run;
pub mod score;
pub mod synth;
mod ui;

pub use run::{Args, USAGE, cli, parse_args, parse_quantise, run};

#[cfg(test)]
#[global_allocator]
static ALLOCATOR: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;
