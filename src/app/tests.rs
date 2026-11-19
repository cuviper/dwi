use super::*;

fn app() -> App {
    App::new(
        crate::dwarf::tests::fixture(5, gimli::RunTimeEndian::Little),
        "fixture".into(),
        false,
    )
    .unwrap()
}

fn key(app: &mut App, code: KeyCode) {
    app.handle_key(KeyEvent::new(code, KeyModifiers::NONE))
        .unwrap();
}

fn control(app: &mut App, c: char) {
    app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
        .unwrap();
}

#[test]
fn goto_cross_unit_target_reveals_ancestors_and_restores_history() {
    let mut app = app();
    let original = app.selected_die();
    let root = app.database.roots[1];
    let namespace = app.database.children(root).unwrap().remove(0);
    let target = app.database.children(namespace.id).unwrap().remove(0).id;
    let location = app.database.location(target);
    key(&mut app, KeyCode::Tab);
    control(&mut app, 'g');
    assert_eq!(app.goto_dialog.as_ref().unwrap().input, "info:0x");
    // Append to the prefilled section and 0x, as in normal interactive use.
    for c in location.strip_prefix("info:0x").unwrap().chars() {
        key(&mut app, KeyCode::Char(c));
    }
    key(&mut app, KeyCode::Enter);
    assert!(app.goto_dialog.is_none());
    assert_eq!(app.selected_die(), Some(target));
    assert!(app.pane == Pane::Dies);
    assert!(app.dies.nodes[app.dies.roots[1]].expanded);
    control(&mut app, 'o');
    assert_eq!(app.selected_die(), original);
    assert!(app.pane == Pane::Attributes);
    assert!(!app.dies.nodes[app.dies.roots[1]].expanded);
    control(&mut app, 'y');
    assert_eq!(app.selected_die(), Some(target));
}

#[test]
fn goto_types_prefill_and_bare_offsets_use_the_selected_section() {
    let mut app = App::new(crate::dwarf::tests::type_fixture(), "types".into(), false).unwrap();
    control(&mut app, 'g');
    assert_eq!(app.goto_dialog.as_ref().unwrap().input, "types:0x");
    control(&mut app, 'u');
    for c in "18".chars() {
        key(&mut app, KeyCode::Char(c));
    }
    key(&mut app, KeyCode::Enter);
    assert!(app.goto_dialog.is_none());
    let selected = app.dies.current().unwrap();
    assert_eq!(app.dies.nodes[selected].value.name, "int");
    assert_eq!(app.dies.nodes[selected].value.location, "types:0x18");
}

#[test]
fn invalid_goto_stays_modal_and_cancel_preserves_navigation() {
    let mut app = app();
    let original = app.selected_die();
    control(&mut app, 'g');
    key(&mut app, KeyCode::Enter);
    let dialog = app.goto_dialog.as_ref().unwrap();
    assert_eq!(dialog.input, "info:0x");
    assert!(dialog.error.is_some());
    key(&mut app, KeyCode::Char('0'));
    key(&mut app, KeyCode::Enter);
    assert!(app.goto_dialog.as_ref().unwrap().error.is_some());
    assert_eq!(app.selected_die(), original);
    assert!(app.back.is_empty());
    // Normal browser shortcuts type into the prompt instead of taking effect.
    key(&mut app, KeyCode::Char('q'));
    key(&mut app, KeyCode::Char('j'));
    key(&mut app, KeyCode::Tab);
    assert_eq!(app.selected_die(), original);
    assert!(app.pane == Pane::Dies);
    assert!(app.goto_dialog.as_ref().unwrap().input.ends_with("qj"));
    key(&mut app, KeyCode::Esc);
    assert!(app.goto_dialog.is_none());
    assert_eq!(app.selected_die(), original);
    assert!(app.back.is_empty());
}

