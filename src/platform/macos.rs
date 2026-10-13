//! macOS backend.
//!
//! macOS toggles the Caps Lock state inside the HID driver, below every event tap, so
//! swallowing the Caps Lock event cannot stop it from latching. Instead, while remapping
//! is enabled, `hidutil` remaps Caps Lock to F18 at the HID level. F18 produces ordinary
//! key-down/key-up events, which act as the layer key, and Caps Lock never toggles.
//!
//! Layered keys are synthesized from a `CGEventTap` callback and posted directly after
//! the tap. Modifier actions are applied by adding their flags to every keyboard and
//! mouse event that passes through the tap, because macOS does not merge synthesized
//! modifier state into events from physical keys.
//!
//! Layered keys mean on macOS what they mean on Windows. The Control action is a word
//! modifier here: Caps Lock + A + J sends Option+Left (word left) rather than Ctrl+Left,
//! which macOS uses to switch Spaces. Home and End become the Command-arrow line jumps.

use crate::config::{Action, InputKey, BINDINGS};
use anyhow::{anyhow, Context, Result};
use core_foundation::base::TCFType;
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::mach_port::{CFMachPort, CFMachPortRef};
use core_foundation::runloop::{kCFRunLoopCommonModes, CFRunLoop};
use core_foundation::string::{CFString, CFStringRef};
use core_graphics::event::{
    CGEvent, CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
    CGEventTapProxy, CGEventType, CGKeyCode, CallbackResult, EventField,
};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use enigo::{Direction, Enigo, Key as EnigoKey, Keyboard, Settings};
use log::{debug, error, info, warn};
use std::cell::{OnceCell, RefCell};
use std::process::Command;
use std::rc::Rc;
use std::sync::{mpsc, Mutex, OnceLock};
use std::thread;

/// Marks events nocaps synthesizes so the tap never processes its own output.
const SYNTHETIC_TAG: i64 = 0x6e6f_6361_7073; // "nocaps"

const HID_CAPS_LOCK: u64 = 0x7_0000_0039;
const HID_F18: u64 = 0x7_0000_006D;
const KEYCODE_F18: CGKeyCode = 0x4F;

// Device-dependent bits identify the left-hand modifier, as a physical key press does.
const LEFT_SHIFT_FLAGS: u64 = CGEventFlags::CGEventFlagShift.bits() | 0x0000_0002;
const LEFT_COMMAND_FLAGS: u64 = CGEventFlags::CGEventFlagCommand.bits() | 0x0000_0008;
const LEFT_OPTION_FLAGS: u64 = CGEventFlags::CGEventFlagAlternate.bits() | 0x0000_0020;
const MODIFIER_FLAGS: u64 = CGEventFlags::CGEventFlagShift.bits()
    | CGEventFlags::CGEventFlagControl.bits()
    | CGEventFlags::CGEventFlagAlternate.bits()
    | CGEventFlags::CGEventFlagCommand.bits();
const KEY_SPECIFIC_FLAGS: u64 =
    CGEventFlags::CGEventFlagNumericPad.bits() | CGEventFlags::CGEventFlagSecondaryFn.bits();

pub struct KeyboardManager {
    remap: Mutex<CapsLockRemap>,
}

impl KeyboardManager {
    pub fn new() -> Result<Self> {
        request_accessibility_access();

        let enabled = BINDINGS.is_enabled();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name("nocaps-macos-input".to_owned())
            .spawn(move || run_tap_thread(ready_tx))
            .context("start macOS keyboard worker")?;
        ready_rx
            .recv()
            .map_err(|_| anyhow!("macOS keyboard worker stopped during startup"))?
            .map_err(|message| anyhow!(message))?;

        // Only take over Caps Lock once the tap is live; otherwise it would become a
        // dead F18 key with nothing listening for it.
        let mut remap = CapsLockRemap::new().context("read the macOS key mapping")?;
        install_termination_handler();
        remap
            .set_active(enabled)
            .context("remap Caps Lock with hidutil")?;
        info!("macOS keyboard tap installed");
        Ok(Self {
            remap: Mutex::new(remap),
        })
    }

