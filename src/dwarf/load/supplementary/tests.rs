use super::*;
use crate::dwarf::{DieSection, LoadLimits, load::open_with_debug_dirs};
use object::{Architecture, BinaryFormat, Endianness, SectionKind};

use super::super::limits::tests::{Compression, compressed, section_elf};

const BUILD_ID: &[u8] = &[0x12, 0x34, 0x56, 0x78];
type Sections = Vec<(&'static str, Vec<u8>)>;

fn uleb(bytes: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        bytes.push(byte | if value != 0 { 0x80 } else { 0 });
        if value == 0 {
            break;
        }
    }
}

fn u32_bytes(value: u32, endian: Endianness) -> [u8; 4] {
    if endian == Endianness::Little {
        value.to_le_bytes()
    } else {
        value.to_be_bytes()
    }
}

fn u16_bytes(value: u16, endian: Endianness) -> [u8; 2] {
    if endian == Endianness::Little {
        value.to_le_bytes()
    } else {
        value.to_be_bytes()
    }
}

fn abbrev(
    bytes: &mut Vec<u8>,
    code: u8,
    tag: gimli::DwTag,
    children: bool,
    attrs: &[(gimli::DwAt, gimli::DwForm)],
) {
    bytes.push(code);
    uleb(bytes, tag.0 as u64);
    bytes.push(u8::from(children));
    for (name, form) in attrs {
        uleb(bytes, name.0 as u64);
        uleb(bytes, form.0 as u64);
    }
    bytes.extend_from_slice(&[0, 0]);
}

fn sections(supplementary: bool, gnu: bool, endian: Endianness) -> Sections {
    use gimli::*;
    let version = if gnu { 4u16 } else { 5 };
    let string_sup = if gnu {
        DW_FORM_GNU_strp_alt
    } else {
        DW_FORM_strp_sup
    };
    let ref_sup = if gnu {
        DW_FORM_GNU_ref_alt
    } else {
        DW_FORM_ref_sup4
    };
    let mut abbreviations = Vec::new();
    abbrev(
        &mut abbreviations,
        1,
        if supplementary {
            DW_TAG_partial_unit
        } else {
            DW_TAG_compile_unit
        },
        true,
        &[
            (DW_AT_name, DW_FORM_strp),
            (
                DW_AT_comp_dir,
                if supplementary {
                    DW_FORM_strp
                } else {
                    string_sup
                },
            ),
        ],
    );
    abbrev(
        &mut abbreviations,
        2,
        DW_TAG_subprogram,
        false,
        &[(DW_AT_name, DW_FORM_strp)],
    );
    abbrev(
        &mut abbreviations,
        3,
        DW_TAG_imported_unit,
        false,
        &[(DW_AT_import, ref_sup)],
    );
    abbrev(
        &mut abbreviations,
        4,
        DW_TAG_subprogram,
        false,
        &[(DW_AT_specification, ref_sup)],
    );
    abbrev(
        &mut abbreviations,
        5,
        DW_TAG_variable,
        false,
        &[(DW_AT_name, string_sup)],
    );
    abbrev(
        &mut abbreviations,
        6,
        DW_TAG_variable,
        false,
        &[(DW_AT_type, DW_FORM_ref_addr)],
    );
    abbrev(
        &mut abbreviations,
        7,
        DW_TAG_variable,
        false,
        &[(DW_AT_type, DW_FORM_ref4)],
    );
    abbreviations.push(0);
    let mut info = vec![0; 4];
    info.extend_from_slice(&u16_bytes(version, endian));
    if version == 5 {
        info.extend_from_slice(&[
            if supplementary {
                DW_UT_partial.0
            } else {
                DW_UT_compile.0
            },
            8,
        ]);
        info.extend_from_slice(&u32_bytes(0, endian));
    } else {
        info.extend_from_slice(&u32_bytes(0, endian));
        info.push(8);
    }
    let root = info.len() as u32;
    info.push(1);
    info.extend_from_slice(&u32_bytes(0, endian));
    info.extend_from_slice(&u32_bytes(0, endian));
    let child = info.len() as u32;
    info.push(2);
    info.extend_from_slice(&u32_bytes(if supplementary { 9 } else { 7 }, endian));
    if !supplementary {
        for (code, offset) in [(3, root), (4, child), (5, 9)] {
            info.push(code);
            info.extend_from_slice(&u32_bytes(offset, endian));
        }
    }
    // Same numeric offsets in both objects must resolve within their own object.
    for code in [6, 7] {
        info.push(code);
        info.extend_from_slice(&u32_bytes(child, endian));
    }
    info.push(0);
    let len = (info.len() - 4) as u32;
    info[..4].copy_from_slice(&u32_bytes(len, endian));
    vec![
        (".debug_abbrev", abbreviations),
        (".debug_info", info),
        (
            ".debug_str",
            if supplementary {
                b"shared.c\0imported_name\0".to_vec()
            } else {
                b"main.c\0local\0".to_vec()
            },
        ),
    ]
}

