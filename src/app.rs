use crate::config::{Action, Config, ConfigStore, InputKey, RuntimeBindings};
use anyhow::{Context, Result};
use eframe::egui;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{TrayIcon, TrayIconBuilder};

const ICON_SIZE: u32 = 64;
const ICON_RGBA: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/nocaps.rgba"));
// Green-phosphor CRT palette: a dark screen, glowing text, amber for "live".
const SCREEN_BG: egui::Color32 = egui::Color32::from_rgb(6, 15, 9);
const PANEL_BG: egui::Color32 = egui::Color32::from_rgb(12, 26, 16);
const KEYCAP_HOT: egui::Color32 = egui::Color32::from_rgb(48, 40, 14);
const PHOSPHOR: egui::Color32 = egui::Color32::from_rgb(128, 240, 152);
const PHOSPHOR_DIM: egui::Color32 = egui::Color32::from_rgb(78, 160, 104);
const PHOSPHOR_FAINT: egui::Color32 = egui::Color32::from_rgb(42, 92, 60);
const AMBER: egui::Color32 = egui::Color32::from_rgb(255, 196, 92);
const ALARM: egui::Color32 = egui::Color32::from_rgb(255, 104, 92);

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
            .with_inner_size([560.0, 560.0])
            .with_min_inner_size([540.0, 520.0])
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
    // Tray menu activations arrive on muda's event handler, which may run on a
    // native callback rather than the egui frame. The handler parks events here
    // and wakes the UI; `process_tray_events` drains them on the next frame.
    tray_events: Arc<Mutex<Vec<MenuEvent>>>,
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
        let tray_events: Arc<Mutex<Vec<MenuEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let (tray, tray_error) = match create_tray(config.enabled) {
            Ok(tray) => {
                // Forward tray activations onto our queue and wake the UI thread,
                // so the egui loop can stay idle (no periodic polling) until the
                // user actually clicks a menu item.
                let queue = Arc::clone(&tray_events);
                let context = context.clone();
                MenuEvent::set_event_handler(Some(move |event| {
                    if let Ok(mut queue) = queue.lock() {
                        queue.push(event);
                    }
                    context.request_repaint();
                }));
                (Some(tray), None)
            }
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
            tray_events,
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
        let events: Vec<MenuEvent> = match self.tray_events.lock() {
            Ok(mut queue) => queue.drain(..).collect(),
            Err(_) => return,
        };
        for event in events {
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
        let binding = self.config.key_for(action);
        ui.horizontal(|ui| {
            ui.set_min_height(20.0);
            let label_color = if selected { AMBER } else { PHOSPHOR_DIM };
            ui.label(
                egui::RichText::new(action.label())
                    .color(label_color)
                    .size(12.0),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Clear is a single glyph, not a word — the row reads like a
                // terminal field you blank out, and stays tight.
                if binding.is_some() {
                    let clear = ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new("×").color(PHOSPHOR_DIM).size(15.0),
                            )
                            .frame(false),
                        )
                        .on_hover_text("clear binding");
                    if clear.clicked() {
                        self.config.unbind(action);
                        self.capturing = None;
                        self.persist(format!("{} cleared", action.label()));
                    }
                } else {
                    // Reserve the glyph's width so every keycap lines up.
                    ui.add_space(15.0);
                }

                let cap_text = if selected {
                    "[?]".to_owned()
                } else if let Some(key) = binding {
                    key.label().to_owned()
                } else {
                    "·".to_owned()
                };
                let cap_color = if selected {
                    AMBER
                } else if binding.is_some() {
                    PHOSPHOR
                } else {
                    PHOSPHOR_FAINT
                };
                let keycap = ui.add(
                    egui::Button::new(egui::RichText::new(cap_text).color(cap_color).size(12.0))
                        .fill(if selected { KEYCAP_HOT } else { SCREEN_BG })
                        .stroke(egui::Stroke::new(
                            1.0,
                            if selected { AMBER } else { PHOSPHOR_FAINT },
                        ))
                        .min_size(egui::vec2(48.0, 18.0)),
                );
                if keycap.clicked() {
                    if selected {
                        self.capturing = None;
                    } else {
                        self.capturing = Some(action);
                        self.status = None;
                    }
                }
            });
        });
    }

    fn category_block(&mut self, ui: &mut egui::Ui, title: &str) {
        egui::Frame::new()
            .fill(PANEL_BG)
            .stroke(egui::Stroke::new(1.0, PHOSPHOR_FAINT))
            .corner_radius(0)
            .inner_margin(egui::Margin::symmetric(8, 5))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    egui::RichText::new(format!("[ {} ]", title.to_uppercase()))
                        .color(AMBER)
                        .size(12.0)
                        .strong(),
                );
                ui.add_space(3.0);
                for action in Action::ALL
                    .iter()
                    .copied()
                    .filter(|action| action.category() == title)
                {
                    self.action_row(ui, action);
                }
            });
        ui.add_space(7.0);
    }

    fn title_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            // Reverse-video brand, the way a DOS app stamps its top line.
            egui::Frame::new()
                .fill(PHOSPHOR)
                .corner_radius(0)
                .inner_margin(egui::Margin::symmetric(7, 2))
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new("nocaps")
                            .color(SCREEN_BG)
                            .strong()
                            .size(16.0),
                    );
                });
            ui.label(
                egui::RichText::new(concat!("v", env!("CARGO_PKG_VERSION")))
                    .color(PHOSPHOR_FAINT)
                    .size(12.0),
            )
            .on_hover_text(format!("config: {}", self.store.path().display()));

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let enabled = self.config.enabled;
                let (text, fg, bg) = if enabled {
                    ("ON  no caps", SCREEN_BG, AMBER)
                } else {
                    ("OFF caps", PHOSPHOR_DIM, PANEL_BG)
                };
                let toggle = ui.add(
                    egui::Button::new(egui::RichText::new(text).color(fg).strong().size(13.0))
                        .fill(bg)
                        .stroke(egui::Stroke::new(
                            1.0,
                            if enabled { AMBER } else { PHOSPHOR_FAINT },
                        ))
                        .min_size(egui::vec2(98.0, 20.0)),
                );
                if toggle.clicked() {
                    self.config.enabled = !self.config.enabled;
                    self.persist(if self.config.enabled {
                        "no caps — enabled".to_owned()
                    } else {
                        "caps — disabled".to_owned()
                    });
                }

                if ui
                    .add(egui::Button::new(
                        egui::RichText::new("restore").color(PHOSPHOR_DIM).size(12.0),
                    ))
                    .on_hover_text("restore default bindings")
                    .clicked()
                {
                    self.config = Config::default();
                    self.capturing = None;
                    self.persist("defaults restored".to_owned());
                }

                if self.tray.is_none()
                    && ui
                        .add(egui::Button::new(
                            egui::RichText::new("quit").color(PHOSPHOR_DIM).size(12.0),
                        ))
                        .clicked()
                {
                    self.running.store(false, Ordering::SeqCst);
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let cursor = if ui.input(|input| input.time).rem_euclid(1.0) < 0.5 {
            "_"
        } else {
            " "
        };
        let (text, color) = if let Some(action) = self.capturing {
            (
                format!(
                    "> press a key for {}{}   [click the slot again to cancel]",
                    action.label().to_uppercase(),
                    cursor
                ),
                AMBER,
            )
        } else if let Some(error) = &self.runtime_error {
            (format!("! {}", error.replace('\n', "  ")), ALARM)
        } else if let Some(status) = &self.status {
            (
                format!("> {}", status.message),
                if status.is_error { ALARM } else { PHOSPHOR },
            )
        } else {
            (
                "> ready — hold CAPS and tap a bound key, or click a slot to rebind".to_owned(),
                PHOSPHOR_DIM,
            )
        };
        ui.label(egui::RichText::new(text).color(color).size(13.0));
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

        // Tray clicks wake the UI via the muda event handler, and egui already
        // repaints on its own window input, so no periodic repaint is needed to
        // stay responsive while idle. Linux is the exception: its AppIndicator
        // menu only emits events while the GTK loop is iterated by
        // `pump_native_tray_events`, so keep a gentle tick to drive that pump.
        #[cfg(target_os = "linux")]
        context.request_repaint_after(std::time::Duration::from_millis(100));

        // The only animation is the status-bar cursor while waiting for a key,
        // so we wake just for that — the window otherwise stays idle at rest.
        if self.capturing.is_some() {
            context.request_repaint_after(std::time::Duration::from_millis(450));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui.visuals_mut().override_text_color = Some(PHOSPHOR);
        let screen = ui.max_rect();
        ui.painter().rect_filled(screen, 0.0, SCREEN_BG);
        draw_scanlines(ui.painter(), screen);

        let bar = || {
            egui::Frame::new()
                .fill(PANEL_BG)
                .inner_margin(egui::Margin::symmetric(12, 6))
        };
        egui::Panel::top("nocaps-title")
            .frame(bar())
            .show_separator_line(false)
            .show_inside(ui, |ui| self.title_bar(ui));
        egui::Panel::bottom("nocaps-status")
            .frame(bar())
            .show_separator_line(false)
            .show_inside(ui, |ui| self.status_bar(ui));

        // Central grid: two columns hold all five groups at once — no scrolling.
        egui::CentralPanel::default()
            .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(10, 8)))
            .show_inside(ui, |ui| {
                ui.columns(2, |columns| {
                    self.category_block(&mut columns[0], "Modifiers");
                    self.category_block(&mut columns[0], "Navigation");
                    self.category_block(&mut columns[1], "Editing");
                    self.category_block(&mut columns[1], "Volume");
                    self.category_block(&mut columns[1], "Media");
                });
            });
    }
}

