//! The setup screen: picking an agent, its models, the test answer, and
//! what is drawn on the way.

use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use teer::config::{ApiFormat, Config, Provider};
use teer::setup::{Found, PRESETS, Tool};
use teer::tui::setup::{Effect, Reply, Setup, Step, draw};

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
            "`e` opens a unit in it",
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
    let mut s = Setup::new(Config::default(), true);
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
    assert!(first.contains("esc not now"), "{first}");
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
    let mut s = Setup::new(Config::default(), false);
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
    let mut s = Setup::new(Config::default(), false);
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
    assert!(screen(&s).contains("esc quit"));
    assert_eq!(press(&mut s, KeyCode::Esc), Some(Effect::Leave));
    let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert_eq!(s.on_key(ctrl_c), Some(Effect::Quit));
}

#[test]
fn programs_show_while_looking() {
    let s = Setup::new(Config::default(), false);
    let looking = screen(&s);
    for name in ["git", "gh", "nvim", "sandbox-exec"] {
        assert!(looking.contains(&format!("    {name}")), "{looking}");
    }
    // The editor is the one the config names.
    let cfg: Config = toml::from_str(
        "editor = \"'/Applications/IntelliJ IDEA.app/Contents/MacOS/idea' --line {line} {file}\"",
    )
    .unwrap();
    let s = Setup::new(cfg, false);
    let looking = screen(&s);
    assert!(looking.contains("    idea "), "{looking}");
    assert!(!looking.contains("nvim"), "{looking}");
}

#[test]
fn an_answer_to_an_older_request_is_dropped() {
    let mut s = Setup::new(Config::default(), false);
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
    let mut s = Setup::new(cfg, false);
    assert_eq!(s.chosen().name, "Ollama");
    press(&mut s, KeyCode::Enter);
    press(&mut s, KeyCode::Enter);
    models(&mut s, &["llama3", "qwen3:8b"]);
    assert!(screen(&s).contains("▌ qwen3:8b"), "{}", screen(&s));
}