fn elf(sections: &Sections, endian: Endianness) -> Vec<u8> {
    let architecture = if endian == Endianness::Little {
        Architecture::X86_64
    } else {
        Architecture::PowerPc64
    };
    let mut object = object::write::Object::new(BinaryFormat::Elf, architecture, endian);
    for (name, data) in sections {
        let kind = if name.starts_with(".note") {
            SectionKind::Note
        } else {
            SectionKind::Debug
        };
        let section = object.add_section(Vec::new(), name.as_bytes().to_vec(), kind);
        object.append_section_data(section, data, if kind == SectionKind::Note { 4 } else { 1 });
    }
    object.write().unwrap()
}

fn sup_record(supplementary: bool, filename: &[u8], id: &[u8], endian: Endianness) -> Vec<u8> {
    let mut bytes = u16_bytes(5, endian).to_vec();
    bytes.push(u8::from(supplementary));
    bytes.extend_from_slice(filename);
    bytes.push(0);
    uleb(&mut bytes, id.len() as u64);
    bytes.extend_from_slice(id);
    bytes
}

fn build_id_note(id: &[u8], endian: Endianness) -> Vec<u8> {
    let mut bytes = Vec::new();
    for value in [4, id.len() as u32, 3] {
        bytes.extend_from_slice(&u32_bytes(value, endian));
    }
    bytes.extend_from_slice(b"GNU\0");
    bytes.extend_from_slice(id);
    bytes.resize(bytes.len().next_multiple_of(4), 0);
    bytes
}

struct Fixture {
    _directory: tempfile::TempDir,
    primary: PathBuf,
    supplement: PathBuf,
    debug_dir: PathBuf,
    primary_sections: Sections,
    supplementary_sections: Sections,
    endian: Endianness,
}

impl Fixture {
    fn new(gnu: bool, endian: Endianness) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let base = directory.path().canonicalize().unwrap();
        let primary = base.join("bin/program.debug");
        let supplement = base.join("shared/types.debug");
        let mut primary_sections = sections(false, gnu, endian);
        let mut supplementary_sections = sections(true, gnu, endian);
        if gnu {
            let mut link = b"../shared/types.debug\0".to_vec();
            link.extend_from_slice(BUILD_ID);
            primary_sections.push((".gnu_debugaltlink", link));
            supplementary_sections.push((".note.gnu.build-id", build_id_note(BUILD_ID, endian)));
        } else {
            primary_sections.push((
                ".debug_sup",
                sup_record(false, b"../shared/types.debug", BUILD_ID, endian),
            ));
            supplementary_sections.push((".debug_sup", sup_record(true, b"", BUILD_ID, endian)));
        }
        let fixture = Self {
            _directory: directory,
            primary,
            supplement,
            debug_dir: base.join("debug-root"),
            primary_sections,
            supplementary_sections,
            endian,
        };
        fixture.write();
        fixture
    }

    fn write(&self) {
        fs::create_dir_all(self.primary.parent().unwrap()).unwrap();
        fs::create_dir_all(self.supplement.parent().unwrap()).unwrap();
        fs::write(&self.primary, elf(&self.primary_sections, self.endian)).unwrap();
        fs::write(
            &self.supplement,
            elf(&self.supplementary_sections, self.endian),
        )
        .unwrap();
    }

    fn open(&self) -> Database {
        open_with_debug_dirs(&self.primary, &[&self.debug_dir], LoadLimits::default()).unwrap()
    }
}

