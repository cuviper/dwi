use anyhow::{Context, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::{
    dwarf::{Attribute, Database, Die, DieId, DieSection},
    tree::Tree,
};

mod goto;
pub use goto::GotoDialog;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Dies,
    Attributes,
}

struct Position {
    die_node: usize,
    expanded: Vec<usize>,
    scroll: usize,
    horizontal: u16,
    attributes: Tree<Attribute>,
    pane: Pane,
}

pub struct App {
    pub database: Database,
    pub file: String,
    pub dies: Tree<Die>,
    pub attributes: Tree<Attribute>,
    pub pane: Pane,
    pub stacked: bool,
    pub fullscreen: bool,
    pub details: bool,
    pub help: bool,
    pub goto_dialog: Option<GotoDialog>,
    pub message: String,
    back: Vec<Position>,
    forward: Vec<Position>,
}

impl App {
    pub fn new(mut database: Database, file: String, stacked: bool) -> Result<Self> {
        let roots = database
            .roots
            .clone()
            .into_iter()
            .map(|id| {
                database.die(id).map(|die| {
                    let children = die.has_children;
                    (die, children)
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let message = database.supplementary_error.clone().unwrap_or_else(|| {
            "? help · Tab switch pane · l expand · Enter follow reference".into()
        });
        let mut app = Self {
            database,
            file,
            dies: Tree::new(roots),
            attributes: Tree::new([]),
            pane: Pane::Dies,
            stacked,
            fullscreen: false,
            details: false,
            help: false,
            goto_dialog: None,
            message,
            back: Vec::new(),
            forward: Vec::new(),
        };
        app.update_attributes()?;
        Ok(app)
    }

    pub fn selected_die(&self) -> Option<DieId> {
        self.dies
            .current()
            .map(|index| self.dies.nodes[index].value.id)
    }

    fn update_attributes(&mut self) -> Result<()> {
        // Clear first so a malformed DIE cannot leave the previous DIE's attributes visible.
        self.attributes = Tree::new([]);
        if let Some(id) = self.selected_die() {
            self.attributes = Tree::new(self.database.attributes(id)?.into_iter().map(|attr| {
                let expandable = attr.has_list()
                    || (attr.target.is_some_and(|target| target != id) && !attr.sibling);
                (attr, expandable)
            }));
        }
        Ok(())
    }

    fn expand_die(&mut self, index: usize) -> Result<()> {
        if !self.dies.nodes[index].expandable {
            return Ok(());
        }
        if self.dies.nodes[index].children.is_none() {
            let id = self.dies.nodes[index].value.id;
            let children = self.database.children(id)?;
            self.dies.set_children(
                index,
                children.into_iter().map(|die| {
                    let expandable = die.has_children;
                    (die, expandable)
                }),
            );
        }
        self.dies.nodes[index].expanded = true;
        self.dies.rebuild();
        Ok(())
    }

    fn expand_attribute(&mut self, index: usize) -> Result<()> {
        let node = &self.attributes.nodes[index];
        if !node.expandable {
            return Ok(());
        }
        if node.children.is_none() {
            if node.value.has_list() {
                let (entries, empty_message) = if let Some(list) = node.value.location_list {
                    (
                        self.database.locations(node.value.owner, list)?,
                        "Location list is empty",
                    )
                } else {
                    let list = node
                        .value
                        .range_list
                        .context("attribute has no range list")?;
                    (
                        self.database.ranges(node.value.owner, list)?,
                        "Range list is empty",
                    )
                };
                if entries.is_empty() {
                    self.message = empty_message.into();
                }
                self.attributes
                    .set_children(index, entries.into_iter().map(|entry| (entry, false)));
            } else {
                let target = node
                    .value
                    .target
                    .context("attribute has no reference target")?;
                let mut ancestors = vec![target];
                let mut current = Some(index);
                while let Some(index) = current {
                    ancestors.push(self.attributes.nodes[index].value.owner);
                    current = self.attributes.nodes[index].parent;
                }
                let attributes = self.database.attributes(target)?;
                self.attributes.set_children(
                    index,
                    attributes.into_iter().map(|attr| {
                        let expandable = attr.has_list()
                            || (attr
                                .target
                                .is_some_and(|target| !ancestors.contains(&target))
                                && !attr.sibling);
                        (attr, expandable)
                    }),
                );
            }
        }
        self.attributes.nodes[index].expanded = true;
        self.attributes.rebuild();
        Ok(())
    }

    fn expand(&mut self, toggle: bool) -> Result<()> {
        match self.pane {
            Pane::Dies => {
                if let Some(index) = self.dies.current() {
                    if self.dies.nodes[index].expanded {
                        if toggle {
                            self.dies.collapse_or_parent();
                        } else if let Some(child) = self.dies.nodes[index]
                            .children
                            .as_ref()
                            .and_then(|v| v.first())
                            .copied()
                        {
                            self.dies.select_node(child);
                        }
                    } else {
                        self.expand_die(index)?;
                    }
                }
            }
            Pane::Attributes => {
                if let Some(index) = self.attributes.current() {
                    if self.attributes.nodes[index].expanded {
                        if toggle {
                            self.attributes.collapse_or_parent();
                        } else if let Some(child) = self.attributes.nodes[index]
                            .children
                            .as_ref()
                            .and_then(|v| v.first())
                            .copied()
                        {
                            self.attributes.select_node(child);
                        }
                    } else {
                        self.expand_attribute(index)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn position(&self) -> Position {
        Position {
            die_node: self.dies.current().unwrap_or(0),
            expanded: self
                .dies
                .nodes
                .iter()
                .enumerate()
                .filter_map(|(i, node)| node.expanded.then_some(i))
                .collect(),
            scroll: self.dies.scroll,
            horizontal: self.dies.horizontal,
            attributes: self.attributes.clone(),
            pane: self.pane,
        }
    }

    fn restore(&mut self, position: Position) {
        for node in &mut self.dies.nodes {
            node.expanded = false;
        }
        for index in position.expanded {
            self.dies.nodes[index].expanded = true;
        }
        self.dies.rebuild();
        self.dies.select_node(position.die_node);
        self.dies.scroll = position.scroll;
        self.dies.horizontal = position.horizontal;
        self.attributes = position.attributes;
        self.pane = position.pane;
    }

    pub fn jump(&mut self, target: DieId) -> Result<()> {
        let path = self.database.path(target)?;
        let previous = self.position();
        let result = (|| -> Result<()> {
            let mut index = *self
                .dies
                .roots
                .iter()
                .find(|index| self.dies.nodes[**index].value.id == path[0])
                .context("destination unit is not in the tree")?;
            for id in path.iter().skip(1) {
                self.expand_die(index)?;
                index = *self.dies.nodes[index]
                    .children
                    .as_ref()
                    .context("missing destination children")?
                    .iter()
                    .find(|index| self.dies.nodes[**index].value.id == *id)
                    .context("destination DIE is not in the tree")?;
            }
            self.dies.select_node(index);
            self.update_attributes()?;
            self.pane = Pane::Dies;
            Ok(())
        })();
        if let Err(error) = result {
            self.restore(previous);
            return Err(error);
        }
        if self.back.len() == 100 {
            self.back.remove(0);
        }
        self.back.push(previous);
        self.forward.clear();
        self.message = format!("Jumped to {} · Ctrl-o back", self.database.location(target));
        Ok(())
    }

    fn history(&mut self, forward: bool) {
        let previous = if forward {
            self.forward.pop()
        } else {
            self.back.pop()
        };
        if let Some(previous) = previous {
            let current = self.position();
            if forward {
                self.back.push(current);
            } else {
                self.forward.push(current);
            }
            self.restore(previous);
            self.message = "Navigation restored".into();
        } else {
            self.message = "No more navigation history".into();
        }
    }

    fn move_by(&mut self, amount: isize) {
        match self.pane {
            Pane::Dies => self.dies.move_by(amount),
            Pane::Attributes => self.attributes.move_by(amount),
        }
    }

    fn handle_goto_key(&mut self, key: KeyEvent) {
        let Some(mut dialog) = self.goto_dialog.take() else {
            return;
        };
        match key.code {
            KeyCode::Esc => return,
            KeyCode::Enter => {
                let result = dialog
                    .destination()
                    .and_then(|(section, offset)| self.database.find_die(section, offset))
                    .and_then(|target| self.jump(target));
                match result {
                    Ok(()) => return,
                    Err(error) => dialog.error = Some(format!("{error:#}")),
                }
            }
            _ => dialog.edit(key),
        }
        self.goto_dialog = Some(dialog);
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Result<bool> {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return Ok(true);
        }
        if self.goto_dialog.is_some() {
            self.handle_goto_key(key);
            return Ok(false);
        }
        if self.help {
            if matches!(
                key.code,
                KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q')
            ) {
                self.help = false;
            }
            return Ok(false);
        }
        if key.modifiers.contains(KeyModifiers::ALT) {
            match key.code {
                KeyCode::Left => self.history(false),
                KeyCode::Right => self.history(true),
                _ => {}
            }
            return Ok(false);
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('g') => {
                    let section = self
                        .selected_die()
                        .map(|id| self.database.section(id))
                        .unwrap_or(DieSection::Info);
                    self.goto_dialog = Some(GotoDialog::new(section));
                }
                KeyCode::Char('o') => self.history(false),
                // Ctrl-i aliases Tab on many terminals; Ctrl-y is unambiguous.
                KeyCode::Char('y') => self.history(true),
                _ => {}
            }
            return Ok(false);
        }
        let before = self.selected_die();
        match key.code {
            KeyCode::Char('q') => return Ok(true),
            KeyCode::Char('?') => self.help = true,
            KeyCode::Tab | KeyCode::BackTab => {
                self.pane = match self.pane {
                    Pane::Dies => Pane::Attributes,
                    Pane::Attributes => Pane::Dies,
                }
            }
            KeyCode::Char('j') | KeyCode::Down => self.move_by(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_by(-1),
            KeyCode::Char('g') | KeyCode::Home => self.move_by(isize::MIN),
            KeyCode::Char('G') | KeyCode::End => self.move_by(isize::MAX),
            KeyCode::PageDown | KeyCode::PageUp => {
                let height = match self.pane {
                    Pane::Dies => self.dies.height,
                    Pane::Attributes => self.attributes.height,
                };
                self.move_by(height as isize * if key.code == KeyCode::PageUp { -1 } else { 1 });
            }
            KeyCode::Char('h') | KeyCode::Left => match self.pane {
                Pane::Dies => self.dies.collapse_or_parent(),
                Pane::Attributes => self.attributes.collapse_or_parent(),
            },
            KeyCode::Char('l') | KeyCode::Right => self.expand(false)?,
            KeyCode::Char(' ') => self.expand(true)?,
            KeyCode::Enter if self.pane == Pane::Attributes => {
                if self
                    .attributes
                    .current()
                    .is_some_and(|index| self.attributes.nodes[index].value.has_list())
                {
                    self.expand(true)?;
                    return Ok(false);
                }
                if let Some(target) = self
                    .attributes
                    .current()
                    .and_then(|index| self.attributes.nodes[index].value.target)
                {
                    self.jump(target)?;
                    return Ok(false);
                }
                self.message = "This attribute has no resolved DIE reference".into();
            }
            KeyCode::Enter => {
                if let Some(index) = self.dies.current()
                    && self.dies.nodes[index].value.import_label.is_some()
                {
                    let target = self
                        .database
                        .imported_unit(self.dies.nodes[index].value.id)?;
                    self.jump(target.id)?;
                    return Ok(false);
                }
                self.expand(true)?;
            }
            KeyCode::Char('d') => self.details = !self.details,
            KeyCode::Char('s') => self.stacked = !self.stacked,
            KeyCode::Char('f') => self.fullscreen = !self.fullscreen,
            KeyCode::Char('H') | KeyCode::Char('L') => {
                let offset = match self.pane {
                    Pane::Dies => &mut self.dies.horizontal,
                    Pane::Attributes => &mut self.attributes.horizontal,
                };
                *offset = if key.code == KeyCode::Char('H') {
                    offset.saturating_sub(8)
                } else {
                    offset.saturating_add(8)
                };
            }
            _ => {}
        }
        if before != self.selected_die() {
            self.update_attributes()?;
        }
        Ok(false)
    }
}

#[cfg(test)]
mod tests;
