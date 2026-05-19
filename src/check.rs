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
            let double_required_width = double_image_cell_width + 2;
            if double_required_width <= term_width {
                return Ok(CellWidthType::Double);
            }
            // Even when Single doesn't strictly "fit", we still pick it
            // and let the renderer cap the graph column to ~40 % of the
            // panel (see `content_column_widths` in
            // `widget/commit_list.rs`). The overflowing lanes get
            // truncated at draw time, which is far friendlier than a
            // hard bail on huge multi-branch repos
            // (rust-lang/rust, linux, etc.) opened in a normal-sized
            // terminal.
            Ok(CellWidthType::Single)
        }
    }
}
