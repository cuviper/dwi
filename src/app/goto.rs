use crate::dwarf::DieSection;
use anyhow::{Context, Result, bail, ensure};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub struct GotoDialog {
    pub input: String,
    // Input is printable ASCII, so byte offsets are also terminal columns.
    pub cursor: usize,
    pub error: Option<String>,
    default_section: DieSection,
}

impl GotoDialog {
    pub fn new(section: DieSection) -> Self {
        let input = format!("{}:0x", section.label());
        Self {
            cursor: input.len(),
            input,
            error: None,
            default_section: section,
        }
    }

    pub fn destination(&self) -> Result<(DieSection, usize)> {
        let text = self.input.trim();
        let (section, offset) = match text.rsplit_once(':') {
            Some(("info", offset)) => (DieSection::Info, offset),
            Some(("types", offset)) => (DieSection::Types, offset),
            Some(("sup:info", offset)) => (DieSection::SupplementaryInfo, offset),
            Some(("sup:types", offset)) => (DieSection::SupplementaryTypes, offset),
            Some(_) => bail!("Use info:, types:, sup:info:, or sup:types:"),
            None => (self.default_section, text),
        };
        let offset = offset.trim();
        let digits = offset
            .strip_prefix("0x")
            .or_else(|| offset.strip_prefix("0X"))
            .unwrap_or(offset);
        ensure!(!digits.is_empty(), "Enter a hexadecimal section offset");
        ensure!(
            digits.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "Offset must contain only hexadecimal digits"
        );
        let offset = usize::from_str_radix(digits, 16).context("Offset is too large")?;
        Ok((section, offset))
    }

    pub fn edit(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            if key.code == KeyCode::Char('u') {
                self.input.clear();
                self.cursor = 0;
                self.error = None;
            }
            return;
        }
        if key.modifiers.contains(KeyModifiers::ALT) {
            return;
        }
        match key.code {
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.input.len()),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.input.len(),
            KeyCode::Backspace if self.cursor > 0 => {
                self.cursor -= 1;
                self.input.remove(self.cursor);
                self.error = None;
            }
            KeyCode::Delete if self.cursor < self.input.len() => {
                self.input.remove(self.cursor);
                self.error = None;
            }
            KeyCode::Char(c) if c.is_ascii() && !c.is_ascii_control() => {
                self.input.insert(self.cursor, c);
                self.cursor += 1;
                self.error = None;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests;
