use crate::config::{Action, Config, ConfigStore, InputKey, RuntimeBindings};
use anyhow::{Context, Result};
use eframe::egui;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{TrayIcon, TrayIconBuilder};

const ICON_SIZE: u32 = 64;
const ICON_RGBA: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/nocaps.rgba"));
const BACKGROUND: egui::Color32 = egui::Color32::from_rgb(20, 22, 27);
const TEXT: egui::Color32 = egui::Color32::from_rgb(242, 244, 248);

pub fn run(
    runtime: Arc<RuntimeBindings>,
    config: Config,
    store: ConfigStore,
    running: Arc<AtomicBool>,
    startup_error: Option<String>,
) -> Result<()> {
    let show_on_start = startup_error.is_some();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("nocaps")
            .with_inner_size([680.0, 760.0])
            .with_min_inner_size([540.0, 480.0])
            .with_resizable(true)
            .with_visible(show_on_start)
            .with_icon(window_icon()),
        ..Default::default()
    };

    eframe::run_native(
        "nocaps",
        options,
        Box::new(move |creation_context| {
            configure_style(&creation_context.egui_ctx);
            let app = NocapsApp::new(
                runtime,
                config,
                store,
                running,
                startup_error,
                &creation_context.egui_ctx,
            )
            .map_err(|error| -> Box<dyn std::error::Error + Send + Sync> { error.into() })?;
            Ok(Box::new(app))
        }),
    )
    .map_err(|error| anyhow::anyhow!(error.to_string()))
}

struct Tray {
    _icon: TrayIcon,
    enabled: MenuItem,
    configure: MenuItem,
    quit: MenuItem,
}

impl Tray {
    fn new(remapping_enabled: bool) -> Result<Self> {
        let menu = Menu::new();
        let enabled = MenuItem::new(enabled_menu_text(remapping_enabled), true, None);
        let configure = MenuItem::new("Configure", true, None);
        let quit = MenuItem::new("Quit nocaps", true, None);
        menu.append(&enabled).context("add Enabled tray item")?;
        menu.append(&configure).context("add Configure tray item")?;
        menu.append(&quit).context("add Quit tray item")?;

        let icon = tray_icon::Icon::from_rgba(ICON_RGBA.to_vec(), ICON_SIZE, ICON_SIZE)
            .context("create nocaps tray icon")?;
        let tray_icon = TrayIconBuilder::new()
            .with_tooltip("nocaps")
            .with_icon(icon)
            .with_menu(Box::new(menu))
            .build()
            .context("create system tray icon")?;

        Ok(Self {
            _icon: tray_icon,
            enabled,
            configure,
            quit,
        })
    }

    fn set_enabled(&self, enabled: bool) {
        self.enabled.set_text(enabled_menu_text(enabled));
    }
}

struct NocapsApp {
    runtime: Arc<RuntimeBindings>,
    config: Config,
    store: ConfigStore,
    running: Arc<AtomicBool>,
    tray: Option<Tray>,
    runtime_error: Option<String>,
    status: Option<Status>,
    capturing: Option<Action>,
}

struct Status {
    is_error: bool,
    message: String,
}

impl NocapsApp {
    fn new(
        runtime: Arc<RuntimeBindings>,
        config: Config,
        store: ConfigStore,
        running: Arc<AtomicBool>,
        startup_error: Option<String>,
        context: &egui::Context,
    ) -> Result<Self> {
        let (tray, tray_error) = match create_tray(config.enabled) {
            Ok(tray) => (Some(tray), None),
            Err(error) => {
                context.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                (None, Some(error.to_string()))
            }
        };

        let mut errors = Vec::new();
        if let Some(message) = startup_error {
            errors.push(format!(
                "{message}\nRestart nocaps after fixing startup issues."
            ));
        }
        if let Some(message) = tray_error {
            errors.push(format!(
                "The tray icon is unavailable: {message}. Keep this window open."
            ));
        }

        Ok(Self {
            runtime,
            config,
            store,
            running,
            tray,
            runtime_error: (!errors.is_empty()).then(|| errors.join("\n")),
            status: None,
            capturing: None,
        })
    }

