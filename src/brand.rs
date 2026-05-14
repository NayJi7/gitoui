use resvg::{tiny_skia, usvg};

static LOGO_SVG: &[u8] = include_bytes!("../assets/brand/logo-nobg.svg");
static WORDMARK_SVG: &[u8] = include_bytes!("../assets/brand/wordmark-nobg.svg");

// 16 cells of the G logo in spiral build order (path data, fill color).
// SVG viewBox: 0 0 809 1008.  The y=601 on cells 12/15/16 is the 1-px fix that
// eliminates the seam visible at fade-out in the animated version.
const CELLS: [(&str, &str); 16] = [
    ("M604 0H804V199H604V0Z", "#F05133"), //  1 R1C4 — spiral start (top-right)
    ("M404 0H604V199H404V0Z", "#F05133"), //  2 R1C3
    ("M205 0H405V199H205V0Z", "#F05133"), //  3 R1C2
    ("M5 0H205V199H5V0Z", "#F05133"),     //  4 R1C1 — top-left
    ("M4 199H204V404H4V199Z", "#F05133"), //  5 R2C1
    ("M4 404H204V603H4V404Z", "#F05133"), //  6 R3C1
    ("M4 603H204V799H4V603Z", "#F05133"), //  7 R4C1
    ("M5 799H204V998H5V799Z", "#F05133"), //  8 R5C1 — bottom-left
    ("M204 799H404V998H204V799Z", "#F05133"), //  9 R5C2
    ("M404 799H604V998H404V799Z", "#F05133"), // 10 R5C3
    ("M604 799H805V998H604V799Z", "#F05133"), // 11 R5C4 — bottom-right
    ("M604 601H805V799H604V601Z", "#F05133"), // 12 R4C4 (y=601 seam fix)
    ("M604 403H804V601H604V403Z", "#F05133"), // 13 R3C4
    ("M404 403H604V601H404V403Z", "#F05133"), // 14 R3C3 — tongue
    ("M404 601H604V799H404V601Z", "#8B2A12"), // 15 R4C3 — shadow
    ("M204 601H404V799H204V601Z", "#8B2A12"), // 16 R4C2 — shadow
];

/// Width in terminal columns of the spinner image.
pub const SPINNER_CELL_WIDTH: usize = 2;

/// Total pre-rendered frames: 16 build + 4 hold + 8 exit.
pub const SPINNER_FRAME_COUNT: usize = 28;