/// Faint horizontal lines across the whole surface for a CRT feel; panels paint
/// over them, so they only show through the screen background and the gutters.
fn draw_scanlines(painter: &egui::Painter, rect: egui::Rect) {
    let line = egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(0, 0, 0, 38));
    let mut y = rect.top();
    while y < rect.bottom() {
        painter.hline(rect.x_range(), y, line);
        y += 3.0;
    }
}

fn configure_style(context: &egui::Context) {
    let mut fonts = egui::FontDefinitions::empty();
    fonts.font_data.insert(
        "nocaps-mono".to_owned(),
        Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/DejaVuSansMono.ttf"
        ))),
    );
    // The whole interface is monospace, so it reads like a terminal.
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .insert(0, "nocaps-mono".to_owned());
    }
    context.set_fonts(fonts);

    let mut visuals = egui::Visuals::dark();
    visuals.override_text_color = Some(PHOSPHOR);
    visuals.window_fill = SCREEN_BG;
    visuals.panel_fill = SCREEN_BG;
    visuals.extreme_bg_color = SCREEN_BG;
    visuals.faint_bg_color = PANEL_BG;
    visuals.warn_fg_color = AMBER;
    visuals.error_fg_color = ALARM;
    visuals.selection.bg_fill = PHOSPHOR_FAINT;
    visuals.selection.stroke = egui::Stroke::new(1.0, SCREEN_BG);

    // Sharp corners everywhere — no rounded edges on a CRT.
    let sharp = egui::CornerRadius::same(0);
    let widgets = &mut visuals.widgets;
    widgets.noninteractive.corner_radius = sharp;
    widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, PHOSPHOR_FAINT);
    widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, PHOSPHOR);
    widgets.inactive.corner_radius = sharp;
    widgets.inactive.bg_fill = PANEL_BG;
    widgets.inactive.weak_bg_fill = PANEL_BG;
    widgets.inactive.bg_stroke = egui::Stroke::new(1.0, PHOSPHOR_FAINT);
    widgets.inactive.fg_stroke = egui::Stroke::new(1.0, PHOSPHOR);
    widgets.hovered.corner_radius = sharp;
    widgets.hovered.bg_fill = egui::Color32::from_rgb(22, 46, 30);
    widgets.hovered.weak_bg_fill = egui::Color32::from_rgb(22, 46, 30);
    widgets.hovered.bg_stroke = egui::Stroke::new(1.0, AMBER);
    widgets.hovered.fg_stroke = egui::Stroke::new(1.0, AMBER);
    widgets.active.corner_radius = sharp;
    widgets.active.bg_fill = PHOSPHOR_FAINT;
    widgets.active.weak_bg_fill = PHOSPHOR_FAINT;
    widgets.active.bg_stroke = egui::Stroke::new(1.0, AMBER);
    widgets.active.fg_stroke = egui::Stroke::new(1.0, SCREEN_BG);
    widgets.open.corner_radius = sharp;
    context.set_visuals(visuals);

    let mut style = (*context.global_style()).clone();
    style.spacing.item_spacing = egui::vec2(6.0, 4.0);
    style.spacing.button_padding = egui::vec2(7.0, 3.0);
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
