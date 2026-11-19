use super::*;
use std::path::PathBuf;

use gimli::{Encoding, Format, RunTimeEndian, write};
use object::{Architecture, BinaryFormat, Endianness, ObjectSection, SectionKind};
use tempfile::TempDir;

use super::limits::tests::{Compression, compressed, section_elf};

mod build_id;

fn debuglink(filename: &[u8], crc: u32, endian: Endianness) -> Vec<u8> {
    let mut bytes = filename.to_vec();
    bytes.push(0);
    bytes.resize(bytes.len().next_multiple_of(4), 0);
    bytes.extend_from_slice(&match endian {
        Endianness::Little => crc.to_le_bytes(),
        Endianness::Big => crc.to_be_bytes(),
    });
    bytes
}

pub(in crate::dwarf::load) fn elf(
    name: Option<&str>,
    link: Option<&[u8]>,
    endian: Endianness,
) -> Vec<u8> {
    elf_with_build_id(name, link, None, endian)
}

pub(in crate::dwarf::load) fn elf_with_build_id(
    name: Option<&str>,
    link: Option<&[u8]>,
    build_id: Option<&[u8]>,
    endian: Endianness,
) -> Vec<u8> {
    let architecture = if endian == Endianness::Little {
        Architecture::X86_64
    } else {
        Architecture::PowerPc64
    };
    let mut object = object::write::Object::new(BinaryFormat::Elf, architecture, endian);
    if let Some(name) = name {
        let endian = if endian == Endianness::Little {
            RunTimeEndian::Little
        } else {
            RunTimeEndian::Big
        };
        let mut dwarf = write::DwarfUnit::new(Encoding {
            format: Format::Dwarf32,
            version: 5,
            address_size: 8,
        });
        let root = dwarf.unit.root();
        dwarf.unit.get_mut(root).set(
            gimli::DW_AT_name,
            write::AttributeValue::String(name.as_bytes().to_vec()),
        );
        let child = dwarf.unit.add(root, gimli::DW_TAG_subprogram);
        dwarf.unit.get_mut(child).set(
            gimli::DW_AT_name,
            write::AttributeValue::String(b"main".to_vec()),
        );
        let mut sections = write::Sections::new(write::EndianVec::new(endian));
        dwarf.write(&mut sections).unwrap();
        sections
            .for_each(|id, data| -> Result<()> {
                if !data.slice().is_empty() {
                    let section = object.add_section(
                        Vec::new(),
                        id.name().as_bytes().to_vec(),
                        SectionKind::Debug,
                    );
                    object.append_section_data(section, data.slice(), 1);
                }
                Ok(())
            })
            .unwrap();
    }
    if let Some(link) = link {
        let section =
            object.add_section(Vec::new(), b".gnu_debuglink".to_vec(), SectionKind::Debug);
        object.append_section_data(section, link, 4);
    }
    if let Some(id) = build_id {
        let mut note = Vec::new();
        for value in [4, id.len() as u32, 3] {
            note.extend_from_slice(&match endian {
                Endianness::Little => value.to_le_bytes(),
                Endianness::Big => value.to_be_bytes(),
            });
        }
        note.extend_from_slice(b"GNU\0");
        note.extend_from_slice(id);
        note.resize(note.len().next_multiple_of(4), 0);
        let section = object.add_section(
            Vec::new(),
            b".note.gnu.build-id".to_vec(),
            SectionKind::Note,
        );
        object.append_section_data(section, &note, 4);
    }
    object.write().unwrap()
}

struct Fixture {
    _directory: TempDir,
    binary: PathBuf,
    global: PathBuf,
    debug: Vec<u8>,
}

