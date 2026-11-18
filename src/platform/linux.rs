use crate::config::{Action, InputKey, BINDINGS};
use anyhow::{anyhow, Context, Result};
use evdev::raw_stream::RawDevice;
use evdev::uinput::VirtualDevice;
use evdev::{AttributeSet, BusType, EventType, InputEvent, InputId, KeyCode, SynchronizationCode};
use log::{debug, error, info, warn};
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Write};
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// Caps Lock held on any keyboard activates the layer on every keyboard.
static LAYER_HOLDERS: AtomicUsize = AtomicUsize::new(0);
/// How many keyboards hold Caps Lock + a mode key (`MODE_WORKSPACE`, `MODE_FOCUS`),
/// which turns layered arrows into a desktop shortcut instead.
static MODE_HOLDERS: [AtomicUsize; 2] = [const { AtomicUsize::new(0) }; 2];
const MODE_WORKSPACE: usize = 0;
const MODE_FOCUS: usize = 1;

const VIRTUAL_NAME: &str = "nocaps virtual keyboard";
const INPUT_DIR: &str = "/dev/input";
/// inotify wakes the watcher the moment a device node appears or its permissions
/// change; the periodic rescan only backstops races such as a node number being
/// reused before the previous owner's worker noticed the unplug.
const RESCAN_INTERVAL: Duration = Duration::from_secs(5);
/// Key codes are below KEY_MAX (0x2ff), so a 768-bit set covers every key.
const KEY_SLOTS: usize = 0x300;

/// Identifies one incarnation of a device node: unplugging and replugging a keyboard
/// can hand out the same `/dev/input/eventN` path, but never the same inode and ctime.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct NodeId {
    rdev: u64,
    ino: u64,
    ctime: (i64, i64),
}

impl NodeId {
    fn of(path: &Path) -> io::Result<Self> {
        let meta = std::fs::metadata(path)?;
        Ok(Self {
            rdev: meta.rdev(),
            ino: meta.ino(),
            ctime: (meta.ctime(), meta.ctime_nsec()),
        })
    }
}

/// Shared between the hotplug watcher and the per-keyboard workers.
struct Hub {
    /// Kept alive for as long as any worker may still write through a duplicate of
    /// its handle; dropping it destroys the virtual keyboard.
    virtual_keyboard: VirtualDevice,
    /// Nodes a worker currently owns.
    grabbed: Mutex<HashMap<PathBuf, NodeId>>,
}

impl Hub {
    fn output(&self) -> Result<File> {
        // Each worker writes through its own handle to the virtual keyboard: uinput
        // accepts whole events per write, so the workers never need to share a lock.
        Ok(File::from(
            self.virtual_keyboard
                .as_fd()
                .try_clone_to_owned()
                .context("duplicate the virtual keyboard handle")?,
        ))
    }
}

pub struct KeyboardManager {
    _hub: Arc<Hub>,
}

impl KeyboardManager {
    pub fn new() -> Result<Self> {
        // The virtual keyboard advertises every keyboard key up front, because uinput
        // capabilities are fixed at creation and keyboards plugged in later may have
        // keys the first ones lacked. Mouse, joystick and touch buttons (0x100-0x15f,
        // 0x2c0-0x2ff) are left out so libinput still classifies it as a keyboard.
        let mut supported = AttributeSet::<KeyCode>::new();
        for code in (1..0x100).chain(0x160..0x2c0) {
            supported.insert(KeyCode::new(code));
        }
        for action in Action::ALL {
            if mode_of(*action).is_none() {
                supported.insert(linux_action_code(*action));
            }
        }

        let virtual_keyboard = VirtualDevice::builder()
            .context("open /dev/uinput")?
            .name(VIRTUAL_NAME)
            .input_id(InputId::new(BusType::BUS_VIRTUAL, 0x6e63, 0x0001, 1))
            .with_keys(&supported)
            .context("configure virtual keyboard")?
            .build()
            .context("create virtual keyboard")?;

        let hub = Arc::new(Hub {
            virtual_keyboard,
            grabbed: Mutex::new(HashMap::new()),
        });

        // The first scan runs synchronously so a machine where nocaps can read no
        // input devices at all is reported at startup instead of failing silently.
        let mut watcher = Watcher::new(Arc::clone(&hub));
        let scan = watcher.scan();
        if scan.grabbed == 0 && scan.denied > 0 {
            return Err(anyhow!(
                "no readable keyboards found; grant access to /dev/input and /dev/uinput"
            ));
        }
        if scan.grabbed == 0 {
            warn!("no keyboards found yet; waiting for one to be plugged in");
        }

        thread::Builder::new()
            .name("nocaps-hotplug".to_owned())
            .spawn(move || watcher.run())
            .context("start the Linux keyboard hotplug watcher")?;

        Ok(Self { _hub: hub })
    }
}

