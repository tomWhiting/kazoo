//! Shared test fixtures.
//!
//! Nothing here touches audio hardware: the engine handle is a detached one
//! backed by an in-memory command channel with no audio threads, and apps
//! are constructed with an injected, empty device list.

use std::ops::{Deref, DerefMut};

use crossbeam_channel::{Receiver, unbounded};
use kazoo_core::engine::{EngineCommand, EngineHandle};
use kazoo_core::synthesis::SynthesisMode;

use crate::app::{App, AudioDevices};

/// Sample rate reported by test engine handles.
pub const TEST_SAMPLE_RATE: u32 = 44_100;

/// Create an [`EngineHandle`] with no audio threads, plus the receiving end
/// of its command channel so tests can observe what the UI sent.
pub fn engine_handle() -> (EngineHandle, Receiver<EngineCommand>) {
    let (cmd_tx, cmd_rx) = unbounded();
    (
        EngineHandle::detached(cmd_tx, TEST_SAMPLE_RATE, 256),
        cmd_rx,
    )
}

/// An [`App`] wired to an in-memory engine whose command channel stays
/// connected for the lifetime of the fixture.
///
/// Dereferences to [`App`], so `&mut test_app` can be passed wherever
/// `&mut App` is expected.
#[derive(Debug)]
pub struct TestApp {
    app: App,
    commands: Option<Receiver<EngineCommand>>,
}

impl TestApp {
    /// An app with no tracks.
    pub fn empty() -> Self {
        let (engine, commands) = engine_handle();
        Self {
            app: App::new_empty(engine, AudioDevices::none()),
            commands: Some(commands),
        }
    }

    /// An app constructed exactly as the real binary constructs it (one
    /// default armed track), minus device enumeration.
    pub fn with_default_track() -> Self {
        let (engine, commands) = engine_handle();
        Self {
            app: App::new(engine, AudioDevices::none()),
            commands: Some(commands),
        }
    }

    /// An app with `count` tracks named `"1"`, `"2"`, ...
    pub fn with_tracks(count: usize) -> Self {
        let mut test_app = Self::empty();
        for i in 0..count {
            assert!(test_app.add_track(format!("{}", i + 1), SynthesisMode::PitchTracked));
        }
        test_app
    }

    /// Drain and return every command the app has sent so far.
    pub fn take_commands(&self) -> Vec<EngineCommand> {
        self.commands
            .as_ref()
            .map_or_else(Vec::new, |rx| rx.try_iter().collect())
    }

    /// Drop the engine side of the command channel, simulating an engine
    /// that has stopped. Every subsequent command send fails.
    pub fn disconnect_engine(&mut self) {
        self.commands = None;
    }
}

impl Deref for TestApp {
    type Target = App;

    fn deref(&self) -> &App {
        &self.app
    }
}

impl DerefMut for TestApp {
    fn deref_mut(&mut self) -> &mut App {
        &mut self.app
    }
}

/// Parameterless effect metadata with the given names, none bypassed.
pub fn effects(names: &[&str]) -> Vec<crate::app::EffectInfo> {
    names
        .iter()
        .map(|name| crate::app::EffectInfo {
            name: (*name).to_owned(),
            bypassed: false,
            param_infos: Vec::new(),
            param_values: Vec::new(),
        })
        .collect()
}
