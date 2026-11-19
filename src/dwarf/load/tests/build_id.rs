use super::*;

const ID: &[u8] = &[0xab, 0xcd, 0xef, 0x12, 0x34];
const ID_PATH: &str = ".build-id/ab/cdef1234.debug";

fn fixture(endian: Endianness, with_debuglink: bool) -> Fixture {
    let mut fixture = Fixture::new(endian);
    fixture.debug = elf_with_build_id(Some("separate.c"), None, Some(ID), endian);
    let link = debuglink(b"app.debug", crc32fast::hash(&fixture.debug), endian);
    fs::write(
        &fixture.binary,
        elf_with_build_id(
            None,
            with_debuglink.then_some(link.as_slice()),
            Some(ID),
            endian,
        ),
    )
    .unwrap();
    fixture
}

#[test]
fn discovers_by_build_id_without_debuglink_in_both_endiannesses() {
    for endian in [Endianness::Little, Endianness::Big] {
        let fixture = fixture(endian, false);
        let candidate = fixture.global.join(ID_PATH);
        fixture.write_debug(&candidate);
        assert_browsable(fixture.open().unwrap(), &candidate);
    }
}

#[test]
fn embedded_then_build_id_then_debuglink_is_the_search_order() {
    let fixture = fixture(Endianness::Little, true);
    let candidate = fixture.global.join(ID_PATH);
    fixture.write_debug(&candidate);
    fixture.write_debug(&fixture.beside());
    assert_browsable(fixture.open().unwrap(), &candidate);

    let original = fs::read(&fixture.binary).unwrap();
    fs::write(
        &fixture.binary,
        elf_with_build_id(
            Some("embedded.c"),
            Some(b"malformed"),
            Some(ID),
            Endianness::Little,
        ),
    )
    .unwrap();
    let mut db = fixture.open().unwrap();
    assert!(db.debug_file.is_none());
    assert_eq!(db.die(db.roots[0]).unwrap().name, "embedded.c");

    fs::write(&fixture.binary, original).unwrap();
    fs::remove_file(candidate).unwrap();
    assert_browsable(fixture.open().unwrap(), &fixture.beside());
}

#[test]
fn checks_all_build_id_directories_before_filename_lookup() {
    let fixture = fixture(Endianness::Little, true);
    let second_directory = fixture._directory.path().join("second-debug-root");
    let first = fixture.global.join(ID_PATH);
    let second = second_directory.join(ID_PATH);
    fixture.write_debug(&first);
    fixture.write_debug(&second);
    fixture.write_debug(&fixture.beside());
    let roots = [fixture.global.as_path(), second_directory.as_path()];
    assert_browsable(
        open_with_debug_dirs(&fixture.binary, &roots, LoadLimits::default()).unwrap(),
        &first,
    );
    fs::write(&first, b"broken object").unwrap();
    assert_browsable(
        open_with_debug_dirs(&fixture.binary, &roots, LoadLimits::default()).unwrap(),
        &second,
    );
}

#[test]
fn unmatched_candidates_allow_debuglink_fallback() {
    let fixture = fixture(Endianness::Little, true);
    let candidate = fixture.global.join(ID_PATH);
    fs::create_dir_all(candidate.parent().unwrap()).unwrap();
    for (bytes, diagnostic) in [
        (
            elf_with_build_id(Some("wrong.c"), None, Some(b"other ID"), Endianness::Little),
            "build ID mismatch",
        ),
        (
            elf(Some("no-id.c"), None, Endianness::Little),
            "debug object has no GNU build ID",
        ),
        (b"not an object".to_vec(), "reading debug object"),
    ] {
        fs::write(&candidate, bytes).unwrap();
        fixture.write_debug(&fixture.beside());
        assert_browsable(fixture.open().unwrap(), &fixture.beside());
        // If both mechanisms fail, retain the reason and path for each attempt.
        fs::write(fixture.beside(), b"CRC mismatch").unwrap();
        let error = fixture.open().err().unwrap().to_string();
        assert!(error.contains(diagnostic), "{error}");
        assert!(error.contains("CRC mismatch"), "{error}");
        assert!(error.contains(candidate.to_str().unwrap()), "{error}");
        assert!(
            error.contains(fixture.beside().to_str().unwrap()),
            "{error}"
        );
    }
}

#[test]
fn matching_build_id_load_failure_stops_before_other_paths() {
    let fixture = fixture(Endianness::Little, true);
    let candidate = fixture.global.join(ID_PATH);
    let second_root = fixture._directory.path().join("second-debug-root");
    let second = second_root.join(ID_PATH);
    fixture.write_debug(&candidate);
    fixture.write_debug(&second);
    fixture.write_debug(&fixture.beside());
    let roots = [fixture.global.as_path(), second_root.as_path()];
    let mut malformed = fixture.debug.clone();
    let object = object::File::parse(malformed.as_slice()).unwrap();
    let (offset, _) = object
        .section_by_name(".debug_info")
        .unwrap()
        .file_range()
        .unwrap();
    // Corrupt the unit header while retaining the matching ELF build ID.
    malformed[offset as usize..offset as usize + 4].fill(0xff);
    // Both later paths contain usable, matching DWARF. Neither should be used
    // once the first file has passed identity validation.
    for (bytes, diagnostic) in [
        (
            elf_with_build_id(None, None, Some(ID), Endianness::Little),
            "file contains no DWARF units",
        ),
        (malformed, "reading .debug_info"),
    ] {
        fs::write(&candidate, bytes).unwrap();
        let error = open_with_debug_dirs(&fixture.binary, &roots, LoadLimits::default())
            .err()
            .unwrap()
            .to_string();
        assert!(
            error.starts_with("could not load matching debug file"),
            "{error}"
        );
        assert!(error.contains(diagnostic), "{error}");
        assert!(error.contains(candidate.to_str().unwrap()), "{error}");
        assert!(!error.contains(second.to_str().unwrap()), "{error}");
        assert!(
            !error.contains(fixture.beside().to_str().unwrap()),
            "{error}"
        );
        assert!(!error.contains("Not found:"), "{error}");
    }
}

#[test]
fn reports_build_id_failures_without_a_debuglink() {
    let fixture = fixture(Endianness::Little, false);
    let candidate = fixture.global.join(ID_PATH);
    let error = fixture.open().err().unwrap().to_string();
    assert!(error.contains("no usable debug information found"));
    assert!(error.contains("no .gnu_debuglink fallback"));
    assert!(error.contains(candidate.to_str().unwrap()));
    fixture.write_debug(&candidate);
    assert_browsable(fixture.open().unwrap(), &candidate);
}