impl Fixture {
    fn new(endian: Endianness) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let binary = root.join("bin/app");
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        let debug = elf(Some("separate.c"), None, endian);
        let link = debuglink(b"app.debug", crc32fast::hash(&debug), endian);
        fs::write(&binary, elf(None, Some(&link), endian)).unwrap();
        Self {
            _directory: directory,
            binary,
            global: root.join("debug-root"),
            debug,
        }
    }

    fn beside(&self) -> PathBuf {
        self.binary.with_file_name("app.debug")
    }
    fn hidden(&self) -> PathBuf {
        self.binary.parent().unwrap().join(".debug/app.debug")
    }
    fn global_file(&self) -> PathBuf {
        let relative: PathBuf = self
            .binary
            .parent()
            .unwrap()
            .components()
            .filter_map(|part| match part {
                std::path::Component::Normal(part) => Some(part),
                _ => None,
            })
            .collect();
        self.global.join(relative).join("app.debug")
    }
    fn write_debug(&self, path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, &self.debug).unwrap();
    }
    fn open(&self) -> Result<Database> {
        open_with_debug_dirs(&self.binary, &[&self.global], LoadLimits::default())
    }
}

fn assert_browsable(mut db: Database, path: &Path) {
    assert_eq!(db.debug_file.as_deref(), Some(path));
    let root = db.roots[0];
    assert_eq!(db.die(root).unwrap().name, "separate.c");
    let children = db.children(root).unwrap();
    assert_eq!(children[0].name, "main");
    assert_eq!(db.path(children[0].id).unwrap(), [root, children[0].id]);
    assert_eq!(db.attributes(children[0].id).unwrap()[0].value, "\"main\"");
}

#[test]
fn debuglink_search_order_and_crc_work_in_both_endiannesses() {
    for endian in [Endianness::Little, Endianness::Big] {
        let fixture = Fixture::new(endian);
        let candidates = [fixture.beside(), fixture.hidden(), fixture.global_file()];
        for path in &candidates {
            fixture.write_debug(path);
        }
        for path in &candidates {
            assert_browsable(fixture.open().unwrap(), path);
            fs::remove_file(path).unwrap();
        }
        let error = fixture.open().err().unwrap().to_string();
        assert!(error.contains("no usable debug information found"));
        assert!(error.contains("Not found:"));
        for path in &candidates {
            assert!(error.contains(path.to_str().unwrap()));
        }
    }
}

#[test]
fn checksum_mismatch_is_reported_or_skipped_for_a_later_candidate() {
    let fixture = Fixture::new(Endianness::Little);
    fs::write(fixture.beside(), b"wrong file").unwrap();
    fixture.write_debug(&fixture.hidden());
    assert_browsable(fixture.open().unwrap(), &fixture.hidden());
    fs::remove_file(fixture.hidden()).unwrap();
    let error = fixture.open().err().unwrap().to_string();
    assert!(error.contains("CRC mismatch (expected"));
    assert!(error.contains(fixture.beside().to_str().unwrap()));
}

#[test]
fn embedded_dwarf_takes_precedence_and_explicit_debug_files_still_work() {
    let fixture = Fixture::new(Endianness::Little);
    // Malformed debuglink metadata must not interfere with valid embedded DWARF.
    fs::write(
        &fixture.binary,
        elf(Some("embedded.c"), Some(b"broken"), Endianness::Little),
    )
    .unwrap();
    let mut db = fixture.open().unwrap();
    assert_eq!(db.die(db.roots[0]).unwrap().name, "embedded.c");
    assert!(db.debug_file.is_none());
    fixture.write_debug(&fixture.beside());
    let mut db = Database::open(&fixture.beside(), LoadLimits::default()).unwrap();
    assert_eq!(db.die(db.roots[0]).unwrap().name, "separate.c");
    assert!(db.debug_file.is_none());
}

