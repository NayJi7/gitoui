use ratatui::crossterm::terminal;

use crate::{
    graph::{CellWidthType, Graph},
    GraphWidthType, Result,
};

pub fn decide_cell_width_type(
    graph: &Graph,
    cell_width_type: Option<GraphWidthType>,
) -> Result<CellWidthType> {
    let (w, h) = terminal::size()?;
    decide_cell_width_type_from(graph.max_pos_x, w as usize, h as usize, cell_width_type)
}

fn decide_cell_width_type_from(
    max_pos_x: usize,
    term_width: usize,
    term_height: usize,
    cell_width_type: Option<GraphWidthType>,
) -> Result<CellWidthType> {
    let single_image_cell_width = max_pos_x + 1;
    let double_image_cell_width = single_image_cell_width * 2;

    match cell_width_type {
        Some(GraphWidthType::Double) => {
            let required_width = double_image_cell_width + 2;
            if required_width > term_width {
                let msg = format!("Terminal too small ({term_width}x{term_height} characters). The current graph needs at least {required_width} columns to display properly.");
                return Err(msg.into());
            }
            Ok(CellWidthType::Double)
        }
        Some(GraphWidthType::Single) => {
            let required_width = single_image_cell_width + 2;
            if required_width > term_width {
                let msg = format!("Terminal too small ({term_width}x{term_height} characters). The current graph needs at least {required_width} columns to display properly.");
                return Err(msg.into());
            }
            Ok(CellWidthType::Single)
        }
        Some(GraphWidthType::Auto) | None => {
            Ok(auto_cell_width(single_image_cell_width, term_width))
        }
    }
}

/// What `graph_width = "auto"` resolves to: Double when a graph of `lanes`
/// lanes fits that wide in `term_width` columns, Single otherwise. The
/// commit list and the settings page both go through here, so what the
/// settings page says and previews is what the list does.
pub fn auto_cell_width(lanes: usize, term_width: usize) -> CellWidthType {
    let double_required_width = lanes * 2 + 2;
    if double_required_width <= term_width {
        return CellWidthType::Double;
    }
    // Even when Single doesn't strictly "fit", we still pick it
    // and let the renderer cap the graph column to ~40 % of the
    // panel (see `content_column_widths` in
    // `widget/commit_list.rs`). The overflowing lanes get
    // truncated at draw time, which is far friendlier than a
    // hard bail on huge multi-branch repos
    // (rust-lang/rust, linux, etc.) opened in a normal-sized
    // terminal.
    CellWidthType::Single
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_picks_double_when_it_fits_and_single_when_it_does_not() {
        // 3 lanes need 3 * 2 + 2 = 8 columns to be shown double.
        assert_eq!(auto_cell_width(3, 8), CellWidthType::Double);
        assert_eq!(auto_cell_width(3, 7), CellWidthType::Single);
        assert_eq!(auto_cell_width(1, 120), CellWidthType::Double);
    }

    #[test]
    fn auto_goes_through_the_same_rule_as_the_list() {
        for (lanes, width) in [(1, 120), (4, 10), (4, 9), (60, 80)] {
            let from_list =
                decide_cell_width_type_from(lanes - 1, width, 40, Some(GraphWidthType::Auto))
                    .unwrap();
            assert_eq!(from_list, auto_cell_width(lanes, width));
        }
    }
}
