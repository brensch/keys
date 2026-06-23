#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use anyhow::Result;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

mod app;
mod config;
mod platform;

use config::{Config, ConfigStore, RuntimeBindings};

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("nocaps=info"))
        .init();

    #[cfg(target_os = "linux")]
    gtk::init()
        .map_err(|error| anyhow::anyhow!("initialize GTK for the Linux tray icon: {error}"))?;

    let store = ConfigStore::discover()?;
    let mut startup_errors = Vec::new();
    let config = match store.load_or_create() {
        Ok(config) => config,
        Err(error) => {
            startup_errors.push(format!("Could not load the configuration: {error:#}"));
            Config::default()
        }
    };
    let runtime = Arc::new(RuntimeBindings::new(&config)?);
    let running = Arc::new(AtomicBool::new(true));

    // Keep the platform hook alive for the full lifetime of the UI event loop.
    let keyboard = match platform::start_keyboard(runtime.clone()) {
        Ok(keyboard) => Some(keyboard),
        Err(error) => {
            log::error!("keyboard remapping is unavailable: {error:#}");
            startup_errors.push(format!("Keyboard remapping is unavailable: {error}"));
            None
        }
    };

    let startup_error = (!startup_errors.is_empty()).then(|| startup_errors.join("\n"));
    let result = app::run(runtime, config, store, running, startup_error);
    drop(keyboard);
    result
}