/// Build a minimal SVG containing only the cells at the given indices.
fn build_frame_svg(visible: impl Iterator<Item = usize>) -> String {
    let mut s = String::from(r#"<svg viewBox="0 0 809 1008" xmlns="http://www.w3.org/2000/svg">"#);
    for idx in visible {
        let (d, fill) = CELLS[idx];
        s.push_str(&format!(r#"<path d="{d}" fill="{fill}"/>"#));
    }
    s.push_str("</svg>");
    s
}

/// Return the cell indices visible in frame `f` (0-based).
fn visible_for_frame(f: usize) -> Box<dyn Iterator<Item = usize>> {
    if f < 16 {
        // Build: one more cell per frame
        Box::new(0..=f)
    } else if f < 20 {
        // Hold: all 16 cells
        Box::new(0..16)
    } else {
        // Exit: shed 2 cells per frame in reverse spiral order
        let exit_frame = f - 20; // 0..7
        let keep = 16usize.saturating_sub((exit_frame + 1) * 2);
        Box::new(0..keep)
    }
}

/// Pre-render all SPINNER_FRAME_COUNT animation frames as PNG bytes.
/// PNG is square (px × px) to match the 2-col × 1-row cell aspect ratio,
/// preventing vertical overflow that would scroll the terminal from the last row.
/// Right-aligned to match `render_logo_png` so the spinner sits in the same
/// pixel position as the static G logo it replaces.
pub fn render_spinner_frames() -> Vec<Vec<u8>> {
    let px = (SPINNER_CELL_WIDTH * 16) as u32; // 32×32 square
    (0..SPINNER_FRAME_COUNT)
        .filter_map(|f| {
            let svg = build_frame_svg(visible_for_frame(f));
            render_svg_to_png_aligned(svg.as_bytes(), px, px, HAlign::Right)
        })
        .collect()
}

/// G logomark: 809×1008 (≈0.8:1 portrait). In a 1-row terminal at 8×16px cells,
/// 2 cols × 1 row = 16×16px → 1:1 display ratio, close to the natural 0.8:1.
pub const LOGO_CELL_WIDTH: usize = 2;

/// "gitoui" wordmark: 1044×500 (≈2.09:1 landscape). At 4 cols × 1 row (2:1 display ratio),
/// the scale is width-limited (SVG slightly wider than canvas), so the text fills the full
/// row width and is ~96% of row height — about 0.7 display-px shorter than the G logo.
pub const WORDMARK_CELL_WIDTH: usize = 4;

/// Gap between the two images, in terminal columns.
pub const GAP_COLS: u16 = 1;

fn render_svg_to_png(svg_data: &[u8], px_w: u32, px_h: u32) -> Option<Vec<u8>> {
    render_svg_to_png_aligned(svg_data, px_w, px_h, HAlign::Center)
}

#[derive(Clone, Copy)]
enum HAlign {
    Center,
    Right,
}

fn render_svg_to_png_aligned(
    svg_data: &[u8],
    px_w: u32,
    px_h: u32,
    halign: HAlign,
) -> Option<Vec<u8>> {
    let opts = usvg::Options::default();
    let tree = usvg::Tree::from_data(svg_data, &opts).ok()?;
    let sx = px_w as f32 / tree.size().width();
    let sy = px_h as f32 / tree.size().height();
    // Use uniform scale so we never distort the SVG within the pixmap.
    let scale = sx.min(sy);
    let rendered_w = tree.size().width() * scale;
    let tx = match halign {
        HAlign::Center => (px_w as f32 - rendered_w) / 2.0,
        HAlign::Right => px_w as f32 - rendered_w,
    };
    let transform = tiny_skia::Transform::from_scale(scale, scale).post_translate(tx, 0.0);
    let mut pixmap = tiny_skia::Pixmap::new(px_w, px_h)?;
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    pixmap.encode_png().ok()
}

/// Render the G logomark.
/// PNG is square (px × px) to match the 2-col × 1-row cell aspect ratio.
/// The SVG is portrait (0.8:1), so it's height-limited in the square canvas — that
/// leaves ~1.58 display-px of horizontal slack. We right-align the rendered G so the
/// slack sits on the LEFT of the image, putting the G flush against the wordmark.
pub fn render_logo_png() -> Option<Vec<u8>> {
    let px = (LOGO_CELL_WIDTH * 32) as u32; // 64×64 square
    render_svg_to_png_aligned(LOGO_SVG, px, px, HAlign::Right)
}

/// Render the "gitoui" wordmark.
/// PNG aspect is 2:1 to match the 4-col × 1-row cell aspect ratio
/// (4 cols × cell_w : 1 row × cell_h = 4×8 : 1×16 = 32:16 = 2:1).
/// The SVG (1.6:1) is narrower than the canvas (2:1) → scale is height-limited
/// → text fills the full row height, matching the G logo.
pub fn render_wordmark_png() -> Option<Vec<u8>> {
    let px_w = (WORDMARK_CELL_WIDTH * 32) as u32; // 128
    let px_h = px_w / 2; // 64  (128:64 = 2:1)
    render_svg_to_png(WORDMARK_SVG, px_w, px_h)
}

/// Render the standalone G logomark at the given cell size (centered in canvas).
pub fn render_logo_sized(cell_w: u32, cell_h: u32) -> Option<Vec<u8>> {
    let px_w = cell_w * 32;
    let px_h = cell_h * 64;
    render_svg_to_png(LOGO_SVG, px_w, px_h)
}

/// Render the standalone "gitoui" wordmark at the given cell size (centered in canvas).
pub fn render_wordmark_sized(cell_w: u32, cell_h: u32) -> Option<Vec<u8>> {
    let px_w = cell_w * 32;
    let px_h = cell_h * 64;
    render_svg_to_png(WORDMARK_SVG, px_w, px_h)
}