#[test]
fn supplementary_strings_imports_and_references_use_their_own_address_spaces() {
    for gnu in [true, false] {
        for endian in [Endianness::Little, Endianness::Big] {
            let fixture = Fixture::new(gnu, endian);
            let mut db = fixture.open();
            assert!(db.supplementary_error.is_none());
            assert_eq!(
                db.supplementary_file.as_deref(),
                Some(fixture.supplement.as_path())
            );
            assert_eq!(db.roots.len(), 2);
            let main_root = db.roots[0];
            let sup_root = db.roots[1];
            assert_eq!(db.die(main_root).unwrap().name, "main.c");
            assert_eq!(db.die(sup_root).unwrap().name, "shared.c");
            assert_eq!(
                db.units[main_root.unit]
                    .comp_dir
                    .as_ref()
                    .unwrap()
                    .to_string_lossy()
                    .unwrap(),
                "shared.c"
            );
            let main = db.children(main_root).unwrap();
            let sup = db.children(sup_root).unwrap();
            assert_eq!(main[0].id.offset, sup[0].id.offset);
            assert_ne!(main[0].id, sup[0].id);
            assert_eq!(main[0].name, "local");
            assert_eq!(sup[0].name, "imported_name");
            assert_eq!(main[2].name, "imported_name");
            assert_eq!(main[3].name, "imported_name");
            assert_eq!(db.imported_unit(main[1].id).unwrap().id, sup_root);
            assert!(
                main[1]
                    .import_label
                    .as_ref()
                    .unwrap()
                    .starts_with("[sup:info:")
            );
            assert_eq!(
                db.attributes(main[4].id).unwrap()[0].target,
                Some(main[0].id)
            );
            assert_eq!(
                db.attributes(main[5].id).unwrap()[0].target,
                Some(main[0].id)
            );
            assert_eq!(db.attributes(sup[1].id).unwrap()[0].target, Some(sup[0].id));
            assert_eq!(db.attributes(sup[2].id).unwrap()[0].target, Some(sup[0].id));
            assert_eq!(
                db.attributes(main[3].id).unwrap()[0].value,
                "\"imported_name\""
            );
            assert_eq!(db.path(sup[0].id).unwrap(), [sup_root, sup[0].id]);
            assert_eq!(
                db.find_die(DieSection::Info, main[0].id.offset).unwrap(),
                main[0].id
            );
            assert_eq!(
                db.find_die(DieSection::SupplementaryInfo, sup[0].id.offset)
                    .unwrap(),
                sup[0].id
            );
        }
    }
}

#[test]
fn supplementary_import_jumps_and_goto_restore_history_and_show_source_path() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let fixture = Fixture::new(true, Endianness::Little);
    let mut app = crate::app::App::new(fixture.open(), "program".into(), false).unwrap();
    for code in [KeyCode::Right, KeyCode::Right, KeyCode::Down] {
        app.handle_key(KeyEvent::new(code, KeyModifiers::NONE))
            .unwrap();
    }
    let import = app.selected_die();
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();
    assert_eq!(app.selected_die(), Some(app.database.roots[1]));
    app.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL))
        .unwrap();
    assert_eq!(app.goto_dialog.as_ref().unwrap().input, "sup:info:0x");
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
        .unwrap();
    app.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL))
        .unwrap();
    assert_eq!(app.selected_die(), import);
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(180, 30)).unwrap();
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
    assert!(text.contains("supplementary:"));
    assert!(text.contains(fixture.supplement.to_str().unwrap()));
    assert!(text.contains("sup:info:"));
}

#[test]
fn missing_and_mismatched_supplements_report_the_cause_without_blocking_primary_data() {
    for gnu in [true, false] {
        let mut fixture = Fixture::new(gnu, Endianness::Little);
        fs::remove_file(&fixture.supplement).unwrap();
        let mut db = fixture.open();
        assert!(db.supplementary_file.is_none());
        assert!(
            db.supplementary_error
                .as_ref()
                .unwrap()
                .contains("types.debug")
        );
        let children = db.children(db.roots[0]).unwrap();
        assert_eq!(children[0].name, "local");
        assert!(
            children[1]
                .import_label
                .as_ref()
                .unwrap()
                .contains("types.debug")
        );
        let record = fixture.supplementary_sections.last_mut().unwrap();
        record.1 = if gnu {
            build_id_note(b"wrong", fixture.endian)
        } else {
            sup_record(true, b"", b"wrong", fixture.endian)
        };
        fixture.write();
        let db = fixture.open();
        assert!(db.supplementary_file.is_none());
        assert!(
            db.supplementary_error
                .as_ref()
                .unwrap()
                .contains("mismatch")
        );
    }
}

