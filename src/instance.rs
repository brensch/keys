//! One nocaps per session. A second launch (from the app grid, a terminal, or a
//! duplicate autostart entry) would find every keyboard already grabbed, so instead
//! it asks the running instance to open its settings window and exits.

use crate::app::{Command, Inbox};
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::Arc;

pub enum Instance {
    /// This process is the only nocaps; later launches connect to the listener.
    Primary(Option<UnixListener>),
    /// Another nocaps is running and has been asked to show itself.
    Secondary,
}

pub fn claim() -> Instance {
    let Some(path) = socket_path() else {
        // No per-user runtime directory to hold the socket; run without the guard.
        return Instance::Primary(None);
    };
    if let Ok(mut stream) = UnixStream::connect(&path) {
        if stream.write_all(b"show").is_ok() {
            return Instance::Secondary;
        }
    }
    // Nobody answered, so any socket file left behind belongs to a nocaps that died.
    let _ = std::fs::remove_file(&path);
    match UnixListener::bind(&path) {
        Ok(listener) => Instance::Primary(Some(listener)),
        Err(error) => {
            log::warn!(
                "single-instance socket {} unavailable: {error}",
                path.display()
            );
            Instance::Primary(None)
        }
    }
}

/// Turns every connection from a later launch into a request to show the window.
pub fn serve(listener: UnixListener, inbox: Arc<Inbox>) {
    let spawned = std::thread::Builder::new()
        .name("nocaps-instance".to_owned())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut request = [0u8; 16];
                let mut stream = stream;
                let _ = stream.read(&mut request);
                inbox.push(Command::Configure);
            }
        });
    if let Err(error) = spawned {
        log::warn!("cannot listen for later nocaps launches: {error}");
    }
}

fn socket_path() -> Option<PathBuf> {
    // XDG_RUNTIME_DIR is private to the user, so no one else can squat the socket.
    let dir = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?);
    dir.is_dir().then(|| dir.join("nocaps.sock"))
}