    fn process_tray_events(&mut self, context: &egui::Context) {
        let Some(tray) = self.tray.as_ref() else {
            return;
        };
        let enabled_id = tray.enabled.id().clone();
        let configure_id = tray.configure.id().clone();
        let quit_id = tray.quit.id().clone();
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            let toggle_enabled = event.id == enabled_id;
            let configure = event.id == configure_id;
            let quit = event.id == quit_id;
            if toggle_enabled {
                self.config.enabled = !self.config.enabled;
                self.persist(if self.config.enabled {
                    "No Caps enabled".to_owned()
                } else {
                    "Caps enabled".to_owned()
                });
            } else if configure {
                context.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                context.send_viewport_cmd(egui::ViewportCommand::Focus);
            } else if quit {
                self.running.store(false, Ordering::SeqCst);
                context.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    fn capture_pressed_key(&mut self, context: &egui::Context) {
        let Some(action) = self.capturing else {
            return;
        };
        let pressed = context.input_mut(|input| {
            let index = input.events.iter().position(|event| {
                matches!(
                    event,
                    egui::Event::Key {
                        pressed: true,
                        repeat: false,
                        ..
                    }
                )
            })?;
            match input.events.remove(index) {
                egui::Event::Key {
                    key, physical_key, ..
                } => Some(physical_key.unwrap_or(key)),
                _ => None,
            }
        });

        let Some(pressed) = pressed else {
            return;
        };
        match input_key_from_egui(pressed) {
            Some(key) => {
                self.config.bind(action, key);
                self.capturing = None;
                self.persist(format!(
                    "{} is now Caps Lock + {}",
                    action.label(),
                    key.label()
                ));
            }
            None => {
                self.status = Some(Status {
                    is_error: true,
                    message: "That key cannot be used as a binding.".to_owned(),
                });
            }
        }
    }

    fn persist(&mut self, success_message: String) {
        let result = self
            .runtime
            .replace(&self.config)
            .and_then(|_| self.store.save(&self.config));
        if let Some(tray) = &self.tray {
            tray.set_enabled(self.config.enabled);
        }
        self.status = Some(match result {
            Ok(()) => Status {
                is_error: false,
                message: success_message,
            },
            Err(error) => Status {
                is_error: true,
                message: error.to_string(),
            },
        });
    }

    fn action_row(&mut self, ui: &mut egui::Ui, action: Action) {
        let selected = self.capturing == Some(action);
        let fill = if selected {
            egui::Color32::from_rgb(44, 67, 116)
        } else {
            egui::Color32::from_rgb(32, 35, 43)
        };
        egui::Frame::new()
            .fill(fill)
            .corner_radius(8)
            .inner_margin(egui::Margin::symmetric(14, 10))
            .show(ui, |ui| {
                ui.set_min_height(34.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(action.label()).size(15.0));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let binding = self.config.key_for(action);
                        let text = if selected {
                            "Press a key…"
                        } else if let Some(key) = binding {
                            key.label()
                        } else {
                            "Set key"
                        };
                        let button = egui::Button::new(egui::RichText::new(text).strong())
                            .min_size(egui::vec2(150.0, 30.0));
                        if ui.add(button).clicked() {
                            self.capturing = Some(action);
                            self.status = None;
                        }
                        if binding.is_some() && ui.small_button("Clear").clicked() {
                            self.config.unbind(action);
                            self.capturing = None;
                            self.persist(format!("{} binding cleared", action.label()));
                        }
                    });
                });
            });
    }
}

