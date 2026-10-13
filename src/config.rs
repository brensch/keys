use anyhow::{anyhow, Context, Result};
use directories::BaseDirs;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

pub const CONFIG_VERSION: u32 = 1;
pub const DEFAULT_CONFIG_JSON: &str = include_str!("../config/default.json");

/// Physical keys that can activate an action while Caps Lock is held.
/// The discriminants are stable array indexes used by the real-time lookup path.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[repr(usize)]
#[serde(rename_all = "snake_case")]
pub enum InputKey {
    A,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
    I,
    J,
    K,
    L,
    M,
    N,
    O,
    P,
    Q,
    R,
    S,
    T,
    U,
    V,
    W,
    X,
    Y,
    Z,
    Digit0,
    Digit1,
    Digit2,
    Digit3,
    Digit4,
    Digit5,
    Digit6,
    Digit7,
    Digit8,
    Digit9,
    Backquote,
    Minus,
    Equal,
    LeftBracket,
    RightBracket,
    Backslash,
    Semicolon,
    Quote,
    Comma,
    Period,
    Slash,
    Tab,
    Space,
    Enter,
    Escape,
    Backspace,
    Delete,
    Insert,
    Home,
    End,
    PageUp,
    PageDown,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    F13,
    F14,
    F15,
    F16,
    F17,
    F18,
    F19,
    F20,
    F21,
    F22,
    F23,
    F24,
}

impl InputKey {
    pub const COUNT: usize = Self::F24 as usize + 1;

    pub const fn index(self) -> usize {
        self as usize
    }

    pub fn label(self) -> &'static str {
        const LABELS: [&str; InputKey::COUNT] = [
            "A",
            "B",
            "C",
            "D",
            "E",
            "F",
            "G",
            "H",
            "I",
            "J",
            "K",
            "L",
            "M",
            "N",
            "O",
            "P",
            "Q",
            "R",
            "S",
            "T",
            "U",
            "V",
            "W",
            "X",
            "Y",
            "Z",
            "0",
            "1",
            "2",
            "3",
            "4",
            "5",
            "6",
            "7",
            "8",
            "9",
            "`",
            "-",
            "=",
            "[",
            "]",
            "\\",
            ";",
            "'",
            ",",
            ".",
            "/",
            "Tab",
            "Space",
            "Enter",
            "Escape",
            "Backspace",
            "Delete",
            "Insert",
            "Home",
            "End",
            "Page Up",
            "Page Down",
            "Up arrow",
            "Down arrow",
            "Left arrow",
            "Right arrow",
            "F1",
            "F2",
            "F3",
            "F4",
            "F5",
            "F6",
            "F7",
            "F8",
            "F9",
            "F10",
            "F11",
            "F12",
            "F13",
            "F14",
            "F15",
            "F16",
            "F17",
            "F18",
            "F19",
            "F20",
            "F21",
            "F22",
            "F23",
            "F24",
        ];
        LABELS[self.index()]
    }
}

/// OS-independent actions. Platform modules translate these to their native output codes.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[repr(u8)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    LeftControl,
    LeftShift,
    LeftAlt,
    LeftMeta,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    Home,
    End,
    PageUp,
    PageDown,
    Backspace,
    Delete,
    Enter,
    Escape,
    Tab,
    Space,
    VolumeUp,
    VolumeDown,
    VolumeMute,
    MediaPrevious,
    MediaPlayPause,
    MediaNext,
}

