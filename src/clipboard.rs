//! Copying text to the system clipboard: through the platform's program, or
//! through the terminal itself (OSC 52) when there is none or teer runs
//! over ssh, where the clipboard that matters is the terminal's.

use std::io::Write;
use std::process::{Command, Stdio};

/// How the text reached the clipboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    Program(&'static str),
    /// OSC 52: the terminal puts it there, if it allows that.
    Terminal,
}

/// The clipboard programs tried, in order, with their arguments.
fn programs() -> &'static [(&'static str, &'static [&'static str])] {
    if cfg!(target_os = "macos") {
        &[("pbcopy", &[])]
    } else {
        &[
            ("wl-copy", &[]),
            ("xclip", &["-selection", "clipboard"]),
            ("xsel", &["--clipboard", "--input"]),
        ]
    }
}

/// Copy `text` to the clipboard.
pub fn copy(text: &str) -> Via {
    let remote =
        std::env::var_os("SSH_TTY").is_some() || std::env::var_os("SSH_CONNECTION").is_some();
    if !remote {
        for (prog, args) in programs() {
            if pipe(prog, args, text) {
                return Via::Program(prog);
            }
        }
    }
    let tmux = std::env::var_os("TMUX").is_some();
    let mut out = std::io::stdout();
    let _ = out.write_all(osc52(text, tmux).as_bytes());
    let _ = out.flush();
    Via::Terminal
}

/// Run `prog` with `text` on its stdin; whether it took it.
fn pipe(prog: &str, args: &[&str], text: &str) -> bool {
    let Ok(mut child) = Command::new(prog)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    let wrote = child
        .stdin
        .take()
        .is_some_and(|mut s| s.write_all(text.as_bytes()).is_ok());
    child.wait().is_ok_and(|s| s.success()) && wrote
}

/// The escape sequence that asks the terminal to set its clipboard to
/// `text`; inside tmux, wrapped so tmux passes it on.
fn osc52(text: &str, tmux: bool) -> String {
    let seq = format!("\x1b]52;c;{}\x07", base64(text.as_bytes()));
    if tmux {
        format!("\x1bPtmux;{}\x1b\\", seq.replace('\x1b', "\x1b\x1b"))
    } else {
        seq
    }
}

fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let byte = |i: usize| u32::from(chunk.get(i).copied().unwrap_or(0));
        let n = (byte(0) << 16) | (byte(1) << 8) | byte(2);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ABC[(n >> (18 - 6 * i)) as usize & 63] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_pads_like_the_standard() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64("ü→".as_bytes()), "w7zihpI=");
    }

    #[test]
    fn osc52_is_wrapped_for_tmux() {
        assert_eq!(osc52("hi", false), "\x1b]52;c;aGk=\x07");
        assert_eq!(osc52("hi", true), "\x1bPtmux;\x1b\x1b]52;c;aGk=\x07\x1b\\");
    }
}
