//! The look every popup of the app shares: a margin of one empty column on
//! each side and one empty row above and below, so the box never blends with
//! what is behind it, and a colored border and title.

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    widgets::{Block, Clear},
    Frame,
};

use crate::color::ColorTheme;

/// `rect` grown by one cell on every side, kept inside `bounds`.
pub fn with_margin(rect: Rect, bounds: Rect) -> Rect {
    // The far edges are measured from the grown rectangle, so a popup that
    // touches the left or the top of the screen does not grow past its right
    // or bottom side.
    let left = rect.x.saturating_sub(1).max(bounds.x);
    let top = rect.y.saturating_sub(1).max(bounds.y);
    let right = rect
        .x
        .saturating_add(rect.width)
        .saturating_add(1)
        .min(bounds.right());
    let bottom = rect
        .y
        .saturating_add(rect.height)
        .saturating_add(1)
        .min(bounds.bottom());
    Rect::new(
        left,
        top,
        right.saturating_sub(left),
        bottom.saturating_sub(top),
    )
}

/// Wipes the popup and its empty margin. Call it before drawing the box.
pub fn clear_with_margin(f: &mut Frame, rect: Rect, bounds: Rect, theme: &ColorTheme) {
    let outer = with_margin(rect, bounds);
    f.render_widget(Clear, outer);
    f.render_widget(Block::default().style(Style::default().bg(theme.bg)), outer);
}

/// The color of the border and of the title of a popup.
pub fn accent(theme: &ColorTheme) -> Color {
    theme.list_head_fg
}

pub fn border_style(theme: &ColorTheme) -> Style {
    Style::default().fg(accent(theme))
}

pub fn title_style(theme: &ColorTheme) -> Style {
    Style::default()
        .fg(accent(theme))
        .add_modifier(Modifier::BOLD)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_margin_is_one_cell_on_every_side_inside_the_screen() {
        let screen = Rect::new(0, 0, 80, 24);
        assert_eq!(
            with_margin(Rect::new(10, 5, 20, 8), screen),
            Rect::new(9, 4, 22, 10)
        );
        // At the edges it stops at the screen.
        assert_eq!(
            with_margin(Rect::new(0, 0, 80, 24), screen),
            Rect::new(0, 0, 80, 24)
        );
        // Touching the left and top edges: one more column and row on the far
        // sides only, not two.
        assert_eq!(
            with_margin(Rect::new(0, 0, 10, 5), screen),
            Rect::new(0, 0, 11, 6)
        );
        assert_eq!(
            with_margin(Rect::new(60, 18, 20, 6), screen),
            Rect::new(59, 17, 21, 7)
        );
    }
}
