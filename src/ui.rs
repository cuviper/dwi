use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap,
    },
};

use crate::{
    app::{App, GotoDialog, Pane},
    text::escape_controls,
    tree::{Node, Tree},
};

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    if area.width < 24 || area.height < 6 {
        frame.render_widget(
            Paragraph::new(if app.goto_dialog.is_some() {
                "Terminal too small\nResize or Esc to cancel"
            } else {
                "Terminal too small\nResize or press q to quit"
            }),
            area,
        );
        return;
    }
    let mut heading = vec![Line::from(vec![
        Span::styled(
            " dwi ",
            Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ),
        Span::raw(escape_controls(&app.file)),
    ])];
    if let Some(debug_file) = &app.database.debug_file {
        heading.push(Line::from(vec![
            Span::styled(" debug: ", Style::new().fg(Color::DarkGray)),
            Span::raw(escape_controls(&debug_file.display().to_string())),
        ]));
    }
    if let Some(supplementary_file) = &app.database.supplementary_file {
        heading.push(Line::from(vec![
            Span::styled(" supplementary: ", Style::new().fg(Color::DarkGray)),
            Span::raw(escape_controls(&supplementary_file.display().to_string())),
        ]));
    }
    let sections = Layout::vertical([
        Constraint::Length(heading.len() as u16),
        Constraint::Min(2),
        Constraint::Length(2),
    ])
    .split(area);
    frame.render_widget(Paragraph::new(heading), sections[0]);
    let panes = Layout::default()
        .direction(if app.stacked || sections[1].width < 90 {
            Direction::Vertical
        } else {
            Direction::Horizontal
        })
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(sections[1]);
    let die_area = if app.fullscreen {
        sections[1]
    } else {
        panes[0]
    };
    let attr_area = if app.fullscreen {
        sections[1]
    } else {
        panes[1]
    };
    let details = app.details;
    if !app.fullscreen || app.pane == Pane::Dies {
        render_tree(
            frame,
            die_area,
            &format!("DIEs · {} units", app.database.roots.len()),
            app.pane == Pane::Dies,
            true,
            &mut app.dies,
            |node| {
                let die = &node.value;
                let mut spans = Vec::new();
                if details {
                    spans.push(Span::styled(
                        format!("[{}] ", die.location),
                        Style::new().fg(Color::DarkGray),
                    ));
                }
                spans.push(Span::styled(die.tag.clone(), Style::new().fg(Color::Blue)));
                if !die.name.is_empty() {
                    spans.push(Span::raw(format!("  {}", escape_controls(&die.name))));
                }
                if let Some(label) = &die.import_label {
                    spans.push(Span::styled(
                        format!(" → {}", escape_controls(label)),
                        Style::new().fg(Color::Blue),
                    ));
                }
                Line::from(spans)
            },
        );
    }
    if !app.fullscreen || app.pane == Pane::Attributes {
        let title = app
            .selected_die()
            .map(|id| format!("Attributes · {}", app.database.location(id)))
            .unwrap_or_else(|| "Attributes".into());
        render_tree(
            frame,
            attr_area,
            &title,
            app.pane == Pane::Attributes,
            false,
            &mut app.attributes,
            |node| {
                let attr = &node.value;
                let mut spans = vec![Span::styled(
                    format!("{}  ", attr.name),
                    Style::new().fg(Color::Yellow),
                )];
                if details && !attr.form.is_empty() {
                    spans.push(Span::styled(
                        format!("({}) ", attr.form),
                        Style::new().fg(Color::DarkGray),
                    ));
                }
                if attr.is_expression {
                    spans.extend(expression_spans(&attr.value));
                } else {
                    spans.push(Span::styled(
                        escape_controls(&attr.value),
                        Style::new().fg(if attr.target.is_some() {
                            Color::Blue
                        } else {
                            Color::Reset
                        }),
                    ));
                }
                if attr.target.is_some()
                    && !node.expandable
                    && node.children.is_none()
                    && !attr.sibling
                {
                    spans.push(Span::styled(" ↩ cycle", Style::new().fg(Color::DarkGray)));
                }
                if details && !attr.raw.is_empty() {
                    spans.push(Span::styled(
                        format!("  [raw: {}]", escape_controls(&attr.raw)),
                        Style::new().fg(Color::DarkGray),
                    ));
                }
                Line::from(spans)
            },
        );
    }
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(escape_controls(&app.message)),
            Line::styled(
                " Tab pane  h/l fold  Enter follow  Ctrl-g goto  d detail  ? help  q quit",
                Style::new().fg(Color::DarkGray),
            ),
        ]),
        sections[2],
    );
    if app.help {
        let width = area.width.min(76);
        let height = area.height.min(24);
        let popup = Rect::new(
            (area.width - width) / 2,
            (area.height - height) / 2,
            width,
            height,
        );
        frame.render_widget(Clear, popup);
        frame.render_widget(
            Paragraph::new(
                "j/k or ↑/↓       Move selection\n\
             h or ←           Collapse, or select parent\n\
             l or →           Expand, or select first child\n\
             Space            Toggle expansion\n\
             Enter            Follow import/reference or toggle DIE/list\n\
             Tab / Shift-Tab  Switch pane\n\
             Home/g, End/G    First / last visible row\n\
             PgUp / PgDn      Page through active pane\n\
             H / L            Scroll horizontally\n\
             Ctrl-g           Go to DIE (hex section offset)\n\
             Ctrl-o / Alt-←   Back through jump history\n\
             Ctrl-y / Alt-→   Forward\n\
             d                Show offsets, forms, and raw values\n\
             s                Switch side-by-side / stacked layout\n\
             f                Focus active pane full-screen\n\
             q / Ctrl-c       Quit (q closes this help first)\n\n\
             References, location and range lists expand on demand. Cycles can be\n\
             followed with Enter but cannot expand recursively.\n\n\
             Esc / ? / q closes help",
            )
            .block(Block::bordered().title(" Help "))
            .wrap(Wrap { trim: false }),
            popup,
        );
    }
    if let Some(dialog) = &app.goto_dialog {
        render_goto(frame, dialog);
    }
}

