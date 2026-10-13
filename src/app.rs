use crate::config::{Action, Config, ConfigStore, InputKey, BINDINGS};
use crate::platform::KeyboardManager;
use anyhow::{Context, Result};
use eframe::egui;
use std::sync::{Arc, Mutex};
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{TrayIcon, TrayIconBuilder};

const ICON_SIZE: u32 = 64;
const ICON_RGBA: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/nocaps.rgba"));
// Paper & ink: warm paper, solid ink keycaps, and the icon's coral for anything live.
const PAPER: egui::Color32 = egui::Color32::from_rgb(244, 241, 234);
const PAPER_HOVER: egui::Color32 = egui::Color32::from_rgb(236, 231, 220);
const CARD: egui::Color32 = egui::Color32::from_rgb(251, 249, 244);
const RULE: egui::Color32 = egui::Color32::from_rgb(226, 220, 205);
const INK: egui::Color32 = egui::Color32::from_rgb(27, 27, 31);
const INK_HOVER: egui::Color32 = egui::Color32::from_rgb(58, 57, 62);
const MUTED: egui::Color32 = egui::Color32::from_rgb(122, 116, 102);
const FAINT: egui::Color32 = egui::Color32::from_rgb(180, 173, 156);
const EMPTY_SLOT: egui::Color32 = egui::Color32::from_rgb(207, 199, 180);
const CORAL: egui::Color32 = egui::Color32::from_rgb(232, 69, 60);
const CORAL_SOFT: egui::Color32 = egui::Color32::from_rgb(251, 227, 223);
const BOLD_FONT: &str = "inter-bold";

