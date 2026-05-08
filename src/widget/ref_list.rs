use std::{collections::HashSet, rc::Rc};

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, Padding, StatefulWidget},
};
use semver::Version;
use tui_tree_widget::{Tree, TreeItem, TreeState};

use crate::{app::AppContext, color::ColorTheme, git::Ref};

const TREE_BRANCH_ROOT_IDENT: &str = "__branches__";
const TREE_REMOTE_ROOT_IDENT: &str = "__remotes__";
const TREE_TAG_ROOT_IDENT: &str = "__tags__";
const TREE_STASH_ROOT_IDENT: &str = "__stashes__";
const ADD_REMOTE_IDENT: &str = "__add_remote__";

const TREE_BRANCH_ROOT_TEXT: &str = "◈ Local";
const TREE_REMOTE_ROOT_TEXT: &str = "⇄ Remotes";
const TREE_TAG_ROOT_TEXT: &str = "# Tags";
const TREE_STASH_ROOT_TEXT: &str = "≡ Stashes";

#[derive(Debug, Default)]
pub struct RefListState {
    tree_state: TreeState<String>,
    /// Identifiers that have children and can be toggled open/closed.
    nodes_with_children: HashSet<Vec<String>>,
}

impl RefListState {
    pub fn new() -> Self {
        let mut tree_state = TreeState::default();
        tree_state.select(vec![TREE_BRANCH_ROOT_IDENT.into()]);
        tree_state.open(vec![TREE_BRANCH_ROOT_IDENT.into()]);
        Self {
            tree_state,
            nodes_with_children: HashSet::new(),
        }
    }
}

impl RefListState {
    pub fn select_next(&mut self) {
        self.tree_state.key_down();
    }

    pub fn select_prev(&mut self) {
        self.tree_state.key_up();
    }

    pub fn scroll_down(&mut self, lines: usize) {
        self.tree_state.scroll_down(lines);
    }

    pub fn scroll_up(&mut self, lines: usize) {
        self.tree_state.scroll_up(lines);
    }

    pub fn select_first(&mut self) {
        self.tree_state.select_first();
    }

    pub fn select_last(&mut self) {
        self.tree_state.select_last();
    }

    pub fn open_node(&mut self) {
        self.tree_state.key_right();
    }

    pub fn close_node(&mut self) {
        self.tree_state.key_left();
    }

    pub fn toggle_selected(&mut self) {
        self.tree_state.toggle_selected();
    }

    pub fn selected_ref_name(&self) -> Option<String> {
        self.tree_state.selected().last().cloned()
    }

    pub fn selected_is_node(&self) -> bool {
        let selected = self.tree_state.selected();
        !selected.is_empty() && self.nodes_with_children.contains(selected)
    }

    pub fn selected_branch(&self) -> Option<String> {
        let selected = self.tree_state.selected();
        if selected.len() > 1
            && (selected[0] == TREE_BRANCH_ROOT_IDENT || selected[0] == TREE_REMOTE_ROOT_IDENT)
        {
            selected.last().cloned()
        } else {
            None
        }
    }

    pub fn selected_tag(&self) -> Option<String> {
        let selected = self.tree_state.selected();
        if selected.len() > 1 && selected[0] == TREE_TAG_ROOT_IDENT {
            selected.last().cloned()
        } else {
            None
        }
    }

    /// Returns the remote name when the selected item is a top-level remote
    /// node (e.g. `origin`) directly under the remotes root, but not the
    /// special `[+ Add remote]` entry.
    pub fn selected_remote_name(&self) -> Option<String> {
        let selected = self.tree_state.selected();
        if selected.len() == 2
            && selected[0] == TREE_REMOTE_ROOT_IDENT
            && selected.last().map(String::as_str) != Some(ADD_REMOTE_IDENT)
            && self.nodes_with_children.contains(selected)
        {
            selected.last().cloned()
        } else {
            None
        }
    }

    /// Returns `true` when the selected item is the `[+ Add remote]` leaf.
    pub fn selected_is_add_remote_item(&self) -> bool {
        let selected = self.tree_state.selected();
        selected.len() == 2
            && selected[0] == TREE_REMOTE_ROOT_IDENT
            && selected.last().map(String::as_str) == Some(ADD_REMOTE_IDENT)
    }

    pub fn current_tree_status(&self) -> (Vec<String>, Vec<Vec<String>>) {
        let selected = self.tree_state.selected().into();
        let opened = self.tree_state.opened().iter().cloned().collect();
        (selected, opened)
    }

    pub fn reset_tree_status(&mut self, selected: Vec<String>, opened: Vec<Vec<String>>) {
        self.tree_state.select(selected);
        for node in opened {
            self.tree_state.open(node);
        }
    }