fn render_goto(frame: &mut Frame, dialog: &GotoDialog) {
    let area = frame.area();
    let width = area.width.min(64);
    let height = area.height.min(8);
    let popup = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    let block = Block::bordered()
        .title(" Go to DIE ")
        .border_style(Style::new().fg(Color::Cyan));
    let inner = block.inner(popup);
    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .split(inner);
    frame.render_widget(Paragraph::new("Section offset (hex):"), rows[0]);
    let input = rows[1];
    if input.width > 0 && input.height > 0 {
        let start = dialog.cursor.saturating_sub(input.width as usize - 1);
        frame.render_widget(Paragraph::new(&dialog.input[start..]), input);
        frame.set_cursor_position((input.x + (dialog.cursor - start) as u16, input.y));
    }
    let (message, color) = match &dialog.error {
        Some(error) => (error.as_str(), Color::Red),
        None => (
            "info:0x1234 or sup:info:0x1234 · Ctrl-u clear",
            Color::DarkGray,
        ),
    };
    frame.render_widget(
        Paragraph::new(message)
            .style(Style::new().fg(color))
            .wrap(Wrap { trim: false }),
        rows[2],
    );
    frame.render_widget(Paragraph::new("Enter jump · Esc cancel"), rows[3]);
}

fn render_tree<T>(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    focused: bool,
    show_unfocused_cursor: bool,
    tree: &mut Tree<T>,
    row: impl Fn(&Node<T>) -> Line<'static>,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {title} "))
        .border_style(Style::new().fg(if focused {
            Color::Cyan
        } else {
            Color::DarkGray
        }));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    tree.adjust_scroll(inner.height as usize);
    let lines: Vec<_> = tree
        .visible
        .iter()
        .enumerate()
        .skip(tree.scroll)
        .take(inner.height as usize)
        .map(|(position, index)| {
            let node = &tree.nodes[*index];
            let selected = position == tree.selected && (focused || show_unfocused_cursor);
            let mut line = prefix(node, selected, focused);
            line.spans.extend(row(node).spans);
            line
        })
        .collect();
    if lines.is_empty() {
        frame.render_widget(
            Paragraph::new("(no attributes)").style(Style::new().fg(Color::DarkGray)),
            inner,
        );
    } else {
        frame.render_widget(Paragraph::new(lines).scroll((0, tree.horizontal)), inner);
    }
    let viewport = inner.height as usize;
    if !inner.is_empty() && tree.visible.len() > viewport {
        // Scrollbar positions represent viewport starts, not selected rows.
        let mut state = ScrollbarState::new(tree.visible.len() - viewport + 1)
            .position(tree.scroll)
            .viewport_content_length(viewport);
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .track_symbol(Some("░"))
            .thumb_symbol("█")
            .track_style(Style::new().fg(Color::DarkGray))
            .thumb_style(Style::new().fg(if focused { Color::Cyan } else { Color::Gray }));
        let track = Rect::new(inner.right(), inner.y, 1, inner.height);
        frame.render_stateful_widget(scrollbar, track, &mut state);
    }
}

fn prefix<T>(node: &Node<T>, selected: bool, focused: bool) -> Line<'static> {
    // Cap visual indentation; deep DWARF and reference chains remain navigable.
    Line::from(vec![
        Span::raw("  ".repeat(node.depth.min(40))),
        Span::styled(
            if selected { "❯ " } else { "  " },
            Style::new().fg(if focused {
                Color::Cyan
            } else {
                Color::DarkGray
            }),
        ),
        Span::raw(if node.expanded && node.expandable {
            "▾ "
        } else if node.expandable {
            "▸ "
        } else {
            "  "
        }),
    ])
}

fn expression_spans(mut text: &str) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    // Only expression values pass through here; ordinary strings may contain
    // DW_OP_ literally. Nested expressions use the same mnemonic syntax.
    while let Some(start) = text.find("DW_OP_") {
        if start > 0 {
            spans.push(Span::raw(escape_controls(&text[..start])));
        }
        text = &text[start + "DW_OP_".len()..];
        let end = text
            .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
            .unwrap_or(text.len());
        spans.push(Span::styled(
            text[..end].to_owned(),
            Style::new().fg(Color::Magenta),
        ));
        text = &text[end..];
    }
    if !text.is_empty() {
        spans.push(Span::raw(escape_controls(text)));
    }
    spans
}