impl Action {
    pub const ALL: &'static [Self] = &[
        Self::LeftControl,
        Self::LeftShift,
        Self::LeftAlt,
        Self::LeftMeta,
        Self::ArrowUp,
        Self::ArrowDown,
        Self::ArrowLeft,
        Self::ArrowRight,
        Self::Home,
        Self::End,
        Self::PageUp,
        Self::PageDown,
        Self::Backspace,
        Self::Delete,
        Self::Enter,
        Self::Escape,
        Self::Tab,
        Self::Space,
        Self::VolumeUp,
        Self::VolumeDown,
        Self::VolumeMute,
        Self::MediaPrevious,
        Self::MediaPlayPause,
        Self::MediaNext,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::LeftControl => "Control",
            Self::LeftShift => "Shift",
            Self::LeftAlt => "Alt / Option",
            Self::LeftMeta => "Windows / Command",
            Self::ArrowUp => "Up arrow",
            Self::ArrowDown => "Down arrow",
            Self::ArrowLeft => "Left arrow",
            Self::ArrowRight => "Right arrow",
            Self::Home => "Home",
            Self::End => "End",
            Self::PageUp => "Page Up",
            Self::PageDown => "Page Down",
            Self::Backspace => "Backspace",
            Self::Delete => "Delete",
            Self::Enter => "Enter",
            Self::Escape => "Escape",
            Self::Tab => "Tab",
            Self::Space => "Space",
            Self::VolumeUp => "Volume up",
            Self::VolumeDown => "Volume down",
            Self::VolumeMute => "Mute",
            Self::MediaPrevious => "Previous track",
            Self::MediaPlayPause => "Play / pause",
            Self::MediaNext => "Next track",
        }
    }

    #[inline(always)]
    const fn from_code(code: u8) -> Option<Self> {
        if (code as usize) < Self::ALL.len() {
            Some(Self::ALL[code as usize])
        } else {
            None
        }
    }

    pub fn category(self) -> &'static str {
        match self {
            Self::LeftControl | Self::LeftShift | Self::LeftAlt | Self::LeftMeta => "Modifiers",
            Self::ArrowUp
            | Self::ArrowDown
            | Self::ArrowLeft
            | Self::ArrowRight
            | Self::Home
            | Self::End
            | Self::PageUp
            | Self::PageDown => "Navigation",
            Self::Backspace
            | Self::Delete
            | Self::Enter
            | Self::Escape
            | Self::Tab
            | Self::Space => "Editing",
            Self::VolumeUp | Self::VolumeDown | Self::VolumeMute => "Volume",
            Self::MediaPrevious | Self::MediaPlayPause | Self::MediaNext => "Media",
        }
    }
}

/// The on-disk format is intentionally direct: each action names the physical key
/// that should invoke it while Caps Lock is held.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Config {
    pub version: u32,
    pub enabled: bool,
    pub bindings: BTreeMap<Action, InputKey>,
}

impl Default for Config {
    fn default() -> Self {
        serde_json::from_str(DEFAULT_CONFIG_JSON)
            .expect("config/default.json must contain a valid nocaps configuration")
    }
}

impl Config {
    pub fn validate(&self) -> Result<()> {
        if self.version != CONFIG_VERSION {
            return Err(anyhow!(
                "unsupported configuration version {}; expected {}",
                self.version,
                CONFIG_VERSION
            ));
        }
        let mut keys = HashSet::new();
        for key in self.bindings.values() {
            if !keys.insert(*key) {
                return Err(anyhow!(
                    "{} is assigned to more than one action",
                    key.label()
                ));
            }
        }
        Ok(())
    }

    pub fn key_for(&self, action: Action) -> Option<InputKey> {
        self.bindings.get(&action).copied()
    }

    pub fn bind(&mut self, action: Action, key: InputKey) {
        self.bindings.retain(|_, current| *current != key);
        self.bindings.insert(action, key);
    }

    pub fn unbind(&mut self, action: Action) {
        self.bindings.remove(&action);
    }
}

/// The bindings the keyboard hooks read on every key event.
///
/// Each physical key owns one atomic byte holding its action's discriminant, so a lookup
/// is a single relaxed load from a static table: no locks, no reference counting, no
/// pointer chasing. Updates happen only when the user edits a binding; a hook racing an
/// update sees each key's old or new action, which is indistinguishable from pressing
/// the key a moment earlier or later.
pub static BINDINGS: RuntimeBindings = RuntimeBindings::new();

pub struct RuntimeBindings {
    enabled: AtomicBool,
    actions: [AtomicU8; InputKey::COUNT],
}

const NO_ACTION: u8 = u8::MAX;

impl RuntimeBindings {
    pub const fn new() -> Self {
        Self {
            enabled: AtomicBool::new(false),
            actions: [const { AtomicU8::new(NO_ACTION) }; InputKey::COUNT],
        }
    }

    #[inline(always)]
    pub fn action_for(&self, key: InputKey) -> Option<Action> {
        Action::from_code(self.actions[key.index()].load(Ordering::Relaxed))
    }