#[derive(Default)]
struct ScanResult {
    grabbed: usize,
    denied: usize,
}

/// Finds keyboards as they appear and hands each one to its own worker thread.
struct Watcher {
    hub: Arc<Hub>,
    /// Nodes already examined and skipped (not a keyboard, unreadable, or grabbed by
    /// someone else). A node is examined again once its identity changes, e.g. when
    /// udev grants access after creating it.
    skipped: HashMap<PathBuf, NodeId>,
    /// Keyboards another program has grabbed. They are retried on every scan so
    /// nocaps takes over once that program exits, but only warned about once.
    busy: HashMap<PathBuf, NodeId>,
    inotify: Option<OwnedFd>,
}

enum Grab {
    Grabbed,
    NotKeyboard,
    Busy,
}

impl Watcher {
    fn new(hub: Arc<Hub>) -> Self {
        let inotify = watch_input_dir()
            .map_err(|error| {
                warn!("cannot watch {INPUT_DIR} for new keyboards ({error}); polling instead")
            })
            .ok();
        Self {
            hub,
            skipped: HashMap::new(),
            busy: HashMap::new(),
            inotify,
        }
    }

    fn run(mut self) {
        loop {
            self.wait();
            self.scan();
        }
    }

    /// Blocks until /dev/input changes or the rescan interval passes.
    fn wait(&self) {
        let Some(fd) = &self.inotify else {
            thread::sleep(RESCAN_INTERVAL);
            return;
        };
        let mut poll = libc::pollfd {
            fd: fd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: `poll` points at one valid pollfd for the duration of the call.
        let ready = unsafe { libc::poll(&mut poll, 1, RESCAN_INTERVAL.as_millis() as i32) };
        if ready > 0 {
            // Drain the queued events; their contents don't matter because every
            // wake-up rescans the whole directory.
            let mut buffer = [0u8; 4096];
            // SAFETY: the buffer is valid for writes of its full length.
            while unsafe { libc::read(fd.as_raw_fd(), buffer.as_mut_ptr().cast(), buffer.len()) }
                > 0
            {}
            // Let udev finish tagging and granting access before opening the node.
            thread::sleep(Duration::from_millis(50));
        }
    }

    fn scan(&mut self) -> ScanResult {
        let mut result = ScanResult::default();
        let entries = match std::fs::read_dir(INPUT_DIR) {
            Ok(entries) => entries,
            Err(error) => {
                error!("cannot list {INPUT_DIR}: {error}");
                return result;
            }
        };
        let mut present = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let is_event_node = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("event"));
            if !is_event_node {
                continue;
            }
            let Ok(id) = NodeId::of(&path) else {
                continue;
            };
            present.push(path.clone());
            if self.skipped.get(&path) == Some(&id) || self.is_grabbed(&path, id) {
                continue;
            }
            match self.try_grab(&path, id) {
                Ok(Grab::Grabbed) => {
                    self.busy.remove(&path);
                    result.grabbed += 1;
                }
                Ok(Grab::NotKeyboard) => {
                    self.skipped.insert(path, id);
                }
                Ok(Grab::Busy) => {
                    if self.busy.insert(path.clone(), id) != Some(id) {
                        warn!(
                            "{} is grabbed by another program; will take over when it lets go",
                            path.display()
                        );
                    }
                }
                Err(error) => {
                    if error.kind() == io::ErrorKind::PermissionDenied {
                        result.denied += 1;
                    }
                    debug!("skipping {}: {error}", path.display());
                    self.skipped.insert(path, id);
                }
            }
        }
        self.skipped.retain(|path, _| present.contains(path));
        self.busy.retain(|path, _| present.contains(path));
        result
    }

    fn is_grabbed(&self, path: &Path, id: NodeId) -> bool {
        self.hub
            .grabbed
            .lock()
            .map(|grabbed| grabbed.get(path) == Some(&id))
            .unwrap_or(false)
    }

    fn try_grab(&self, path: &Path, id: NodeId) -> io::Result<Grab> {
        let mut device = RawDevice::open(path)?;
        if !is_keyboard(&device) {
            return Ok(Grab::NotKeyboard);
        }
        let name = device.name().unwrap_or("keyboard").to_owned();
        if let Err(error) = device.grab() {
            if error.raw_os_error() == Some(libc::EBUSY) {
                // Another remapper (or an older nocaps) already owns this keyboard.
                return Ok(Grab::Busy);
            }
            return Err(error);
        }
        let output = self.hub.output().map_err(io::Error::other)?;

        if let Ok(mut grabbed) = self.hub.grabbed.lock() {
            grabbed.insert(path.to_owned(), id);
        }
        let hub = Arc::clone(&self.hub);
        let worker_path = path.to_owned();
        let spawned = thread::Builder::new()
            .name(format!("nocaps-{}", path.display()))
            .spawn(move || {
                run_device(device, output, &name);
                // Only forget the node if it still refers to this device: a replugged
                // keyboard may already own the same path under a new worker.
                if let Ok(mut grabbed) = hub.grabbed.lock() {
                    if grabbed.get(&worker_path) == Some(&id) {
                        grabbed.remove(&worker_path);
                    }
                }
            });
        if let Err(error) = spawned {
            if let Ok(mut grabbed) = self.hub.grabbed.lock() {
                grabbed.remove(path);
            }
            return Err(error);
        }
        info!("remapping {}", path.display());
        Ok(Grab::Grabbed)
    }
}