    /// Caps Lock behaves natively while remapping is disabled.
    pub fn set_enabled(&self, enabled: bool) -> Result<()> {
        self.remap
            .lock()
            .map_err(|_| anyhow!("Caps Lock remap lock is poisoned"))?
            .set_active(enabled)
    }
}

impl Drop for KeyboardManager {
    fn drop(&mut self) {
        if let Ok(mut remap) = self.remap.lock() {
            if let Err(error) = remap.set_active(false) {
                error!("could not restore Caps Lock: {error:#}");
            }
        }
    }
}

fn run_tap_thread(ready: mpsc::SyncSender<Result<(), String>>) {
    super::elevate_input_thread();

    let state = match TapState::new() {
        Ok(state) => RefCell::new(state),
        Err(error) => {
            let _ = ready.send(Err(format!("{error:#}")));
            return;
        }
    };
    let port: Rc<OnceCell<CFMachPort>> = Rc::new(OnceCell::new());
    let callback_port = Rc::clone(&port);
    let callback = move |proxy: CGEventTapProxy, event_type: CGEventType, event: &CGEvent| {
        if matches!(
            event_type,
            CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput
        ) {
            warn!("macOS disabled the keyboard tap; re-enabling it");
            if let Some(port) = callback_port.get() {
                unsafe { CGEventTapEnable(port.as_concrete_TypeRef(), true) };
            }
            return CallbackResult::Keep;
        }
        match state.try_borrow_mut() {
            Ok(mut state) => state.handle(proxy, event_type, event),
            Err(_) => CallbackResult::Keep,
        }
    };

    // SAFETY: the tap is only added to this thread's run loop and lives until the run
    // loop returns, so the non-`Send` callback state is only touched on this thread.
    let tap = unsafe {
        CGEventTap::new_unchecked(
            CGEventTapLocation::HID,
            CGEventTapPlacement::HeadInsertEventTap,
            CGEventTapOptions::Default,
            vec![
                CGEventType::KeyDown,
                CGEventType::KeyUp,
                CGEventType::FlagsChanged,
                CGEventType::LeftMouseDown,
                CGEventType::LeftMouseUp,
                CGEventType::RightMouseDown,
                CGEventType::RightMouseUp,
                CGEventType::OtherMouseDown,
                CGEventType::OtherMouseUp,
            ],
            callback,
        )
    };
    let Ok(tap) = tap else {
        let _ = ready.send(Err(
            "grant nocaps access in System Settings → Privacy & Security → \
             Accessibility, then restart it"
                .to_owned(),
        ));
        return;
    };
    let Ok(source) = tap.mach_port().create_runloop_source(0) else {
        let _ = ready.send(Err(
            "create the macOS keyboard tap run loop source".to_owned()
        ));
        return;
    };
    let _ = port.set(tap.mach_port().clone());
    CFRunLoop::get_current().add_source(&source, unsafe { kCFRunLoopCommonModes });
    tap.enable();

    if ready.send(Ok(())).is_err() {
        return;
    }
    CFRunLoop::run_current();
    error!("macOS keyboard tap stopped");
}

#[derive(Clone, Copy)]
enum Held {
    /// A layered key, translated once when pressed so its repeats and its release send
    /// the same keystroke even if the word modifier changes in between.
    Key { code: CGKeyCode, flags: u64 },
    /// A modifier, word or media action whose release must release it.
    Action(Action),
    /// The action was already released when the layer key went up; swallow the rest of
    /// this physical press so the application never sees an unmatched key-up.
    Released,
}

struct TapState {
    source: CGEventSource,
    enigo: Enigo,
    layer_held: bool,
    /// Caps Lock + the Control action's key is held: movement and deletion go by words.
    word_held: bool,
    layer_flags: u64,
    held: [Option<Held>; InputKey::COUNT],
    /// Number of `Some` entries in `held`, so the idle fast path needs no scan.
    held_count: u32,
}

