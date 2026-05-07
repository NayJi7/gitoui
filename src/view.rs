mod views;

pub mod branch_detail;
mod config;
mod detail;
pub mod dialog;
mod diff;
mod help;
mod list;
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
    fn adaptive_detail_height_keeps_list_visible() {
        assert_eq!(adaptive_detail_height(12, 16, 10), 7);
    }

    #[test]
    fn adaptive_detail_height_respects_preferred_height_on_normal_terminals() {
        assert_eq!(adaptive_detail_height(30, 16, 10), 18);
    }
}
