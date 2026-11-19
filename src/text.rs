/// Escape terminal controls at display boundaries, preserving printable Unicode.
pub fn escape_controls(text: &str) -> String {
    text.chars()
        .flat_map(|c| {
            if c.is_control() {
                c.escape_default().collect::<Vec<_>>()
            } else {
                vec![c]
            }
        })
        .collect()
}

/// Diagnostic lines supplied by the application. Each line's contents may
/// include filenames or external errors, which are escaped when printed.
#[derive(Debug)]
pub struct MultilineError {
    lines: Vec<String>,
}

impl MultilineError {
    pub fn new(heading: String, details: Vec<String>) -> Self {
        Self {
            lines: std::iter::once(heading).chain(details).collect(),
        }
    }
}

impl std::fmt::Display for MultilineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (index, line) in self.lines.iter().enumerate() {
            if index > 0 {
                f.write_str("\n")?;
            }
            f.write_str(line)?;
        }
        Ok(())
    }
}

impl std::error::Error for MultilineError {}

/// Preserve intentional diagnostic layout, without interpreting embedded
/// control characters (including newlines) as formatting instructions.
pub fn format_error(error: &anyhow::Error) -> String {
    error
        .chain()
        .map(|cause| {
            if let Some(report) = cause.downcast_ref::<MultilineError>() {
                report
                    .lines
                    .iter()
                    .map(|line| escape_controls(line))
                    .collect::<Vec<_>>()
                    .join("\n")
            } else {
                escape_controls(&cause.to_string())
            }
        })
        .collect::<Vec<_>>()
        .join(": ")
}

#[cfg(test)]
mod tests {
    use super::escape_controls;

    #[test]
    fn escapes_c0_del_and_c1_controls() {
        assert_eq!(
            escape_controls("\u{1b}]0;title\u{7}\r\n\t\0\u{7f}\u{9b}31m"),
            "\\u{1b}]0;title\\u{7}\\r\\n\\t\\u{0}\\u{7f}\\u{9b}31m"
        );
        for code in (0..=0x1f).chain(0x7f..=0x9f) {
            let escaped = escape_controls(&char::from_u32(code).unwrap().to_string());
            assert!(!escaped.chars().any(char::is_control));
        }
    }

    #[test]
    fn preserves_printable_unicode_and_punctuation() {
        let text = "café 日本語 🦀 → 'quoted' \"text\" \\path";
        assert_eq!(escape_controls(text), text);
    }
}