#[test]
fn goto_prompt_keeps_cursor_visible_when_resized_and_shows_errors() {
    use ratatui::{Terminal, backend::TestBackend};
    let mut app = app();
    control(&mut app, 'g');
    for _ in 0..80 {
        key(&mut app, KeyCode::Char('f'));
    }
    key(&mut app, KeyCode::Enter);
    for (width, height) in [(24, 6), (40, 12), (120, 30)] {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        for keycode in [KeyCode::Home, KeyCode::End] {
            key(&mut app, keycode);
            terminal
                .draw(|frame| crate::ui::draw(frame, &mut app))
                .unwrap();
            let cursor = terminal.get_cursor_position().unwrap();
            assert!(cursor.x < width && cursor.y < height);
            let text = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            assert!(text.contains("Go to DIE"));
            assert!(text.contains("Offset is too large"));
        }
    }
}

#[test]
fn inline_cycles_are_bounded_but_can_still_be_followed() {
    let mut app = app();
    key(&mut app, KeyCode::Right);
    key(&mut app, KeyCode::Right);
    let reference = app
        .attributes
        .nodes
        .iter()
        .position(|node| node.value.name == "type")
        .unwrap();
    app.attributes.select_node(reference);
    key(&mut app, KeyCode::Tab);
    key(&mut app, KeyCode::Right);
    let containing = *app.attributes.nodes[reference]
        .children
        .as_ref()
        .unwrap()
        .iter()
        .find(|index| app.attributes.nodes[**index].value.name == "containing_type")
        .unwrap();
    app.attributes.select_node(containing);
    key(&mut app, KeyCode::Right);
    let cycle = app.attributes.nodes[containing].children.as_ref().unwrap()[0];
    assert!(!app.attributes.nodes[cycle].expandable);
    app.attributes.select_node(cycle);
    let count = app.attributes.nodes.len();
    key(&mut app, KeyCode::Right);
    assert_eq!(app.attributes.nodes.len(), count);
    key(&mut app, KeyCode::Enter);
    assert_eq!(
        app.dies.nodes[app.dies.current().unwrap()].value.name,
        "Node"
    );
}

#[test]
fn cross_unit_jump_history_restores_folds_attribute_selection_and_focus() {
    let mut app = app();
    key(&mut app, KeyCode::Right);
    key(&mut app, KeyCode::Right);
    let original = app.selected_die();
    let reference = app
        .attributes
        .nodes
        .iter()
        .position(|node| node.value.name == "type")
        .unwrap();
    app.attributes.select_node(reference);
    key(&mut app, KeyCode::Tab);
    key(&mut app, KeyCode::Right);
    let original_rows = app.attributes.visible.clone();
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.selected_die().unwrap().unit, 1);
    assert!(app.dies.nodes[app.dies.roots[1]].expanded);
    app.history(false);
    assert_eq!(app.selected_die(), original);
    assert!(app.pane == Pane::Attributes);
    assert_eq!(app.attributes.current(), Some(reference));
    assert_eq!(app.attributes.visible, original_rows);
    assert!(!app.dies.nodes[app.dies.roots[1]].expanded);
    app.history(true);
    assert_eq!(app.selected_die().unwrap().unit, 1);
    assert!(app.pane == Pane::Dies);
}

#[test]
fn failed_jump_does_not_change_navigation() {
    let mut app = app();
    let original = app.selected_die();
    let invalid = DieId {
        unit: 1,
        offset: usize::MAX,
    };
    assert!(app.jump(invalid).is_err());
    assert_eq!(app.selected_die(), original);
    assert!(app.back.is_empty());
}