    pub fn handle_click(&mut self, col: u16, row: u16) -> bool {
        let position = ratatui::layout::Position::new(col, row);
        self.tree_state.click_at(position)
    }

    pub fn handle_mouse_move(&mut self, col: u16, row: u16) -> bool {
        let position = ratatui::layout::Position::new(col, row);
        if let Some(identifier) = self.tree_state.rendered_at(position) {
            let identifier = identifier.to_vec();
            if self.tree_state.selected() != identifier.as_slice() {
                return self.tree_state.select(identifier);
            }
        }
        false
    }
}

pub struct RefList {
    items: Vec<TreeItem<'static, String>>,
    ctx: Rc<AppContext>,
}

impl RefList {
    pub fn new(refs: &[Ref], ctx: Rc<AppContext>) -> RefList {
        let items = build_ref_tree_items(refs, &ctx);
        RefList { items, ctx }
    }
}

fn collect_node_identifiers(
    items: &[TreeItem<'_, String>],
    prefix: &mut Vec<String>,
    out: &mut HashSet<Vec<String>>,
) {
    for item in items {
        let mut path = prefix.clone();
        path.push(item.identifier().clone());
        if !item.children().is_empty() {
            out.insert(path.clone());
            collect_node_identifiers(item.children(), &mut path, out);
        }
    }
}

impl StatefulWidget for RefList {
    type State = RefListState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        state.nodes_with_children.clear();
        collect_node_identifiers(&self.items, &mut Vec::new(), &mut state.nodes_with_children);

        let tree = Tree::new(&self.items)
            .unwrap()
            .node_closed_symbol("\u{25b8} ") // ▸
            .node_open_symbol("\u{25be} ") // ▾
            .node_no_children_symbol("  ")
            .highlight_style(
                Style::default()
                    .bg(self.ctx.color_theme.ref_selected_bg)
                    .fg(self.ctx.color_theme.ref_selected_fg),
            )
            .block(
                Block::default()
                    .borders(Borders::LEFT)
                    .style(Style::default().fg(self.ctx.color_theme.divider_fg))
                    .padding(Padding::horizontal(1)),
            );
        tree.render(area, buf, &mut state.tree_state);
    }
}

fn build_ref_tree_items(refs: &[Ref], ctx: &AppContext) -> Vec<TreeItem<'static, String>> {
    let color_theme = &ctx.color_theme;
    let branch_color_map = &ctx.branch_color_map;

    let mut branch_refs = Vec::new();
    let mut remote_refs = Vec::new();
    let mut tag_refs = Vec::new();
    let mut stash_refs = Vec::new();

    for r in refs {
        match r {
            Ref::Tag { name, .. } => tag_refs.push(name.into()),
            Ref::Branch { name, .. } => branch_refs.push(name.into()),
            Ref::RemoteBranch { name, .. } => remote_refs.push(name.into()),
            Ref::Stash { name, message, .. } => stash_refs.push((name.into(), message.into())),
        }
    }

    let mut branch_nodes = refs_to_ref_tree_nodes(branch_refs);
    let mut remote_nodes = refs_to_ref_tree_nodes(remote_refs);
    let mut tag_nodes = refs_to_ref_tree_nodes(tag_refs);
    let mut stash_nodes = refs_to_stash_ref_tree_nodes(stash_refs);

    sort_branch_tree_nodes(&mut branch_nodes);
    sort_branch_tree_nodes(&mut remote_nodes);
    sort_tag_tree_nodes(&mut tag_nodes);
    sort_stash_tree_nodes(&mut stash_nodes);

    let branch_items = branch_tree_nodes_to_tree_items(branch_nodes, branch_color_map, color_theme);
    let mut remote_items =
        remote_tree_nodes_to_tree_items(remote_nodes, branch_color_map, color_theme, 0);
    remote_items.push(
        TreeItem::new_leaf(
            ADD_REMOTE_IDENT.to_string(),
            ratatui::text::Line::from(vec![ratatui::text::Span::styled(
                "+ Add remote",
                ratatui::style::Style::default()
                    .fg(color_theme.status_info_fg)
                    .add_modifier(ratatui::style::Modifier::BOLD),
            )]),
        ),
    );
    let tag_items = tag_tree_nodes_to_tree_items(tag_nodes, color_theme);
    let stash_items = stash_tree_nodes_to_tree_items(stash_nodes, color_theme);

    vec![
        tree_item(
            TREE_BRANCH_ROOT_IDENT.into(),
            TREE_BRANCH_ROOT_TEXT.into(),
            branch_items,
            color_theme,
        ),
        tree_item(
            TREE_REMOTE_ROOT_IDENT.into(),
            TREE_REMOTE_ROOT_TEXT.into(),
            remote_items,
            color_theme,
        ),
        tree_item(
            TREE_TAG_ROOT_IDENT.into(),
            TREE_TAG_ROOT_TEXT.into(),
            tag_items,
            color_theme,
        ),
        tree_item(
            TREE_STASH_ROOT_IDENT.into(),
            TREE_STASH_ROOT_TEXT.into(),
            stash_items,
            color_theme,
        ),
    ]
}

