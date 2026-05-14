mod views;

pub mod blame;
pub mod branch_detail;
mod compare;
mod config;
pub mod conflict;
mod detail;
pub mod dialog;
mod diff;
pub mod file_history;
mod graph_preview;
mod help;
pub mod issue;
mod list;
pub mod pr;
pub mod rebase;
mod refs;
pub mod tag_detail;
pub mod uncommitted;
mod user_command;

pub use views::*;

pub(crate) fn adaptive_detail_height(
    area_height: u16,
    preferred_height: u16,
    min_height: u16,
) -> u16 {
    let scaled_height = area_height.saturating_mul(3) / 5;
    let target = preferred_height.max(min_height).max(scaled_height);
    target.min(area_height.saturating_sub(5)).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adaptive_detail_height_grows_on_tall_terminals() {
        assert_eq!(adaptive_detail_height(80, 16, 10), 48);
    }

    #[test]
    fn adaptive_detail_height_respects_min() {
        assert_eq!(adaptive_detail_height(10, 16, 10), 5);
    }

    #[test]
    fn adaptive_detail_height_small_terminal_clamps() {
        assert_eq!(adaptive_detail_height(8, 16, 10), 3);
    }

    #[test]
    fn adaptive_detail_height_never_zero() {
        assert_eq!(adaptive_detail_height(1, 1, 1), 1);
    }
}
