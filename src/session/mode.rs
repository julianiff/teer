//! The review's mode in the session (see [`crate::mode`]): which actions
//! it has, changing it, and the picker (`<space>m`) that lists the modes
//! with what each is for.

use super::app::{App, Effect, Overlay, Screen};
use crate::keymap::{Action, Edit, Key};
use crate::mode::Mode;

impl App {
    /// Does the mode have `a`?
    pub fn allows(&self, a: Action) -> bool {
        self.mode.allows(a)
    }

    /// What a key says whose action the mode leaves out.
    pub(crate) fn not_in_mode(&self, a: Action) -> String {
        format!(
            "Not in {} mode: {}. {} changes the mode.",
            self.mode.name(),
            a.describe(),
            self.keys.label(Action::ToolMode)
        )
    }

    /// Put the review in `mode`: the queue in its order (the security
    /// order for `security`), the unit view with its tiles. The cursor
    /// stays on its unit. The caller says so and saves.
    pub fn set_mode(&mut self, mode: Mode) {
        let was = self.mode;
        self.mode = mode;
        if mode.security_order() {
            self.security.by_exposure = self.security.enabled;
        } else if mode != Mode::All || was.security_order() {
            // `all` keeps what `zs` set, but not another mode's order.
            self.security.by_exposure = false;
        }
        self.rebuild_order();
        if self.screen == Screen::Unit {
            self.unit_mode_changed();
        }
    }

    /// `<space>m`: the picker, on the mode the review is in.
    pub(crate) fn open_mode_picker(&mut self) {
        let sel = Mode::EVERY
            .iter()
            .position(|&m| m == self.mode)
            .unwrap_or(0);
        self.overlay = Overlay::Mode { sel };
    }

    /// Keys of the mode picker: `j`/`k` (and the list's other moves)
    /// choose, `enter` (or `view.open`) takes the mode, `esc`, `h` or `q`
    /// leave it as it was.
    pub(crate) fn mode_key(&mut self, k: Key) -> Vec<Effect> {
        let action = match self.keys.edit(k) {
            Edit::Submit => {
                self.keys.reset();
                Some(Action::ViewOpen)
            }
            Edit::Cancel => {
                self.keys.reset();
                Some(Action::ViewBack)
            }
            _ => self.panel_action(k),
        };
        let last = Mode::EVERY.len() - 1;
        let Overlay::Mode { sel } = &mut self.overlay else {
            return Vec::new();
        };
        match action {
            Some(Action::MoveDown) => *sel = (*sel + 1).min(last),
            Some(Action::MoveUp) => *sel = sel.saturating_sub(1),
            Some(Action::MoveFirst) => *sel = 0,
            Some(Action::MoveLast) => *sel = last,
            Some(Action::ViewOpen) => {
                let i = *sel;
                return self.mode_pick(i);
            }
            Some(Action::ViewBack | Action::ViewClose | Action::ToolMode) => {
                self.overlay = Overlay::None;
            }
            _ => {}
        }
        Vec::new()
    }

    /// Take mode `i` of the picker (`enter`, or a click on it): the
    /// picker closes, the review is in that mode from now on, and it is
    /// saved with it.
    pub fn mode_pick(&mut self, i: usize) -> Vec<Effect> {
        if !matches!(self.overlay, Overlay::Mode { .. }) {
            return Vec::new();
        }
        self.overlay = Overlay::None;
        let Some(&mode) = Mode::EVERY.get(i) else {
            return Vec::new();
        };
        self.choose_mode(mode)
    }

    /// The reviewer chose `mode`: set it, keep it with the review, say so.
    pub fn choose_mode(&mut self, mode: Mode) -> Vec<Effect> {
        self.set_mode(mode);
        self.review.mode = Some(mode);
        self.status = if mode == Mode::Security && !self.security.enabled {
            "Mode: security, but the security lens is off (enabled = false in security.toml)."
                .into()
        } else {
            format!("Mode: {}. {}.", mode.name(), capitalized(mode.about()))
        };
        vec![Effect::Save]
    }
}

