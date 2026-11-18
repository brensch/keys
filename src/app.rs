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

/// Starts the tray utility. On Linux the settings window only exists while it is
/// open: Wayland cannot hide a window, and a hidden-but-alive window would still
/// sit in the dock. Elsewhere the window lives for the whole session and is hidden
/// while nocaps sits in the tray.
pub fn run(
    keyboard: Option<Arc<KeyboardManager>>,
    config: Config,
    store: ConfigStore,
    startup_error: Option<String>,
    inbox: Arc<Inbox>,
) -> Result<()> {
    let mut core = Core::new(keyboard, config, store, startup_error, inbox);
    run_platform(&mut core)
}

#[cfg(target_os = "linux")]
fn run_platform(core: &mut Core) -> Result<()> {
    if let Err(error) = install_desktop_entry() {
        log::warn!("could not install the nocaps desktop entry: {error:#}");
    }

    // Problems at startup need the user's attention, so open the window at once.
    let mut show = core.runtime_error.is_some();
    loop {
        if std::mem::take(&mut show) {
            open_window(core, true)?;
            // Without a tray there is no way back to the window, so closing it quits.
            if core.quitting || core.tray.is_none() {
                return Ok(());
            }
        }
        // Sleep in GTK until the tray menu (or a second launch of nocaps) wakes us.
        gtk::main_iteration();
        for command in core.inbox.take() {
            match command {
                Command::ToggleEnabled => core.toggle_enabled(),
                Command::Configure => show = true,
                Command::Quit => return Ok(()),
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn run_platform(core: &mut Core) -> Result<()> {
    // The window starts in the tray unless something needs the user's attention.
    let show_on_start = core.runtime_error.is_some() || core.tray.is_none();
    open_window(core, show_on_start)
}

fn open_window(core: &mut Core, visible: bool) -> Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("nocaps")
            // Matches nocaps.desktop, so docks and task switchers show the nocaps icon.
            .with_app_id("nocaps")
            .with_inner_size([560.0, 580.0])
            .with_min_inner_size([540.0, 520.0])
            .with_resizable(true)
            .with_visible(visible)
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
                core,
                visible,
                &creation_context.egui_ctx,
            )))
        }),
    )
    .map_err(|error| anyhow::anyhow!(error.to_string()))
}

/// Requests that reach the app from outside the settings window: the tray menu, and
/// on Linux a second launch of nocaps asking the running one to show itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    ToggleEnabled,
    Configure,
    Quit,
}

/// Tray activations arrive on muda's event handler, which may run on a native
/// callback rather than the egui frame, and single-instance requests arrive on their
/// own thread. Both park commands here and wake whichever loop is idle: the open
/// window's egui loop, or on Linux the GTK loop that runs while the window is closed.
#[derive(Default)]
pub struct Inbox {
    commands: Mutex<Vec<Command>>,
    window: Mutex<Option<egui::Context>>,
}

impl Inbox {
    pub fn push(&self, command: Command) {
        if let Ok(mut commands) = self.commands.lock() {
            commands.push(command);
        }
        if let Ok(window) = self.window.lock() {
            if let Some(context) = window.as_ref() {
                context.request_repaint();
            }
        }
        #[cfg(target_os = "linux")]
        gtk::glib::MainContext::default().wakeup();
    }

    fn take(&self) -> Vec<Command> {
        self.commands
            .lock()
            .map(|mut commands| std::mem::take(&mut *commands))
            .unwrap_or_default()
    }

    fn set_window(&self, context: Option<egui::Context>) {
        if let Ok(mut window) = self.window.lock() {
            *window = context;
        }
    }
}

struct Tray {
    _icon: TrayIcon,
    enabled: MenuItem,
}

impl Tray {
    fn new(remapping_enabled: bool, inbox: Arc<Inbox>) -> Result<Self> {
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

        let ids = [
            (enabled.id().clone(), Command::ToggleEnabled),
            (configure.id().clone(), Command::Configure),
            (quit.id().clone(), Command::Quit),
        ];
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if let Some((_, command)) = ids.iter().find(|(id, _)| *id == event.id) {
                inbox.push(*command);
            }
        }));

        Ok(Self {
            _icon: tray_icon,
            enabled,
        })
    }

    fn set_enabled(&self, enabled: bool) {
        self.enabled.set_text(enabled_menu_text(enabled));
    }
}

/// Everything that outlives a settings window: the keyboard hook, the configuration,
/// the tray icon and the last status message.
struct Core {
    keyboard: Option<Arc<KeyboardManager>>,
    config: Config,
    store: ConfigStore,
    tray: Option<Tray>,
    inbox: Arc<Inbox>,
    runtime_error: Option<String>,
    status: Option<Status>,
    quitting: bool,
}

struct Status {
    is_error: bool,
    message: String,
}