pub fn run(
    keyboard: Option<Arc<KeyboardManager>>,
    config: Config,
    store: ConfigStore,
    startup_error: Option<String>,
) -> Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("nocaps")
            .with_inner_size([560.0, 580.0])
            .with_min_inner_size([540.0, 520.0])
            .with_resizable(true)
            .with_visible(false)
            .with_icon(window_icon()),
        event_loop_builder: event_loop_builder(),
        ..Default::default()
    };

    eframe::run_native(
        "nocaps",
        options,
        Box::new(move |creation_context| {
            configure_style(&creation_context.egui_ctx);
            Ok(Box::new(NocapsApp::new(
                keyboard,
                config,
                store,
                startup_error,
                &creation_context.egui_ctx,
            )))
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
    keyboard: Option<Arc<KeyboardManager>>,
    config: Config,
    store: ConfigStore,
    /// The window starts in the tray unless something needs the user's attention.
    /// eframe always shows the window after its first frame, so it is hidden again
    /// from `logic` on that frame.
    show_on_start: bool,
    started: bool,
    quitting: bool,
    tray: Option<Tray>,
    // Tray menu activations arrive on muda's event handler, which may run on a
    // native callback rather than the egui frame. The handler parks events here
    // and wakes the UI; `process_tray_events` drains them on the next frame.
    tray_events: Arc<Mutex<Vec<MenuEvent>>>,
    icon: egui::TextureHandle,
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
        keyboard: Option<Arc<KeyboardManager>>,
        config: Config,
        store: ConfigStore,
        startup_error: Option<String>,
        context: &egui::Context,
    ) -> Self {
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
            Err(error) => (None, Some(format!("{error:#}"))),
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

        Self {
            keyboard,
            config,
            store,
            show_on_start: !errors.is_empty(),
            started: false,
            quitting: false,
            tray,
            tray_events,
            runtime_error: (!errors.is_empty()).then(|| errors.join("\n")),
            status: None,
            capturing: None,
            icon: context.load_texture(
                "nocaps-icon",
                egui::ColorImage::from_rgba_unmultiplied(
                    [ICON_SIZE as usize, ICON_SIZE as usize],
                    ICON_RGBA,
                ),
                egui::TextureOptions::LINEAR,
            ),
        }
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
                self.toggle_enabled();
            } else if configure {
                context.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                context.send_viewport_cmd(egui::ViewportCommand::Focus);
            } else if quit {
                self.quit(context);
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

    fn toggle_enabled(&mut self) {
        self.config.enabled = !self.config.enabled;
        self.persist(if self.config.enabled {
            "Remapping on".to_owned()
        } else {
            "Remapping off".to_owned()
        });
    }

    fn quit(&mut self, context: &egui::Context) {
        self.quitting = true;
        context.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    fn persist(&mut self, success_message: String) {
        let keyboard = self.keyboard.as_deref();
        let result = BINDINGS
            .apply(&self.config)
            .and_then(|()| keyboard.map_or(Ok(()), |k| k.set_enabled(self.config.enabled)))
            .and_then(|()| self.store.save(&self.config));
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
                message: format!("{error:#}"),
            },
        });
    }

    fn action_row(&mut self, ui: &mut egui::Ui, action: Action) {
        let capturing = self.capturing == Some(action);
        let binding = self.config.key_for(action);
        ui.horizontal(|ui| {
            ui.set_min_height(21.0);
            let label = ui.label(
                egui::RichText::new(action.label())
                    .size(12.5)
                    .color(if capturing { CORAL } else { INK }),
            );
            if let Some(hint) = action.hint() {
                label.on_hover_text(hint);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Always allocated so every keycap lines up, bound or not.
                let clear = ui.add_visible(
                    binding.is_some(),
                    egui::Button::new(egui::RichText::new("×").size(14.0).color(FAINT))
                        .frame(false),
                );
                if clear.on_hover_text("Clear binding").clicked() {
                    self.config.unbind(action);
                    self.capturing = None;
                    self.persist(format!("{} cleared", action.label()));
                }

                if keycap(ui, binding, capturing).clicked() {
                    if capturing {
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
        self.card(ui, title, |app, ui| app.category_rows(ui, title));
    }

    /// Navigation keys, followed by the modifiers that only matter alongside them.
    fn navigation_block(&mut self, ui: &mut egui::Ui) {
        self.card(ui, "Navigation", |app, ui| {
            app.category_rows(ui, "Navigation");
            ui.add_space(4.0);
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("With movement")
                        .font(bold(12.0))
                        .color(INK),
                );
                ui.label(
                    egui::RichText::new("hold with a key above")
                        .size(11.5)
                        .color(MUTED),
                );
            });
            ui.add_space(1.0);
            app.category_rows(ui, "With movement");
        });
    }

    fn card(
        &mut self,
        ui: &mut egui::Ui,
        title: &str,
        body: impl FnOnce(&mut Self, &mut egui::Ui),
    ) {
        egui::Frame::new()
            .fill(CARD)
            .stroke(egui::Stroke::new(1.0_f32, RULE))
            .corner_radius(4)
            .inner_margin(egui::Margin::symmetric(12, 9))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(egui::RichText::new(title).font(bold(13.0)).color(INK));
                ui.add_space(3.0);
                body(self, ui);
            });
        ui.add_space(10.0);
    }

    fn category_rows(&mut self, ui: &mut egui::Ui, category: &str) {
        for action in Action::ALL
            .iter()
            .copied()
            .filter(|action| action.category() == category)
        {
            self.action_row(ui, action);
        }
    }

    fn title_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.add(egui::Image::new(&self.icon).fit_to_exact_size(egui::vec2(28.0, 28.0)));
            ui.label(egui::RichText::new("nocaps").font(bold(17.0)).color(INK));
            ui.label(
                egui::RichText::new(concat!("v", env!("CARGO_PKG_VERSION")))
                    .size(11.5)
                    .color(MUTED),
            )
            .on_hover_text(format!("Config: {}", self.store.path().display()));

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let enabled = self.config.enabled;
                if toggle_switch(ui, enabled)
                    .on_hover_text("Turn Caps Lock remapping on or off")
                    .clicked()
                {
                    self.toggle_enabled();
                }
                // Reserve the width of the longer word so the buttons to the left stay
                // put when the label flips.
                let font = egui::FontId::proportional(12.5);
                let slot = ["Enabled", "Disabled"]
                    .map(|word| {
                        ui.painter()
                            .layout_no_wrap(word.to_owned(), font.clone(), INK)
                            .size()
                            .x
                    })
                    .into_iter()
                    .fold(0.0, f32::max);
                ui.allocate_ui_with_layout(
                    egui::vec2(slot, ui.available_height()),
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        ui.set_min_width(slot);
                        ui.label(
                            egui::RichText::new(if enabled { "Enabled" } else { "Disabled" })
                                .font(font)
                                .color(INK),
                        );
                    },
                );
                ui.add_space(10.0);

                if ui
                    .add(outlined_button("Reset"))
                    .on_hover_text("Restore the default bindings")
                    .clicked()
                {
                    self.config = Config::default();
                    self.capturing = None;
                    self.persist("Defaults restored".to_owned());
                }
                if self.tray.is_none() && ui.add(outlined_button("Quit")).clicked() {
                    self.quit(ui.ctx());
                }
            });
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let (text, color, dot) = if let Some(action) = self.capturing {
            (
                format!(
                    "Press the key for {} · click it again to cancel",
                    action.label()
                ),
                MUTED,
                CORAL,
            )
        } else if let Some(error) = &self.runtime_error {
            (error.replace('\n', "  "), CORAL, CORAL)
        } else if let Some(status) = &self.status {
            if status.is_error {
                (status.message.clone(), CORAL, CORAL)
            } else {
                (status.message.clone(), MUTED, INK)
            }
        } else {
            (
                "Hold Caps Lock and tap a bound key. Click a key to rebind it.".to_owned(),
                MUTED,
                FAINT,
            )
        };
        ui.horizontal_top(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 15.0), egui::Sense::hover());
            ui.painter().circle_filled(rect.center(), 4.0, dot);
            ui.add_space(2.0);
            ui.add(egui::Label::new(egui::RichText::new(text).size(12.0).color(color)).wrap());
        });
    }
}