fn watch_input_dir() -> io::Result<OwnedFd> {
    // SAFETY: plain syscalls; the returned descriptor is checked before being owned.
    unsafe {
        let fd = libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC);
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let fd = OwnedFd::from_raw_fd(fd);
        let dir = std::ffi::CString::new(INPUT_DIR).expect("path has no NUL");
        if libc::inotify_add_watch(
            fd.as_raw_fd(),
            dir.as_ptr(),
            libc::IN_CREATE | libc::IN_ATTRIB,
        ) < 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(fd)
    }
}

fn is_keyboard(device: &RawDevice) -> bool {
    // Never grab our own output, or keystrokes would loop back through nocaps.
    if device.name() == Some(VIRTUAL_NAME) {
        return false;
    }
    device.supported_keys().is_some_and(|keys| {
        keys.contains(KeyCode::KEY_CAPSLOCK)
            && keys.contains(KeyCode::KEY_A)
            && keys.contains(KeyCode::KEY_Z)
            && keys.contains(KeyCode::KEY_ENTER)
    })
}

/// Keys this worker currently holds down on the virtual keyboard, so they can all be
/// released if the physical keyboard vanishes mid-press (otherwise a Shift held while
/// unplugging a dock stays stuck down).
struct HeldKeys([u64; KEY_SLOTS / 64]);

impl HeldKeys {
    fn new() -> Self {
        Self([0; KEY_SLOTS / 64])
    }

    fn record(&mut self, code: u16, value: i32) {
        let code = code as usize;
        if code >= KEY_SLOTS {
            return;
        }
        let (word, bit) = (code / 64, 1u64 << (code % 64));
        match value {
            0 => self.0[word] &= !bit,
            1 => self.0[word] |= bit,
            _ => {}
        }
    }

    fn is_down(&self, code: KeyCode) -> bool {
        let code = code.code() as usize;
        code < KEY_SLOTS && self.0[code / 64] & (1 << (code % 64)) != 0
    }

    fn codes(&self) -> impl Iterator<Item = u16> + '_ {
        (0..KEY_SLOTS)
            .filter(|code| self.0[code / 64] & (1 << (code % 64)) != 0)
            .map(|code| code as u16)
    }
}