impl TapState {
    fn new() -> Result<Self> {
        let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
            .map_err(|_| anyhow!("create the macOS event source"))?;
        let enigo = Enigo::new(&Settings {
            event_source_user_data: Some(SYNTHETIC_TAG),
            ..Settings::default()
        })
        .map_err(|error| anyhow!("initialize macOS media key injection: {error}"))?;
        Ok(Self {
            source,
            enigo,
            layer_held: false,
            word_held: false,
            layer_flags: 0,
            held: [None; InputKey::COUNT],
            held_count: 0,
        })
    }

    fn handle(
        &mut self,
        proxy: CGEventTapProxy,
        event_type: CGEventType,
        event: &CGEvent,
    ) -> CallbackResult {
        if log::log_enabled!(log::Level::Debug) {
            log_event("seen", event_type, event);
        }
        let down = match event_type {
            CGEventType::KeyDown => true,
            CGEventType::KeyUp => false,
            // Modifier and mouse events only need work while a layer modifier is held.
            _ => return self.pass_through(event),
        };

        // Idle fast path, taken by nearly every keystroke: one field read and the event
        // is handed straight back.
        let code = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as CGKeyCode;
        if code != KEYCODE_F18 && !self.layer_held && self.held_count == 0 && self.layer_flags == 0
        {
            return CallbackResult::Keep;
        }
        if event.get_integer_value_field(EventField::EVENT_SOURCE_USER_DATA) == SYNTHETIC_TAG {
            return CallbackResult::Keep;
        }
        self.key(proxy, event, code, down)
    }

    fn key(
        &mut self,
        proxy: CGEventTapProxy,
        event: &CGEvent,
        code: CGKeyCode,
        down: bool,
    ) -> CallbackResult {
        let repeat = event.get_integer_value_field(EventField::KEYBOARD_EVENT_AUTOREPEAT) != 0;
        let flags = event.get_flags().bits();

        if code == KEYCODE_F18 && (self.layer_held || BINDINGS.is_enabled()) {
            if down {
                self.layer_held = true;
            } else {
                self.layer_held = false;
                self.release_all(proxy, flags);
            }
            return CallbackResult::Drop;
        }

        let Some(key) = input_key_from_mac(code) else {
            return self.pass_through(event);
        };
        let slot = &mut self.held[key.index()];
        match (*slot, down) {
            (Some(Held::Key { code, flags: extra }), true) => {
                self.post_layered_key(proxy, code, extra, true, repeat, flags);
                CallbackResult::Drop
            }
            (Some(Held::Key { code, flags: extra }), false) => {
                *slot = None;
                self.held_count -= 1;
                self.post_layered_key(proxy, code, extra, false, false, flags);
                CallbackResult::Drop
            }
            (Some(Held::Action(action)), true) => {
                self.emit(proxy, action, true, repeat, flags);
                CallbackResult::Drop
            }
            (Some(Held::Action(action)), false) => {
                *slot = None;
                self.held_count -= 1;
                self.emit(proxy, action, false, false, flags);
                CallbackResult::Drop
            }
            (Some(Held::Released), _) => {
                if !down {
                    *slot = None;
                    self.held_count -= 1;
                }
                CallbackResult::Drop
            }
            (None, true) if self.layer_held && !repeat => match BINDINGS.action_for(key) {
                Some(action) => {
                    self.held_count += 1;
                    if let MacOutput::Key(code) = mac_output(action) {
                        let (code, extra) = mac_keystroke(code, self.word_held);
                        *slot = Some(Held::Key { code, flags: extra });
                        self.post_layered_key(proxy, code, extra, true, false, flags);
                    } else {
                        *slot = Some(Held::Action(action));
                        self.emit(proxy, action, true, false, flags);
                    }
                    CallbackResult::Drop
                }
                None => self.pass_through(event),
            },
            (None, _) => self.pass_through(event),
        }
    }

    /// Physical events keep flowing while a layer modifier is held; add its flags so
    /// that, for example, Caps Lock + S followed by a letter types a capital.
    fn pass_through(&self, event: &CGEvent) -> CallbackResult {
        if self.layer_flags != 0 {
            let flags = event.get_flags().bits() | self.layer_flags;
            event.set_flags(CGEventFlags::from_bits_retain(flags));
        }
        CallbackResult::Keep
    }

