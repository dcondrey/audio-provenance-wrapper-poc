//! Colour, only when a human is looking at it.

use std::io::IsTerminal;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Good,
    Warn,
    Bad,
    /// A result that is neither good nor bad. `not_found` lives here on purpose: a missing mark is
    /// never proof of synthetic origin, and colouring it as a failure would say otherwise.
    Neutral,
    Dim,
}

#[derive(Debug, Clone, Copy)]
pub struct Style {
    colour: bool,
}

impl Style {
    /// `--no-color`, then `NO_COLOR` at any value, then whether stdout is actually a terminal.
    pub fn resolve(no_color_flag: bool) -> Self {
        let colour = !no_color_flag
            && std::env::var_os("NO_COLOR").is_none()
            && std::io::stdout().is_terminal();
        Self { colour }
    }

    pub const fn plain() -> Self {
        Self { colour: false }
    }

    pub fn paint(self, tone: Tone, text: &str) -> String {
        if !self.colour {
            return text.to_string();
        }
        let code = match tone {
            Tone::Good => "32",
            Tone::Warn => "33",
            Tone::Bad => "31",
            Tone::Neutral => "36",
            Tone::Dim => "90",
        };
        format!("\u{1b}[{code}m{text}\u{1b}[0m")
    }
}
