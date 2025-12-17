use anyhow::Result;
use rdev::{grab, Event, EventType, Key};
use std::thread;

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
    // Placeholder for remapping logic
    // For now just pass through
    Some(event)
}