    #[inline(always)]
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn apply(&self, config: &Config) -> Result<()> {
        config.validate()?;
        let mut actions = [NO_ACTION; InputKey::COUNT];
        if config.enabled {
            for (action, key) in &config.bindings {
                actions[key.index()] = *action as u8;
            }
        }
        for (slot, action) in self.actions.iter().zip(actions) {
            slot.store(action, Ordering::Relaxed);
        }
        self.enabled.store(config.enabled, Ordering::Relaxed);
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct ConfigStore {
    path: PathBuf,
}

impl ConfigStore {
    pub fn discover() -> Result<Self> {
        let base = BaseDirs::new()
            .ok_or_else(|| anyhow!("could not determine the user configuration directory"))?;
        Ok(Self {
            path: base.config_dir().join("nocaps").join("config.json"),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load_or_create(&self) -> Result<Config> {
        if !self.path.exists() {
            let config = Config::default();
            self.save(&config)?;
            return Ok(config);
        }

        let bytes = fs::read(&self.path)
            .with_context(|| format!("read configuration from {}", self.path.display()))?;
        let config: Config = serde_json::from_slice(&bytes)
            .with_context(|| format!("parse configuration from {}", self.path.display()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn save(&self, config: &Config) -> Result<()> {
        config.validate()?;
        let parent = self
            .path
            .parent()
            .ok_or_else(|| anyhow!("configuration path has no parent"))?;
        fs::create_dir_all(parent)
            .with_context(|| format!("create configuration directory {}", parent.display()))?;
        // Write a sibling file and rename it over the original so an interrupted save
        // never leaves a truncated configuration behind.
        let temporary = self.path.with_extension("json.tmp");
        fs::write(&temporary, serde_json::to_vec_pretty(config)?)
            .with_context(|| format!("write configuration to {}", temporary.display()))?;
        fs::rename(&temporary, &self.path)
            .with_context(|| format!("replace configuration at {}", self.path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_compile_to_constant_time_lookup() {
        let runtime = RuntimeBindings::new();
        runtime.apply(&Config::default()).unwrap();
        assert_eq!(runtime.action_for(InputKey::I), Some(Action::ArrowUp));
        assert_eq!(runtime.action_for(InputKey::J), Some(Action::ArrowLeft));
    }

    #[test]
    fn action_codes_round_trip() {
        for (index, action) in Action::ALL.iter().enumerate() {
            assert_eq!(*action as usize, index);
            assert_eq!(Action::from_code(*action as u8), Some(*action));
        }
        assert_eq!(Action::from_code(NO_ACTION), None);
    }

    #[test]
    fn disabling_clears_every_lookup() {
        let runtime = RuntimeBindings::new();
        let config = Config {
            enabled: false,
            ..Config::default()
        };
        runtime.apply(&config).unwrap();
        assert!(!runtime.is_enabled());
        assert_eq!(runtime.action_for(InputKey::I), None);
    }

    #[test]
    fn rebinding_a_key_removes_its_previous_action() {
        let mut config = Config::default();
        config.bind(Action::VolumeUp, InputKey::I);
        assert_eq!(config.key_for(Action::VolumeUp), Some(InputKey::I));
        assert_eq!(config.key_for(Action::ArrowUp), None);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn json_is_action_first_and_readable() {
        let json = serde_json::to_string_pretty(&Config::default()).unwrap();
        assert!(json.contains("\"arrow_up\": \"i\""));
        let decoded: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, Config::default());
    }

    #[test]
    fn repository_default_preserves_the_original_bindings() {
        let config: Config = serde_json::from_str(DEFAULT_CONFIG_JSON).unwrap();
        config.validate().unwrap();
        assert!(config.enabled);
        assert_eq!(config.key_for(Action::LeftControl), Some(InputKey::A));
        assert_eq!(config.key_for(Action::LeftShift), Some(InputKey::S));
        assert_eq!(config.key_for(Action::ArrowUp), Some(InputKey::I));
        assert_eq!(config.key_for(Action::ArrowDown), Some(InputKey::K));
        assert_eq!(config.key_for(Action::ArrowLeft), Some(InputKey::J));
        assert_eq!(config.key_for(Action::ArrowRight), Some(InputKey::L));
    }
}