    fn release_all(&mut self, proxy: CGEventTapProxy, flags: u64) {
        for index in 0..InputKey::COUNT {
            match self.held[index] {
                Some(Held::Key { code, flags: extra }) => {
                    self.held[index] = Some(Held::Released);
                    self.post_layered_key(proxy, code, extra, false, false, flags);
                }
                Some(Held::Action(action)) => {
                    self.held[index] = Some(Held::Released);
                    self.emit(proxy, action, false, false, flags);
                }
                Some(Held::Released) | None => {}
            }
        }
    }

    fn emit(
        &mut self,
        proxy: CGEventTapProxy,
        action: Action,
        down: bool,
        repeat: bool,
        physical_flags: u64,
    ) {
        match mac_output(action) {
            MacOutput::Key(code) => {
                self.post_layered_key(proxy, code, 0, down, repeat, physical_flags);
            }
            MacOutput::Word => {
                if !repeat {
                    self.word_held = down;
                }
            }
            MacOutput::Modifier(code, modifier_flags) => {
                if repeat {
                    return;
                }
                if down {
                    self.layer_flags |= modifier_flags;
                } else {
                    self.layer_flags &= !modifier_flags;
                }
                let flags = (physical_flags & !KEY_SPECIFIC_FLAGS) | self.layer_flags;
                self.post_key(proxy, code, down, false, CGEventType::FlagsChanged, flags);
            }
            MacOutput::Media(key) => {
                // Media keys are single system events; volume keys repeat while held.
                let repeats = matches!(key, EnigoKey::VolumeUp | EnigoKey::VolumeDown);
                if down && (!repeat || repeats) {
                    if let Err(error) = self.enigo.key(key, Direction::Click) {
                        error!("could not send macOS media key: {error}");
                    }
                }
            }
        }
    }

    /// Sends a layered key with the physical modifiers, the layer's modifiers, the
    /// translation's own modifiers, and the flags macOS attaches to that key.
    fn post_layered_key(
        &self,
        proxy: CGEventTapProxy,
        code: CGKeyCode,
        extra_flags: u64,
        down: bool,
        repeat: bool,
        physical_flags: u64,
    ) {
        let base = (physical_flags & !KEY_SPECIFIC_FLAGS) | self.layer_flags;
        // A real keyboard announces a modifier with its own press and release around the
        // key it modifies, and some apps track modifiers from those announcements rather
        // than from the key event's flags. Send the same sequence, unless that modifier
        // is already physically held.
        let announced = (base & extra_flags & MODIFIER_FLAGS == 0)
            .then(|| modifier_keycode(extra_flags))
            .flatten();
        if let (Some(modifier), true, false) = (announced, down, repeat) {
            self.post_key(
                proxy,
                modifier,
                true,
                false,
                CGEventType::FlagsChanged,
                base | extra_flags,
            );
        }
        let flags = base | extra_flags | key_specific_flags(code);
        self.post_key(proxy, code, down, repeat, CGEventType::KeyDown, flags);
        if let (Some(modifier), false) = (announced, down) {
            self.post_key(
                proxy,
                modifier,
                false,
                false,
                CGEventType::FlagsChanged,
                base,
            );
        }
    }

    fn post_key(
        &self,
        proxy: CGEventTapProxy,
        code: CGKeyCode,
        down: bool,
        repeat: bool,
        event_type: CGEventType,
        flags: u64,
    ) {
        let Ok(event) = CGEvent::new_keyboard_event(self.source.clone(), code, down) else {
            error!("could not create macOS key event");
            return;
        };
        if matches!(event_type, CGEventType::FlagsChanged) {
            event.set_type(CGEventType::FlagsChanged);
        }
        event.set_flags(CGEventFlags::from_bits_retain(flags));
        event.set_integer_value_field(EventField::KEYBOARD_EVENT_AUTOREPEAT, i64::from(repeat));
        event.set_integer_value_field(EventField::EVENT_SOURCE_USER_DATA, SYNTHETIC_TAG);
        if log::log_enabled!(log::Level::Debug) {
            log_event("sent", event.get_type(), &event);
        }
        event.post_from_tap(proxy);
    }
}