impl eframe::App for NocapsApp {
    fn logic(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        pump_native_tray_events();
        self.process_tray_events(context);
        self.capture_pressed_key(context);

        if context.input(|input| input.viewport().close_requested())
            && self.running.load(Ordering::SeqCst)
        {
            if self.tray.is_some() {
                context.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                context.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            } else {
                self.running.store(false, Ordering::SeqCst);
            }
        }

        context.request_repaint_after(Duration::from_millis(75));
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Do not inherit host theme text colors: the nocaps surface is always dark.
        ui.visuals_mut().override_text_color = Some(TEXT);
        ui.painter().rect_filled(ui.max_rect(), 0.0, BACKGROUND);
        egui::Frame::new()
            .inner_margin(egui::Margin::same(24))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.heading(egui::RichText::new("nocaps").size(28.0).strong());
                        ui.label("This will make using a keyboard more ergonomic, no caps.");
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let label = if self.config.enabled {
                            "No Caps"
                        } else {
                            "Caps"
                        };
                        let changed = ui.toggle_value(&mut self.config.enabled, label).changed();
                        if changed {
                            self.persist(if self.config.enabled {
                                "No Caps enabled".to_owned()
                            } else {
                                "Caps enabled".to_owned()
                            });
                        }
                    });
                });

                ui.add_space(12.0);
                if let Some(action) = self.capturing {
                    egui::Frame::new()
                        .fill(egui::Color32::from_rgb(35, 58, 104))
                        .corner_radius(8)
                        .inner_margin(egui::Margin::same(12))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(format!(
                                    "Press the key that should trigger {}.",
                                    action.label()
                                ));
                                if ui.button("Cancel").clicked() {
                                    self.capturing = None;
                                }
                            });
                        });
                } else {
                    ui.label("Click a binding, then press the physical key you want to use.");
                }

                if let Some(message) = &self.runtime_error {
                    ui.add_space(8.0);
                    ui.colored_label(ui.visuals().error_fg_color, message);
                }
                if let Some(status) = &self.status {
                    ui.add_space(8.0);
                    let color = if status.is_error {
                        ui.visuals().error_fg_color
                    } else {
                        egui::Color32::from_rgb(126, 211, 157)
                    };
                    ui.colored_label(color, &status.message);
                }

                ui.add_space(12.0);
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let mut category = "";
                        for action in Action::ALL.iter().copied() {
                            if action.category() != category {
                                category = action.category();
                                ui.add_space(8.0);
                                ui.label(
                                    egui::RichText::new(category)
                                        .strong()
                                        .color(egui::Color32::from_rgb(160, 169, 190)),
                                );
                            }
                            ui.add_space(4.0);
                            self.action_row(ui, action);
                        }

                        ui.add_space(16.0);
                        ui.horizontal(|ui| {
                            if ui.button("Restore defaults").clicked() {
                                self.config = Config::default();
                                self.capturing = None;
                                self.persist("Default bindings restored".to_owned());
                            }
                            if self.tray.is_none() && ui.button("Quit").clicked() {
                                self.running.store(false, Ordering::SeqCst);
                                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                        });
                        ui.add_space(8.0);
                        ui.small(format!(
                            "Saved automatically to {}",
                            self.store.path().display()
                        ));
                    });
            });
    }
}

fn configure_style(context: &egui::Context) {
    let mut fonts = egui::FontDefinitions::empty();
    fonts.font_data.insert(
        "nocaps-sans".to_owned(),
        Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/DejaVuSans.ttf"
        ))),
    );
    fonts
        .families
        .get_mut(&egui::FontFamily::Proportional)
        .expect("proportional font family exists")
        .push("nocaps-sans".to_owned());
    fonts
        .families
        .get_mut(&egui::FontFamily::Monospace)
        .expect("monospace font family exists")
        .push("nocaps-sans".to_owned());
    context.set_fonts(fonts);

    let mut visuals = egui::Visuals::dark();
    visuals.override_text_color = Some(TEXT);
    visuals.window_fill = BACKGROUND;
    visuals.panel_fill = visuals.window_fill;
    visuals.selection.bg_fill = egui::Color32::from_rgb(63, 102, 184);
    visuals.widgets.inactive.fg_stroke.color = TEXT;
    visuals.widgets.hovered.fg_stroke.color = egui::Color32::WHITE;
    visuals.widgets.active.fg_stroke.color = egui::Color32::WHITE;
    context.set_visuals(visuals);
    let mut style = (*context.global_style()).clone();
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(12.0, 7.0);
    context.set_global_style(style);
}