impl eframe::App for NocapsApp {
    fn logic(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        if !std::mem::replace(&mut self.started, true) && !self.show_on_start {
            context.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }

        pump_native_tray_events();
        self.process_tray_events(context);
        self.capture_pressed_key(context);

        // Closing the window sends it back to the tray; only Quit exits.
        if context.input(|input| input.viewport().close_requested())
            && !self.quitting
            && self.tray.is_some()
        {
            context.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            context.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            self.capturing = None;
        }

        // Tray clicks wake the UI via the muda event handler, and egui already
        // repaints on its own window input, so no periodic repaint is needed to
        // stay responsive while idle. Linux is the exception: its AppIndicator
        // menu only emits events while the GTK loop is iterated by
        // `pump_native_tray_events`, so keep a gentle tick to drive that pump.
        #[cfg(target_os = "linux")]
        context.request_repaint_after(std::time::Duration::from_millis(100));
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let bar = || {
            egui::Frame::new()
                .fill(PAPER)
                .inner_margin(egui::Margin::symmetric(16, 10))
        };
        egui::Panel::top("nocaps-title")
            .frame(bar())
            .show_inside(ui, |ui| self.title_bar(ui));
        egui::Panel::bottom("nocaps-status")
            .frame(bar())
            .show_inside(ui, |ui| self.status_bar(ui));

        // Two columns hold all five groups at once — no scrolling.
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(PAPER)
                    .inner_margin(egui::Margin::symmetric(16, 14)),
            )
            .show_inside(ui, |ui| {
                ui.columns(2, |columns| {
                    self.navigation_block(&mut columns[0]);
                    self.category_block(&mut columns[0], "Modifiers");
                    self.category_block(&mut columns[1], "Editing");
                    self.category_block(&mut columns[1], "Volume");
                    self.category_block(&mut columns[1], "Media");
                });
            });
    }
}

/// A binding slot drawn as a keycap: solid ink when bound, a dashed outline when
/// empty, coral while waiting for a key press. Wide keys grow to fit their label.
fn keycap(ui: &mut egui::Ui, binding: Option<InputKey>, capturing: bool) -> egui::Response {
    let (text, font, color) = if capturing {
        ("press…", egui::FontId::proportional(11.0), CORAL)
    } else {
        (binding.map_or("", InputKey::label), bold(12.0), PAPER)
    };
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, color);
    let width = (galley.size().x + 18.0).max(52.0);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 21.0), egui::Sense::click());
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    if !ui.is_rect_visible(rect) {
        return response;
    }

    let painter = ui.painter();
    if capturing {
        painter.rect(
            rect,
            3,
            CORAL_SOFT,
            egui::Stroke::new(1.5_f32, CORAL),
            egui::StrokeKind::Inside,
        );
    } else if binding.is_some() {
        painter.rect_filled(rect, 3, if response.hovered() { INK_HOVER } else { INK });
    } else {
        let stroke = egui::Stroke::new(
            1.0_f32,
            if response.hovered() {
                MUTED
            } else {
                EMPTY_SLOT
            },
        );
        let rect = rect.shrink(0.5);
        let outline = [
            rect.left_top(),
            rect.right_top(),
            rect.right_bottom(),
            rect.left_bottom(),
            rect.left_top(),
        ];
        painter.extend(egui::Shape::dashed_line(&outline, stroke, 3.0, 3.0));
    }
    painter.galley(rect.center() - galley.size() / 2.0, galley, color);
    response
}

