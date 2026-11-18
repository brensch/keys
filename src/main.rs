#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use anyhow::Result;
use std::sync::Arc;

mod app;
mod config;
#[cfg(target_os = "linux")]
mod instance;
mod platform;

use config::{Config, ConfigStore, BINDINGS};

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("nocaps=info"))
        .init();

    let inbox = Arc::new(app::Inbox::default());

    #[cfg(target_os = "linux")]
    match instance::claim() {
        instance::Instance::Secondary => {
            log::info!("nocaps is already running; asked it to open its window");
            return Ok(());
        }
        instance::Instance::Primary(Some(listener)) => {
            instance::serve(listener, Arc::clone(&inbox))
        }
        instance::Instance::Primary(None) => {}
    }

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
    BINDINGS.apply(&config)?;

    // Keep the platform hook alive for the full lifetime of the UI event loop.
    let keyboard = match platform::start_keyboard() {
        Ok(keyboard) => Some(Arc::new(keyboard)),
        Err(error) => {
            log::error!("keyboard remapping is unavailable: {error:#}");
            startup_errors.push(format!("Keyboard remapping is unavailable: {error:#}"));
            None
        }
    };

    let startup_error = (!startup_errors.is_empty()).then(|| startup_errors.join("\n"));
    let result = app::run(keyboard.clone(), config, store, startup_error, inbox);
    drop(keyboard);
    result
}
