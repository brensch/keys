use anyhow::Result;
use eframe::egui;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tray_icon::{TrayIconBuilder, menu::Menu, menu::MenuItem, menu::MenuEvent, TrayIconEvent};
use log::info;

pub struct TrayManager {
    _tray: tray_icon::TrayIcon,
    pub quit_item: MenuItem,
    pub show_item: MenuItem,
}

impl TrayManager {
    pub fn new() -> Result<Self> {
        let tray_menu = Menu::new();
        let show_item = MenuItem::new("Settings", true, None);
        let quit_item = MenuItem::new("Quit", true, None);
        
        tray_menu.append(&show_item).unwrap();
        tray_menu.append(&quit_item).unwrap();

        let tray_icon = TrayIconBuilder::new()
            .with_menu(Box::new(tray_menu))
            .with_tooltip("keys")
            .with_icon(load_icon())
            .build()?;

        Ok(Self { 
            _tray: tray_icon,
            quit_item,
            show_item,
        })
    }
}

fn load_icon() -> tray_icon::Icon {
    let (icon_rgba, icon_width, icon_height) = {
        let image = image::load_from_memory(include_bytes!("../favicon.ico"))
            .expect("Failed to open icon path")
            .into_rgba8();
        let (width, height) = image.dimensions();
        let rgba = image.into_raw();
        (rgba, width, height)
    };
    tray_icon::Icon::from_rgba(icon_rgba, icon_width, icon_height).expect("Failed to create icon")
}

fn load_eframe_icon() -> eframe::icon_data::IconData {
     let (icon_rgba, icon_width, icon_height) = {
        let image = image::load_from_memory(include_bytes!("../favicon.ico"))
            .expect("Failed to open icon path")
            .into_rgba8();
        let (width, height) = image.dimensions();
        let rgba = image.into_raw();
        (rgba, width, height)
    };
    eframe::icon_data::IconData {
        rgba: icon_rgba,
        width: icon_width,
        height: icon_height,
    }
}

pub fn run_event_loop(running: Arc<AtomicBool>, tray: TrayManager) {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_visible(false)
            .with_taskbar(true)
            .with_inner_size([400.0, 300.0])
            .with_icon(load_eframe_icon()),
        ..Default::default()
    };

    let _ = eframe::run_native(
        "Keys Settings",
        options,
        Box::new(move |cc| Ok(Box::new(MyApp::new(running, tray, cc.egui_ctx.clone())))),
    );
}

struct MyApp {
    running: Arc<AtomicBool>,
    _tray_manager: TrayManager,
    first_frame: bool,
}

impl MyApp {
    fn new(running: Arc<AtomicBool>, tray_manager: TrayManager, _ctx: egui::Context) -> Self {
        info!("MyApp initialized - UI starting");
        Self {
            running,
            _tray_manager: tray_manager,
            first_frame: true,
        }
    }
}

impl eframe::App for MyApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.first_frame {
            self.first_frame = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }

        // Check Menu Events
        if let Ok(event) = MenuEvent::receiver().try_recv() {
            info!("Menu event received: {:?}", event.id);
            if event.id == self._tray_manager.quit_item.id() {
                info!("Quit item clicked");
                self.running.store(false, Ordering::SeqCst);
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            } else if event.id == self._tray_manager.show_item.id() {
                info!("Show item clicked");
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            } else {
                info!("Unknown menu item clicked");
            }
        }

        // Check Tray Icon Events
        if let Ok(event) = TrayIconEvent::receiver().try_recv() {
                info!("Tray icon event received: {:?}", event);
                match event {
                TrayIconEvent::Click { .. } => {
                    info!("Tray icon clicked");
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                _ => {}
                }
        }

        // Handle Window Close Request
        if ctx.input(|i| i.viewport().close_requested()) {
             if self.running.load(Ordering::SeqCst) {
                 ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                 ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
             }
        }

        // UI Content
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Keys Settings");
            ui.add_space(20.0);
            ui.label("About:");
            ui.label("Keys - Keyboard Remapper");
            ui.label("Version: 0.1.0");
            ui.add_space(20.0);
            
            if ui.button("Quit Application").clicked() {
                self.running.store(false, Ordering::SeqCst);
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });

        // Ensure we keep checking for tray events even when the window is not focused/visible
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
}
