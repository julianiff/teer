//! Colors. The default view stays quiet: one accent color, muted chrome,
//! and color on markers only where it carries meaning (risk).

use ratatui::style::{Color, Modifier, Style};

use crate::config::Theme as ThemeName;
use crate::review::model::{Marker, Risk};

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub name: ThemeName,
    pub accent: Color,
    pub muted: Color,
    pub text: Color,
    pub line_no: Color,
    pub risk_medium: Color,
    pub risk_high: Color,
    pub added: Color,
    pub removed: Color,
    pub changed: Color,
    pub keyword: Color,
    pub string: Color,
    pub comment: Color,
    pub number: Color,
    pub function: Color,
    /// Classes and other types.
    pub class: Color,
    pub variable: Color,
    /// What the agent wrote into the code: a pale accent, apart from it,
    /// in a card with a muted accent border.
    pub note: Color,
    pub note_border: Color,
    pub bar: Color,
    /// Behind removed lines of a code change, whose text keeps its syntax
    /// colours.
    pub removed_bg: Color,
}

impl Theme {
    pub fn new(name: ThemeName) -> Theme {
        match name {
            ThemeName::Dark => Theme {
                name,
                accent: Color::Cyan,
                muted: Color::DarkGray,
                text: Color::Reset,
                line_no: Color::DarkGray,
                risk_medium: Color::Yellow,
                risk_high: Color::Red,
                added: Color::Green,
                removed: Color::Red,
                changed: Color::Yellow,
                keyword: Color::Rgb(198, 120, 221),
                string: Color::Rgb(152, 195, 121),
                comment: Color::Rgb(127, 132, 142),
                number: Color::Rgb(209, 154, 102),
                function: Color::Rgb(97, 175, 239),
                class: Color::Rgb(229, 192, 123),
                variable: Color::Rgb(86, 182, 194),
                note: Color::Rgb(162, 200, 214),
                note_border: Color::Rgb(68, 104, 118),
                bar: Color::Rgb(60, 60, 70),
                removed_bg: Color::Rgb(56, 24, 28),
            },
            ThemeName::Light => Theme {
                name,
                accent: Color::Blue,
                muted: Color::Gray,
                text: Color::Reset,
                line_no: Color::Gray,
                risk_medium: Color::Rgb(176, 120, 0),
                risk_high: Color::Red,
                added: Color::Rgb(0, 130, 0),
                removed: Color::Red,
                changed: Color::Rgb(176, 120, 0),
                keyword: Color::Rgb(166, 38, 164),
                string: Color::Rgb(80, 161, 79),
                comment: Color::Rgb(160, 161, 167),
                number: Color::Rgb(152, 104, 1),
                function: Color::Rgb(64, 120, 242),
                class: Color::Rgb(193, 132, 1),
                variable: Color::Rgb(1, 132, 188),
                note: Color::Rgb(44, 102, 128),
                note_border: Color::Rgb(160, 192, 206),
                bar: Color::Rgb(225, 225, 232),
                removed_bg: Color::Rgb(255, 235, 233),
            },
            ThemeName::Mono => Theme {
                name,
                accent: Color::Reset,
                muted: Color::Reset,
                text: Color::Reset,
                line_no: Color::Reset,
                risk_medium: Color::Reset,
                risk_high: Color::Reset,
                added: Color::Reset,
                removed: Color::Reset,
                changed: Color::Reset,
                keyword: Color::Reset,
                string: Color::Reset,
                comment: Color::Reset,
                number: Color::Reset,
                function: Color::Reset,
                class: Color::Reset,
                variable: Color::Reset,
                note: Color::Reset,
                note_border: Color::Reset,
                bar: Color::Reset,
                removed_bg: Color::Reset,
            },
        }
    }

    fn mono(&self) -> bool {
        self.name == ThemeName::Mono
    }

    pub fn accent(&self) -> Style {
        if self.mono() {
            Style::new().add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(self.accent)
        }
    }

    pub fn muted(&self) -> Style {
        if self.mono() {
            Style::new().add_modifier(Modifier::DIM)
        } else {
            Style::new().fg(self.muted)
        }
    }

    /// Text of a row the user already accepted: pushed back, never hidden.
    pub fn accepted_row(&self) -> Style {
        self.muted()
    }

    pub fn text(&self) -> Style {
        Style::new().fg(self.text)
    }

    pub fn line_no(&self) -> Style {
        if self.mono() {
            Style::new().add_modifier(Modifier::DIM)
        } else {
            Style::new().fg(self.line_no)
        }
    }