/// What a layered key sends: a key, plus modifiers pressed around it when it is a
/// shortcut (Workspace + Left/Right is Ctrl+Alt+arrow).
#[derive(Clone, Copy)]
struct Target {
    /// `KEY_RESERVED` for a mode key itself, which only changes what arrows send.
    key: KeyCode,
    /// The mode a mode key's press turned on, so its release can turn it off.
    mode: usize,
    /// Bit `i` set: this press pressed `CHORD_MODIFIERS[i]` and its release must release
    /// it. Modifiers already held (physically or through the layer) are left alone, so
    /// Caps Lock + S + D + J adds Shift and GNOME moves the window to the workspace.
    modifiers: u8,
}

const CHORD_MODIFIERS: [KeyCode; 3] = [
    KeyCode::KEY_LEFTCTRL,
    KeyCode::KEY_LEFTALT,
    KeyCode::KEY_LEFTMETA,
];
const CTRL: u8 = 1;
const ALT: u8 = 2;
const SUPER: u8 = 4;

impl Target {
    const fn plain(key: KeyCode) -> Self {
        Self {
            key,
            mode: 0,
            modifiers: 0,
        }
    }
}

const fn mode_of(action: Action) -> Option<usize> {
    match action {
        Action::Workspace => Some(MODE_WORKSPACE),
        Action::Focus => Some(MODE_FOCUS),
        _ => None,
    }
}

fn mode_active(mode: usize) -> bool {
    MODE_HOLDERS[mode].load(Ordering::Relaxed) > 0
}