struct RefTreeNode {
    identifier: String,
    name: String,
    children: Vec<RefTreeNode>,
}

fn refs_to_stash_ref_tree_nodes(ref_name_messages: Vec<(String, String)>) -> Vec<RefTreeNode> {
    let mut nodes: Vec<RefTreeNode> = Vec::new();
    for (name, message) in ref_name_messages {
        let node = RefTreeNode {
            identifier: name.clone(),
            name: message.to_string(),
            children: Vec::new(),
        };
        nodes.push(node);
    }
    nodes
}

fn refs_to_ref_tree_nodes(ref_names: Vec<String>) -> Vec<RefTreeNode> {
    let mut nodes: Vec<RefTreeNode> = Vec::new();

    for ref_name in ref_names {
        let mut parts = ref_name.split('/').collect::<Vec<_>>();
        let mut current_nodes = &mut nodes;
        let mut parent_identifier = String::new();

        while !parts.is_empty() {
            let part = parts.remove(0);
            if let Some(index) = current_nodes.iter().position(|n| n.name == part) {
                let node = &mut current_nodes[index];
                current_nodes = &mut node.children;
                parent_identifier.clone_from(&node.identifier);
            } else {
                let identifier = if parent_identifier.is_empty() {
                    part.to_string()
                } else {
                    format!("{parent_identifier}/{part}")
                };
                let node = RefTreeNode {
                    identifier: identifier.clone(),
                    name: part.to_string(),
                    children: Vec::new(),
                };
                current_nodes.push(node);
                current_nodes = current_nodes.last_mut().unwrap().children.as_mut();
                parent_identifier = identifier;
            }
        }
    }

    nodes
}

fn find_first_branch_color(
    node: &RefTreeNode,
    branch_color_map: &rustc_hash::FxHashMap<String, Color>,
) -> Option<Color> {
    if node.children.is_empty() {
        return branch_color_map.get(&node.identifier).copied();
    }
    for child in &node.children {
        if let Some(color) = find_first_branch_color(child, branch_color_map) {
            return Some(color);
        }
    }
    None
}

fn branch_tree_nodes_to_tree_items(
    nodes: Vec<RefTreeNode>,
    branch_color_map: &rustc_hash::FxHashMap<String, Color>,
    color_theme: &ColorTheme,
) -> Vec<TreeItem<'static, String>> {
    let mut items = Vec::new();
    for node in nodes {
        let fg = if node.children.is_empty() {
            branch_color_map
                .get(&node.identifier)
                .copied()
                .unwrap_or(color_theme.list_ref_branch_fg)
        } else {
            // For folders, find the color from the first leaf descendant
            find_first_branch_color(&node, branch_color_map)
                .unwrap_or(color_theme.list_ref_branch_fg)
        };
        if node.children.is_empty() {
            let line = Line::from(vec![
                Span::raw("⎇ ").fg(fg).bold(),
                Span::raw(node.name).fg(fg).bold(),
            ]);
            items.push(tree_item_with_line(node.identifier, line, Vec::new()));
        } else {
            let children =
                branch_tree_nodes_to_tree_items(node.children, branch_color_map, color_theme);
            let line = Line::from(vec![
                Span::raw("⎇ ").fg(fg).bold(),
                Span::raw(node.name).fg(fg).bold(),
            ]);
            items.push(tree_item_with_line(node.identifier, line, children));
        }
    }
    items
}

fn remote_tree_nodes_to_tree_items(
    nodes: Vec<RefTreeNode>,
    branch_color_map: &rustc_hash::FxHashMap<String, Color>,
    color_theme: &ColorTheme,
    depth: usize,
) -> Vec<TreeItem<'static, String>> {
    let mut items = Vec::new();
    for node in nodes {
        if node.children.is_empty() {
            // Feuille = branche distante
            let fg = branch_color_map
                .get(&node.identifier)
                .copied()
                .unwrap_or(color_theme.list_ref_remote_branch_fg);
            let line = Line::from(vec![
                Span::raw("⎇ ").fg(fg).bold(),
                Span::raw(node.name).fg(fg).bold(),
            ]);
            items.push(tree_item_with_line(node.identifier, line, Vec::new()));
        } else {
            let fg = if depth == 0 {
                None
            } else {
                find_first_branch_color(&node, branch_color_map)
            };
            let children = remote_tree_nodes_to_tree_items(
                node.children,
                branch_color_map,
                color_theme,
                depth + 1,
            );
            let line = if depth == 0 {
                // Premier niveau = nom du remote (ex: "origin") → pas de logo
                Line::from(vec![Span::raw(node.name).fg(color_theme.fg)])
            } else {
                // Niveaux suivants → logo branche, inherit color from first leaf
                let fg = fg.unwrap_or(color_theme.list_ref_remote_branch_fg);
                Line::from(vec![
                    Span::raw("⎇ ").fg(fg).bold(),
                    Span::raw(node.name).fg(fg).bold(),
                ])
            };
            items.push(tree_item_with_line(node.identifier, line, children));
        }
    }
    items
}