/// Debug trace for diagnosing how apps see keys. Only navigation, editing and
/// modifier keys are logged, never characters, so the log cannot capture typed text.
fn log_event(direction: &str, event_type: CGEventType, event: &CGEvent) {
    let code = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as CGKeyCode;
    let kind = match event_type {
        CGEventType::KeyDown => "down",
        CGEventType::KeyUp => "up",
        CGEventType::FlagsChanged => "flags",
        _ => return,
    };
    let loggable = matches!(
        code,
        KEY_BACKSPACE | KEY_FORWARD_DELETE | KEY_HOME | KEY_END | KEY_PAGE_UP | KEY_PAGE_DOWN
    ) || (KEY_LEFT..=KEY_UP).contains(&code)
        || (0x36..=0x3F).contains(&code)
        || code == KEYCODE_F18;
    if loggable {
        debug!(
            "{direction} {kind:<5} key={code:#04x} flags={:#010x} repeat={} source={}",
            event.get_flags().bits(),
            event.get_integer_value_field(EventField::KEYBOARD_EVENT_AUTOREPEAT),
            event.get_integer_value_field(EventField::EVENT_SOURCE_STATE_ID),
        );
    }
}

/// The modifier key whose press announces `flags`, if the translation adds one.
fn modifier_keycode(flags: u64) -> Option<CGKeyCode> {
    match flags {
        LEFT_OPTION_FLAGS => Some(0x3A),
        LEFT_COMMAND_FLAGS => Some(0x37),
        _ => None,
    }
}

/// Snapshot of the user's `hidutil` key mapping, so nocaps can add its Caps Lock entry
/// and later put back exactly what was there before.
struct CapsLockRemap {
    original: Vec<(u64, u64)>,
    active: bool,
}

static ORIGINAL_MAPPING: OnceLock<Vec<(u64, u64)>> = OnceLock::new();

impl CapsLockRemap {
    fn new() -> Result<Self> {
        // A previous run that was killed may have left the Caps Lock → F18 entry behind.
        let original: Vec<_> = read_hid_mapping()?
            .into_iter()
            .filter(|&mapping| mapping != (HID_CAPS_LOCK, HID_F18))
            .collect();
        let _ = ORIGINAL_MAPPING.set(original.clone());
        Ok(Self {
            original,
            active: false,
        })
    }

    fn set_active(&mut self, active: bool) -> Result<()> {
        if self.active == active {
            return Ok(());
        }
        if active {
            let mut mapping: Vec<_> = self
                .original
                .iter()
                .copied()
                .filter(|&(source, _)| source != HID_CAPS_LOCK)
                .collect();
            mapping.push((HID_CAPS_LOCK, HID_F18));
            write_hid_mapping(&mapping)?;
        } else {
            write_hid_mapping(&self.original)?;
        }
        self.active = active;
        Ok(())
    }
}

fn read_hid_mapping() -> Result<Vec<(u64, u64)>> {
    let output = Command::new("/usr/bin/hidutil")
        .args(["property", "--get", "UserKeyMapping"])
        .output()
        .context("run hidutil")?;
    if !output.status.success() {
        return Err(anyhow!("hidutil exited with {}", output.status));
    }
    Ok(parse_hid_mapping(&String::from_utf8_lossy(&output.stdout)))
}

/// Parses `hidutil property --get UserKeyMapping`, which prints `(null)` or a
/// property-list array of `{ HIDKeyboardModifierMappingDst = n; ...Src = n; }`.
fn parse_hid_mapping(text: &str) -> Vec<(u64, u64)> {
    let field = |entry: &str, name: &str| -> Option<u64> {
        let rest = &entry[entry.find(name)? + name.len()..];
        let value = rest.trim_start().strip_prefix('=')?.trim_start();
        let end = value
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(value.len());
        value[..end].parse().ok()
    };
    text.split('{')
        .skip(1)
        .filter_map(|entry| {
            Some((
                field(entry, "HIDKeyboardModifierMappingSrc")?,
                field(entry, "HIDKeyboardModifierMappingDst")?,
            ))
        })
        .collect()
}

