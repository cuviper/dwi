use std::{
    collections::HashSet,
    fs,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use object::{BinaryFormat, Endianness};

use super::{
    Database, DebugIdentity, LoadLimits, SearchFailures, build_id_paths, open_bytes, read_candidate,
};

mod modules;

fn running_release() -> Result<String> {
    let uname = rustix::system::uname();
    Ok(uname
        .release()
        .to_str()
        .context("running kernel release is not UTF-8")?
        .to_owned())
}

pub(super) fn open_module(
    name: &str,
    release: Option<&str>,
    limits: LoadLimits,
) -> Result<(Database, PathBuf)> {
    let running = running_release()?;
    modules::open(
        name,
        release.unwrap_or(&running),
        Path::new("/"),
        &[Path::new("/usr/lib/debug")],
        limits,
    )
}

pub(super) fn open(release: Option<&str>, limits: LoadLimits) -> Result<(Database, PathBuf)> {
    let running = running_release()?;
    let release = release.unwrap_or(&running);
    validate_release(release)?;
    let mut failures = SearchFailures::default();
    let id = match running_build_id(release, &running, Path::new("/sys/kernel/notes")) {
        Ok(id) => id,
        Err(error) => {
            failures.note(error);
            None
        }
    };
    discover(
        release,
        id.as_deref(),
        Path::new("/"),
        &[Path::new("/usr/lib/debug")],
        limits,
        failures,
    )
}

fn validate_release(release: &str) -> Result<()> {
    let mut parts = Path::new(release).components();
    ensure!(
        matches!(parts.next(), Some(Component::Normal(_)))
            && parts.next().is_none()
            && Path::new(release)
                .file_name()
                .is_some_and(|name| name == release),
        "kernel release must be a single directory name"
    );
    Ok(())
}

fn running_build_id(release: &str, running: &str, notes: &Path) -> Result<Option<Vec<u8>>> {
    // /sys/kernel/notes describes only the running kernel, never a different
    // installed release selected on the command line.
    if release != running {
        return Ok(None);
    }
    let bytes = match fs::read(notes) {
        Ok(bytes) => bytes,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
            ) =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error).with_context(|| format!("reading {}", notes.display())),
    };
    let endian = if cfg!(target_endian = "little") {
        Endianness::Little
    } else {
        Endianness::Big
    };
    notes_build_id(&bytes, endian)
        .with_context(|| format!("reading kernel build ID from {}", notes.display()))
}

fn notes_build_id(bytes: &[u8], endian: Endianness) -> Result<Option<Vec<u8>>> {
    // Linux exports raw, four-byte-aligned ELF notes. The note header has the
    // same three u32 fields for ELF32 and ELF64.
    let mut notes = object::read::elf::NoteIterator::<object::elf::FileHeader64<Endianness>>::new(
        endian, 4, bytes,
    )?;
    while let Some(note) = notes.next()? {
        if note.name() == object::elf::ELF_NOTE_GNU
            && note.n_type(endian) == object::elf::NT_GNU_BUILD_ID
        {
            ensure!(!note.desc().is_empty(), "kernel GNU build ID is empty");
            return Ok(Some(note.desc().to_vec()));
        }
    }
    Ok(None)
}

fn candidates(release: &str, root: &Path, debug_dirs: &[&Path]) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for debug in debug_dirs {
        paths.push(debug.join("lib/modules").join(release).join("vmlinux"));
        paths.push(debug.join("usr/lib/modules").join(release).join("vmlinux"));
        paths.push(debug.join("boot").join(format!("vmlinux-{release}")));
    }
    paths.push(root.join("lib/modules").join(release).join("vmlinux"));
    paths.push(root.join("usr/lib/modules").join(release).join("vmlinux"));
    paths.push(root.join("boot").join(format!("vmlinux-{release}")));
    paths
}

fn discover(
    release: &str,
    expected_id: Option<&[u8]>,
    root: &Path,
    debug_dirs: &[&Path],
    limits: LoadLimits,
    mut failures: SearchFailures,
) -> Result<(Database, PathBuf)> {
    validate_release(release)?;
    let input = PathBuf::from(format!("kernel {release}"));
    let mut paths = expected_id
        .map(|id| build_id_paths(id, debug_dirs))
        .unwrap_or_default();
    paths.extend(candidates(release, root, debug_dirs));
    let mut seen = HashSet::new();
    for candidate in paths {
        let canonical = candidate
            .canonicalize()
            .unwrap_or_else(|_| candidate.clone());
        if !seen.insert(canonical.clone()) {
            continue;
        }
        let matched = (|| -> Result<Vec<u8>> {
            let bytes = if let Some(id) = expected_id {
                read_candidate(&candidate, DebugIdentity::BuildId(id))?
            } else {
                fs::read(&candidate)?
            };
            let object = object::File::parse(bytes.as_slice()).context("reading kernel ELF")?;
            ensure!(
                object.format() == BinaryFormat::Elf,
                "vmlinux must be an ELF file"
            );
            Ok(bytes)
        })();
        match matched {
            Ok(bytes) => {
                // The release-specific ELF (and build ID, when available) matched.
                // Reuse normal embedded/build-ID/debuglink/supplementary loading.
                let mut database = open_bytes(&canonical, &bytes, debug_dirs, limits)
                    .with_context(|| format!("loading kernel {release}"))?;
                if database.debug_file.as_ref() == Some(&canonical) {
                    database.debug_file = None;
                }
                return Ok((database, canonical));
            }
            Err(error) => failures.candidate(&candidate, error),
        }
    }
    failures.note(anyhow::anyhow!(
        "Install kernel debuginfo for {release}, or open its vmlinux file directly"
    ));
    Err(failures.finish(&input))
}

#[cfg(test)]
mod tests;
