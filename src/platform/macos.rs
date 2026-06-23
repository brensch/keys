use crate::config::{Action, InputKey, RuntimeBindings};
use anyhow::{anyhow, Context, Result};
use enigo::{Direction, Enigo, Key as EnigoKey, Keyboard, Settings};
use log::{error, info};
use rdev::{grab, Event, EventType, Key};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};

static CAPS_CAPTURED: AtomicBool = AtomicBool::new(false);
static INJECTING: AtomicBool = AtomicBool::new(false);
static HELD_ACTIONS: [AtomicU8; InputKey::COUNT] = [const { AtomicU8::new(0) }; InputKey::COUNT];
static ENIGO: OnceLock<Mutex<Enigo>> = OnceLock::new();

pub struct KeyboardManager {
    _thread: JoinHandle<()>,
}

impl KeyboardManager {
    pub fn new(runtime: Arc<RuntimeBindings>) -> Result<Self> {
        let enigo = Enigo::new(&Settings::default())
            .map_err(|error| anyhow!("initialize macOS input injection: {error}"))?;
        ENIGO
            .set(Mutex::new(enigo))
            .map_err(|_| anyhow!("macOS input injection has already been initialized"))?;

        let thread = thread::Builder::new()
            .name("nocaps-macos-input".to_owned())
            .spawn(move || {
                super::elevate_input_thread();
                if let Err(error) = grab(move |event| callback(event, &runtime)) {
                    error!("macOS keyboard capture stopped: {error:?}");
                }
            })
            .context("start macOS keyboard worker")?;
        info!("macOS keyboard worker started");
        Ok(Self { _thread: thread })
    }
}

fn callback(event: Event, runtime: &RuntimeBindings) -> Option<Event> {
    if INJECTING.load(Ordering::SeqCst) {
        return Some(event);
    }

    let (source, down) = match event.event_type {
        EventType::KeyPress(key) => (key, true),
        EventType::KeyRelease(key) => (key, false),
        _ => return Some(event),
    };

    if source == Key::CapsLock {
        if down && runtime.is_enabled() {
            CAPS_CAPTURED.store(true, Ordering::SeqCst);
            return None;
        }
        if !down && CAPS_CAPTURED.swap(false, Ordering::SeqCst) {
            return None;
        }
    }

    let Some(input_key) = input_key_from_rdev(source) else {
        return Some(event);
    };

    if !down {
        let held = HELD_ACTIONS[input_key.index()].swap(0, Ordering::SeqCst);
        if let Some(action) = Action::from_held_code(held) {
            inject(action, Direction::Release);
            return None;
        }
    }

    if down && CAPS_CAPTURED.load(Ordering::SeqCst) {
        if let Some(action) = runtime.action_for(input_key) {
            if action_to_enigo(action).is_some() {
                HELD_ACTIONS[input_key.index()].store(action.held_code(), Ordering::SeqCst);
                inject(action, Direction::Press);
                return None;
            }
        }
    }

    Some(event)
}

fn inject(action: Action, direction: Direction) {
    let Some(key) = action_to_enigo(action) else {
        return;
    };
    INJECTING.store(true, Ordering::SeqCst);
    let result = ENIGO
        .get()
        .and_then(|enigo| enigo.lock().ok())
        .ok_or_else(|| anyhow!("macOS input injector is unavailable"))
        .and_then(|mut enigo| {
            enigo
                .key(key, direction)
                .map_err(|error| anyhow!(error.to_string()))
        });
    INJECTING.store(false, Ordering::SeqCst);
    if let Err(error) = result {
        error!("could not inject macOS key event: {error}");
    }
}