fn write_hid_mapping(mapping: &[(u64, u64)]) -> Result<()> {
    let entries: Vec<String> = mapping
        .iter()
        .map(|(source, destination)| {
            format!(
                r#"{{"HIDKeyboardModifierMappingSrc":{source},"HIDKeyboardModifierMappingDst":{destination}}}"#
            )
        })
        .collect();
    let property = format!(r#"{{"UserKeyMapping":[{}]}}"#, entries.join(","));
    let output = Command::new("/usr/bin/hidutil")
        .args(["property", "--set", &property])
        .output()
        .context("run hidutil")?;
    if !output.status.success() {
        return Err(anyhow!("hidutil exited with {}", output.status));
    }
    Ok(())
}

/// The Caps Lock → F18 mapping outlives the process, so restore the user's mapping
/// when nocaps is interrupted or terminated instead of leaving Caps Lock dead.
fn install_termination_handler() {
    let result = ctrlc::set_handler(|| {
        if let Some(original) = ORIGINAL_MAPPING.get() {
            if let Err(error) = write_hid_mapping(original) {
                error!("could not restore Caps Lock: {error:#}");
            }
        }
        std::process::exit(130);
    });
    if let Err(error) = result {
        warn!("could not install the termination handler: {error}");
    }
}

/// Shows the system Accessibility prompt the first time nocaps runs.
fn request_accessibility_access() {
    let prompt = unsafe { CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt) };
    let options = CFDictionary::from_CFType_pairs(&[(prompt, CFBoolean::true_value())]);
    if !unsafe { AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef()) } {
        warn!("nocaps does not have Accessibility access yet");
    }
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    static kAXTrustedCheckOptionPrompt: CFStringRef;
    fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> bool;
    fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
}

// macOS virtual key codes (`kVK_*`) for the keys nocaps sends.
const KEY_RETURN: CGKeyCode = 0x24;
const KEY_TAB: CGKeyCode = 0x30;
const KEY_SPACE: CGKeyCode = 0x31;
const KEY_BACKSPACE: CGKeyCode = 0x33;
const KEY_ESCAPE: CGKeyCode = 0x35;
const KEY_HOME: CGKeyCode = 0x73;
const KEY_PAGE_UP: CGKeyCode = 0x74;
const KEY_FORWARD_DELETE: CGKeyCode = 0x75;
const KEY_END: CGKeyCode = 0x77;
const KEY_PAGE_DOWN: CGKeyCode = 0x79;
const KEY_LEFT: CGKeyCode = 0x7B;
const KEY_RIGHT: CGKeyCode = 0x7C;
const KEY_DOWN: CGKeyCode = 0x7D;
const KEY_UP: CGKeyCode = 0x7E;

/// Translates a layered key into the Mac keystroke that does what the same combination
/// does on Windows, returning the key and the modifiers to add.
fn mac_keystroke(code: CGKeyCode, word: bool) -> (CGKeyCode, u64) {
    match (code, word) {
        // Line start/end, and document start/end with the word modifier.
        (KEY_HOME, false) => (KEY_LEFT, LEFT_COMMAND_FLAGS),
        (KEY_END, false) => (KEY_RIGHT, LEFT_COMMAND_FLAGS),
        (KEY_HOME, true) => (KEY_UP, LEFT_COMMAND_FLAGS),
        (KEY_END, true) => (KEY_DOWN, LEFT_COMMAND_FLAGS),
        // Move or delete by word (left/right) or by paragraph (up/down).
        (KEY_LEFT | KEY_RIGHT | KEY_UP | KEY_DOWN | KEY_BACKSPACE | KEY_FORWARD_DELETE, true) => {
            (code, LEFT_OPTION_FLAGS)
        }
        _ => (code, 0),
    }
}

enum MacOutput {
    Key(CGKeyCode),
    /// Changes how layered movement keys are translated; sends nothing by itself.
    Word,
    Modifier(CGKeyCode, u64),
    Media(EnigoKey),
}

