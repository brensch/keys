use crate::config::{Action, InputKey, RuntimeBindings};
use anyhow::{anyhow, Context, Result};
use evdev::uinput::VirtualDevice;
use evdev::{AttributeSet, Device, EventType, InputEvent, KeyCode};
use log::{error, info};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

static LAYER_HOLDERS: AtomicUsize = AtomicUsize::new(0);

pub struct KeyboardManager {
    _threads: Vec<JoinHandle<()>>,
}

impl KeyboardManager {
    pub fn new(runtime: Arc<RuntimeBindings>) -> Result<Self> {
        let mut devices: Vec<Device> = evdev::enumerate()
            .map(|(_, device)| device)
            .filter(is_keyboard)
            .collect();

        if devices.is_empty() {
            return Err(anyhow!(
                "no readable keyboards found; grant access to /dev/input and /dev/uinput"
            ));
        }

        let mut supported = AttributeSet::<KeyCode>::new();
        for device in &devices {
            if let Some(keys) = device.supported_keys() {
                for key in keys {
                    supported.insert(key);
                }
            }
        }
        for action in Action::ALL {
            supported.insert(linux_action_code(*action));
        }

        let virtual_keyboard = VirtualDevice::builder()
            .context("open /dev/uinput")?
            .name("nocaps virtual keyboard")
            .with_keys(&supported)
            .context("configure virtual keyboard")?
            .build()
            .context("create virtual keyboard")?;
        let virtual_keyboard = Arc::new(Mutex::new(virtual_keyboard));

        for device in &mut devices {
            device
                .grab()
                .with_context(|| format!("grab {}", device.name().unwrap_or("keyboard")))?;
        }

        let mut threads = Vec::with_capacity(devices.len());
        for device in devices {
            let runtime = runtime.clone();
            let output = virtual_keyboard.clone();
            let name = device.name().unwrap_or("keyboard").to_owned();
            threads.push(
                thread::Builder::new()
                    .name(format!("nocaps-{name}"))
                    .spawn(move || run_device(device, runtime, output, &name))
                    .context("start Linux keyboard worker")?,
            );
        }

        info!("remapping {} Linux keyboard device(s)", threads.len());
        Ok(Self { _threads: threads })
    }
}

fn is_keyboard(device: &Device) -> bool {
    device.supported_keys().is_some_and(|keys| {
        keys.contains(KeyCode::KEY_CAPSLOCK)
            && keys.contains(KeyCode::KEY_A)
            && keys.contains(KeyCode::KEY_Z)
            && keys.contains(KeyCode::KEY_ENTER)
    })
}

fn run_device(
    mut device: Device,
    runtime: Arc<RuntimeBindings>,
    output: Arc<Mutex<VirtualDevice>>,
    name: &str,
) {
    super::elevate_input_thread();
    let mut held_targets = [None; InputKey::COUNT];
    let mut captured_caps = false;
    let mut translated = Vec::with_capacity(16);

    loop {
        translated.clear();
        let events = match device.fetch_events() {
            Ok(events) => events,
            Err(error) => {
                error!("stopped reading {name}: {error}");
                cleanup_state(captured_caps, &held_targets, &output);
                return;
            }
        };

        for event in events {
            if event.event_type() != EventType::KEY {
                continue;
            }

            let source = KeyCode::new(event.code());
            let value = event.value();
            if source == KeyCode::KEY_CAPSLOCK {
                let enabled = runtime.is_enabled();
                if value == 1 && enabled && !captured_caps {
                    captured_caps = true;
                    LAYER_HOLDERS.fetch_add(1, Ordering::SeqCst);
                    continue;
                }
                if value == 0 && captured_caps {
                    captured_caps = false;
                    LAYER_HOLDERS.fetch_sub(1, Ordering::SeqCst);
                    continue;
                }
                if captured_caps {
                    continue;
                }
            }

            let input_key = key_from_linux(source);
            let target = if value == 0 {
                input_key
                    .and_then(|key| held_targets[key.index()].take())
                    .unwrap_or(source)
            } else if value == 2 {
                input_key
                    .and_then(|key| held_targets[key.index()])
                    .unwrap_or(source)
            } else if LAYER_HOLDERS.load(Ordering::SeqCst) > 0 {
                let target = input_key
                    .and_then(|key| runtime.action_for(key))
                    .map(linux_action_code)
                    .unwrap_or(source);
                if let Some(key) = input_key.filter(|_| target != source) {
                    held_targets[key.index()] = Some(target);
                }
                target
            } else {
                source
            };

            translated.push(InputEvent::new(EventType::KEY.0, target.code(), value));
        }

        if !translated.is_empty() {
            let result = output
                .lock()
                .map_err(|_| anyhow!("virtual keyboard lock is poisoned"))
                .and_then(|mut output| output.emit(&translated).context("emit keyboard events"));
            if let Err(error) = result {
                error!("stopped writing events for {name}: {error:#}");
                cleanup_state(captured_caps, &held_targets, &output);
                return;
            }
        }
    }
}