    /// Line numbers of removed lines, which are the old file's: italic and
    /// dimmed, in the removed colour where there is colour.
    pub fn old_line_no(&self) -> Style {
        let s = Style::new().add_modifier(Modifier::DIM | Modifier::ITALIC);
        if self.mono() { s } else { s.fg(self.removed) }
    }

    /// The bar in front of what the agent wrote: its label and summary,
    /// its statements, and its notes in the code.
    pub fn agent(&self) -> Style {
        self.accent()
    }

    /// A statement written into the code: italic, in a pale accent, inside
    /// its card.
    pub fn note(&self) -> Style {
        let s = Style::new().add_modifier(Modifier::ITALIC);
        if self.mono() { s } else { s.fg(self.note) }
    }

    /// The border of a statement's card in the code; dimmed without colour.
    pub fn note_border(&self) -> Style {
        if self.mono() {
            Style::new().add_modifier(Modifier::DIM)
        } else {
            Style::new().fg(self.note_border)
        }
    }

    /// Code of a `class`: its colour, nothing without colour.
    pub fn syntax(&self, class: crate::tui::syntax::Class) -> Style {
        use crate::tui::syntax::Class;
        if self.mono() {
            return Style::new();
        }
        let fg = match class {
            Class::Keyword => self.keyword,
            Class::Function => self.function,
            Class::Type => self.class,
            Class::Variable => self.variable,
            Class::String => self.string,
            Class::Comment => self.comment,
            Class::Constant => self.number,
        };
        let s = Style::new().fg(fg);
        if class == Class::Comment {
            s.add_modifier(Modifier::ITALIC)
        } else {
            s
        }
    }

    /// Behind a removed line of a code change; without colour it is dimmed
    /// and the `-` says it.
    pub fn removed_line(&self) -> Style {
        if self.mono() {
            Style::new().add_modifier(Modifier::DIM)
        } else {
            Style::new().bg(self.removed_bg)
        }
    }

    pub fn bold(&self) -> Style {
        Style::new().add_modifier(Modifier::BOLD)
    }

    /// Marker color by risk.
    pub fn risk(&self, r: Risk) -> Style {
        match r {
            Risk::Low => self.text().add_modifier(Modifier::BOLD),
            Risk::Medium => Style::new()
                .fg(self.risk_medium)
                .add_modifier(Modifier::BOLD),
            Risk::High => {
                let s = Style::new().fg(self.risk_high).add_modifier(Modifier::BOLD);
                if self.mono() {
                    s.add_modifier(Modifier::UNDERLINED)
                } else {
                    s
                }
            }
        }
    }

    pub fn marker(&self, m: Marker) -> Style {
        match m {
            Marker::Added => Style::new().fg(self.added),
            Marker::Changed => Style::new().fg(self.changed),
            Marker::Removed => Style::new().fg(self.removed),
        }
    }

    pub fn button(&self) -> Style {
        if self.mono() {
            Style::new().add_modifier(Modifier::REVERSED)
        } else {
            Style::new().bg(self.bar)
        }
    }

    /// Name of an Investigate-flagged row in the list: a style, not a glyph.
    pub fn flagged_row(&self) -> Style {
        if self.mono() {
            Style::new().add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            Style::new().fg(self.removed).add_modifier(Modifier::BOLD)
        }
    }

    /// `⚑` badge.
    pub fn badge_investigate(&self) -> Style {
        if self.mono() {
            Style::new().add_modifier(Modifier::BOLD | Modifier::REVERSED)
        } else {
            Style::new().fg(self.removed).add_modifier(Modifier::BOLD)
        }
    }

    /// Something good: accepted, resolved as fine (`●`), committed.
    pub fn ok(&self) -> Style {
        Style::new().fg(self.added).add_modifier(Modifier::BOLD)
    }

    /// Something flagged.
    pub fn bad(&self) -> Style {
        Style::new().fg(self.removed).add_modifier(Modifier::BOLD)
    }

    /// `✗` glyph: a flagged unit that needs a fix.
    pub fn needs_fix(&self) -> Style {
        if self.mono() {
            self.bad().add_modifier(Modifier::UNDERLINED)
        } else {
            self.bad()
        }
    }

    /// The swipe card's accept button: dark text on green, reversed without
    /// colour.
    pub fn accept_button(&self) -> Style {
        if self.mono() {
            Style::new().add_modifier(Modifier::BOLD | Modifier::REVERSED)
        } else {
            Style::new()
                .bg(self.added)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD)
        }
    }
}