#[test]
fn malformed_metadata_and_matching_but_unusable_debug_files_are_reported() {
    let fixture = Fixture::new(Endianness::Little);
    fs::write(&fixture.binary, elf(None, None, Endianness::Little)).unwrap();
    assert!(
        fixture
            .open()
            .err()
            .unwrap()
            .to_string()
            .contains("no DWARF units, GNU build ID, or .gnu_debuglink")
    );
    fs::write(
        &fixture.binary,
        elf(None, Some(b"broken"), Endianness::Little),
    )
    .unwrap();
    assert!(
        fixture
            .open()
            .err()
            .unwrap()
            .to_string()
            .contains("reading .gnu_debuglink")
    );
    for (bytes, message) in [
        (b"not an ELF file".to_vec(), "reading debug object"),
        (
            elf(None, None, Endianness::Little),
            "file contains no DWARF units",
        ),
    ] {
        let link = debuglink(b"app.debug", crc32fast::hash(&bytes), Endianness::Little);
        fs::write(&fixture.binary, elf(None, Some(&link), Endianness::Little)).unwrap();
        fs::write(fixture.beside(), &bytes).unwrap();
        fs::create_dir_all(fixture.hidden().parent().unwrap()).unwrap();
        fs::write(fixture.hidden(), &bytes).unwrap();
        let error = fixture.open().err().unwrap().to_string();
        assert!(error.starts_with("could not load matching debug file"));
        assert!(error.contains(message));
        assert!(error.contains(fixture.beside().to_str().unwrap()));
        assert!(!error.contains(fixture.hidden().to_str().unwrap()));
        assert!(!error.contains("Not found:"));
    }
}

#[cfg(unix)]
#[test]
fn symlinks_use_the_target_directory_and_non_utf8_filenames_are_preserved() {
    use std::os::unix::{ffi::OsStrExt, fs::symlink};
    let fixture = Fixture::new(Endianness::Little);
    let filename = b"app-\xff.debug";
    let link = debuglink(
        filename,
        crc32fast::hash(&fixture.debug),
        Endianness::Little,
    );
    fs::write(&fixture.binary, elf(None, Some(&link), Endianness::Little)).unwrap();
    let path = fixture.binary.with_file_name(OsStr::from_bytes(filename));
    fixture.write_debug(&path);
    let alias = fixture._directory.path().join("alias");
    symlink(&fixture.binary, &alias).unwrap();
    let db = open_with_debug_dirs(&alias, &[&fixture.global], LoadLimits::default()).unwrap();
    assert_browsable(db, &path);
}

#[test]
fn loaded_debug_file_path_is_shown_in_the_ui() {
    let fixture = Fixture::new(Endianness::Little);
    fixture.write_debug(&fixture.beside());
    let mut app = crate::app::App::new(fixture.open().unwrap(), "app".into(), true).unwrap();
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(160, 24)).unwrap();
    terminal
        .draw(|frame| crate::ui::draw(frame, &mut app))
        .unwrap();
    let text = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();
    assert!(text.contains("debug:"));
    assert!(text.contains(fixture.beside().to_str().unwrap()));
}

#[test]
fn compressed_sections_are_bounded_before_allocation_through_open() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("oversized.debug");
    // A tiny payload declaring more than the small-section allowance is rejected
    // before the decoder notices that its actual output is empty.
    for compression in [Compression::Gnu, Compression::Zlib, Compression::Zstd] {
        let name = if matches!(compression, Compression::Gnu) {
            ".zdebug_str"
        } else {
            ".debug_str"
        };
        let (bytes, flags) = compressed(b"", 32 * 1024 * 1024, compression, Endianness::Little);
        fs::write(
            &path,
            section_elf(&[(name, bytes, flags)], Endianness::Little),
        )
        .unwrap();
        let error = Database::open(&path, LoadLimits::default()).err().unwrap();
        let error = format!("{error:#}");
        assert!(error.contains("--max-decompression-ratio"), "{error}");
        assert!(error.contains(name), "{error}");
        assert!(error.contains(path.to_str().unwrap()), "{error}");
    }
}

#[test]
fn uncompressed_primary_and_separate_debug_files_allow_any_positive_ratio() {
    let fixture = Fixture::new(Endianness::Little);
    fixture.write_debug(&fixture.beside());
    let limits = LoadLimits {
        max_decompression_ratio: 1,
    };
    for path in [&fixture.binary, &fixture.beside()] {
        let db = Database::open(path, limits).unwrap();
        assert_eq!(db.roots.len(), 1);
    }
}
