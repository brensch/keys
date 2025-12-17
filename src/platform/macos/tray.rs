use anyhow::Result;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tray_icon::{TrayIconBuilder, menu::Menu, menu::MenuItem};

pub struct TrayManager {
    _tray: tray_icon::TrayIcon,
}

impl TrayManager {
    pub fn new(_running: Arc<AtomicBool>) -> Result<Self> {
        let tray_menu = Menu::new();
        let quit_i = MenuItem::new("Quit", true, None);
        tray_menu.append(&quit_i).unwrap();

        let tray_icon = TrayIconBuilder::new()
            .with_menu(Box::new(tray_menu))
            .with_tooltip("keys")
            .build()?;

        Ok(Self { _tray: tray_icon })
    }
}
