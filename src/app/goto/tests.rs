use super::*;

#[test]
fn parses_section_relative_hex_with_default_section_and_rejects_bad_input() {
    let mut dialog = GotoDialog::new(DieSection::Types);
    assert_eq!(dialog.input, "types:0x");
    assert_eq!(dialog.cursor, dialog.input.len());
    for (input, section, offset) in [
        ("info:0x1234", DieSection::Info, 0x1234),
        ("types:AB", DieSection::Types, 0xab),
        (" 0XFF ", DieSection::Types, 0xff),
        ("10", DieSection::Types, 0x10),
        ("sup:info:0x1234", DieSection::SupplementaryInfo, 0x1234),
        ("sup:types:AB", DieSection::SupplementaryTypes, 0xab),
    ] {
        dialog.input = input.into();
        assert_eq!(dialog.destination().unwrap(), (section, offset));
    }
    for input in [
        "",
        "info:0x",
        "types:",
        "foo:12",
        "-1",
        "+1",
        "0xgg",
        "1:2:3",
        "12 34",
        "0xffffffffffffffffffffffffffffffff",
    ] {
        dialog.input = input.into();
        assert!(dialog.destination().is_err(), "{input}");
    }
}

#[test]
fn edits_prefilled_offset_at_cursor() {
    let mut dialog = GotoDialog::new(DieSection::Info);
    assert_eq!(dialog.input, "info:0x");
    for code in [
        KeyCode::Char('2'),
        KeyCode::Char('f'),
        KeyCode::Left,
        KeyCode::Backspace,
        KeyCode::Char('1'),
    ] {
        dialog.edit(KeyEvent::new(code, KeyModifiers::NONE));
    }
    assert_eq!(dialog.input, "info:0x1f");
    dialog.edit(KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE));
    assert_eq!(dialog.input, "info:0x1");
    dialog.edit(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
    assert_eq!(dialog.cursor, 0);
    dialog.edit(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
    assert_eq!(dialog.cursor, dialog.input.len());
    dialog.edit(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    assert_eq!(dialog.input, "");
    assert_eq!(dialog.cursor, 0);
}
