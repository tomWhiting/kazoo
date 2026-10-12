//! Error reports, installed once per process.

use std::sync::Once;

/// Install `color_eyre`'s error and panic reports, once per process.
///
/// The first call installs them and returns what installing returned; every later call installs
/// nothing and returns `Ok(())`, so each instrument's `run` under the one
/// `kazoo` binary may call it.
///
/// # Errors
///
/// The first call fails if reports were already installed by other means.
pub fn install() -> color_eyre::Result<()> {
    static ONCE: Once = Once::new();
    let mut installed = Ok(());
    ONCE.call_once(|| installed = color_eyre::install());
    installed
}

#[cfg(test)]
mod tests {
    use super::install;

    #[test]
    fn install_twice_is_fine() {
        install().expect("first install");
        install().expect("second install is a quiet no-op");
    }
}