fn cleanup_state(
    captured_caps: bool,
    held_targets: &[Option<KeyCode>; InputKey::COUNT],
    output: &Arc<Mutex<VirtualDevice>>,
) {
    if captured_caps {
        LAYER_HOLDERS.fetch_sub(1, Ordering::SeqCst);
    }
    let releases: Vec<InputEvent> = held_targets
        .iter()
        .flatten()
        .map(|key| InputEvent::new(EventType::KEY.0, key.code(), 0))
        .collect();
    if !releases.is_empty() {
        if let Ok(mut output) = output.lock() {
            let _ = output.emit(&releases);
        }
    }
}

fn key_from_linux(code: KeyCode) -> Option<InputKey> {
    Some(match code {
        KeyCode::KEY_A => InputKey::A,
        KeyCode::KEY_B => InputKey::B,
        KeyCode::KEY_C => InputKey::C,
        KeyCode::KEY_D => InputKey::D,
        KeyCode::KEY_E => InputKey::E,
        KeyCode::KEY_F => InputKey::F,
        KeyCode::KEY_G => InputKey::G,
        KeyCode::KEY_H => InputKey::H,
        KeyCode::KEY_I => InputKey::I,
        KeyCode::KEY_J => InputKey::J,
        KeyCode::KEY_K => InputKey::K,
        KeyCode::KEY_L => InputKey::L,
        KeyCode::KEY_M => InputKey::M,
        KeyCode::KEY_N => InputKey::N,
        KeyCode::KEY_O => InputKey::O,
        KeyCode::KEY_P => InputKey::P,
        KeyCode::KEY_Q => InputKey::Q,
        KeyCode::KEY_R => InputKey::R,
        KeyCode::KEY_S => InputKey::S,
        KeyCode::KEY_T => InputKey::T,
        KeyCode::KEY_U => InputKey::U,
        KeyCode::KEY_V => InputKey::V,
        KeyCode::KEY_W => InputKey::W,
        KeyCode::KEY_X => InputKey::X,
        KeyCode::KEY_Y => InputKey::Y,
        KeyCode::KEY_Z => InputKey::Z,
        KeyCode::KEY_0 => InputKey::Digit0,
        KeyCode::KEY_1 => InputKey::Digit1,
        KeyCode::KEY_2 => InputKey::Digit2,
        KeyCode::KEY_3 => InputKey::Digit3,
        KeyCode::KEY_4 => InputKey::Digit4,
        KeyCode::KEY_5 => InputKey::Digit5,
        KeyCode::KEY_6 => InputKey::Digit6,
        KeyCode::KEY_7 => InputKey::Digit7,
        KeyCode::KEY_8 => InputKey::Digit8,
        KeyCode::KEY_9 => InputKey::Digit9,
        KeyCode::KEY_GRAVE => InputKey::Backquote,
        KeyCode::KEY_MINUS => InputKey::Minus,
        KeyCode::KEY_EQUAL => InputKey::Equal,
        KeyCode::KEY_LEFTBRACE => InputKey::LeftBracket,
        KeyCode::KEY_RIGHTBRACE => InputKey::RightBracket,
        KeyCode::KEY_BACKSLASH => InputKey::Backslash,
        KeyCode::KEY_SEMICOLON => InputKey::Semicolon,
        KeyCode::KEY_APOSTROPHE => InputKey::Quote,
        KeyCode::KEY_COMMA => InputKey::Comma,
        KeyCode::KEY_DOT => InputKey::Period,
        KeyCode::KEY_SLASH => InputKey::Slash,
        KeyCode::KEY_TAB => InputKey::Tab,
        KeyCode::KEY_SPACE => InputKey::Space,
        KeyCode::KEY_ENTER => InputKey::Enter,
        KeyCode::KEY_ESC => InputKey::Escape,
        KeyCode::KEY_BACKSPACE => InputKey::Backspace,
        KeyCode::KEY_DELETE => InputKey::Delete,
        KeyCode::KEY_INSERT => InputKey::Insert,
        KeyCode::KEY_HOME => InputKey::Home,
        KeyCode::KEY_END => InputKey::End,
        KeyCode::KEY_PAGEUP => InputKey::PageUp,
        KeyCode::KEY_PAGEDOWN => InputKey::PageDown,
        KeyCode::KEY_UP => InputKey::ArrowUp,
        KeyCode::KEY_DOWN => InputKey::ArrowDown,
        KeyCode::KEY_LEFT => InputKey::ArrowLeft,
        KeyCode::KEY_RIGHT => InputKey::ArrowRight,
        KeyCode::KEY_F1 => InputKey::F1,
        KeyCode::KEY_F2 => InputKey::F2,
        KeyCode::KEY_F3 => InputKey::F3,
        KeyCode::KEY_F4 => InputKey::F4,
        KeyCode::KEY_F5 => InputKey::F5,
        KeyCode::KEY_F6 => InputKey::F6,
        KeyCode::KEY_F7 => InputKey::F7,
        KeyCode::KEY_F8 => InputKey::F8,
        KeyCode::KEY_F9 => InputKey::F9,
        KeyCode::KEY_F10 => InputKey::F10,
        KeyCode::KEY_F11 => InputKey::F11,
        KeyCode::KEY_F12 => InputKey::F12,
        KeyCode::KEY_F13 => InputKey::F13,
        KeyCode::KEY_F14 => InputKey::F14,
        KeyCode::KEY_F15 => InputKey::F15,
        KeyCode::KEY_F16 => InputKey::F16,
        KeyCode::KEY_F17 => InputKey::F17,
        KeyCode::KEY_F18 => InputKey::F18,
        KeyCode::KEY_F19 => InputKey::F19,
        KeyCode::KEY_F20 => InputKey::F20,
        KeyCode::KEY_F21 => InputKey::F21,
        KeyCode::KEY_F22 => InputKey::F22,
        KeyCode::KEY_F23 => InputKey::F23,
        KeyCode::KEY_F24 => InputKey::F24,
        _ => return None,
    })
}