#[test]
fn gnu_supplementary_build_id_fallback_and_absolute_paths_work() {
    let mut fixture = Fixture::new(true, Endianness::Little);
    let by_id = fixture.debug_dir.join(".build-id/12/345678.debug");
    fs::create_dir_all(by_id.parent().unwrap()).unwrap();
    fs::rename(&fixture.supplement, &by_id).unwrap();
    assert_eq!(
        fixture.open().supplementary_file.as_deref(),
        Some(by_id.as_path())
    );
    let mut link = by_id.to_str().unwrap().as_bytes().to_vec();
    link.push(0);
    link.extend_from_slice(BUILD_ID);
    fixture.primary_sections.last_mut().unwrap().1 = link;
    fs::write(
        &fixture.primary,
        elf(&fixture.primary_sections, fixture.endian),
    )
    .unwrap();
    assert_eq!(
        fixture.open().supplementary_file.as_deref(),
        Some(by_id.as_path())
    );
}

#[test]
fn debuglink_resolves_supplement_relative_to_the_separate_debug_file() {
    let fixture = Fixture::new(true, Endianness::Little);
    let root = fixture.primary.parent().unwrap().parent().unwrap();
    let debug = root.join(".debug/program.debug");
    fs::create_dir_all(debug.parent().unwrap()).unwrap();
    fs::rename(&fixture.primary, &debug).unwrap();
    let mut link = b"program.debug\0".to_vec();
    link.resize(link.len().next_multiple_of(4), 0);
    link.extend_from_slice(&crc32fast::hash(&fs::read(&debug).unwrap()).to_le_bytes());
    let executable = root.join("program");
    fs::write(
        &executable,
        elf(&vec![(".gnu_debuglink", link)], fixture.endian),
    )
    .unwrap();
    let db =
        open_with_debug_dirs(&executable, &[&fixture.debug_dir], LoadLimits::default()).unwrap();
    assert_eq!(db.debug_file.as_deref(), Some(debug.as_path()));
    assert_eq!(
        db.supplementary_file.as_deref(),
        Some(fixture.supplement.as_path())
    );
    assert!(db.supplementary_error.is_none());
}

#[cfg(unix)]
#[test]
fn build_id_discovery_resolves_supplement_relative_to_the_symlink_target() {
    let mut fixture = Fixture::new(true, Endianness::Little);
    let primary_id = &[0xab, 0xcd, 0xef, 0x12, 0x34];
    fixture.primary_sections.push((
        ".note.gnu.build-id",
        build_id_note(primary_id, fixture.endian),
    ));
    fixture.write();
    let by_id = fixture.debug_dir.join(".build-id/ab/cdef1234.debug");
    fs::create_dir_all(by_id.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&fixture.primary, &by_id).unwrap();
    let executable = fixture._directory.path().join("program");
    fs::write(
        &executable,
        elf(
            &vec![(
                ".note.gnu.build-id",
                build_id_note(primary_id, fixture.endian),
            )],
            fixture.endian,
        ),
    )
    .unwrap();
    let mut db =
        open_with_debug_dirs(&executable, &[&fixture.debug_dir], LoadLimits::default()).unwrap();
    assert_eq!(db.debug_file.as_deref(), Some(fixture.primary.as_path()));
    assert_eq!(
        db.supplementary_file.as_deref(),
        Some(fixture.supplement.as_path())
    );
    assert!(db.supplementary_error.is_none());
    let children = db.children(db.roots[0]).unwrap();
    assert_eq!(db.imported_unit(children[1].id).unwrap().id, db.roots[1]);
}

#[test]
fn dwarf5_sup_handles_empty_identifiers_and_string_only_supplements() {
    let mut fixture = Fixture::new(false, Endianness::Little);
    fixture.primary_sections.last_mut().unwrap().1 =
        sup_record(false, b"../shared/types.debug", b"", fixture.endian);
    fixture
        .supplementary_sections
        .retain(|(name, _)| *name != ".debug_info" && *name != ".debug_abbrev");
    fixture.write();
    let mut db = fixture.open();
    assert!(db.supplementary_error.is_none());
    assert_eq!(db.roots.len(), 1);
    let children = db.children(db.roots[0]).unwrap();
    assert_eq!(children[3].name, "imported_name");
    assert!(
        children[1]
            .import_label
            .as_ref()
            .unwrap()
            .contains("outside supplementary")
    );
}

