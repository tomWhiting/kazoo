//! Kazoo TUI application entry point.
//!
//! Sets up error handling, initializes the audio engine, enters the
//! alternate-screen terminal, and drives the main event loop. The terminal
//! is always restored on exit — whether the application quits normally,
//! returns an error, or panics.

mod app;
mod input;
mod state;
mod status;
#[cfg(test)]
mod test_support;
mod theme;
mod ui;

use color_eyre::Result;

use kazoo_core::engine::EngineConfig;

/// The name this synth plugs into the kazoo-mix desk under.
const DESK_NAME: &str = "kazoo-tui";

#[tokio::main]
async fn main() -> Result<()> {
    // Install color-eyre error and panic hooks first.
    color_eyre::install()?;

    // Wrap color-eyre's panic hook so the terminal is restored before the
    // panic message is printed — otherwise the user sees garbage in the
    // alternate screen.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        ratatui::restore();
        default_hook(info);
    }));

    // Start the audio engine *before* entering the alternate screen so that
    // initialisation errors (missing audio device, etc.) are printed in the
    // user's normal terminal. The engine plugs into the kazoo-mix desk (the
    // studio's hub) whenever one is running, and plays standalone otherwise.
    let engine = kazoo_core::engine::start(EngineConfig {
        desk_instrument_name: Some(DESK_NAME.to_owned()),
        ..EngineConfig::default()
    })?;

    // Enumerate audio devices for the Audio I/O view. Done here, not inside
    // `App`, so constructing an `App` never touches audio hardware.
    let devices = app::AudioDevices::enumerate();

    // The header shows the desk link's state, including why it could not
    // start if so; stderr would be hidden by the alternate screen.
    let mut app = app::App::new(engine, devices);

    // Enter the alternate screen and enable raw mode.
    let mut terminal = ratatui::init();

    // Run the application event loop.
    let result = app.run(&mut terminal).await;

    // Always restore the terminal, even if the event loop errored.
    ratatui::restore();

    Ok(result?)
}