#[test]
fn location_lists_expand_inline_and_through_die_references() {
    let mut app = App::new(
        crate::dwarf::tests::location_fixture(5, gimli::RunTimeEndian::Little),
        "locations".into(),
        false,
    )
    .unwrap();
    key(&mut app, KeyCode::Right);
    key(&mut app, KeyCode::Right);
    key(&mut app, KeyCode::Tab);
    let list = app.attributes.current().unwrap();
    assert!(app.attributes.nodes[list].expandable);
    assert!(app.attributes.nodes[list].children.is_none());
    key(&mut app, KeyCode::Enter);
    assert_eq!(
        app.attributes.nodes[list].children.as_ref().unwrap().len(),
        4
    );
    let count = app.attributes.nodes.len();
    key(&mut app, KeyCode::Left);
    assert_eq!(app.attributes.visible.len(), 1);
    key(&mut app, KeyCode::Right);
    assert_eq!(app.attributes.nodes.len(), count);

    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(140, 24)).unwrap();
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
    assert!(text.contains("fbreg -16"));
    assert!(text.contains("[0x2020, 0x2030)"));

    key(&mut app, KeyCode::Tab);
    key(&mut app, KeyCode::Down); // Alias referring to the DIE with the location list.
    key(&mut app, KeyCode::Tab);
    key(&mut app, KeyCode::Right);
    key(&mut app, KeyCode::Right);
    let nested_list = app.attributes.current().unwrap();
    assert!(
        app.attributes.nodes[nested_list]
            .value
            .location_list
            .is_some()
    );
    key(&mut app, KeyCode::Right);
    assert_eq!(
        app.attributes.nodes[nested_list]
            .children
            .as_ref()
            .unwrap()
            .len(),
        4
    );
    key(&mut app, KeyCode::Left);
    key(&mut app, KeyCode::Left); // Back to the specification reference.
    key(&mut app, KeyCode::Enter);
    app.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL))
        .unwrap();
    assert!(app.pane == Pane::Attributes);
    assert!(app.attributes.nodes[nested_list].children.is_some());
}

#[test]
fn bad_location_list_remains_collapsed_and_empty_list_is_reported() {
    let mut app = App::new(
        crate::dwarf::tests::location_fixture(5, gimli::RunTimeEndian::Little),
        "locations".into(),
        false,
    )
    .unwrap();
    key(&mut app, KeyCode::Right);
    key(&mut app, KeyCode::Right);
    key(&mut app, KeyCode::Tab);
    let index = app.attributes.current().unwrap();
    app.attributes.nodes[index].value.location_list = Some(crate::dwarf::LocationListRef::Offset(
        gimli::LocationListsOffset(usize::MAX),
    ));
    assert!(
        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE))
            .is_err()
    );
    assert!(app.attributes.nodes[index].children.is_none());
    assert!(!app.attributes.nodes[index].expanded);
    key(&mut app, KeyCode::Tab);
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Tab);
    key(&mut app, KeyCode::Right);
    assert_eq!(app.message, "Location list is empty");
    assert_eq!(app.attributes.visible.len(), 1);
}

#[test]
fn renders_small_and_large_terminals_in_all_layouts() {
    use ratatui::{Terminal, backend::TestBackend};
    let mut app = app();
    for (width, height) in [(10, 4), (40, 12), (120, 30)] {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        for stacked in [false, true] {
            for fullscreen in [false, true] {
                app.stacked = stacked;
                app.fullscreen = fullscreen;
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
                assert!(text.contains(if width < 24 { "Terminal" } else { "dwi" }));
            }
        }
    }
}

#[test]
fn ranges_expand_lazily_directly_and_through_references() {
    let mut app = App::new(
        crate::dwarf::tests::ranges::fixture(5, gimli::RunTimeEndian::Little),
        "ranges".into(),
        false,
    )
    .unwrap();
    key(&mut app, KeyCode::Right);
    key(&mut app, KeyCode::Right);
    key(&mut app, KeyCode::Tab);
    let list = app.attributes.current().unwrap();
    assert!(app.attributes.nodes[list].expandable);
    assert!(app.attributes.nodes[list].children.is_none());
    key(&mut app, KeyCode::Enter);
    assert_eq!(
        app.attributes.nodes[list].children.as_ref().unwrap().len(),
        2
    );
    let count = app.attributes.nodes.len();
    key(&mut app, KeyCode::Left);
    assert_eq!(app.attributes.visible.len(), 1);
    key(&mut app, KeyCode::Right);
    assert_eq!(app.attributes.nodes.len(), count);
    key(&mut app, KeyCode::Char('d'));
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(140, 24)).unwrap();
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
    assert!(text.contains("[0x1010, 0x1020)"));
    assert!(!text.contains("[raw: ]"));
    key(&mut app, KeyCode::Tab);
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Tab);
    key(&mut app, KeyCode::Right);
    key(&mut app, KeyCode::Right);
    let nested = app.attributes.current().unwrap();
    assert!(app.attributes.nodes[nested].value.range_list.is_some());
    key(&mut app, KeyCode::Enter);
    assert_eq!(
        app.attributes.nodes[nested]
            .children
            .as_ref()
            .unwrap()
            .len(),
        2
    );
    key(&mut app, KeyCode::Tab);
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Tab);
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.message, "Range list is empty");
    assert_eq!(app.attributes.visible.len(), 1);
}

