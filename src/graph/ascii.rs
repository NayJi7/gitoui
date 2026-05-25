use ratatui::style::Color;

use crate::{
    git::{CommitHash, CommitType},
    graph::{
        calc::{Edge, EdgeType, Graph},
        image::GraphStyle,
    },
};

/// A single character cell in the ASCII graph output, with its display color.
pub struct AsciiGraphCell {
    pub ch: char,
    pub color: Color,
}

/// Map an `EdgeType` to its Unicode box-drawing character, dependent on the
/// chosen `GraphStyle`. Only `Rounded` and `Angular` differ at the corner
/// glyphs; `Smooth` uses the same characters as `Rounded`.
fn edge_char(edge_type: EdgeType, style: GraphStyle) -> char {
    match edge_type {
        EdgeType::Vertical => '\u{2502}',   // │
        EdgeType::Horizontal => '\u{2500}', // -
        EdgeType::Up => '\u{2502}',         // │
        EdgeType::Down => '\u{2502}',       // │
        EdgeType::Left => '\u{2500}',       // -
        EdgeType::Right => '\u{2500}',      // -
        EdgeType::RightTop => match style {
            GraphStyle::Angular => '\u{2510}', // ┐
            _ => '\u{256E}',                   // ╮
        },
        EdgeType::RightBottom => match style {
            GraphStyle::Angular => '\u{2518}', // ┘
            _ => '\u{256F}',                   // ╯
        },
        EdgeType::LeftTop => match style {
            GraphStyle::Angular => '\u{250C}', // ┌
            _ => '\u{256D}',                   // ╭
        },
        EdgeType::LeftBottom => match style {
            GraphStyle::Angular => '\u{2514}', // └
            _ => '\u{2570}',                   // ╰
        },
    }
}

/// Returns true for edge types that occupy both the primary cell AND the
/// trailing padding cell (i.e. pure horizontal connectors that span a full
/// 2-character lane slot).
fn is_full_horizontal(edge_type: EdgeType) -> bool {
    matches!(
        edge_type,
        EdgeType::Horizontal | EdgeType::Left | EdgeType::Right
    )
}

fn edge_color_from_index(color_index: usize, edge_colors: &[Color]) -> Color {
    if edge_colors.is_empty() {
        Color::Reset
    } else {
        edge_colors[color_index % edge_colors.len()]
    }
}

/// Render one row of the ASCII graph for the commit identified by `commit_hash`.
///
/// The returned `Vec<AsciiGraphCell>` represents the graph column characters
/// for that commit row. Each lane occupies two character positions:
/// - position `2 * lane`: the glyph (node symbol or edge character)
/// - position `2 * lane + 1`: padding space, or `─` when a horizontal edge
///   spans through it
///
/// Trailing space cells are trimmed from the result (compact output).
///
/// Returns an empty `Vec` when `commit_hash` is not found in the graph.
pub fn render_ascii_row(
    graph: &Graph,
    commit_hash: &CommitHash,
    commit_type: &CommitType,
    is_head: bool,
    graph_style: GraphStyle,
    edge_colors: &[Color],
    uncommitted_color: Color,
    uncommitted_lane: Option<(usize, usize)>,
    lane_width: usize,
) -> Vec<AsciiGraphCell> {
    // Look up lane position; bail out gracefully for unknown hashes.
    let Some(&(pos_x, pos_y)) = graph.commit_pos_map.get(commit_hash) else {
        return Vec::new();
    };

    // Width: one slot (2 chars) per lane, plus one extra for the node lane
    // itself so the trailing padding cell is always included.
    let width = (graph.max_pos_x + 1) * lane_width;

    let mut cells: Vec<AsciiGraphCell> = (0..width)
        .map(|_| AsciiGraphCell {
            ch: ' ',
            color: Color::Reset,
        })
        .collect();

    if let Some(row_edges) = graph.edges.get(pos_y) {
        place_edges(
            &mut cells,
            row_edges,
            graph_style,
            edge_colors,
            uncommitted_color,
            uncommitted_lane,
            pos_y,
            lane_width,
        );
    }

    let (node_ch, node_color) = node_symbol(
        commit_type,
        commit_hash,
        is_head,
        graph,
        edge_colors,
        uncommitted_color,
    );

    let node_idx = lane_width * pos_x;
    if node_idx < cells.len() {
        cells[node_idx] = AsciiGraphCell {
            ch: node_ch,
            color: node_color,
        };
    }

    // Trim trailing spaces for compact output.
    while cells.last().map(|c| c.ch == ' ').unwrap_or(false) {
        cells.pop();
    }

    cells
}

/// Place all edge glyphs for a single row into `cells`.
fn place_edges(
    cells: &mut [AsciiGraphCell],
    row_edges: &[Edge],
    graph_style: GraphStyle,
    edge_colors: &[Color],
    uncommitted_color: Color,
    uncommitted_lane: Option<(usize, usize)>,
    pos_y: usize,
    lane_width: usize,
) {
    for edge in row_edges {
        let primary_idx = lane_width * edge.pos_x;
        if primary_idx >= cells.len() {
            continue;
        }

        let ch = edge_char(edge.edge_type, graph_style);
        let on_uncommitted_lane = uncommitted_lane.is_some_and(|(unco_x, head_y)| {
            edge.associated_line_pos_x == unco_x && pos_y > 0 && pos_y <= head_y
        });
        let color = if on_uncommitted_lane {
            uncommitted_color
        } else {
            edge_color_from_index(edge.associated_line_pos_x, edge_colors)
        };

        cells[primary_idx] = AsciiGraphCell { ch, color };

        if is_full_horizontal(edge.edge_type) {
            for pad in 1..lane_width {
                let idx = primary_idx + pad;
                if idx < cells.len() {
                    cells[idx] = AsciiGraphCell {
                        ch: '\u{2500}',
                        color,
                    };
                }
            }
        }
    }
}

/// Return the node symbol character and color for the commit being rendered.
fn node_symbol(
    commit_type: &CommitType,
    commit_hash: &CommitHash,
    is_head: bool,
    graph: &Graph,
    edge_colors: &[Color],
    uncommitted_color: Color,
) -> (char, Color) {
    match commit_type {
        CommitType::Uncommitted => (
            '\u{25CB}', // ○
            uncommitted_color,
        ),
        CommitType::Stash => {
            let color = commit_color(commit_hash, graph, edge_colors);
            ('\u{25C9}', color) // ◉
        }
        CommitType::Commit => {
            let color = commit_color(commit_hash, graph, edge_colors);
            if is_head {
                ('\u{25CB}', color) // ○ - hollow circle for HEAD
            } else {
                ('\u{25CF}', color) // ● - filled circle for regular commits
            }
        }
    }
}

/// Resolve the display color for a commit node from `commit_color_map`.
fn commit_color(commit_hash: &CommitHash, graph: &Graph, edge_colors: &[Color]) -> Color {
    if edge_colors.is_empty() {
        return Color::Reset;
    }
    let color_idx = graph
        .commit_pos_map
        .get(commit_hash)
        .map(|&(px, _)| px)
        .unwrap_or(0);
    edge_colors[color_idx % edge_colors.len()]
}