impl Core {
    fn new(
        keyboard: Option<Arc<KeyboardManager>>,
        config: Config,
        store: ConfigStore,
        startup_error: Option<String>,
        inbox: Arc<Inbox>,
    ) -> Self {
        let (tray, tray_error) = match create_tray(config.enabled, Arc::clone(&inbox)) {
            Ok(tray) => (Some(tray), None),
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
            tray,
            inbox,
            runtime_error: (!errors.is_empty()).then(|| errors.join("\n")),
            status: None,
            quitting: false,
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
}

/// The settings window: a view over [`Core`] that lives only as long as the window.
struct NocapsApp<'a> {
    core: &'a mut Core,
    /// eframe always shows the window after its first frame, so a window meant to
    /// start in the tray is hidden again from `logic` on that frame.
    hide_on_start: bool,
    started: bool,
    icon: egui::TextureHandle,
    capturing: Option<Action>,
}

impl<'a> NocapsApp<'a> {
    fn new(core: &'a mut Core, visible: bool, context: &egui::Context) -> Self {
        core.inbox.set_window(Some(context.clone()));
        // A status from an earlier window session would be stale by now.
        core.status = None;
        Self {
            core,
            hide_on_start: !visible,
            started: false,
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

    fn process_commands(&mut self, context: &egui::Context) {
        for command in self.core.inbox.take() {
            match command {
                Command::ToggleEnabled => self.core.toggle_enabled(),
                Command::Configure => {
                    context.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    context.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                Command::Quit => self.quit(context),
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
                self.core.config.bind(action, key);
                self.capturing = None;
                self.core.persist(format!(
                    "{} is now Caps Lock + {}",
                    action.label(),
                    key.label()
                ));
            }
            None => {
                self.core.status = Some(Status {
                    is_error: true,
                    message: "That key cannot be used as a binding.".to_owned(),
                });
            }
        }
    }

    fn quit(&mut self, context: &egui::Context) {
        self.core.quitting = true;
        context.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    fn action_row(&mut self, ui: &mut egui::Ui, action: Action) {
        let capturing = self.capturing == Some(action);
        let binding = self.core.config.key_for(action);
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
                    self.core.config.unbind(action);
                    self.capturing = None;
                    self.core.persist(format!("{} cleared", action.label()));
                }

                if keycap(ui, binding, capturing).clicked() {
                    if capturing {
                        self.capturing = None;
                    } else {
                        self.capturing = Some(action);
                        self.core.status = None;
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
            .on_hover_text(format!("Config: {}", self.core.store.path().display()));

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let enabled = self.core.config.enabled;
                if toggle_switch(ui, enabled)
                    .on_hover_text("Turn Caps Lock remapping on or off")
                    .clicked()
                {
                    self.core.toggle_enabled();
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
                    self.core.config = Config::default();
                    self.capturing = None;
                    self.core.persist("Defaults restored".to_owned());
                }
                if self.core.tray.is_none() && ui.add(outlined_button("Quit")).clicked() {
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
        } else if let Some(error) = &self.core.runtime_error {
            (error.replace('\n', "  "), CORAL, CORAL)
        } else if let Some(status) = &self.core.status {
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

impl Drop for NocapsApp<'_> {
    fn drop(&mut self) {
        self.core.inbox.set_window(None);
    }
}

impl eframe::App for NocapsApp<'_> {
    fn logic(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        if !std::mem::replace(&mut self.started, true) && self.hide_on_start {
            context.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }

        pump_native_tray_events();
        self.process_commands(context);
        self.capture_pressed_key(context);

        // Closing the window sends it back to the tray; only Quit exits. On Linux the
        // window really closes, which returns control to the tray loop in `run`.
        if context.input(|input| input.viewport().close_requested()) {
            self.capturing = None;
            if cfg!(not(target_os = "linux")) && !self.core.quitting && self.core.tray.is_some() {
                context.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                context.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            }
        }

        // Tray clicks wake the UI through the inbox, and egui already repaints on its
        // own window input, so no periodic repaint is needed to stay responsive while
        // idle. Linux is the exception: its AppIndicator menu only emits events while
        // the GTK loop is iterated by `pump_native_tray_events`, so keep a gentle tick
        // to drive that pump while the window is open.
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
fn create_tray(enabled: bool, inbox: Arc<Inbox>) -> Result<Tray> {
    let appindicator_available = ["libayatana-appindicator3.so.1", "libappindicator3.so.1"]
        .iter()
        .any(|name| unsafe { libloading::Library::new(name).is_ok() });
    if !appindicator_available {
        return Err(anyhow::anyhow!(
            "install the Ayatana AppIndicator 3 runtime library"
        ));
    }
    std::panic::catch_unwind(|| Tray::new(enabled, inbox))
        .map_err(|_| anyhow::anyhow!("create Linux tray icon"))?
}

#[cfg(not(target_os = "linux"))]
fn create_tray(enabled: bool, inbox: Arc<Inbox>) -> Result<Tray> {
    Tray::new(enabled, inbox)
}

/// GNOME and other docks pick a window's icon by matching its app id to a desktop
/// entry; without one, the settings window shows up as a generic cog. The entry also
/// lists nocaps in the app grid, where launching it again opens the running
/// instance's window. Rewritten only when its content changes, e.g. after the binary
/// moves.
#[cfg(target_os = "linux")]
fn install_desktop_entry() -> Result<()> {
    const ICON_PNG: &[u8] = include_bytes!("../assets/nocaps-icon.png");

    let data_dir = directories::BaseDirs::new()
        .context("find the home directory")?
        .data_dir()
        .to_owned();
    let icon_path = data_dir.join("nocaps").join("nocaps.png");
    let entry_path = data_dir.join("applications").join("nocaps.desktop");
    let exe = std::env::current_exe().context("find the nocaps executable")?;

    let entry = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=nocaps\n\
         Comment=Hold Caps Lock for navigation, editing and media keys\n\
         Exec=\"{}\"\n\
         Icon={}\n\
         Terminal=false\n\
         Categories=Utility;\n\
         StartupWMClass=nocaps\n",
        exe.display(),
        icon_path.display()
    );

    write_if_changed(&icon_path, ICON_PNG)?;
    write_if_changed(&entry_path, entry.as_bytes())
}

#[cfg(target_os = "linux")]
fn write_if_changed(path: &std::path::Path, content: &[u8]) -> Result<()> {
    if std::fs::read(path).is_ok_and(|existing| existing == content) {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    std::fs::write(path, content).with_context(|| format!("write {}", path.display()))
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