#[test]
fn imported_unit_links_jump_with_history_without_nesting() {
    let mut app = App::new(
        crate::dwarf::tests::imports_fixture(5),
        "imports".into(),
        false,
    )
    .unwrap();
    assert_eq!(app.dies.nodes.len(), 2);
    let main = app.dies.roots[0];
    let target = app.dies.roots[1];
    app.expand_die(main).unwrap();
    let imports = app.dies.nodes[main].children.clone().unwrap();
    assert!(!app.dies.nodes[imports[0]].expandable);
    assert!(app.dies.nodes[imports[0]].children.is_none());
    assert!(
        app.dies.nodes[imports[0]]
            .value
            .import_label
            .as_ref()
            .unwrap()
            .ends_with("partial_unit shared")
    );
    app.dies.select_node(imports[0]);
    app.update_attributes().unwrap();
    let count = app.dies.nodes.len();
    key(&mut app, KeyCode::Right);
    key(&mut app, KeyCode::Char(' '));
    assert_eq!(app.dies.nodes.len(), count);
    assert_eq!(app.dies.current(), Some(imports[0]));
    assert!(app.back.is_empty());
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(140, 30)).unwrap();
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
    assert!(text.contains("imported_unit → [info:"));
    assert!(!text.contains("↩ cycle"));

    key(&mut app, KeyCode::Enter);
    assert_eq!(app.dies.current(), Some(target));
    assert_eq!(app.dies.nodes.len(), count);
    control(&mut app, 'o');
    assert_eq!(app.dies.current(), Some(imports[0]));
    control(&mut app, 'y');
    assert_eq!(app.dies.current(), Some(target));

    // A cyclic import is a normal jump back to the existing unit, not a new
    // occurrence in the tree. Both directions retain their navigation context.
    key(&mut app, KeyCode::Right);
    let children = app.dies.nodes[target].children.clone().unwrap();
    app.dies.select_node(children[1]);
    app.update_attributes().unwrap();
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.dies.current(), Some(main));
    control(&mut app, 'o');
    assert_eq!(app.dies.current(), Some(children[1]));
    app.dies.select_node(imports[1]);
    app.update_attributes().unwrap();
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.dies.current(), Some(target));
    control(&mut app, 'o');
    assert_eq!(app.dies.current(), Some(imports[1]));
    assert_eq!(app.dies.nodes.len(), count + children.len());
    for index in imports {
        assert!(app.dies.nodes[index].children.is_none());
    }
}

#[test]
fn broken_and_self_imports_remain_browsable() {
    let mut app = App::new(
        crate::dwarf::tests::imports_fixture(4),
        "imports".into(),
        false,
    )
    .unwrap();
    let root = app.dies.roots[0];
    app.expand_die(root).unwrap();
    let imports = app.dies.nodes[root].children.clone().unwrap();
    app.dies.select_node(imports[2]);
    app.update_attributes().unwrap();
    assert!(app.attributes.nodes[0].value.value.contains("unresolved"));
    assert!(
        app.dies.nodes[imports[2]]
            .value
            .import_label
            .as_ref()
            .unwrap()
            .contains("unresolved")
    );
    assert!(
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .is_err()
    );
    assert_eq!(app.dies.current(), Some(imports[2]));
    assert!(app.back.is_empty());
    let count = app.dies.nodes.len();
    app.dies.select_node(imports[3]);
    app.update_attributes().unwrap();
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.dies.current(), Some(imports[3]));
    assert_eq!(app.dies.nodes.len(), count);
    assert!(app.dies.nodes[imports[3]].children.is_none());
    control(&mut app, 'o');
    assert_eq!(app.dies.current(), Some(imports[3]));
}