fn tag_tree_nodes_to_tree_items(
    nodes: Vec<RefTreeNode>,
    color_theme: &ColorTheme,
) -> Vec<TreeItem<'static, String>> {
    let mut items = Vec::new();
    for node in nodes {
        if node.children.is_empty() {
            let line = Line::from(vec![
                Span::raw("🏷  ").fg(color_theme.list_ref_tag_fg).bold(),
                Span::raw(node.name).fg(color_theme.list_ref_tag_fg).bold(),
            ]);
            items.push(tree_item_with_line(node.identifier, line, Vec::new()));
        } else {
            let children = tag_tree_nodes_to_tree_items(node.children, color_theme);
            let line = Line::from(vec![
                Span::raw("🏷  ").fg(color_theme.list_ref_tag_fg).bold(),
                Span::raw(node.name).fg(color_theme.list_ref_tag_fg).bold(),
            ]);
            items.push(tree_item_with_line(node.identifier, line, children));
        }
    }
    items
}

fn stash_tree_nodes_to_tree_items(
    nodes: Vec<RefTreeNode>,
    color_theme: &ColorTheme,
) -> Vec<TreeItem<'static, String>> {
    let mut items = Vec::new();
    for node in nodes {
        let line = Line::from(vec![
            Span::raw("⌧ ").fg(color_theme.list_ref_stash_fg).bold(),
            Span::raw(node.name)
                .fg(color_theme.list_ref_stash_fg)
                .bold(),
        ]);
        items.push(tree_item_with_line(node.identifier, line, Vec::new()));
    }
    items
}

fn sort_branch_tree_nodes(nodes: &mut [RefTreeNode]) {
    nodes.sort_by(|a, b| {
        b.children
            .len()
            .cmp(&a.children.len())
            .then(a.name.cmp(&b.name))
    });
    for node in nodes {
        sort_branch_tree_nodes(&mut node.children);
    }
}

fn sort_tag_tree_nodes(nodes: &mut [RefTreeNode]) {
    nodes.sort_by(|a, b| {
        let a_version = parse_semantic_version_tag(&a.name);
        let b_version = parse_semantic_version_tag(&b.name);
        if a_version.is_none() && b_version.is_none() {
            // if both are not semantic versions, sort by name asc
            a.name.cmp(&b.name)
        } else {
            // if both are semantic versions, sort by version desc
            // if only one is a semantic version, it will be sorted first
            b_version.cmp(&a_version)
        }
    });
}

fn sort_stash_tree_nodes(nodes: &mut [RefTreeNode]) {
    nodes.sort_by(|a, b| a.identifier.cmp(&b.identifier));
}

fn parse_semantic_version_tag(tag: &str) -> Option<Version> {
    let tag = tag.trim_start_matches('v');
    Version::parse(tag).ok()
}

fn tree_item(
    identifier: String,
    name: String,
    children: Vec<TreeItem<'static, String>>,
    color_theme: &ColorTheme,
) -> TreeItem<'static, String> {
    TreeItem::new(identifier, name.fg(color_theme.fg), children).unwrap()
}

fn tree_item_with_line(
    identifier: String,
    line: Line<'static>,
    children: Vec<TreeItem<'static, String>>,
) -> TreeItem<'static, String> {
    TreeItem::new(identifier, line, children).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_remote_name_returns_name_for_remote_node() {
        let path: Vec<String> = vec![TREE_REMOTE_ROOT_IDENT.into(), "origin".into()];
        let result = if path.len() == 2 && path[0] == TREE_REMOTE_ROOT_IDENT {
            Some(path[1].clone())
        } else {
            None
        };
        assert_eq!(result, Some("origin".to_string()));
    }

    #[test]
    fn selected_remote_name_returns_none_for_branch_under_remote() {
        let path: Vec<String> = vec![
            TREE_REMOTE_ROOT_IDENT.into(),
            "origin".into(),
            "main".into(),
        ];
        let result = if path.len() == 2 && path[0] == TREE_REMOTE_ROOT_IDENT {
            Some(path[1].clone())
        } else {
            None
        };
        assert_eq!(result, None);
    }
}
