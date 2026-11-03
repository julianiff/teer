//! The setup screen: picking an agent, its models, the test answer, and
//! what is drawn on the way.

use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use teer::config::{ApiFormat, Config, Provider};
use teer::keymap::{Keymap, parse_keys};
use teer::setup::{Found, PRESETS, Tool};
use teer::tui::setup::{Effect, Opened, Reply, Setup, Step, draw};

fn setup(cfg: Config, opened: Opened) -> Setup {
    Setup::new(cfg, opened, Keymap::default())
}

/// Type `keys`, in Neovim notation; what the last one did.
fn keys(s: &mut Setup, keys: &str) -> Option<Effect> {
    let mut last = None;
    for k in parse_keys(keys).unwrap() {
        last = s.on_key(k.event());
    }
    last
}

fn press(s: &mut Setup, code: KeyCode) -> Option<Effect> {
    s.on_key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn typed(s: &mut Setup, text: &str) {
    for c in text.chars() {
        press(s, KeyCode::Char(c));
    }
}

fn screen(s: &Setup) -> String {
    let mut t = Terminal::new(TestBackend::new(100, 22)).unwrap();
    t.draw(|f| draw(f, s)).unwrap();
    let buf = t.backend().buffer().clone();
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn found(s: &mut Setup) {
    let f = PRESETS
        .iter()
        .map(|p| match p.provider {
            Provider::Claude => Some(true),
            Provider::Codex => Some(false),
            Provider::Api if p.key_env.is_empty() => None,
            Provider::Api => Some(p.key_env == "ANTHROPIC_API_KEY"),
        })
        .collect();
    s.apply(Reply::Found(f));
    let tool = |name: &str, needed, found, about: &str| Tool {
        name: name.into(),
        needed,
        found,
        about: about.into(),
    };
    s.apply(Reply::Tools(vec![
        tool(
            "git",
            true,
            Found::Ready("found".into()),
            "reads the changes",
        ),
        tool(
            "gh",
            false,
            Found::Broken("not logged in".into()),
            "run `gh auth login`",
        ),
        tool(
            "nvim",
            false,
            Found::Missing("not installed".into()),
            "`gf` opens a unit in it",
        ),
        tool(
            "sandbox-exec",
            false,
            Found::Ready("found".into()),
            "Behavior runs code in it",
        ),
    ]));
}

fn models(s: &mut Setup, list: &[&str]) {
    let n = s.asked;
    s.apply(Reply::Models(
        n,
        Ok(list.iter().map(|m| m.to_string()).collect()),
    ));
}

#[test]
fn claude_code_with_its_suggested_models() {
    let mut s = setup(Config::default(), Opened::First);
    found(&mut s);
    let first = screen(&s);
    assert!(first.contains("What prepares your reviews?"), "{first}");
    assert!(first.contains("▌ Claude Code"), "{first}");
    assert!(first.contains("found"), "{first}");
    assert!(first.contains("not installed"), "{first}");
    assert!(first.contains("ANTHROPIC_API_KEY is set"), "{first}");
    assert!(first.contains("Also used"), "{first}");
    assert!(
        first.contains("gh              not logged in") && first.contains("run `gh auth login`"),
        "{first}"
    );
    assert!(
        first.contains("l/⏎ next   j/k move   h/esc not now"),
        "{first}"
    );
    assert!(first.contains("1/4"), "{first}");
    insta::assert_snapshot!("agents", first);

    assert_eq!(press(&mut s, KeyCode::Enter), Some(Effect::Models));
    assert_eq!(s.step, Step::Fast);
    models(&mut s, &["haiku", "sonnet", "opus"]);
    let fast = screen(&s);
    assert!(fast.contains("Fast model"), "{fast}");
    assert!(
        fast.contains("▌ haiku") && fast.contains("suggested"),
        "{fast}"
    );
    assert!(
        fast.contains("default") && fast.contains("Claude Code's own model"),
        "{fast}"
    );

    assert_eq!(press(&mut s, KeyCode::Enter), None);
    assert_eq!(s.step, Step::Second);
    let second = screen(&s);
    assert!(second.contains("▌ sonnet"), "{second}");
    assert!(
        second.contains("none") && second.contains("no second opinion"),
        "{second}"
    );

    assert_eq!(press(&mut s, KeyCode::Enter), Some(Effect::Check));
    assert!(screen(&s).contains("asking haiku for a test answer"));
    // Saving waits for the test answer.
    assert_eq!(press(&mut s, KeyCode::Enter), None);
    let n = s.asked;
    s.apply(Reply::Checked(n, Ok(Duration::from_millis(6200))));
    let ready = screen(&s);
    assert!(ready.contains("✓ haiku answered in 6.2s"), "{ready}");
    assert_eq!(press(&mut s, KeyCode::Enter), Some(Effect::Save));
    let c = s.choice(s.second_choice());
    assert_eq!(
        (c.provider, c.fast.as_str(), c.second.as_deref()),
        (Provider::Claude, "haiku", Some("sonnet"))
    );
}

#[test]
fn an_api_with_a_typed_model_and_no_second_opinion() {
    let mut s = setup(Config::default(), Opened::Command);
    let at = PRESETS.iter().position(|p| p.name == "OpenRouter").unwrap();
    for _ in 0..at {
        press(&mut s, KeyCode::Down);
    }
    assert_eq!(press(&mut s, KeyCode::Enter), None);
    assert_eq!(s.step, Step::Api);
    let api = screen(&s);
    assert!(api.contains("https://openrouter.ai/api/v1"), "{api}");
    assert!(api.contains("OPENROUTER_API_KEY"), "{api}");
    assert!(api.contains("2/5"), "{api}");
    assert_eq!(press(&mut s, KeyCode::Enter), Some(Effect::Models));
    let n = s.asked;
    s.apply(Reply::Models(
        n,
        Err("$OPENROUTER_API_KEY isn't set".into()),
    ));
    let failed = screen(&s);
    assert!(
        failed.contains("couldn't list the models: $OPENROUTER_API_KEY isn't set"),
        "{failed}"
    );
    // No list: what is typed is the model.
    typed(&mut s, "vendor/small:free");
    assert!(screen(&s).contains("as typed"));
    press(&mut s, KeyCode::Enter);
    assert_eq!(s.step, Step::Second);
    // `none` ends the list.
    press(&mut s, KeyCode::Down);
    assert_eq!(press(&mut s, KeyCode::Enter), Some(Effect::Check));
    let n = s.asked;
    s.apply(Reply::Checked(n, Err("401: invalid key".into())));
    let check = screen(&s);
    assert!(check.contains("✗ the test answer failed"), "{check}");
    assert!(check.contains("save anyway"), "{check}");
    let c = s.choice(s.second_choice());
    assert_eq!(c.provider, Provider::Api);
    assert_eq!(c.api.format, ApiFormat::Openai);
    assert_eq!(c.api.base_url, "https://openrouter.ai/api/v1");
    assert_eq!(c.api.key_env, "OPENROUTER_API_KEY");
    assert_eq!((c.fast.as_str(), c.second), ("vendor/small:free", None));
}

#[test]
fn filtering_back_and_leaving() {
    let mut s = setup(Config::default(), Opened::Command);
    press(&mut s, KeyCode::Enter);
    models(&mut s, &["haiku", "sonnet", "opus"]);
    typed(&mut s, "op");
    let f = screen(&s);
    assert!(f.contains("filter  op"), "{f}");
    assert!(f.contains("▌ op") && f.contains("as typed"), "{f}");
    assert!(f.contains("opus") && !f.contains("haiku"), "{f}");
    press(&mut s, KeyCode::Down);
    press(&mut s, KeyCode::Enter);
    assert_eq!(s.fast.as_deref(), Some("opus"));
    // Back to the fast model, back to the agents, and out.
    press(&mut s, KeyCode::Esc);
    assert_eq!(s.step, Step::Fast);
    press(&mut s, KeyCode::Esc);
    assert_eq!(s.step, Step::Provider);
    assert!(screen(&s).contains("h/esc quit"));
    assert_eq!(press(&mut s, KeyCode::Esc), Some(Effect::Leave));
    let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert_eq!(s.on_key(ctrl_c), Some(Effect::Quit));
}

#[test]
fn programs_show_while_looking() {
    let s = setup(Config::default(), Opened::Command);
    let looking = screen(&s);
    for name in ["git", "gh", "nvim", "sandbox-exec"] {
        assert!(looking.contains(&format!("    {name}")), "{looking}");
    }
    // The editor is the one the config names.
    let cfg: Config = toml::from_str(
        "editor = \"'/Applications/IntelliJ IDEA.app/Contents/MacOS/idea' --line {line} {file}\"",
    )
    .unwrap();
    let s = setup(cfg, Opened::Command);
    let looking = screen(&s);
    assert!(looking.contains("    idea "), "{looking}");
    assert!(!looking.contains("nvim"), "{looking}");
}

#[test]
fn an_answer_to_an_older_request_is_dropped() {
    let mut s = setup(Config::default(), Opened::Command);
    press(&mut s, KeyCode::Enter);
    s.asked = 2;
    s.apply(Reply::Models(1, Ok(vec!["stale".into()])));
    assert!(!screen(&s).contains("stale"));
}

#[test]
fn it_starts_on_what_the_config_has() {
    let cfg: Config = toml::from_str(
        "provider = \"api\"\nprepare_model = \"qwen3:8b\"\n[api]\nformat = \"openai\"\nbase_url = \"http://localhost:11434/v1\"\nkey_env = \"\"\n",
    )
    .unwrap();
    let mut s = setup(cfg, Opened::Command);
    assert_eq!(s.chosen().name, "Ollama");
    press(&mut s, KeyCode::Enter);
    press(&mut s, KeyCode::Enter);
    models(&mut s, &["llama3", "qwen3:8b"]);
    assert!(screen(&s).contains("▌ qwen3:8b"), "{}", screen(&s));
}

#[test]
fn the_agents_move_with_the_keymap() {
    let mut s = setup(Config::default(), Opened::First);
    let last = PRESETS.len() - 1;
    keys(&mut s, "j");
    assert_eq!(s.preset, 1);
    keys(&mut s, "kk");
    assert_eq!(s.preset, last, "wraps around");
    keys(&mut s, "gg");
    assert_eq!(s.preset, 0);
    keys(&mut s, "G");
    assert_eq!(s.preset, last);
    keys(&mut s, "gg2j");
    assert_eq!(s.preset, 2);
    keys(&mut s, "<home><down>");
    assert_eq!(s.preset, 1);
    // `l` picks, as enter does.
    keys(&mut s, "gg");
    assert_eq!(keys(&mut s, "l"), Some(Effect::Models));
    assert_eq!(s.step, Step::Fast);
}

#[test]
fn in_the_model_filter_letters_are_text() {
    let mut s = setup(Config::default(), Opened::Command);
    keys(&mut s, "<cr>");
    models(&mut s, &["haiku", "sonnet", "opus"]);
    keys(&mut s, "hjklq");
    assert_eq!(s.step, Step::Fast);
    assert_eq!(s.filter, "hjklq");
    keys(&mut s, "<c-u>");
    assert_eq!(s.filter, "");
    // Arrows and ctrl+n / ctrl+p move.
    s.sel = 0;
    keys(&mut s, "<c-n><c-n>");
    assert_eq!(s.sel, 2);
    keys(&mut s, "<c-p><up>");
    assert_eq!(s.sel, 0);
    keys(&mut s, "<down>");
    assert_eq!(s.sel, 1);
}

#[test]
fn h_leaves_the_first_step_and_q_the_last() {
    let mut s = setup(Config::default(), Opened::Command);
    assert_eq!(keys(&mut s, "h"), Some(Effect::Leave));
    assert_eq!(keys(&mut s, "q"), Some(Effect::Leave));
    keys(&mut s, "<cr>");
    models(&mut s, &["haiku", "sonnet"]);
    keys(&mut s, "<cr>");
    assert_eq!(keys(&mut s, "<cr>"), Some(Effect::Check));
    let n = s.asked;
    s.apply(Reply::Checked(n, Ok(Duration::from_millis(900))));
    let check = screen(&s);
    assert!(check.contains("l/⏎ save   h/esc back   q quit"), "{check}");
    assert!(check.contains("l/⏎ saves it to "), "{check}");
    // `h` goes back a step; `q` leaves without saving.
    keys(&mut s, "h");
    assert_eq!(s.step, Step::Second);
    assert_eq!(keys(&mut s, "<cr>"), Some(Effect::Check));
    assert_eq!(keys(&mut s, "l"), None, "saving waits for the test answer");
    s.apply(Reply::Checked(n, Ok(Duration::from_millis(900))));
    assert_eq!(keys(&mut s, "q"), Some(Effect::Leave));
    assert_eq!(keys(&mut s, "l"), Some(Effect::Save));
}

#[test]
fn from_a_review_leaving_goes_back_to_it() {
    let s = setup(Config::default(), Opened::Review);
    let first = screen(&s);
    assert!(first.contains("h/esc back"), "{first}");
    assert!(!first.contains("`teer setup` changes it later"), "{first}");
}

#[test]
fn rebound_keys_work_and_are_shown() {
    use std::collections::HashMap;
    use teer::keymap::KeySpec;
    let over = HashMap::from([
        ("view.open".to_string(), KeySpec::One("o".into())),
        ("move.down".to_string(), KeySpec::One("<c-j>".into())),
    ]);
    let (km, _) = Keymap::new(&over, 1000).unwrap();
    let mut s = Setup::new(Config::default(), Opened::First, km);
    found(&mut s);
    let first = screen(&s);
    assert!(first.contains("o next   ^j/k move"), "{first}");
    keys(&mut s, "j");
    assert_eq!(s.preset, 0, "j no longer moves");
    keys(&mut s, "<c-j>");
    assert_eq!(s.preset, 1);
    assert_eq!(keys(&mut s, "<cr>"), None, "enter no longer picks");
    assert_eq!(keys(&mut s, "o"), Some(Effect::Models));
}