fn input_key_from_rdev(key: Key) -> Option<InputKey> {
    Some(match key {
        Key::KeyA => InputKey::A,
        Key::KeyB => InputKey::B,
        Key::KeyC => InputKey::C,
        Key::KeyD => InputKey::D,
        Key::KeyE => InputKey::E,
        Key::KeyF => InputKey::F,
        Key::KeyG => InputKey::G,
        Key::KeyH => InputKey::H,
        Key::KeyI => InputKey::I,
        Key::KeyJ => InputKey::J,
        Key::KeyK => InputKey::K,
        Key::KeyL => InputKey::L,
        Key::KeyM => InputKey::M,
        Key::KeyN => InputKey::N,
        Key::KeyO => InputKey::O,
        Key::KeyP => InputKey::P,
        Key::KeyQ => InputKey::Q,
        Key::KeyR => InputKey::R,
        Key::KeyS => InputKey::S,
        Key::KeyT => InputKey::T,
        Key::KeyU => InputKey::U,
        Key::KeyV => InputKey::V,
        Key::KeyW => InputKey::W,
        Key::KeyX => InputKey::X,
        Key::KeyY => InputKey::Y,
        Key::KeyZ => InputKey::Z,
        Key::Num0 => InputKey::Digit0,
        Key::Num1 => InputKey::Digit1,
        Key::Num2 => InputKey::Digit2,
        Key::Num3 => InputKey::Digit3,
        Key::Num4 => InputKey::Digit4,
        Key::Num5 => InputKey::Digit5,
        Key::Num6 => InputKey::Digit6,
        Key::Num7 => InputKey::Digit7,
        Key::Num8 => InputKey::Digit8,
        Key::Num9 => InputKey::Digit9,
        Key::BackQuote => InputKey::Backquote,
        Key::Minus => InputKey::Minus,
        Key::Equal => InputKey::Equal,
        Key::LeftBracket => InputKey::LeftBracket,
        Key::RightBracket => InputKey::RightBracket,
        Key::BackSlash => InputKey::Backslash,
        Key::SemiColon => InputKey::Semicolon,
        Key::Quote => InputKey::Quote,
        Key::Comma => InputKey::Comma,
        Key::Dot => InputKey::Period,
        Key::Slash => InputKey::Slash,
        Key::Tab => InputKey::Tab,
        Key::Space => InputKey::Space,
        Key::Return => InputKey::Enter,
        Key::Escape => InputKey::Escape,
        Key::Backspace => InputKey::Backspace,
        Key::Unknown(117) => InputKey::Delete,
        Key::Unknown(115) => InputKey::Home,
        Key::Unknown(119) => InputKey::End,
        Key::Unknown(116) => InputKey::PageUp,
        Key::Unknown(121) => InputKey::PageDown,
        Key::UpArrow => InputKey::ArrowUp,
        Key::DownArrow => InputKey::ArrowDown,
        Key::LeftArrow => InputKey::ArrowLeft,
        Key::RightArrow => InputKey::ArrowRight,
        Key::F1 => InputKey::F1,
        Key::F2 => InputKey::F2,
        Key::F3 => InputKey::F3,
        Key::F4 => InputKey::F4,
        Key::F5 => InputKey::F5,
        Key::F6 => InputKey::F6,
        Key::F7 => InputKey::F7,
        Key::F8 => InputKey::F8,
        Key::F9 => InputKey::F9,
        Key::F10 => InputKey::F10,
        Key::F11 => InputKey::F11,
        Key::F12 => InputKey::F12,
        Key::Unknown(105) => InputKey::F13,
        Key::Unknown(107) => InputKey::F14,
        Key::Unknown(113) => InputKey::F15,
        Key::Unknown(106) => InputKey::F16,
        Key::Unknown(64) => InputKey::F17,
        Key::Unknown(79) => InputKey::F18,
        Key::Unknown(80) => InputKey::F19,
        Key::Unknown(90) => InputKey::F20,
        _ => return None,
    })
}

fn action_to_enigo(action: Action) -> Option<EnigoKey> {
    Some(match action {
        Action::LeftControl => EnigoKey::Control,
        Action::LeftShift => EnigoKey::Shift,
        Action::LeftAlt => EnigoKey::Option,
        Action::LeftMeta => EnigoKey::Meta,
        Action::ArrowUp => EnigoKey::UpArrow,
        Action::ArrowDown => EnigoKey::DownArrow,
        Action::ArrowLeft => EnigoKey::LeftArrow,
        Action::ArrowRight => EnigoKey::RightArrow,
        Action::Home => EnigoKey::Home,
        Action::End => EnigoKey::End,
        Action::PageUp => EnigoKey::PageUp,
        Action::PageDown => EnigoKey::PageDown,
        Action::Backspace => EnigoKey::Backspace,
        Action::Delete => EnigoKey::Delete,
        Action::Enter => EnigoKey::Return,
        Action::Escape => EnigoKey::Escape,
        Action::Tab => EnigoKey::Tab,
        Action::Space => EnigoKey::Space,
        Action::VolumeUp => EnigoKey::VolumeUp,
        Action::VolumeDown => EnigoKey::VolumeDown,
        Action::VolumeMute => EnigoKey::VolumeMute,
        Action::MediaPrevious => EnigoKey::MediaPrevTrack,
        Action::MediaPlayPause => EnigoKey::MediaPlayPause,
        Action::MediaNext => EnigoKey::MediaNextTrack,
    })
}
