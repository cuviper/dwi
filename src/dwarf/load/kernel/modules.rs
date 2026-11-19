use std::{
    collections::HashSet,
    fs,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, ensure};

use super::{Database, LoadLimits, validate_release};
use crate::{
    dwarf::load::{compression, open_bytes},
    text::MultilineError,
};

pub(super) fn open(
    name: &str,
    release: &str,
    root: &Path,
    debug_dirs: &[&Path],
    limits: LoadLimits,
) -> Result<(Database, PathBuf)> {
    validate_release(release)?;
    let name = normalize_name(name)?;
    let roots = unique_paths([
        root.join("lib/modules").join(release),
        root.join("usr/lib/modules").join(release),
    ]);
    for directory in &roots {
        if let Some(index) = read_optional(&directory.join("modules.dep"))? {
            for line in index.lines() {
                let Some((path, _dependencies)) = line.split_once(':') else {
                    continue;
                };
                if module_name(Path::new(path.trim()))
                    .is_some_and(|(module, debug)| module == name && !debug)
                {
                    let relative = Path::new(path.trim());
                    ensure!(
                        relative
                            .components()
                            .all(|part| matches!(part, Component::Normal(_) | Component::CurDir)),
                        "invalid module path in {}",
                        directory.join("modules.dep").display()
                    );
                    return load_module(&directory.join(relative), debug_dirs, limits);
                }
            }
        }
        if let Some(index) = read_optional(&directory.join("modules.builtin"))?
            && index.lines().any(|line| {
                module_name(Path::new(line.trim())).is_some_and(|(module, _)| module == name)
            })
        {
            return Err(MultilineError::new(
                format!("module {name} is built into kernel {release}"),
                vec![
                    "  Browse its DWARF in vmlinux:".into(),
                    format!("    dwi --kernel {release}"),
                ],
            )
            .into());
        }
    }

    // Fall back to filename lookup when depmod metadata is missing or stale.
    let mut matches = Vec::new();
    for directory in &roots {
        scan(directory, &name, false, &mut matches)?;
    }
    let mut matches = unique_paths(matches);
    let mut searched = roots;
    if matches.is_empty() {
        let debug_roots = unique_paths(debug_dirs.iter().flat_map(|directory| {
            [
                directory.join("lib/modules").join(release),
                directory.join("usr/lib/modules").join(release),
            ]
        }));
        for directory in &debug_roots {
            scan(directory, &name, true, &mut matches)?;
        }
        matches = unique_paths(matches);
        searched.extend(debug_roots);
    }
    match matches.as_slice() {
        [path] => load_module(path, debug_dirs, limits),
        [] => {
            let mut lines = vec!["  Searched:".into()];
            lines.extend(
                searched
                    .iter()
                    .map(|path| format!("    {}", path.display())),
            );
            lines.push(format!(
                "  Install kernel modules or debuginfo for {release}."
            ));
            Err(MultilineError::new(
                format!("module {name} not found for kernel {release}"),
                lines,
            )
            .into())
        }
        _ => {
            let mut lines = matches
                .iter()
                .map(|path| format!("    {}", path.display()))
                .collect::<Vec<_>>();
            lines.push("  Open the desired module file directly to disambiguate.".into());
            Err(MultilineError::new(
                format!("multiple modules named {name} for kernel {release}"),
                lines,
            )
            .into())
        }
    }
}

fn load_module(
    path: &Path,
    debug_dirs: &[&Path],
    limits: LoadLimits,
) -> Result<(Database, PathBuf)> {
    let path = path
        .canonicalize()
        .with_context(|| format!("resolving module {}", path.display()))?;
    let bytes = compression::read_object(&path, limits)?;
    let database = open_bytes(&path, &bytes, debug_dirs, limits)?;
    Ok((database, path))
}

fn normalize_name(name: &str) -> Result<String> {
    ensure!(
        Path::new(name).file_name().is_some_and(|base| base == name),
        "expected a module name (use a positional file argument to open a path)"
    );
    let name = module_name(Path::new(name))
        .map(|(name, _)| name)
        .unwrap_or_else(|| name.replace('-', "_"));
    ensure!(
        !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.'),
        "expected a module name (use a positional file argument to open a path)"
    );
    Ok(name)
}

fn module_name(path: &Path) -> Option<(String, bool)> {
    let name = path.file_name()?.to_str()?;
    let name = [".xz", ".zst", ".gz"]
        .iter()
        .find_map(|suffix| name.strip_suffix(suffix))
        .unwrap_or(name);
    let (name, debug) = match name.strip_suffix(".debug") {
        Some(name) => (name, true),
        None => (name, false),
    };
    Some((name.strip_suffix(".ko")?.replace('-', "_"), debug))
}

fn read_optional(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}

fn unique_paths(paths: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    paths
        .into_iter()
        .map(|path| path.canonicalize().unwrap_or(path))
        .filter(|path| seen.insert(path.clone()))
        .collect()
}

fn scan(directory: &Path, name: &str, debug_files: bool, matches: &mut Vec<PathBuf>) -> Result<()> {
    let mut pending = vec![directory.to_owned()];
    while let Some(directory) = pending.pop() {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) =>
            {
                continue;
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("searching modules in {}", directory.display()));
            }
        };
        let mut entries = entries.collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let kind = entry.file_type()?;
            if kind.is_dir() {
                pending.push(path);
            } else if module_name(&path)
                .is_some_and(|(module, debug)| module == name && (debug_files || !debug))
            {
                matches.push(path);
            }
            // Directory symlinks such as build/source are intentionally not
            // traversed. Indexed paths and module-file symlinks still work.
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