fn mac_output(action: Action) -> MacOutput {
    use MacOutput::{Key, Media, Modifier, Word};
    match action {
        Action::LeftControl => Word,
        Action::LeftShift => Modifier(0x38, LEFT_SHIFT_FLAGS),
        Action::LeftAlt => Modifier(0x3A, LEFT_OPTION_FLAGS),
        Action::LeftMeta => Modifier(0x37, LEFT_COMMAND_FLAGS),
        Action::ArrowUp => Key(KEY_UP),
        Action::ArrowDown => Key(KEY_DOWN),
        Action::ArrowLeft => Key(KEY_LEFT),
        Action::ArrowRight => Key(KEY_RIGHT),
        Action::Home => Key(KEY_HOME),
        Action::End => Key(KEY_END),
        Action::PageUp => Key(KEY_PAGE_UP),
        Action::PageDown => Key(KEY_PAGE_DOWN),
        Action::Backspace => Key(KEY_BACKSPACE),
        Action::Delete => Key(KEY_FORWARD_DELETE),
        Action::Enter => Key(KEY_RETURN),
        Action::Escape => Key(KEY_ESCAPE),
        Action::Tab => Key(KEY_TAB),
        Action::Space => Key(KEY_SPACE),
        Action::VolumeUp => Media(EnigoKey::VolumeUp),
        Action::VolumeDown => Media(EnigoKey::VolumeDown),
        Action::VolumeMute => Media(EnigoKey::VolumeMute),
        Action::MediaPrevious => Media(EnigoKey::MediaPrevTrack),
        Action::MediaPlayPause => Media(EnigoKey::MediaPlayPause),
        Action::MediaNext => Media(EnigoKey::MediaNextTrack),
    }
}

/// Flags macOS attaches to these keys when they are pressed physically; some
/// applications only recognize arrow and navigation keys that carry them.
fn key_specific_flags(code: CGKeyCode) -> u64 {
    match code {
        KEY_LEFT | KEY_RIGHT | KEY_DOWN | KEY_UP => KEY_SPECIFIC_FLAGS,
        KEY_HOME | KEY_PAGE_UP | KEY_FORWARD_DELETE | KEY_END | KEY_PAGE_DOWN => {
            CGEventFlags::CGEventFlagSecondaryFn.bits()
        }
        _ => 0,
    }
}

