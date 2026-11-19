use std::{
    io,
    path::{Path, PathBuf},
};

use super::limits::{ExpansionExceeded, SectionError};
use crate::text::MultilineError;

#[derive(Default)]
pub(super) struct SearchFailures {
    rejected: Vec<(PathBuf, Vec<Vec<String>>)>,
    missing: Vec<PathBuf>,
    notes: Vec<Vec<String>>,
}

impl SearchFailures {
    pub(super) fn is_empty(&self) -> bool {
        self.rejected.is_empty() && self.missing.is_empty() && self.notes.is_empty()
    }

    pub(super) fn candidate(&mut self, path: &Path, error: anyhow::Error) {
        if error.downcast_ref::<io::Error>().is_some_and(|error| {
            matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            )
        }) {
            if !self.missing.iter().any(|missing| missing == path) {
                self.missing.push(path.to_owned());
            }
            return;
        }
        let path = path.canonicalize().unwrap_or_else(|_| path.to_owned());
        let detail = details(&error);
        if let Some((_, reasons)) = self.rejected.iter_mut().find(|(file, _)| *file == path) {
            // Different lookup methods may reach the same file through symlinks.
            // Keep distinct reasons, but print identical failures only once.
            if !reasons.contains(&detail) {
                reasons.push(detail);
            }
        } else {
            self.rejected.push((path, vec![detail]));
        }
    }

    pub(super) fn note(&mut self, error: anyhow::Error) {
        self.notes.push(details(&error));
    }

    pub(super) fn finish(self, input: &Path) -> anyhow::Error {
        let mut lines = vec!["  Input:".into(), format!("    {}", input.display())];
        if !self.rejected.is_empty() {
            lines.extend([String::new(), "  Rejected files:".into()]);
            for (path, reasons) in self.rejected {
                lines.push(format!("    {}", path.display()));
                for reason in reasons {
                    lines.extend(reason.into_iter().map(|line| format!("      {line}")));
                }
            }
        }
        if !self.missing.is_empty() {
            lines.extend([String::new(), "  Not found:".into()]);
            lines.extend(
                self.missing
                    .into_iter()
                    .map(|path| format!("    {}", path.display())),
            );
        }
        if !self.notes.is_empty() {
            lines.extend([String::new(), "  Lookup notes:".into()]);
            for note in self.notes {
                lines.extend(note.into_iter().map(|line| format!("    {line}")));
            }
        }
        MultilineError::new("no usable debug information found".into(), lines).into()
    }
}

pub(super) fn matched_file_error(
    input: &Path,
    candidate: &Path,
    error: anyhow::Error,
) -> anyhow::Error {
    let path = candidate
        .canonicalize()
        .unwrap_or_else(|_| candidate.to_owned());
    let mut lines = vec![
        "  Input:".into(),
        format!("    {}", input.display()),
        String::new(),
        "  Debug file:".into(),
        format!("    {}", path.display()),
    ];
    lines.extend(
        details(&error)
            .into_iter()
            .map(|line| format!("      {line}")),
    );
    MultilineError::new("could not load matching debug file".into(), lines).into()
}

fn details(error: &anyhow::Error) -> Vec<String> {
    let mut context = Vec::new();
    let mut hint = None;
    for cause in error.chain() {
        if let Some(section) = cause.downcast_ref::<SectionError>() {
            // The candidate's canonical path is already on its own line.
            context.push(section.name.clone());
        } else if let Some(limit) = cause.downcast_ref::<ExpansionExceeded>() {
            context.push(limit.description());
            hint = Some(limit.hint());
        } else {
            context.push(cause.to_string());
        }
    }
    let mut lines = vec![context.join(": ")];
    lines.extend(hint);
    lines
}

#[cfg(test)]
mod tests;
