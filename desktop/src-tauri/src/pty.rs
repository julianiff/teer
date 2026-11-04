//! The window's terminal pane: a program (the agent CLI the notes go to, a
//! terminal editor) in a pseudo-terminal, its output sent to the window
//! (xterm.js draws it) and the window's keys written back. When the
//! program exits, the pane says so and the caller hears how it ended.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use base64::Engine;
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde::Serialize;
use tauri::{AppHandle, Emitter, EventTarget, Manager};

/// The panes open, by id.
#[derive(Default)]
pub struct Ptys(Mutex<HashMap<u32, Pane>>);

struct Pane {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,
}

static NEXT: AtomicU32 = AtomicU32::new(1);

/// What the window is told when a pane opens.
#[derive(Debug, Clone, Serialize)]
pub struct Opened {
    pub id: u32,
    /// What runs in it: `Claude Code`, `nvim`.
    pub title: String,
}

#[derive(Debug, Clone, Serialize)]
struct Data {
    id: u32,
    /// The bytes, base64.
    data: String,
}

#[derive(Debug, Clone, Serialize)]
struct Exited {
    id: u32,
    code: u32,
}

/// The size a pane starts at, until the window says its own.
const START: PtySize = PtySize {
    rows: 40,
    cols: 120,
    pixel_width: 0,
    pixel_height: 0,
};

/// Run `cmd` in a new pane of window `label`, titled `title`. `exited`
/// runs once it has ended, with how.
pub fn spawn(
    handle: &AppHandle,
    label: &str,
    title: &str,
    mut cmd: CommandBuilder,
    exited: Box<dyn FnOnce(std::io::Result<std::process::ExitStatus>) + Send>,
) -> anyhow::Result<u32> {
    let pair = native_pty_system().openpty(START)?;
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    let mut child = pair.slave.spawn_command(cmd)?;
    // The program has the slave now; the pane keeps the master.
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader()?;
    let writer = pair.master.take_writer()?;
    let id = NEXT.fetch_add(1, Ordering::SeqCst);
    handle.state::<Ptys>().0.lock().expect("ptys").insert(
        id,
        Pane {
            master: pair.master,
            writer,
            killer: child.clone_killer(),
        },
    );
    let target = EventTarget::webview_window(label);
    let _ = handle.emit_to(
        target.clone(),
        "pty-open",
        Opened {
            id,
            title: title.to_string(),
        },
    );
    let h = handle.clone();
    let out = target.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 16 * 1024];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let data = base64::engine::general_purpose::STANDARD.encode(&buf[..n]);
                    let _ = h.emit_to(out.clone(), "pty-data", Data { id, data });
                }
            }
        }
    });
    let h = handle.clone();
    std::thread::spawn(move || {
        let status = child.wait();
        h.state::<Ptys>().0.lock().expect("ptys").remove(&id);
        let code = status.as_ref().map_or(1, |s| s.exit_code());
        let _ = h.emit_to(target, "pty-exit", Exited { id, code });
        exited(status.map(|s| exit_status(s.exit_code())));
    });
    Ok(id)
}

/// `code` as the standard library's exit status.
fn exit_status(code: u32) -> std::process::ExitStatus {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        std::process::ExitStatus::from_raw(((code & 0xff) << 8) as i32)
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::ExitStatusExt;
        std::process::ExitStatus::from_raw(code)
    }
}

/// Keys the window read in pane `id`.
#[tauri::command]
pub fn pty_write(ptys: tauri::State<'_, Ptys>, id: u32, data: String) -> Result<(), String> {
    let mut map = ptys.0.lock().map_err(|e| e.to_string())?;
    let pane = map.get_mut(&id).ok_or("The program has ended.")?;
    pane.writer
        .write_all(data.as_bytes())
        .and_then(|_| pane.writer.flush())
        .map_err(|e| e.to_string())
}

/// The pane was laid out at `cols` × `rows`.
#[tauri::command]
pub fn pty_resize(
    ptys: tauri::State<'_, Ptys>,
    id: u32,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let map = ptys.0.lock().map_err(|e| e.to_string())?;
    let pane = map.get(&id).ok_or("The program has ended.")?;
    pane.master
        .resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| e.to_string())
}

/// End the program in pane `id` (the window closed it).
#[tauri::command]
pub fn pty_kill(ptys: tauri::State<'_, Ptys>, id: u32) -> Result<(), String> {
    let mut map = ptys.0.lock().map_err(|e| e.to_string())?;
    if let Some(pane) = map.get_mut(&id) {
        pane.killer.kill().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_survive_the_conversion() {
        assert!(exit_status(0).success());
        assert_eq!(exit_status(3).code(), Some(3));
    }
}