#[test]
fn malformed_debug_sup_metadata_is_reported() {
    let mut fixture = Fixture::new(false, Endianness::Little);
    for bytes in [vec![5], vec![5, 0, 2, 0, 0], vec![5, 0, 0, b'x', 0, 20]] {
        fixture.primary_sections.last_mut().unwrap().1 = bytes;
        fixture.write();
        let db = fixture.open();
        assert!(db.supplementary_file.is_none());
        assert!(
            db.supplementary_error
                .as_ref()
                .unwrap()
                .contains(".debug_sup")
        );
    }
}

#[test]
fn uncompressed_primary_and_supplement_load_with_any_positive_ratio() {
    for gnu in [true, false] {
        let fixture = Fixture::new(gnu, Endianness::Little);
        let mut db = Database::open(
            &fixture.primary,
            LoadLimits {
                max_decompression_ratio: 1,
            },
        )
        .unwrap();
        assert!(db.supplementary_error.is_none());
        assert_eq!(db.roots.len(), 2);
        assert_eq!(db.children(db.roots[0]).unwrap()[0].name, "local");
    }
}

#[test]
fn matching_supplement_load_failure_stops_before_build_id_fallback() {
    let mut fixture = Fixture::new(true, Endianness::Little);
    let by_id = fixture.debug_dir.join(".build-id/12/345678.debug");
    fs::create_dir_all(by_id.parent().unwrap()).unwrap();
    fs::copy(&fixture.supplement, &by_id).unwrap();
    // The identity still matches after changing the sections. Even though the
    // build-ID path holds a smaller usable copy, stop on this loading failure.
    let strings = fixture
        .supplementary_sections
        .iter_mut()
        .find(|(name, _)| *name == ".debug_str")
        .unwrap();
    strings.0 = ".zdebug_str";
    strings.1 = compressed(
        &strings.1,
        32 * 1024 * 1024,
        Compression::Gnu,
        fixture.endian,
    )
    .0;
    fixture.write();
    let db = open_with_debug_dirs(
        &fixture.primary,
        &[&fixture.debug_dir],
        LoadLimits::default(),
    )
    .unwrap();
    let error = db.supplementary_error.as_ref().unwrap();
    assert!(error.contains("--max-decompression-ratio"), "{error}");
    assert!(!error.contains(by_id.to_str().unwrap()), "{error}");
    assert!(db.supplementary_file.is_none());
    assert_eq!(db.roots.len(), 1);
}

#[test]
fn compressed_debug_sup_is_bounded_in_both_primary_and_supplementary_objects() {
    for primary in [true, false] {
        for compression in [Compression::Zlib, Compression::Zstd] {
            let fixture = Fixture::new(false, Endianness::Little);
            let (sections, path) = if primary {
                (&fixture.primary_sections, &fixture.primary)
            } else {
                (&fixture.supplementary_sections, &fixture.supplement)
            };
            let oversized: Vec<_> = sections
                .iter()
                .map(|(name, bytes)| {
                    if *name == ".debug_sup" {
                        let (bytes, flags) =
                            compressed(bytes, 32 * 1024 * 1024, compression, fixture.endian);
                        (*name, bytes, flags)
                    } else {
                        (*name, bytes.clone(), 0)
                    }
                })
                .collect();
            fs::write(path, section_elf(&oversized, fixture.endian)).unwrap();
            let mut db = Database::open(&fixture.primary, LoadLimits::default()).unwrap();
            let error = db.supplementary_error.as_ref().unwrap();
            assert!(
                error.contains(".debug_sup") && error.contains("--max-decompression-ratio"),
                "{error}"
            );
            let source = if primary {
                fixture.primary.clone()
            } else {
                fixture
                    .primary
                    .parent()
                    .unwrap()
                    .join("../shared/types.debug")
            };
            assert!(error.contains(source.to_str().unwrap()), "{error}");
            assert_eq!(db.children(db.roots[0]).unwrap()[0].name, "local");
        }
    }
}
