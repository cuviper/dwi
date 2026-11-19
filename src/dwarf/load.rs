use std::{
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use gimli::{Reader, Section};
use object::Object;

use super::Database;
use diagnostics::{SearchFailures, matched_file_error};

mod compression;
mod diagnostics;
#[cfg(target_os = "linux")]
mod kernel;
mod limits;
mod supplementary;

pub use limits::LoadLimits;
pub(super) use limits::SectionLoader;

pub(super) fn open(path: &Path, limits: LoadLimits) -> Result<Database> {
    open_with_debug_dirs(path, &[Path::new("/usr/lib/debug")], limits)
}

pub(super) fn open_kernel(
    release: Option<&str>,
    limits: LoadLimits,
) -> Result<(Database, PathBuf)> {
    #[cfg(target_os = "linux")]
    {
        kernel::open(release, limits)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (release, limits);
        bail!("kernel discovery is supported on Linux");
    }
}

pub(super) fn open_module(
    name: &str,
    release: Option<&str>,
    limits: LoadLimits,
) -> Result<(Database, PathBuf)> {
    #[cfg(target_os = "linux")]
    {
        kernel::open_module(name, release, limits)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (name, release, limits);
        bail!("kernel module discovery is supported on Linux");
    }
}

fn open_with_debug_dirs(path: &Path, debug_dirs: &[&Path], limits: LoadLimits) -> Result<Database> {
    let bytes = compression::read_object(path, limits)?;
    open_bytes(path, &bytes, debug_dirs, limits)
}

fn open_bytes(
    path: &Path,
    bytes: &[u8],
    debug_dirs: &[&Path],
    limits: LoadLimits,
) -> Result<Database> {
    let object = object::File::parse(bytes)
        .with_context(|| format!("reading object file {}", path.display()))?;
    if let Some(database) = load_database(&object, path, debug_dirs, limits)? {
        return Ok(database);
    }
    let mut failures = SearchFailures::default();
    match object.build_id() {
        Ok(Some(id)) if !id.is_empty() => {
            for candidate in build_id_paths(id, debug_dirs) {
                match read_candidate(&candidate, DebugIdentity::BuildId(id)) {
                    Ok(bytes) => {
                        return load_matched_file(path, &candidate, &bytes, debug_dirs, limits);
                    }
                    Err(error) => failures.candidate(&candidate, error),
                }
            }
        }
        Err(error) => failures.note(anyhow::Error::new(error).context("reading GNU build ID")),
        _ => {}
    }
    let debuglink = object.gnu_debuglink().context("reading .gnu_debuglink");
    let (filename, expected_crc) = match debuglink {
        Ok(Some(link)) => link,
        Ok(None) if failures.is_empty() => bail!(
            "no DWARF units, GNU build ID, or .gnu_debuglink found in {}; supply a file containing debug information",
            path.display()
        ),
        Ok(None) => {
            failures.note(anyhow::anyhow!("no .gnu_debuglink fallback"));
            return Err(failures.finish(path));
        }
        Err(error) => {
            failures.note(error);
            return Err(failures.finish(path));
        }
    };
    let filename = debuglink_name(filename)?;
    // The debuglink name is a basename, relative to the executable's real
    // directory (including when the supplied executable path is a symlink).
    ensure!(
        Path::new(filename).file_name() == Some(filename),
        ".gnu_debuglink must contain a filename, not a path"
    );
    let executable = fs::canonicalize(path)
        .with_context(|| format!("resolving executable path {}", path.display()))?;
    let directory = executable
        .parent()
        .context("executable has no parent directory")?;
    let mut candidates = vec![
        directory.join(filename),
        directory.join(".debug").join(filename),
    ];
    // A global debug directory mirrors the executable's absolute directory,
    // with the filesystem root removed so Path::join does not discard it.
    let relative: std::path::PathBuf = directory
        .components()
        .filter_map(|part| match part {
            std::path::Component::Normal(part) => Some(part),
            _ => None,
        })
        .collect();
    candidates.extend(
        debug_dirs
            .iter()
            .map(|root| root.join(&relative).join(filename)),
    );
    for candidate in candidates {
        match read_candidate(&candidate, DebugIdentity::Crc(expected_crc)) {
            Ok(bytes) => return load_matched_file(path, &candidate, &bytes, debug_dirs, limits),
            Err(error) => failures.candidate(&candidate, error),
        }
    }
    Err(failures.finish(path))
}

fn build_id_hex(id: &[u8]) -> String {
    id.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn build_id_paths(id: &[u8], debug_dirs: &[&Path]) -> Vec<PathBuf> {
    if id.is_empty() {
        return Vec::new();
    }
    let hex = build_id_hex(id);
    let (prefix, rest) = hex.split_at(2);
    debug_dirs
        .iter()
        .map(|root| {
            root.join(".build-id")
                .join(prefix)
                .join(format!("{rest}.debug"))
        })
        .collect()
}

enum DebugIdentity<'a> {
    BuildId(&'a [u8]),
    Crc(u32),
}

/// Read and validate identity only. Once this succeeds, the caller commits to
/// this file; errors while loading its DWARF must not resume the search.
fn read_candidate(path: &Path, identity: DebugIdentity<'_>) -> Result<Vec<u8>> {
    let bytes = fs::read(path)?;
    if let DebugIdentity::Crc(expected_crc) = identity {
        let actual_crc = crc32fast::hash(&bytes);
        ensure!(
            actual_crc == expected_crc,
            "CRC mismatch (expected {expected_crc:08x}, got {actual_crc:08x})"
        );
    }
    if let DebugIdentity::BuildId(expected) = identity {
        let object = object::File::parse(bytes.as_slice()).context("reading debug object")?;
        let actual = object
            .build_id()
            .context("reading debug object's build ID")?
            .context("debug object has no GNU build ID")?;
        ensure!(
            actual == expected,
            "build ID mismatch (expected {}, got {})",
            build_id_hex(expected),
            build_id_hex(actual)
        );
    }
    Ok(bytes)
}

fn load_matched_file(
    input: &Path,
    candidate: &Path,
    bytes: &[u8],
    debug_dirs: &[&Path],
    limits: LoadLimits,
) -> Result<Database> {
    (|| {
        let object = object::File::parse(bytes).context("reading debug object")?;
        let mut database = load_database(&object, candidate, debug_dirs, limits)?
            .context("file contains no DWARF units")?;
        database.debug_file = Some(
            candidate
                .canonicalize()
                .unwrap_or_else(|_| candidate.to_owned()),
        );
        Ok(database)
    })()
    .map_err(|error| matched_file_error(input, candidate, error))
}

fn load_database(
    object: &object::File<'_>,
    path: &Path,
    debug_dirs: &[&Path],
    limits: LoadLimits,
) -> Result<Option<Database>> {
    let loader = SectionLoader::new(limits);
    let mut dwarf = Database::dwarf_from_object(object, path, &loader)?;
    if dwarf.debug_info.reader().is_empty() && dwarf.debug_types.reader().is_empty() {
        return Ok(None);
    }
    let mut supplementary_file = None;
    let mut supplementary_error = None;
    match supplementary::load(object, path, debug_dirs, &loader) {
        Ok(Some((file, sup))) => {
            // Check unit decoding before attaching it so a broken supplement
            // doesn't prevent browsing the primary object's remaining data.
            match Database::append_units(&sup, &mut Vec::new()) {
                Ok(()) => {
                    dwarf.set_sup(sup);
                    supplementary_file = Some(file);
                }
                Err(error) => {
                    supplementary_error =
                        Some(format!("supplementary file {}: {error:#}", file.display()))
                }
            }
        }
        Ok(None) => {}
        Err(error) => supplementary_error = Some(format!("supplementary debug object: {error:#}")),
    }
    let mut database = Database::from_dwarf(dwarf)?;
    database.supplementary_file = supplementary_file;
    database.supplementary_error = supplementary_error;
    Ok(Some(database))
}

fn debuglink_name(bytes: &[u8]) -> Result<&OsStr> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Ok(OsStr::from_bytes(bytes))
    }
    #[cfg(not(unix))]
    {
        Ok(OsStr::new(
            std::str::from_utf8(bytes).context("debuglink filename is not UTF-8")?,
        ))
    }
}

#[cfg(test)]
mod tests;