fn toggle_switch(ui: &mut egui::Ui, on: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(38.0, 22.0), egui::Sense::click());
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    if ui.is_rect_visible(rect) {
        let t = ui.ctx().animate_bool_responsive(response.id, on);
        let knob_x = egui::lerp(rect.left() + 11.0..=rect.right() - 11.0, t);
        let painter = ui.painter();
        painter.rect_filled(rect, 11, FAINT.lerp_to_gamma(CORAL, t));
        painter.circle_filled(
            egui::pos2(knob_x, rect.center().y),
            8.0,
            egui::Color32::WHITE,
        );
    }
    response
}

fn outlined_button(text: &str) -> egui::Button<'_> {
    egui::Button::new(egui::RichText::new(text).size(12.0).color(INK))
        .fill(CARD)
        .stroke(egui::Stroke::new(1.0_f32, INK))
        .min_size(egui::vec2(58.0, 24.0))
}

fn bold(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name(BOLD_FONT.into()))
}

fn configure_style(context: &egui::Context) {
    let mut fonts = egui::FontDefinitions::empty();
    fonts.font_data.insert(
        "inter".to_owned(),
        Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/Inter-Regular.otf"
        ))),
    );
    fonts.font_data.insert(
        BOLD_FONT.to_owned(),
        Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/Inter-Bold.otf"
        ))),
    );
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts.families.insert(family, vec!["inter".to_owned()]);
    }
    fonts.families.insert(
        egui::FontFamily::Name(BOLD_FONT.into()),
        vec![BOLD_FONT.to_owned()],
    );
    context.set_fonts(fonts);

    let mut visuals = egui::Visuals::light();
    visuals.override_text_color = Some(INK);
    visuals.window_fill = CARD;
    visuals.window_stroke = egui::Stroke::new(1.0_f32, RULE);
    visuals.panel_fill = PAPER;
    visuals.extreme_bg_color = CARD;
    visuals.faint_bg_color = CARD;
    visuals.warn_fg_color = CORAL;
    visuals.error_fg_color = CORAL;
    visuals.selection.bg_fill = CORAL_SOFT;
    visuals.selection.stroke = egui::Stroke::new(1.0_f32, CORAL);

    let corner = egui::CornerRadius::same(3);
    let widgets = &mut visuals.widgets;
    widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, RULE);
    widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0_f32, INK);
    for (state, fill) in [
        (&mut widgets.inactive, CARD),
        (&mut widgets.hovered, PAPER_HOVER),
        (&mut widgets.active, RULE),
        (&mut widgets.open, PAPER_HOVER),
    ] {
        state.corner_radius = corner;
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
        state.fg_stroke = egui::Stroke::new(1.0_f32, INK);
        state.expansion = 0.0;
    }
    widgets.inactive.bg_stroke = egui::Stroke::new(1.0_f32, RULE);
    widgets.hovered.bg_stroke = egui::Stroke::new(1.0_f32, INK);
    widgets.active.bg_stroke = egui::Stroke::new(1.0_f32, INK);
    context.set_visuals(visuals);

    let mut style = (*context.global_style()).clone();
    style.spacing.item_spacing = egui::vec2(6.0, 4.0);
    style.spacing.button_padding = egui::vec2(10.0, 4.0);
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

/// A tray utility has no Dock icon on macOS.
#[cfg(target_os = "macos")]
fn event_loop_builder() -> Option<eframe::EventLoopBuilderHook> {
    use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
    Some(Box::new(|builder| {
        builder.with_activation_policy(ActivationPolicy::Accessory);
    }))
}

#[cfg(not(target_os = "macos"))]
fn event_loop_builder() -> Option<eframe::EventLoopBuilderHook> {
    None
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
