/// Display occurrences have their own IDs, independent of DWARF identity. A DIE
/// can appear through several attribute references with different fold states.
#[derive(Clone)]
pub struct Node<T> {
    pub value: T,
    pub parent: Option<usize>,
    pub depth: usize,
    pub expandable: bool,
    pub expanded: bool,
    pub children: Option<Vec<usize>>,
}

#[derive(Clone)]
pub struct Tree<T> {
    pub nodes: Vec<Node<T>>,
    pub roots: Vec<usize>,
    pub visible: Vec<usize>,
    pub selected: usize,
    pub scroll: usize,
    pub horizontal: u16,
    pub height: usize,
}

impl<T> Tree<T> {
    pub fn new(values: impl IntoIterator<Item = (T, bool)>) -> Self {
        let nodes: Vec<_> = values
            .into_iter()
            .map(|(value, expandable)| Node {
                value,
                parent: None,
                depth: 0,
                expandable,
                expanded: false,
                children: None,
            })
            .collect();
        let roots: Vec<_> = (0..nodes.len()).collect();
        Self {
            nodes,
            visible: roots.clone(),
            roots,
            selected: 0,
            scroll: 0,
            horizontal: 0,
            height: 1,
        }
    }

    pub fn current(&self) -> Option<usize> {
        self.visible.get(self.selected).copied()
    }

    pub fn set_children(&mut self, parent: usize, values: impl IntoIterator<Item = (T, bool)>) {
        let mut children = Vec::new();
        let depth = self.nodes[parent].depth + 1;
        for (value, expandable) in values {
            children.push(self.nodes.len());
            self.nodes.push(Node {
                value,
                parent: Some(parent),
                depth,
                expandable,
                expanded: false,
                children: None,
            });
        }
        self.nodes[parent].expandable = !children.is_empty();
        self.nodes[parent].children = Some(children);
    }

    pub fn rebuild(&mut self) {
        let selected = self.current();
        self.visible.clear();
        let mut pending: Vec<_> = self.roots.iter().rev().copied().collect();
        while let Some(index) = pending.pop() {
            self.visible.push(index);
            let node = &self.nodes[index];
            if node.expanded
                && let Some(children) = &node.children
            {
                pending.extend(children.iter().rev().copied());
            }
        }
        if let Some(index) =
            selected.and_then(|index| self.visible.iter().position(|id| *id == index))
        {
            self.selected = index;
        }
        self.selected = self.selected.min(self.visible.len().saturating_sub(1));
    }

    pub fn select_node(&mut self, index: usize) {
        if let Some(position) = self.visible.iter().position(|id| *id == index) {
            self.selected = position;
        }
    }

    pub fn move_by(&mut self, amount: isize) {
        self.selected = self
            .selected
            .saturating_add_signed(amount)
            .min(self.visible.len().saturating_sub(1));
    }

    pub fn collapse_or_parent(&mut self) {
        if let Some(index) = self.current() {
            if self.nodes[index].expanded {
                self.nodes[index].expanded = false;
                self.rebuild();
            } else if let Some(parent) = self.nodes[index].parent {
                self.select_node(parent);
            }
        }
    }

    pub fn adjust_scroll(&mut self, height: usize) {
        self.height = height.max(1);
        // Keep one context row above and below the cursor when the pane fits
        // both margins. At the ends of the tree, use the available context.
        let margin = 1.min((self.height - 1) / 2);
        let min_scroll = self
            .selected
            .saturating_add(margin + 1)
            .saturating_sub(self.height);
        let max_scroll = self.selected.saturating_sub(margin);
        self.scroll = self
            .scroll
            .clamp(min_scroll, max_scroll)
            .min(self.visible.len().saturating_sub(self.height));
    }
}
