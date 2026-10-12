//! What every kazoo terminal program shares.
//!
//! [`reports`] installs error reports once per process, so any number of
//! instruments can run under one binary. [`terminal`] enters raw mode and
//! the alternate screen, and puts the terminal back on every way out:
//! returning, an error, or a panic.

pub mod reports;
pub mod terminal;
