use std::{fs, process::Command};

use object::{Architecture, BinaryFormat, Endianness, SectionKind};

fn debuglink_error(filename: &str) -> String {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("input.elf");
    let mut link = filename.as_bytes().to_vec();
    link.push(0);
    link.resize(link.len().next_multiple_of(4), 0);
    link.extend_from_slice(&0u32.to_le_bytes());

    let mut object =
        object::write::Object::new(BinaryFormat::Elf, Architecture::X86_64, Endianness::Little);
    let section = object.add_section(Vec::new(), b".gnu_debuglink".to_vec(), SectionKind::Debug);
    object.append_section_data(section, &link, 4);
    fs::write(&path, object.write().unwrap()).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_dwi"))
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    String::from_utf8(output.stderr).unwrap()
}

#[test]
fn debuglink_errors_escape_terminal_controls() {
    let stderr = debuglink_error("\u{1b}]0;injected-title\u{7}\r\n\t\u{9b}31m-café.debug");
    let message = stderr.strip_suffix('\n').expect("final newline");
    assert!(
        !message.chars().any(|c| c.is_control() && c != '\n'),
        "unescaped control characters in stderr: {stderr:?}"
    );
    assert!(message.starts_with("Error: no usable debug information found\n"));
    assert!(message.contains("\\u{1b}]0;injected-title\\u{7}\\r\\n\\t\\u{9b}31m-café.debug"));
    assert!(message.contains("input.elf"));
    // Only the application's layout introduces line breaks, not the filename.
    assert_eq!(message.lines().count(), 8);
    assert!(
        message
            .lines()
            .skip(1)
            .all(|line| line.is_empty() || line.starts_with("  "))
    );
}

#[test]
fn debuglink_search_attempts_print_on_separate_lines() {
    let stderr = debuglink_error("missing.debug");
    let lines: Vec<_> = stderr.lines().collect();
    assert_eq!(lines.len(), 8);
    assert_eq!(lines[0], "Error: no usable debug information found");
    assert_eq!(lines[4], "  Not found:");
    assert!(
        lines[5..]
            .iter()
            .all(|line| line.starts_with("    ") && line.ends_with("missing.debug"))
    );
    assert!(!stderr.contains("os error"));
    assert!(!stderr.contains("\\n"));
}
