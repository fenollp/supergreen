//! What we print gets colors the way cargo decides: `$CARGO_TERM_COLOR` then the terminal.

use std::io::{self, IsTerminal};

use termimad::{
    CompoundStyle, MadSkin,
    crossterm::style::{Attribute, Color},
    terminal_size,
};

use crate::redact::Vars;

impl Vars {
    /// Whether what we print on stdout gets colors, the way cargo decides it: `$CARGO_TERM_COLOR`
    /// when it says so (that's how CI logs get theirs, not being terminals), else terminals only
    /// and unless `$NO_COLOR`.
    #[must_use]
    pub(crate) fn color(&self) -> bool {
        wants_colors(
            self.get_nonempty(CARGO_TERM_COLOR!()),
            self.get_nonempty(NO_COLOR!()),
            io::stdout().is_terminal(),
        )
    }
}

#[must_use]
fn wants_colors(term_color: Option<&str>, no_color: Option<&str>, terminal: bool) -> bool {
    match term_color {
        Some("always") => true,
        Some("never") => false,
        _ => terminal && no_color.is_none_or(str::is_empty),
    }
}

#[cfg(test)]
#[test_case::test_case(Some("always"), None, false => true)]
#[test_case::test_case(Some("always"), None, true => true)]
#[test_case::test_case(Some("never"), None, true => false)]
#[test_case::test_case(Some("never"), None, false => false)]
#[test_case::test_case(Some("auto"), None, true => true)]
#[test_case::test_case(Some("auto"), None, false => false)]
#[test_case::test_case(Some("auto"), Some("1"), true => false)]
#[test_case::test_case(None, None, true => true)]
#[test_case::test_case(None, None, false => false)]
#[test_case::test_case(None, Some("1"), true => false)]
#[test_case::test_case(None, Some(""), true => true)]
#[test_case::test_case(Some("nonsense"), None, false => false)]
fn colors_follow_cargo_then_the_terminal(
    term_color: Option<&str>,
    no_color: Option<&str>,
    terminal: bool,
) -> bool {
    wants_colors(term_color, no_color, terminal)
}

/// How reports get laid out: our text goes through `skin`, what's not ours stays as it came.
pub(crate) struct Paint {
    /// Wraps paragraphs, shows `*labels:*` and what's between backticks
    skin: MadSkin,
    pub(crate) title: CompoundStyle,
    pub(crate) label: CompoundStyle,
    pub(crate) ok: CompoundStyle,
    pub(crate) skip: CompoundStyle,
    pub(crate) fail: CompoundStyle,
    width: usize,
}

impl Paint {
    const MAX_WIDTH: usize = 100;
    /// Where the details of a check start
    pub(crate) const INDENT: usize = 8;

    #[must_use]
    pub(crate) fn plain(width: usize) -> Self {
        let none = CompoundStyle::default;
        let (title, label, ok, skip, fail) = (none(), none(), none(), none(), none());
        Self { skin: MadSkin::no_style(), title, label, ok, skip, fail, width }
    }

    /// Colored when `color` (see [`Vars::color`]), as wide as the terminal up to `MAX_WIDTH`
    #[must_use]
    pub(crate) fn for_stdout(color: bool) -> Self {
        if !color {
            return Self::plain(Self::MAX_WIDTH);
        }

        // Cargo's palette, named as in its src/cargo/util/style.rs: there it's anstyle's bright
        // colors, which are crossterm's plain ones.
        let bold = |color| {
            let mut style = CompoundStyle::with_fg(color);
            style.add_attr(Attribute::Bold);
            style
        };
        let label = CompoundStyle::with_fg(Color::Blue); // Not cargo's

        let inline_code = bold(Color::Cyan); // LITERAL
        let skin = MadSkin { italic: label, inline_code, ..Default::default() };

        Self {
            skin,
            title: bold(Color::Green), // HEADER
            label,
            ok: bold(Color::Green),    // GOOD
            skip: bold(Color::Yellow), // WARN
            fail: bold(Color::Red),    // ERROR
            width: if io::stdout().is_terminal() {
                (terminal_size().0 as usize).min(Self::MAX_WIDTH)
            } else {
                Self::MAX_WIDTH // CI logs have colors but no terminal to measure
            },
        }
    }

    /// A title, a blank line, then `body` as it came. Only the title when there's no body.
    #[must_use]
    pub(crate) fn section(&self, title: &str, body: &str) -> String {
        let mut text = format!("{}\n", self.title.apply_to(title));
        if body.trim_end().is_empty() {
            return text;
        }
        text.push('\n');
        for line in body.trim_end().lines() {
            text.push_str(format!("  {line}").trim_end());
            text.push('\n');
        }
        text
    }

    /// Markdown wrapped to fit, every line starting at `indent`
    #[must_use]
    pub(crate) fn paragraph(&self, indent: usize, markdown: &str) -> String {
        let pad = " ".repeat(indent);
        let width = self.width.saturating_sub(indent).max(20);
        let wrapped = self.skin.text(markdown, Some(width)).to_string();
        wrapped.lines().map(|line| format!("{pad}{}\n", line.trim_end())).collect()
    }
}