/// One blocking `read` per batch of kernel events and one `write` per batch of output:
/// the kernel's own SYN_REPORT frame markers are forwarded instead of adding new ones.
fn run_device(mut device: RawDevice, mut output: File, name: &str) {
    super::elevate_input_thread();
    let mut held_targets = [None; InputKey::COUNT];
    let mut held = HeldKeys::new();
    let mut captured_caps = false;
    let mut mode_holds = [0usize; 2];
    let mut translated = Vec::with_capacity(64);

    loop {
        translated.clear();
        let mut has_keys = false;
        let events = match device.fetch_events() {
            Ok(events) => events,
            Err(error) => {
                if error.raw_os_error() == Some(libc::ENODEV) {
                    info!("{name} disconnected");
                } else {
                    error!("stopped reading {name}: {error}");
                }
                cleanup_state(captured_caps, mode_holds, &held, &mut output);
                return;
            }
        };

        for event in events {
            match event.event_type() {
                EventType::KEY => {}
                EventType::SYNCHRONIZATION if event.code() == SynchronizationCode::SYN_REPORT.0 => {
                    translated.push(event);
                    continue;
                }
                _ => continue,
            }

            let source = KeyCode::new(event.code());
            let value = event.value();
            if source == KeyCode::KEY_CAPSLOCK {
                if value == 1 && !captured_caps && BINDINGS.is_enabled() {
                    captured_caps = true;
                    LAYER_HOLDERS.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                if value == 0 && captured_caps {
                    captured_caps = false;
                    LAYER_HOLDERS.fetch_sub(1, Ordering::Relaxed);
                    continue;
                }
                if captured_caps {
                    continue;
                }
            }

            let input_key = key_from_linux(source);
            let target = match value {
                0 => input_key
                    .and_then(|key| held_targets[key.index()].take())
                    .unwrap_or(Target::plain(source)),
                2 => input_key
                    .and_then(|key| held_targets[key.index()])
                    .unwrap_or(Target::plain(source)),
                _ if LAYER_HOLDERS.load(Ordering::Relaxed) > 0 => {
                    match input_key.and_then(|key| Some((key, BINDINGS.action_for(key)?))) {
                        Some((key, action)) if mode_of(action).is_some() => {
                            let mode = mode_of(action).unwrap_or_default();
                            mode_holds[mode] += 1;
                            MODE_HOLDERS[mode].fetch_add(1, Ordering::Relaxed);
                            let target = Target {
                                mode,
                                ..Target::plain(KeyCode::KEY_RESERVED)
                            };
                            held_targets[key.index()] = Some(target);
                            target
                        }
                        Some((key, action)) => {
                            let target = press_chord(action, &mut held, &mut translated);
                            held_targets[key.index()] = Some(target);
                            target
                        }
                        None => Target::plain(source),
                    }
                }
                _ => Target::plain(source),
            };

            if target.key == KeyCode::KEY_RESERVED {
                if value == 0 {
                    mode_holds[target.mode] -= 1;
                    MODE_HOLDERS[target.mode].fetch_sub(1, Ordering::Relaxed);
                }
                continue;
            }
            has_keys = true;
            held.record(target.key.code(), value);
            translated.push(InputEvent::new(EventType::KEY.0, target.key.code(), value));
            if value == 0 && target.modifiers != 0 {
                for (bit, modifier) in CHORD_MODIFIERS.iter().enumerate() {
                    if target.modifiers & (1 << bit) != 0 {
                        held.record(modifier.code(), 0);
                        translated.push(InputEvent::new(EventType::KEY.0, modifier.code(), 0));
                    }
                }
            }
        }

        // A batch whose only key event was Caps Lock leaves nothing worth writing.
        if has_keys {
            if let Err(error) = output.write_all(as_bytes(&translated)) {
                error!("stopped writing events for {name}: {error}");
                cleanup_state(captured_caps, mode_holds, &held, &mut output);
                return;
            }
        }
    }
}

/// Presses the modifiers `action`'s shortcut needs that aren't already down, in their
/// own frame so the compositor sees them before the key, and returns what to send.
fn press_chord(action: Action, held: &mut HeldKeys, out: &mut Vec<InputEvent>) -> Target {
    let mut target = Target::plain(linux_action_code(action));
    let wanted = chord_modifiers(action, mode_active(MODE_WORKSPACE), mode_active(MODE_FOCUS));
    for (bit, modifier) in CHORD_MODIFIERS.iter().enumerate() {
        if wanted & (1 << bit) != 0 && !held.is_down(*modifier) {
            target.modifiers |= 1 << bit;
            held.record(modifier.code(), 1);
            out.push(InputEvent::new(EventType::KEY.0, modifier.code(), 1));
        }
    }
    if target.modifiers != 0 {
        out.push(InputEvent::new(
            EventType::SYNCHRONIZATION.0,
            SynchronizationCode::SYN_REPORT.0,
            0,
        ));
    }
    target
}

/// Modifiers (as `CHORD_MODIFIERS` bits) an action sends with its key. GNOME, KDE and
/// most other Linux desktops switch workspace with Ctrl+Alt+Left/Right. Focus uses
/// Super+Ctrl+Alt+arrow, which nothing in GNOME binds, for a tiling extension to take.
const fn chord_modifiers(action: Action, workspace: bool, focus: bool) -> u8 {
    match action {
        Action::ArrowLeft | Action::ArrowRight | Action::ArrowUp | Action::ArrowDown if focus => {
            SUPER | CTRL | ALT
        }
        Action::ArrowLeft | Action::ArrowRight if workspace => CTRL | ALT,
        _ => 0,
    }
}

fn cleanup_state(captured_caps: bool, mode_holds: [usize; 2], held: &HeldKeys, output: &mut File) {
    if captured_caps {
        LAYER_HOLDERS.fetch_sub(1, Ordering::Relaxed);
    }
    for (holders, holds) in MODE_HOLDERS.iter().zip(mode_holds) {
        holders.fetch_sub(holds, Ordering::Relaxed);
    }
    let mut releases: Vec<InputEvent> = held
        .codes()
        .map(|code| InputEvent::new(EventType::KEY.0, code, 0))
        .collect();
    if !releases.is_empty() {
        releases.push(InputEvent::new(
            EventType::SYNCHRONIZATION.0,
            SynchronizationCode::SYN_REPORT.0,
            0,
        ));
        let _ = output.write_all(as_bytes(&releases));
    }
}

fn as_bytes(events: &[InputEvent]) -> &[u8] {
    // SAFETY: `InputEvent` is `#[repr(transparent)]` over the kernel's plain-old-data
    // `input_event`, which is exactly the record format uinput reads.
    unsafe { std::slice::from_raw_parts(events.as_ptr().cast(), std::mem::size_of_val(events)) }
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
        Action::Workspace | Action::Focus => KeyCode::KEY_RESERVED,
    }
}