/// Maps macOS virtual key codes (`kVK_*`) to physical keys. Keypad keys share the
/// binding of the matching main-keyboard key, as on Windows.
fn input_key_from_mac(code: CGKeyCode) -> Option<InputKey> {
    Some(match code {
        0x00 => InputKey::A,
        0x0B => InputKey::B,
        0x08 => InputKey::C,
        0x02 => InputKey::D,
        0x0E => InputKey::E,
        0x03 => InputKey::F,
        0x05 => InputKey::G,
        0x04 => InputKey::H,
        0x22 => InputKey::I,
        0x26 => InputKey::J,
        0x28 => InputKey::K,
        0x25 => InputKey::L,
        0x2E => InputKey::M,
        0x2D => InputKey::N,
        0x1F => InputKey::O,
        0x23 => InputKey::P,
        0x0C => InputKey::Q,
        0x0F => InputKey::R,
        0x01 => InputKey::S,
        0x11 => InputKey::T,
        0x20 => InputKey::U,
        0x09 => InputKey::V,
        0x0D => InputKey::W,
        0x07 => InputKey::X,
        0x10 => InputKey::Y,
        0x06 => InputKey::Z,
        0x1D | 0x52 => InputKey::Digit0,
        0x12 | 0x53 => InputKey::Digit1,
        0x13 | 0x54 => InputKey::Digit2,
        0x14 | 0x55 => InputKey::Digit3,
        0x15 | 0x56 => InputKey::Digit4,
        0x17 | 0x57 => InputKey::Digit5,
        0x16 | 0x58 => InputKey::Digit6,
        0x1A | 0x59 => InputKey::Digit7,
        0x1C | 0x5B => InputKey::Digit8,
        0x19 | 0x5C => InputKey::Digit9,
        0x32 => InputKey::Backquote,
        0x1B | 0x4E => InputKey::Minus,
        0x18 | 0x45 | 0x51 => InputKey::Equal,
        0x21 => InputKey::LeftBracket,
        0x1E => InputKey::RightBracket,
        0x2A => InputKey::Backslash,
        0x29 => InputKey::Semicolon,
        0x27 => InputKey::Quote,
        0x2B => InputKey::Comma,
        0x2F | 0x41 => InputKey::Period,
        0x2C | 0x4B => InputKey::Slash,
        0x30 => InputKey::Tab,
        0x31 => InputKey::Space,
        0x24 | 0x4C => InputKey::Enter,
        0x35 => InputKey::Escape,
        0x33 => InputKey::Backspace,
        0x75 => InputKey::Delete,
        0x72 => InputKey::Insert,
        0x73 => InputKey::Home,
        0x77 => InputKey::End,
        0x74 => InputKey::PageUp,
        0x79 => InputKey::PageDown,
        0x7E => InputKey::ArrowUp,
        0x7D => InputKey::ArrowDown,
        0x7B => InputKey::ArrowLeft,
        0x7C => InputKey::ArrowRight,
        0x7A => InputKey::F1,
        0x78 => InputKey::F2,
        0x63 => InputKey::F3,
        0x76 => InputKey::F4,
        0x60 => InputKey::F5,
        0x61 => InputKey::F6,
        0x62 => InputKey::F7,
        0x64 => InputKey::F8,
        0x65 => InputKey::F9,
        0x6D => InputKey::F10,
        0x67 => InputKey::F11,
        0x6F => InputKey::F12,
        0x69 => InputKey::F13,
        0x6B => InputKey::F14,
        0x71 => InputKey::F15,
        0x6A => InputKey::F16,
        0x40 => InputKey::F17,
        // F18 is the layer key.
        0x50 => InputKey::F19,
        0x5A => InputKey::F20,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hidutil_mapping_output() {
        let output = "(\n    {\n        HIDKeyboardModifierMappingDst = 30064771181;\n        \
                      HIDKeyboardModifierMappingSrc = 30064771129;\n    },\n    {\n        \
                      HIDKeyboardModifierMappingDst = 30064771113;\n        \
                      HIDKeyboardModifierMappingSrc = 30064771300;\n    }\n)\n";
        assert_eq!(
            parse_hid_mapping(output),
            vec![(HID_CAPS_LOCK, HID_F18), (0x7_0000_00E4, 0x7_0000_0029)]
        );
        assert!(parse_hid_mapping("(null)\n").is_empty());
    }

    #[test]
    fn layered_keys_translate_to_mac_equivalents() {
        assert_eq!(mac_keystroke(KEY_LEFT, false), (KEY_LEFT, 0));
        assert_eq!(mac_keystroke(KEY_LEFT, true), (KEY_LEFT, LEFT_OPTION_FLAGS));
        assert_eq!(
            mac_keystroke(KEY_BACKSPACE, true),
            (KEY_BACKSPACE, LEFT_OPTION_FLAGS)
        );
        assert_eq!(
            mac_keystroke(KEY_HOME, false),
            (KEY_LEFT, LEFT_COMMAND_FLAGS)
        );
        assert_eq!(mac_keystroke(KEY_END, true), (KEY_DOWN, LEFT_COMMAND_FLAGS));
        assert_eq!(mac_keystroke(KEY_RETURN, true), (KEY_RETURN, 0));
    }

    #[test]
    fn translated_modifiers_are_announced_with_their_own_key() {
        assert_eq!(modifier_keycode(LEFT_OPTION_FLAGS), Some(0x3A));
        assert_eq!(modifier_keycode(LEFT_COMMAND_FLAGS), Some(0x37));
        assert_eq!(modifier_keycode(0), None);
    }

    #[test]
    fn layer_key_is_not_a_bindable_key() {
        assert_eq!(input_key_from_mac(KEYCODE_F18), None);
        assert_eq!(input_key_from_mac(0x26), Some(InputKey::J));
    }
}