fn linux_action_code(action: Action) -> KeyCode {
    match action {
        Action::LeftControl => KeyCode::KEY_LEFTCTRL,
        Action::LeftShift => KeyCode::KEY_LEFTSHIFT,
        Action::LeftAlt => KeyCode::KEY_LEFTALT,
        Action::LeftMeta => KeyCode::KEY_LEFTMETA,
        Action::ArrowUp => KeyCode::KEY_UP,
        Action::ArrowDown => KeyCode::KEY_DOWN,
        Action::ArrowLeft => KeyCode::KEY_LEFT,
        Action::ArrowRight => KeyCode::KEY_RIGHT,
        Action::Home => KeyCode::KEY_HOME,
        Action::End => KeyCode::KEY_END,
        Action::PageUp => KeyCode::KEY_PAGEUP,
        Action::PageDown => KeyCode::KEY_PAGEDOWN,
        Action::Backspace => KeyCode::KEY_BACKSPACE,
        Action::Delete => KeyCode::KEY_DELETE,
        Action::Enter => KeyCode::KEY_ENTER,
        Action::Escape => KeyCode::KEY_ESC,
        Action::Tab => KeyCode::KEY_TAB,
        Action::Space => KeyCode::KEY_SPACE,
        Action::VolumeUp => KeyCode::KEY_VOLUMEUP,
        Action::VolumeDown => KeyCode::KEY_VOLUMEDOWN,
        Action::VolumeMute => KeyCode::KEY_MUTE,
        Action::MediaPrevious => KeyCode::KEY_PREVIOUSSONG,
        Action::MediaPlayPause => KeyCode::KEY_PLAYPAUSE,
        Action::MediaNext => KeyCode::KEY_NEXTSONG,
    }
}