fn capitalized(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::parse_keys;
    use crate::review::model::DiveKind;
    use crate::session::unit::Panel;

    fn app() -> App {
        let mut a = crate::session::app::tests::app(3);
        a.security.enabled = true;
        a
    }

    fn press(a: &mut App, keys: &str) -> Vec<Effect> {
        let mut out = Vec::new();
        for k in parse_keys(keys).unwrap() {
            out.extend(a.on_key(k));
        }
        out
    }

    #[test]
    fn an_app_starts_with_every_tool_until_a_mode_is_named() {
        let a = app();
        assert_eq!(a.mode, Mode::All);
        assert!(Action::all().all(|x| a.allows(x)));
    }

    #[test]
    fn a_key_the_mode_leaves_out_does_nothing_but_say_so() {
        let mut a = app();
        a.set_mode(Mode::Review);
        let e = press(&mut a, "<space>x");
        assert!(e.is_empty(), "{e:?}");
        assert_eq!(a.screen, Screen::Queue);
        assert_eq!(
            a.status,
            "Not in review mode: security: how the change can be abused. ␣m changes the mode."
        );
        // What the mode has still works.
        press(&mut a, "l");
        assert_eq!(a.screen, Screen::Unit);
    }

    #[test]
    fn the_picker_changes_the_mode_and_keeps_it_with_the_review() {
        let mut a = app();
        a.set_mode(Mode::Review);
        press(&mut a, "<space>m");
        assert_eq!(a.overlay, Overlay::Mode { sel: 0 });
        press(&mut a, "j");
        assert_eq!(a.overlay, Overlay::Mode { sel: 1 });
        let e = press(&mut a, "<cr>");
        assert_eq!(e, vec![Effect::Save]);
        assert_eq!(a.overlay, Overlay::None);
        assert_eq!(a.mode, Mode::Security);
        assert_eq!(a.review.mode, Some(Mode::Security));
        assert!(a.security.by_exposure);
        assert!(a.status.starts_with("Mode: security."), "{}", a.status);
        // esc leaves the mode as it was.
        press(&mut a, "<space>m<esc>");
        assert_eq!(a.overlay, Overlay::None);
        assert_eq!(a.mode, Mode::Security);
    }

    #[test]
    fn leaving_security_ends_its_order() {
        let mut a = app();
        a.set_mode(Mode::Security);
        assert!(a.security.by_exposure);
        a.set_mode(Mode::Review);
        assert!(!a.security.by_exposure);
        a.set_mode(Mode::Security);
        a.set_mode(Mode::All);
        assert!(!a.security.by_exposure);
    }

    #[test]
    fn the_unit_view_opens_on_the_ask_tile_in_every_mode() {
        let mut a = app();
        a.has_agent = true;
        a.review
            .set_session(Some(crate::review::model::SessionRef::fresh(
                "claude",
                a.review.repo_root.clone(),
            )));
        let security = Panel::Tool(DiveKind::Security);
        a.set_mode(Mode::Security);
        let e = press(&mut a, "l");
        // The ask tile, as in every mode, and nothing asked yet.
        assert_eq!(a.unit.tiles, vec![Panel::Ask]);
        assert!(!e.iter().any(|x| matches!(x, Effect::Dive { .. })), "{e:?}");
        // The full check's key opens its tile and asks.
        let e = press(&mut a, "<space>x");
        assert!(
            e.iter().any(|x| matches!(
                x,
                Effect::Dive {
                    kind: DiveKind::Security,
                    ..
                }
            )),
            "{e:?}"
        );
        assert_eq!(a.unit.panel, Some(security));
        // Another mode: its tools' tiles close.
        press(&mut a, "<space>mgg<cr>");
        assert_eq!(a.mode, Mode::Review);
        assert_eq!(a.unit.tiles, vec![Panel::Ask]);
    }
}