fn input_key_from_egui(key: egui::Key) -> Option<InputKey> {
    Some(match key {
        egui::Key::A => InputKey::A,
        egui::Key::B => InputKey::B,
        egui::Key::C => InputKey::C,
        egui::Key::D => InputKey::D,
        egui::Key::E => InputKey::E,
        egui::Key::F => InputKey::F,
        egui::Key::G => InputKey::G,
        egui::Key::H => InputKey::H,
        egui::Key::I => InputKey::I,
        egui::Key::J => InputKey::J,
        egui::Key::K => InputKey::K,
        egui::Key::L => InputKey::L,
        egui::Key::M => InputKey::M,
        egui::Key::N => InputKey::N,
        egui::Key::O => InputKey::O,
        egui::Key::P => InputKey::P,
        egui::Key::Q => InputKey::Q,
        egui::Key::R => InputKey::R,
        egui::Key::S => InputKey::S,
        egui::Key::T => InputKey::T,
        egui::Key::U => InputKey::U,
        egui::Key::V => InputKey::V,
        egui::Key::W => InputKey::W,
        egui::Key::X => InputKey::X,
        egui::Key::Y => InputKey::Y,
        egui::Key::Z => InputKey::Z,
        egui::Key::Num0 => InputKey::Digit0,
        egui::Key::Num1 => InputKey::Digit1,
        egui::Key::Num2 => InputKey::Digit2,
        egui::Key::Num3 => InputKey::Digit3,
        egui::Key::Num4 => InputKey::Digit4,
        egui::Key::Num5 => InputKey::Digit5,
        egui::Key::Num6 => InputKey::Digit6,
        egui::Key::Num7 => InputKey::Digit7,
        egui::Key::Num8 => InputKey::Digit8,
        egui::Key::Num9 => InputKey::Digit9,
        egui::Key::Backtick => InputKey::Backquote,
        egui::Key::Minus => InputKey::Minus,
        egui::Key::Equals | egui::Key::Plus => InputKey::Equal,
        egui::Key::OpenBracket | egui::Key::OpenCurlyBracket => InputKey::LeftBracket,
        egui::Key::CloseBracket | egui::Key::CloseCurlyBracket => InputKey::RightBracket,
        egui::Key::Backslash | egui::Key::Pipe => InputKey::Backslash,
        egui::Key::Semicolon | egui::Key::Colon => InputKey::Semicolon,
        egui::Key::Quote => InputKey::Quote,
        egui::Key::Comma => InputKey::Comma,
        egui::Key::Period => InputKey::Period,
        egui::Key::Slash | egui::Key::Questionmark => InputKey::Slash,
        egui::Key::Tab => InputKey::Tab,
        egui::Key::Space => InputKey::Space,
        egui::Key::Enter => InputKey::Enter,
        egui::Key::Escape => InputKey::Escape,
        egui::Key::Backspace => InputKey::Backspace,
        egui::Key::Delete => InputKey::Delete,
        egui::Key::Insert => InputKey::Insert,
        egui::Key::Home => InputKey::Home,
        egui::Key::End => InputKey::End,
        egui::Key::PageUp => InputKey::PageUp,
        egui::Key::PageDown => InputKey::PageDown,
        egui::Key::ArrowUp => InputKey::ArrowUp,
        egui::Key::ArrowDown => InputKey::ArrowDown,
        egui::Key::ArrowLeft => InputKey::ArrowLeft,
        egui::Key::ArrowRight => InputKey::ArrowRight,
        egui::Key::F1 => InputKey::F1,
        egui::Key::F2 => InputKey::F2,
        egui::Key::F3 => InputKey::F3,
        egui::Key::F4 => InputKey::F4,
        egui::Key::F5 => InputKey::F5,
        egui::Key::F6 => InputKey::F6,
        egui::Key::F7 => InputKey::F7,
        egui::Key::F8 => InputKey::F8,
        egui::Key::F9 => InputKey::F9,
        egui::Key::F10 => InputKey::F10,
        egui::Key::F11 => InputKey::F11,
        egui::Key::F12 => InputKey::F12,
        egui::Key::F13 => InputKey::F13,
        egui::Key::F14 => InputKey::F14,
        egui::Key::F15 => InputKey::F15,
        egui::Key::F16 => InputKey::F16,
        egui::Key::F17 => InputKey::F17,
        egui::Key::F18 => InputKey::F18,
        egui::Key::F19 => InputKey::F19,
        egui::Key::F20 => InputKey::F20,
        egui::Key::F21 => InputKey::F21,
        egui::Key::F22 => InputKey::F22,
        egui::Key::F23 => InputKey::F23,
        egui::Key::F24 => InputKey::F24,
        _ => return None,
    })
}

fn window_icon() -> egui::IconData {
    egui::IconData {
        rgba: ICON_RGBA.to_vec(),
        width: ICON_SIZE,
        height: ICON_SIZE,
    }
}

#[cfg(target_os = "linux")]
fn create_tray(enabled: bool) -> Result<Tray> {
    let appindicator_available = ["libayatana-appindicator3.so.1", "libappindicator3.so.1"]
        .iter()
        .any(|name| unsafe { libloading::Library::new(name).is_ok() });
    if !appindicator_available {
        return Err(anyhow::anyhow!(
            "install the Ayatana AppIndicator 3 runtime library"
        ));
    }
    std::panic::catch_unwind(|| Tray::new(enabled))
        .map_err(|_| anyhow::anyhow!("create Linux tray icon"))?
}

#[cfg(not(target_os = "linux"))]
fn create_tray(enabled: bool) -> Result<Tray> {
    Tray::new(enabled)
}

fn enabled_menu_text(enabled: bool) -> &'static str {
    if enabled {
        "Enabled"
    } else {
        "Disabled"
    }
}

#[cfg(target_os = "linux")]
fn pump_native_tray_events() {
    while gtk::events_pending() {
        gtk::main_iteration_do(false);
    }
}

#[cfg(not(target_os = "linux"))]
fn pump_native_tray_events() {}
