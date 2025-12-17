use anyhow::Result;
use rdev::{grab, Event, EventType, Key};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

static CAPS_LOCK_HELD: AtomicBool = AtomicBool::new(false);

pub struct KeyboardManager;

impl KeyboardManager {
    pub fn new() -> Result<Self> {
        thread::spawn(|| {
            if let Err(error) = grab(callback) {
                println!("Error: {:?}", error)
            }
        });
        Ok(Self)
    }
}

fn callback(event: Event) -> Option<Event> {
    match event.event_type {
        EventType::KeyPress(key) => {
            if key == Key::CapsLock {
                CAPS_LOCK_HELD.store(true, Ordering::SeqCst);
                return None;
            }

            if CAPS_LOCK_HELD.load(Ordering::SeqCst) {
                let new_key = match key {
                    Key::KeyA => Some(Key::ControlLeft),
                    Key::KeyS => Some(Key::ShiftLeft),
                    Key::KeyI => Some(Key::UpArrow),
                    Key::KeyK => Some(Key::DownArrow),
                    Key::KeyJ => Some(Key::LeftArrow),
                    Key::KeyL => Some(Key::RightArrow),
                    Key::KeyH => Some(Key::Home), // Assuming Home exists
                    Key::SemiColon => Some(Key::End), // Assuming End exists
                    Key::KeyW => Some(Key::VolumeUp), // Assuming VolumeUp exists
                    _ => None,
                };

                if let Some(k) = new_key {
                    return Some(Event {
                        event_type: EventType::KeyPress(k),
                        name: None,
                        time: event.time,
                    });
                }
            }
        }
        EventType::KeyRelease(key) => {
            if key == Key::CapsLock {
                CAPS_LOCK_HELD.store(false, Ordering::SeqCst);
                return None;
            }

            if CAPS_LOCK_HELD.load(Ordering::SeqCst) {
                let new_key = match key {
                    Key::KeyA => Some(Key::ControlLeft),
                    Key::KeyS => Some(Key::ShiftLeft),
                    Key::KeyI => Some(Key::UpArrow),
                    Key::KeyK => Some(Key::DownArrow),
                    Key::KeyJ => Some(Key::LeftArrow),
                    Key::KeyL => Some(Key::RightArrow),
                    Key::KeyH => Some(Key::Home),
                    Key::SemiColon => Some(Key::End),
                    Key::KeyW => Some(Key::VolumeUp),
                    _ => None,
                };

                if let Some(k) = new_key {
                    return Some(Event {
                        event_type: EventType::KeyRelease(k),
                        name: None,
                        time: event.time,
                    });
                }
            }
        }
        _ => {}
    }
    Some(event)
}
