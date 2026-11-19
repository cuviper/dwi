use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use gimli::{Dwarf, EndianSlice, Reader, RunTimeEndian};
use object::Object;

use super::{Database, SectionLoader, build_id_paths, debuglink_name};
use crate::dwarf::DwarfReader;

enum Identity {
    BuildId(Vec<u8>),
    DebugSup(Vec<u8>),
}

struct DebugSup {
    supplementary: bool,
    filename: OsString,
    checksum: Vec<u8>,
}

fn debug_sup(
    object: &object::File<'_>,
    path: &Path,
    loader: &SectionLoader,
) -> Result<Option<DebugSup>> {
    let Some(section) = object.section_by_name(".debug_sup") else {
        return Ok(None);
    };
    let bytes = loader.read_section(&section, path)?;
    let endian = if object.is_little_endian() {
        RunTimeEndian::Little
    } else {
        RunTimeEndian::Big
    };
    let mut reader = EndianSlice::new(&bytes, endian);
    ensure!(reader.read_u16()? == 5, "unsupported .debug_sup version");
    let flag = reader.read_u8()?;
    ensure!(flag <= 1, "invalid .debug_sup is_supplementary flag");
    let filename = reader.read_null_terminated_slice()?;
    let filename = debuglink_name(filename.slice())?.to_owned();
    let length = usize::try_from(reader.read_uleb128()?)
        .context(".debug_sup checksum length is too large")?;
    let checksum = reader.split(length)?.slice().to_vec();
    ensure!(
        reader.is_empty(),
        "unexpected data after .debug_sup checksum"
    );
    ensure!(
        flag == 0 || filename.is_empty(),
        "supplementary .debug_sup filename must be empty"
    );
    Ok(Some(DebugSup {
        supplementary: flag == 1,
        filename,
        checksum,
    }))
}

pub(super) fn load(
    object: &object::File<'_>,
    source: &Path,
    debug_dirs: &[&Path],
    loader: &SectionLoader,
) -> Result<Option<(PathBuf, Dwarf<DwarfReader>)>> {
    let (filename, identity) =
        if let Some(sup) = debug_sup(object, source, loader).context("parsing .debug_sup")? {
            if sup.supplementary {
                return Ok(None);
            }
            (sup.filename, Identity::DebugSup(sup.checksum))
        } else if let Some((filename, id)) = object
            .gnu_debugaltlink()
            .context("reading .gnu_debugaltlink")?
        {
            ensure!(!id.is_empty(), ".gnu_debugaltlink has no build ID");
            (
                debuglink_name(filename)?.to_owned(),
                Identity::BuildId(id.to_vec()),
            )
        } else {
            return Ok(None);
        };
    ensure!(
        !filename.is_empty(),
        "supplementary file link has no filename"
    );
    let source = source
        .canonicalize()
        .context("resolving referencing debug file")?;
    let mut candidates = vec![
        source
            .parent()
            .context("debug file has no parent")?
            .join(&filename),
    ];
    if let Identity::BuildId(id) = &identity {
        candidates.extend(build_id_paths(id, debug_dirs));
    }
    let mut failures = Vec::new();
    for candidate in candidates {
        match read_candidate(&candidate, &identity, loader) {
            Ok(bytes) => {
                // Identity matched: report loading failures for this object,
                // rather than trying other paths for the same supplement.
                let object = object::File::parse(bytes.as_slice())
                    .context("reading supplementary object")?;
                let dwarf = Database::dwarf_from_object(&object, &candidate, loader)?;
                return Ok(Some((candidate.canonicalize().unwrap_or(candidate), dwarf)));
            }
            Err(error) => failures.push(format!("{}: {error:#}", candidate.display())),
        }
    }
    bail!(
        "could not load {} referenced by {}: {}",
        Path::new(&filename).display(),
        source.display(),
        failures.join("; ")
    )
}

fn read_candidate(path: &Path, identity: &Identity, loader: &SectionLoader) -> Result<Vec<u8>> {
    let bytes = fs::read(path)?;
    let object = object::File::parse(bytes.as_slice()).context("reading supplementary object")?;
    match identity {
        Identity::BuildId(expected) => {
            let actual = object
                .build_id()?
                .context("supplementary object has no GNU build ID")?;
            ensure!(actual == expected, "supplementary build ID mismatch");
        }
        Identity::DebugSup(expected) => {
            let actual = debug_sup(&object, path, loader)?
                .context("supplementary object has no .debug_sup")?;
            ensure!(
                actual.supplementary,
                ".debug_sup does not identify a supplementary object"
            );
            // This is an implementation-defined identifier, not a checksum
            // algorithm to recompute over the file. Compare the two records.
            ensure!(
                expected.is_empty() || *expected == actual.checksum,
                "supplementary .debug_sup checksum mismatch"
            );
        }
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests;
