//! Live check of sending to a real, running agent session. Ignored by
//! default: it messages the session, which then runs a turn (tokens).
//! Start a throwaway session in a throwaway repo, then:
//!
//! ```sh
//! TEER_LIVE_REPO=/path/to/repo TEER_LIVE_SESSION=<session id> \
//!   cargo test --test it live_send:: -- --ignored --nocapture
//! ```
//!
//! It reads the real agent folders (`~/.claude`, `~/.codex`, `~/.pi`)
//! and sends to the one session named, never to another.

use std::path::PathBuf;

use teer::send::{self, Dirs};

#[test]
#[ignore]
fn the_session_named_gets_the_message() {
    let (Ok(repo), Ok(id)) = (
        std::env::var("TEER_LIVE_REPO"),
        std::env::var("TEER_LIVE_SESSION"),
    ) else {
        eprintln!("TEER_LIVE_REPO and TEER_LIVE_SESSION are not set: skipped");
        return;
    };
    let home = PathBuf::from(std::env::var("HOME").unwrap());
    let dirs = Dirs {
        claude: home.join(".claude"),
        codex: home.join(".codex"),
        pi: home.join(".pi").join("agent"),
    };
    let targets = send::list(&dirs, &PathBuf::from(&repo));
    for t in &targets {
        eprintln!("{} {} {} {}", t.agent.title(), t.name, t.id, t.status);
    }
    let t = targets
        .iter()
        .find(|t| t.id == id)
        .unwrap_or_else(|| panic!("session {id} isn't among {targets:?}"));
    send::deliver(
        &dirs,
        t,
        "This is a delivery test from teer. Reply with exactly: pong-teer",
    )
    .unwrap();
}
