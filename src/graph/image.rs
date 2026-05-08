use std::{
    fmt::{self, Debug, Formatter},
    hash::{Hash, Hasher},
    io::Cursor,
    process,
    time::{SystemTime, UNIX_EPOCH},
};

use rustc_hash::{FxHashMap, FxHashSet};

use crate::{
    color::GraphColorSet,
    git::CommitHash,
    graph::{
        geometry::{bounding_box_u32, Point},
        Edge, EdgeType, Graph,
    },
    protocol::{ImageProtocol, PreparedImage},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphStyle {
    Rounded,
    Angular,
    Smooth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GraphImageWidthMode {
    Compact,
    Fixed,
}

#[derive(Debug)]
pub struct GraphImageManager<'a> {
    prepared_image_map: FxHashMap<CommitHash, PreparedImage>,
    image_ids: FxHashSet<u32>,
    pending_uploads: Vec<String>,
    head_commit_hash: Option<CommitHash>,

    graph: &'a Graph<'a>,
    cell_width_type: CellWidthType,
    graph_style: GraphStyle,
    image_width_mode: GraphImageWidthMode,
    image_params: ImageParams,
    drawing_pixels: DrawingPixels,
    image_protocol: ImageProtocol,
    session_nonce: u32,
}

impl<'a> GraphImageManager<'a> {
    pub fn new(
        graph: &'a Graph,
        graph_color_set: &GraphColorSet,
        cell_width_type: CellWidthType,
        graph_style: GraphStyle,
        image_width_mode: GraphImageWidthMode,
        image_protocol: ImageProtocol,
    ) -> Self {
        let image_params = ImageParams::new(graph_color_set, cell_width_type);
        let drawing_pixels = DrawingPixels::new(&image_params);

        GraphImageManager {
            prepared_image_map: FxHashMap::default(),
            image_ids: FxHashSet::default(),
            pending_uploads: Vec::default(),
            head_commit_hash: None,
            graph,
            cell_width_type,
            graph_style,
            image_width_mode,
            image_params,
            drawing_pixels,
            image_protocol,
            session_nonce: create_session_nonce(),
        }
    }

    pub fn prepared_image(&self, commit_hash: &CommitHash) -> Option<&PreparedImage> {
        self.prepared_image_map.get(commit_hash)
    }

    pub fn image_ids(&self) -> &FxHashSet<u32> {
        &self.image_ids
    }

    pub fn drain_pending_uploads(&mut self) -> Vec<String> {
        std::mem::take(&mut self.pending_uploads)
    }

    pub fn clear_prepared_images(&mut self) {
        self.prepared_image_map.clear();
        self.image_ids.clear();
        self.pending_uploads.clear();
    }

    /// Update the background color used when rendering new graph images.
    /// Clears all cached images so they get re-generated with the new color.
    pub fn update_background_color(&mut self, r: u8, g: u8, b: u8) {
        self.image_params.background_color = image::Rgba([r, g, b, 0xff]);
        self.clear_prepared_images();
    }

    pub fn ensure_uploaded(&mut self, commit_hash: &CommitHash) {
        if self.prepared_image_map.contains_key(commit_hash) {
            return;
        }
        let is_head = self.head_commit_hash.as_ref() == Some(commit_hash);
        // HEAD est toujours dessiné comme un cercle vide (hollow circle)
        let head = is_head;
        let image_id = graph_image_id(self.session_nonce, commit_hash, head);

        let graph_row_image = build_single_graph_row_image(
            self.graph,
            &self.image_params,
            &self.drawing_pixels,
            self.graph_style,
            self.image_width_mode,
            commit_hash,
            head,
        );
        let mut image =
            graph_row_image.prepare(self.cell_width_type, self.image_protocol, image_id);
        if let Some(upload_data) = image.take_upload_data() {
            self.pending_uploads.push(upload_data);
        }
        self.prepared_image_map.insert(commit_hash.clone(), image);
        self.image_ids.insert(image_id);
    }

    pub fn head_commit_hash(&self) -> Option<&CommitHash> {
        self.head_commit_hash.as_ref()
    }

    pub fn set_head_commit_hash(&mut self, commit_hash: Option<&CommitHash>) {
        if self.head_commit_hash.as_ref() == commit_hash {
            return;
        }
        self.head_commit_hash = commit_hash.cloned();
    }

    pub fn invalidate(&mut self, commit_hash: &CommitHash) {
        self.prepared_image_map.remove(commit_hash);
        self.image_ids
            .remove(&graph_image_id(self.session_nonce, commit_hash, true));
        self.image_ids
            .remove(&graph_image_id(self.session_nonce, commit_hash, false));
    }
}

#[derive(Debug, Default)]
pub struct GraphImage {
    pub images: FxHashMap<Vec<Edge>, GraphRowImage>,
}

pub struct GraphRowImage {
    pub bytes: Vec<u8>,
    pub cell_count: usize,
}

impl Debug for GraphRowImage {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "GraphRowImage {{ bytes: [{} bytes], cell_count: {} }}",
            self.bytes.len(),
            self.cell_count
        )
    }
}

impl GraphRowImage {
    fn prepare(
        &self,
        cell_width_type: CellWidthType,
        image_protocol: ImageProtocol,
        image_id: u32,
    ) -> PreparedImage {
        let image_cell_width = match cell_width_type {
            CellWidthType::Double => self.cell_count * 2,
            CellWidthType::Single => self.cell_count,
        };
        image_protocol.prepare_image(&self.bytes, image_cell_width, image_id)
    }
}

fn create_session_nonce() -> u32 {
    let mut hasher = rustc_hash::FxHasher::default();
    process::id().hash(&mut hasher);
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .hash(&mut hasher);
    hasher.finish() as u32
}

fn graph_image_id(session_nonce: u32, commit_hash: &CommitHash, head: bool) -> u32 {
    let mut hasher = rustc_hash::FxHasher::default();
    session_nonce.hash(&mut hasher);
    commit_hash.hash(&mut hasher);
    head.hash(&mut hasher);
    hasher.finish() as u32
}

#[derive(Debug)]
pub struct ImageParams {
    width: u16,
    height: u16,
    line_width: u16,
    circle_inner_radius: u16,
    circle_outer_radius: u16,
    edge_colors: Vec<image::Rgba<u8>>,
    circle_edge_color: image::Rgba<u8>,
    background_color: image::Rgba<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellWidthType {
    Double, // 2 cells
    Single,
}

impl ImageParams {
    pub fn new(graph_color_set: &GraphColorSet, cell_width_type: CellWidthType) -> Self {
        let (width, height, line_width, circle_inner_radius, circle_outer_radius) =
            match cell_width_type {
                CellWidthType::Double => (50, 56, 5, 11, 14),
                CellWidthType::Single => (25, 56, 3, 7, 10),
            };
        let edge_colors = graph_color_set
            .colors
            .iter()
            .map(|c| c.to_image_color())
            .collect();
        let circle_edge_color = graph_color_set.edge_color.to_image_color();
        let background_color = graph_color_set.background_color.to_image_color();
        Self {
            width,
            height,
            line_width,
            circle_inner_radius,
            circle_outer_radius,
            edge_colors,
            circle_edge_color,
            background_color,
        }
    }

    fn edge_color(&self, index: usize) -> image::Rgba<u8> {
        self.edge_colors[index % self.edge_colors.len()]
    }

    fn corner_radius(&self) -> u16 {
        if self.width < self.height {
            self.width / 2
        } else {
            self.height / 2
        }
    }
}

fn build_single_graph_row_image(
    graph: &Graph<'_>,
    image_params: &ImageParams,
    drawing_pixels: &DrawingPixels,
    graph_style: GraphStyle,
    image_width_mode: GraphImageWidthMode,
    commit_hash: &CommitHash,
    head: bool,
) -> GraphRowImage {
    let (pos_x, pos_y) = graph.commit_pos_map[&commit_hash];
    let edges = &graph.edges[pos_y];

    // Determine render style based on commit type
    let commit = graph
        .commits
        .iter()
        .find(|c| c.commit_hash == *commit_hash)
        .unwrap();
    let is_stash = matches!(commit.commit_type, crate::git::CommitType::Stash);
    let is_uncommitted = matches!(commit.commit_type, crate::git::CommitType::Uncommitted);

    // Uncommitted uses #808080, matching VS Code Git Graph's convention
    const UNCOMMITTED_COLOR: image::Rgba<u8> = image::Rgba([0x80, 0x80, 0x80, 0xff]);
    let commit_color = if is_uncommitted {
        UNCOMMITTED_COLOR
    } else if graph_style == GraphStyle::Smooth {
        let color_index = graph
            .commit_color_map
            .get(commit_hash)
            .copied()
            .unwrap_or(pos_x);
        image_params.edge_color(color_index)
    } else {
        image_params.edge_color(pos_x)
    };

    // For rows between uncommitted (pos_y=0) and HEAD, color edges on the
    // uncommitted lane with the same bluish-grey so the full segment is uniform.
    let uncommitted_lane: Option<(usize, usize, image::Rgba<u8>)> = graph
        .commits
        .iter()
        .find(|c| matches!(c.commit_type, crate::git::CommitType::Uncommitted))
        .and_then(|unco| {
            let &(unco_x, _) = graph.commit_pos_map.get(&unco.commit_hash)?;
            let &(_, head_pos_y) = unco
                .parent_commit_hashes
                .first()
                .and_then(|h| graph.commit_pos_map.get(h))?;
            Some((unco_x, head_pos_y, UNCOMMITTED_COLOR))
        });

    let max_pos_x = match image_width_mode {
        GraphImageWidthMode::Compact => edges.iter().map(|e| e.pos_x).fold(pos_x, usize::max),
        GraphImageWidthMode::Fixed => graph.max_pos_x,
    };

    let cell_count = max_pos_x + 1;

    calc_graph_row_image(
        pos_x,
        cell_count,
        edges,
        image_params,
        drawing_pixels,
        graph_style,
        &graph.branch_segments,
        pos_y,
        head,
        is_stash,
        is_uncommitted,
        commit_color,
        uncommitted_lane,
    )
}

type Pixels = FxHashSet<(i32, i32)>;

#[derive(Debug)]
pub struct DrawingPixels {
    circle: Pixels,
    circle_edge: Pixels,
    circle_gap: Pixels,
    commit_circle_gap: Pixels,
    vertical_edge: Pixels,
    horizontal_edge: Pixels,
    up_edge: Pixels,
    down_edge: Pixels,
    left_edge: Pixels,
    right_edge: Pixels,
    right_top_edge: Pixels,
    left_top_edge: Pixels,
    right_bottom_edge: Pixels,
    left_bottom_edge: Pixels,
}

impl DrawingPixels {
    pub fn new(image_params: &ImageParams) -> Self {
        let circle = calc_commit_circle_drawing_pixels(image_params);
        let circle_edge = calc_circle_edge_drawing_pixels(image_params);
        let circle_gap = calc_circle_gap_drawing_pixels(image_params);
        let commit_circle_gap = calc_commit_circle_gap_drawing_pixels(image_params);
        let vertical_edge = calc_vertical_edge_drawing_pixels(image_params);
        let horizontal_edge = calc_horizontal_edge_drawing_pixels(image_params);
        let up_edge = calc_up_edge_drawing_pixels(image_params);
        let down_edge = calc_down_edge_drawing_pixels(image_params);
        let left_edge = calc_left_edge_drawing_pixels(image_params);
        let right_edge = calc_right_edge_drawing_pixels(image_params);
        let right_top_edge = calc_right_top_edge_drawing_pixels(image_params);
        let left_top_edge = calc_left_top_edge_drawing_pixels(image_params);
        let right_bottom_edge = calc_right_bottom_edge_drawing_pixels(image_params);
        let left_bottom_edge = calc_left_bottom_edge_drawing_pixels(image_params);

        Self {
            circle,
            circle_edge,
            circle_gap,
            commit_circle_gap,
            vertical_edge,
            horizontal_edge,
            up_edge,
            down_edge,
            left_edge,
            right_edge,
            right_top_edge,
            left_top_edge,
            right_bottom_edge,
            left_bottom_edge,
        }
    }
}

fn calc_commit_circle_drawing_pixels(image_params: &ImageParams) -> Pixels {
    calc_circle_drawing_pixels(image_params, image_params.circle_inner_radius as i32)
}

fn calc_circle_edge_drawing_pixels(image_params: &ImageParams) -> Pixels {
    let inner = calc_circle_drawing_pixels(image_params, image_params.circle_inner_radius as i32);
    let outer = calc_circle_drawing_pixels(image_params, image_params.circle_outer_radius as i32);
    outer.difference(&inner).cloned().collect()
}

fn calc_circle_gap_drawing_pixels(image_params: &ImageParams) -> Pixels {
    let inner = calc_circle_drawing_pixels(image_params, image_params.circle_outer_radius as i32);
    let outer =
        calc_circle_drawing_pixels(image_params, (image_params.circle_outer_radius + 1) as i32);
    outer.difference(&inner).cloned().collect()
}

fn calc_commit_circle_gap_drawing_pixels(image_params: &ImageParams) -> Pixels {
    let inner = calc_circle_drawing_pixels(image_params, image_params.circle_inner_radius as i32);
    let outer =
        calc_circle_drawing_pixels(image_params, (image_params.circle_inner_radius + 1) as i32);
    outer.difference(&inner).cloned().collect()
}

fn calc_circle_drawing_pixels(image_params: &ImageParams, radius: i32) -> Pixels {
    // Bresenham's circle algorithm
    let center_x = (image_params.width / 2) as i32;
    let center_y = (image_params.height / 2) as i32;

    let mut x = radius;
    let mut y = 0;
    let mut p = 1 - radius;

    let mut pixels = Pixels::default();

    while x >= y {
        for dx in -x..=x {
            pixels.insert((center_x + dx, center_y + y));
            pixels.insert((center_x + dx, center_y - y));
        }
        for dx in -y..=y {
            pixels.insert((center_x + dx, center_y + x));
            pixels.insert((center_x + dx, center_y - x));
        }

        y += 1;
        if p <= 0 {
            p += 2 * y + 1;
        } else {
            x -= 1;
            p += 2 * y - 2 * x + 1;
        }
    }

    pixels
}

fn calc_vertical_edge_drawing_pixels(image_params: &ImageParams) -> Pixels {
    let center_x = (image_params.width / 2) as i32;
    let line_width = image_params.line_width as i32;
    let x_start = center_x - line_width / 2;

    let mut pixels = Pixels::default();
    for y in 0..image_params.height as i32 {
        for x in x_start..(x_start + line_width) {
            pixels.insert((x, y));
        }
    }
    pixels
}

fn calc_horizontal_edge_drawing_pixels(image_params: &ImageParams) -> Pixels {
    let center_y = (image_params.height / 2) as i32;
    let line_width = image_params.line_width as i32;
    let y_start = center_y - line_width / 2;

    let mut pixels = Pixels::default();
    for y in y_start..(y_start + line_width) {
        for x in 0..image_params.width as i32 {
            pixels.insert((x, y));
        }
    }
    pixels
}

fn calc_up_edge_drawing_pixels(image_params: &ImageParams) -> Pixels {
    let center_x = (image_params.width / 2) as i32;
    let line_width = image_params.line_width as i32;
    let x_start = center_x - line_width / 2;
    let circle_center_y = (image_params.height / 2) as i32;
    let circle_outer_radius = image_params.circle_outer_radius as i32;

    let mut pixels = Pixels::default();
    for y in 0..(circle_center_y - circle_outer_radius) {
        for x in x_start..(x_start + line_width) {
            pixels.insert((x, y));
        }
    }
    pixels
}

fn calc_down_edge_drawing_pixels(image_params: &ImageParams) -> Pixels {
    let center_x = (image_params.width / 2) as i32;
    let line_width = image_params.line_width as i32;
    let x_start = center_x - line_width / 2;
    let circle_center_y = (image_params.height / 2) as i32;
    let circle_outer_radius = image_params.circle_outer_radius as i32;

    let mut pixels = Pixels::default();
    for y in (circle_center_y + circle_outer_radius + 1)..(image_params.height as i32) {
        for x in x_start..(x_start + line_width) {
            pixels.insert((x, y));
        }
    }
    pixels
}

fn calc_left_edge_drawing_pixels(image_params: &ImageParams) -> Pixels {
    let center_y = (image_params.height / 2) as i32;
    let line_width = image_params.line_width as i32;
    let y_start = center_y - line_width / 2;
    let circle_center_x = (image_params.width / 2) as i32;
    let circle_outer_radius = image_params.circle_outer_radius as i32;

    let mut pixels = Pixels::default();
    for y in y_start..(y_start + line_width) {
        for x in 0..(circle_center_x - circle_outer_radius) {
            pixels.insert((x, y));
        }
    }
    pixels
}

fn calc_right_edge_drawing_pixels(image_params: &ImageParams) -> Pixels {
    let center_y = (image_params.height / 2) as i32;
    let line_width = image_params.line_width as i32;
    let y_start = center_y - line_width / 2;
    let circle_center_x = (image_params.width / 2) as i32;
    let circle_outer_radius = image_params.circle_outer_radius as i32;

    let mut pixels = Pixels::default();
    for y in y_start..(y_start + line_width) {
        for x in (circle_center_x + circle_outer_radius + 1)..(image_params.width as i32) {
            pixels.insert((x, y));
        }
    }
    pixels
}

fn calc_right_top_edge_drawing_pixels(image_params: &ImageParams) -> Pixels {
    let (w, h, r) = (
        image_params.width as i32,
        image_params.height as i32,
        image_params.corner_radius() as i32,
    );
    let (x_offset, y_offset) = if w < h {
        (0, r - (h / 2))
    } else {
        ((w / 2) - r, 0)
    };
    calc_corner_edge_drawing_pixels(image_params, 0, h, x_offset, y_offset)
}

fn calc_left_top_edge_drawing_pixels(image_params: &ImageParams) -> Pixels {
    let (w, h, r) = (
        image_params.width as i32,
        image_params.height as i32,
        image_params.corner_radius() as i32,
    );
    let (x_offset, y_offset) = if w < h {
        (0, r - (h / 2))
    } else {
        (r - (w / 2), 0)
    };
    calc_corner_edge_drawing_pixels(image_params, w, h, x_offset, y_offset)
}

fn calc_right_bottom_edge_drawing_pixels(image_params: &ImageParams) -> Pixels {
    let (w, h, r) = (
        image_params.width as i32,
        image_params.height as i32,
        image_params.corner_radius() as i32,
    );
    let (x_offset, y_offset) = if w < h {
        (0, (h / 2) - r)
    } else {
        ((w / 2) - r, 0)
    };
    calc_corner_edge_drawing_pixels(image_params, 0, 0, x_offset, y_offset)
}

fn calc_left_bottom_edge_drawing_pixels(image_params: &ImageParams) -> Pixels {
    let (w, h, r) = (
        image_params.width as i32,
        image_params.height as i32,
        image_params.corner_radius() as i32,
    );
    let (x_offset, y_offset) = if w < h {
        (0, (h / 2) - r)
    } else {
        (r - (w / 2), 0)
    };
    calc_corner_edge_drawing_pixels(image_params, w, 0, x_offset, y_offset)
}

fn calc_corner_edge_drawing_pixels(
    image_params: &ImageParams,
    base_center_x: i32,
    base_center_y: i32,
    x_offset: i32,
    y_offset: i32,
) -> Pixels {
    // Bresenham's circle algorithm
    let curve_center_x = base_center_x;
    let curve_center_y = base_center_y;
    let line_width = image_params.line_width as i32;
    let half_line_width = line_width / 2;
    let adjust = if image_params.line_width.is_multiple_of(2) {
        0
    } else {
        1
    };
    let radius_base_length = image_params.corner_radius() as i32;
    let inner_radius = radius_base_length - half_line_width - adjust;
    let outer_radius = radius_base_length + half_line_width;

    let mut x = inner_radius;
    let mut y = 0;
    let mut p = 1 - inner_radius;

    let mut inner_pixels = Pixels::default();

    while x >= y {
        for dx in -x..=x {
            inner_pixels.insert((curve_center_x + dx, curve_center_y + y));
            inner_pixels.insert((curve_center_x + dx, curve_center_y - y));
        }
        for dx in -y..=y {
            inner_pixels.insert((curve_center_x + dx, curve_center_y + x));
            inner_pixels.insert((curve_center_x + dx, curve_center_y - x));
        }

        y += 1;
        if p <= 0 {
            p += 2 * y + 1;
        } else {
            x -= 1;
            p += 2 * y - 2 * x + 1;
        }
    }

    let mut x = outer_radius;
    let mut y = 0;
    let mut p = 1 - outer_radius;

    let mut outer_pixels = Pixels::default();

    while x >= y {
        for dx in -x..=x {
            outer_pixels.insert((curve_center_x + dx, curve_center_y + y));
            outer_pixels.insert((curve_center_x + dx, curve_center_y - y));
        }
        for dx in -y..=y {
            outer_pixels.insert((curve_center_x + dx, curve_center_y + x));
            outer_pixels.insert((curve_center_x + dx, curve_center_y - x));
        }

        y += 1;
        if p <= 0 {
            p += 2 * y + 1;
        } else {
            x -= 1;
            p += 2 * y - 2 * x + 1;
        }
    }

    let mut pixels: Pixels = outer_pixels
        .difference(&inner_pixels)
        .filter(|p| {
            p.0 >= 0
                && p.0 < image_params.width as i32
                && p.1 >= 0
                && p.1 < image_params.height as i32
        })
        .map(|p| (p.0 + x_offset, p.1 + y_offset))
        .collect();

    if image_params.width < image_params.height {
        let (ys, ye) = if y_offset < 0 {
            (base_center_y + y_offset, base_center_y)
        } else {
            (base_center_y, base_center_y + y_offset)
        };
        let center_x = (image_params.width / 2) as i32;
        let x_start = center_x - line_width / 2;
        for x in x_start..(x_start + line_width) {
            for y in ys..ye {
                pixels.insert((x, y));
            }
        }
    }
    if image_params.width > image_params.height {
        let (xs, xe) = if x_offset < 0 {
            (base_center_x + x_offset, base_center_x)
        } else {
            (base_center_x, base_center_x + x_offset)
        };
        let center_y = (image_params.height / 2) as i32;
        let y_start = center_y - line_width / 2;
        for y in y_start..(y_start + line_width) {
            for x in xs..xe {
                pixels.insert((x, y));
            }
        }
    }

    pixels
}

pub fn calc_graph_row_image(
    commit_pos_x: usize,
    cell_count: usize,
    edges: &[Edge],
    image_params: &ImageParams,
    drawing_pixels: &DrawingPixels,
    graph_style: GraphStyle,
    branch_segments: &[crate::graph::calc::BranchSegment],
    pos_y: usize,
    head: bool,
    is_stash: bool,
    is_uncommitted: bool,
    commit_color: image::Rgba<u8>,
    uncommitted_lane: Option<(usize, usize, image::Rgba<u8>)>,
) -> GraphRowImage {
    let image_width = (image_params.width as usize * cell_count) as u32;
    let image_height = image_params.height as u32;

    let mut img_buf = image::ImageBuffer::new(image_width, image_height);

    draw_background(&mut img_buf, image_params);

    // Draw edges FIRST so that commit circles are rendered on top (matching VS Code Git Graph
    // where SVG circles have higher z-index than path elements).
    let edge_color = |edge: &Edge| -> image::Rgba<u8> {
        if is_uncommitted {
            commit_color
        } else {
            image_params.edge_color(edge.associated_line_pos_x)
        }
    };

    match graph_style {
        GraphStyle::Rounded => {
            for edge in edges {
                draw_edge(
                    &mut img_buf,
                    edge,
                    image_params,
                    drawing_pixels,
                    edge_color(edge),
                )
            }
        }
        GraphStyle::Angular => {
            let (vertial_edges, horizontal_edges): (Vec<&Edge>, Vec<&Edge>) = edges
                .iter()
                .partition(|e| e.edge_type.is_vertically_related());
            for edge in vertial_edges {
                draw_edge(
                    &mut img_buf,
                    edge,
                    image_params,
                    drawing_pixels,
                    edge_color(edge),
                )
            }
            let mut horizontal_edges_map: FxHashMap<usize, Vec<&Edge>> = FxHashMap::default();
            for edge in horizontal_edges {
                horizontal_edges_map
                    .entry(edge.associated_line_pos_x)
                    .or_default()
                    .push(edge);
            }
            for edges in horizontal_edges_map.values() {
                draw_diagonal_connected_edge(&mut img_buf, edges, image_params);
            }
        }
        GraphStyle::Smooth => {
            let is_uncommitted_segment = |s: &&crate::graph::calc::BranchSegment| -> bool {
                if let Some((_, head_y, _)) = uncommitted_lane {
                    s.is_uncommitted && s.source_pos_y <= head_y
                } else {
                    false
                }
            };
            let uncommitted_continuation_color = |s: &crate::graph::calc::BranchSegment| {
                let (_, head_y, _) = uncommitted_lane?;
                if !s.is_uncommitted || s.source_pos_y != head_y {
                    return None;
                }
                branch_segments
                    .iter()
                    .find(|other| {
                        !other.is_uncommitted
                            && other.source_pos_y == s.target_pos_y
                            && other.source_pos_x == s.target_pos_x
                    })
                    .map(|other| image_params.edge_color(other.color_index))
            };

            let mut segs: Vec<_> = branch_segments.iter().collect();
            // Uncommitted segments first (behind); then sort by rightmost column.
            segs.sort_by_key(|s| {
                (
                    !is_uncommitted_segment(s) as usize,
                    s.source_pos_x.max(s.target_pos_x),
                )
            });

            for seg in segs {
                let min_y = seg.target_pos_y.min(seg.source_pos_y);
                let max_y = seg.target_pos_y.max(seg.source_pos_y);
                if pos_y < min_y || pos_y > max_y {
                    continue;
                }
                let color_override = if is_uncommitted {
                    Some(commit_color)
                } else if let Some(color) = uncommitted_continuation_color(seg) {
                    Some(color)
                } else if is_uncommitted_segment(&seg) {
                    uncommitted_lane.map(|(_, _, c)| c)
                } else {
                    None
                };
                draw_smooth_bezier_segment(
                    &mut img_buf,
                    seg,
                    pos_y,
                    image_params,
                    cell_count,
                    color_override,
                );
            }
        }
    }

    // Overlay for Rounded/Angular only (Smooth handles coloring via color_override above).
    if graph_style != GraphStyle::Smooth {
        if let Some((lane_x, head_y, lane_color)) = uncommitted_lane {
            draw_uncommitted_overlay(
                &mut img_buf,
                lane_x,
                head_y,
                pos_y,
                image_params,
                lane_color,
            );
        }
    }

    // Draw commit circle on top of edges (mirrors VS Code Git Graph SVG z-ordering)
    let node_color = if graph_style == GraphStyle::Smooth || is_uncommitted || is_stash {
        commit_color
    } else {
        image_params.edge_color(commit_pos_x)
    };
    if head {
        draw_head_commit(
            &mut img_buf,
            commit_pos_x,
            image_params,
            drawing_pixels,
            node_color,
        );
    } else if is_stash {
        draw_stash_commit(
            &mut img_buf,
            commit_pos_x,
            image_params,
            drawing_pixels,
            node_color,
        );
    } else if is_uncommitted {
        draw_hollow_circle(
            &mut img_buf,
            commit_pos_x,
            image_params,
            drawing_pixels,
            node_color,
        );
    } else {
        draw_commit_circle(
            &mut img_buf,
            commit_pos_x,
            image_params,
            drawing_pixels,
            node_color,
        );
    }

    let bytes = build_image(&img_buf, image_width, image_height);

    GraphRowImage { bytes, cell_count }
}

fn draw_uncommitted_overlay(
    img_buf: &mut image::ImageBuffer<image::Rgba<u8>, Vec<u8>>,
    lane_x: usize,
    head_y: usize,
    pos_y: usize,
    image_params: &ImageParams,
    color: image::Rgba<u8>,
) {
    if pos_y == 0 || pos_y > head_y {
        return;
    }

    let cell_width = image_params.width as i32;
    let cell_height = image_params.height as i32;
    let x = lane_x as i32 * cell_width + cell_width / 2;
    let radius = (image_params.line_width as i32).max(1) / 2;

    if pos_y < head_y {
        for py in 0..cell_height {
            draw_filled_circle(img_buf, x, py, radius, color);
        }
    } else {
        let center_y = cell_height / 2;
        let circle_outer_radius = image_params.circle_outer_radius as i32;
        let y_end = (center_y - circle_outer_radius).max(0);
        for py in 0..y_end {
            draw_filled_circle(img_buf, x, py, radius, color);
        }
    }
}

#[allow(dead_code)]
fn ratatui_color_to_rgba(color: ratatui::style::Color) -> image::Rgba<u8> {
    match color {
        ratatui::style::Color::Rgb(r, g, b) => image::Rgba([r, g, b, 0xff]),
        ratatui::style::Color::Gray => image::Rgba([0x80, 0x80, 0x80, 0xff]),
        ratatui::style::Color::DarkGray => image::Rgba([0x40, 0x40, 0x40, 0xff]),
        ratatui::style::Color::White => image::Rgba([0xff, 0xff, 0xff, 0xff]),
        ratatui::style::Color::Black => image::Rgba([0x00, 0x00, 0x00, 0xff]),
        ratatui::style::Color::Red => image::Rgba([0xff, 0x00, 0x00, 0xff]),
        ratatui::style::Color::Green => image::Rgba([0x00, 0xff, 0x00, 0xff]),
        ratatui::style::Color::Yellow => image::Rgba([0xff, 0xff, 0x00, 0xff]),
        ratatui::style::Color::Blue => image::Rgba([0x00, 0x00, 0xff, 0xff]),
        ratatui::style::Color::Magenta => image::Rgba([0xff, 0x00, 0xff, 0xff]),
        ratatui::style::Color::Cyan => image::Rgba([0x00, 0xff, 0xff, 0xff]),
        _ => image::Rgba([0xc0, 0xca, 0xf5, 0xff]),
    }
}

fn draw_stash_commit(
    img_buf: &mut image::ImageBuffer<image::Rgba<u8>, Vec<u8>>,
    commit_pos_x: usize,
    image_params: &ImageParams,
    drawing_pixels: &DrawingPixels,
    color: image::Rgba<u8>,
) {
    draw_hollow_circle(img_buf, commit_pos_x, image_params, drawing_pixels, color);
    let x_offset = (commit_pos_x * image_params.width as usize) as i32;
    let center_x = x_offset + (image_params.width as i32 / 2);
    let center_y = (image_params.height / 2) as i32;
    let dot_radius = (image_params.line_width as i32).max(1);
    draw_filled_circle(img_buf, center_x, center_y, dot_radius, color);
}

fn draw_hollow_circle(
    img_buf: &mut image::ImageBuffer<image::Rgba<u8>, Vec<u8>>,
    circle_pos_x: usize,
    image_params: &ImageParams,
    drawing_pixels: &DrawingPixels,
    color: image::Rgba<u8>,
) {
    let x_offset = (circle_pos_x * image_params.width as usize) as i32;
    let bg = image_params.background_color;

    for (x, y) in &drawing_pixels.circle_gap {
        let px = (*x + x_offset) as u32;
        let py = *y as u32;
        if px < img_buf.width() && py < img_buf.height() {
            *img_buf.get_pixel_mut(px, py) = bg;
        }
    }

    // Fill the interior with background color so underlying curves are hidden.
    for (x, y) in &drawing_pixels.circle {
        let x = (*x + x_offset) as u32;
        let y = *y as u32;
        let pixel = img_buf.get_pixel_mut(x, y);
        *pixel = bg;
    }

    for (x, y) in &drawing_pixels.circle_edge {
        let x = (*x + x_offset) as u32;
        let y = *y as u32;

        let pixel = img_buf.get_pixel_mut(x, y);
        *pixel = color;
    }
}

fn draw_head_commit(
    img_buf: &mut image::ImageBuffer<image::Rgba<u8>, Vec<u8>>,
    circle_pos_x: usize,
    image_params: &ImageParams,
    drawing_pixels: &DrawingPixels,
    color: image::Rgba<u8>,
) {
    let x_offset = (circle_pos_x * image_params.width as usize) as i32;
    let bg = image_params.background_color;

    for (x, y) in &drawing_pixels.circle_gap {
        let px = (*x + x_offset) as u32;
        let py = *y as u32;
        if px < img_buf.width() && py < img_buf.height() {
            *img_buf.get_pixel_mut(px, py) = bg;
        }
    }

    // Fill the interior with background color so underlying curves are hidden.
    for (x, y) in &drawing_pixels.circle {
        let x = (*x + x_offset) as u32;
        let y = *y as u32;
        let pixel = img_buf.get_pixel_mut(x, y);
        *pixel = bg;
    }

    for (x, y) in &drawing_pixels.circle_edge {
        let x = (*x + x_offset) as u32;
        let y = *y as u32;

        let pixel = img_buf.get_pixel_mut(x, y);
        *pixel = color;
    }
}

fn draw_background(
    img_buf: &mut image::ImageBuffer<image::Rgba<u8>, Vec<u8>>,
    image_params: &ImageParams,
) {
    if image_params.background_color[3] == 0 {
        // If the alpha value is 0, the background is transparent, so we don't need to draw it.
        return;
    }
    for pixel in img_buf.pixels_mut() {
        *pixel = image_params.background_color;
    }
}

fn draw_commit_circle(
    img_buf: &mut image::ImageBuffer<image::Rgba<u8>, Vec<u8>>,
    circle_pos_x: usize,
    image_params: &ImageParams,
    drawing_pixels: &DrawingPixels,
    color: image::Rgba<u8>,
) {
    let x_offset = (circle_pos_x * image_params.width as usize) as i32;

    for (x, y) in &drawing_pixels.commit_circle_gap {
        let px = (*x + x_offset) as u32;
        let py = *y as u32;
        if px < img_buf.width() && py < img_buf.height() {
            *img_buf.get_pixel_mut(px, py) = image_params.background_color;
        }
    }

    for (x, y) in &drawing_pixels.circle {
        let x = (*x + x_offset) as u32;
        let y = *y as u32;

        let pixel = img_buf.get_pixel_mut(x, y);
        *pixel = color;
    }

    if image_params.circle_edge_color[3] == 0 {
        // If the alpha value is 0, the circle edge is transparent, so we don't need to draw it.
        return;
    }

    for (x, y) in &drawing_pixels.circle_edge {
        let x = (*x + x_offset) as u32;
        let y = *y as u32;

        let pixel = img_buf.get_pixel_mut(x, y);
        *pixel = image_params.circle_edge_color;
    }
}

fn draw_edge(
    img_buf: &mut image::ImageBuffer<image::Rgba<u8>, Vec<u8>>,
    edge: &Edge,
    image_params: &ImageParams,
    drawing_pixels: &DrawingPixels,
    color: image::Rgba<u8>,
) {
    let pixels = match edge.edge_type {
        EdgeType::Vertical => &drawing_pixels.vertical_edge,
        EdgeType::Horizontal => &drawing_pixels.horizontal_edge,
        EdgeType::Up => &drawing_pixels.up_edge,
        EdgeType::Down => &drawing_pixels.down_edge,
        EdgeType::Left => &drawing_pixels.left_edge,
        EdgeType::Right => &drawing_pixels.right_edge,
        EdgeType::RightTop => &drawing_pixels.right_top_edge,
        EdgeType::RightBottom => &drawing_pixels.right_bottom_edge,
        EdgeType::LeftTop => &drawing_pixels.left_top_edge,
        EdgeType::LeftBottom => &drawing_pixels.left_bottom_edge,
    };

    let x_offset = (edge.pos_x * image_params.width as usize) as i32;

    for (x, y) in pixels {
        let x = (*x + x_offset) as u32;
        let y = *y as u32;

        let pixel = img_buf.get_pixel_mut(x, y);
        *pixel = color;
    }
}

// fixme: cache edge drawing range calculations
fn draw_diagonal_connected_edge(
    img_buf: &mut image::ImageBuffer<image::Rgba<u8>, Vec<u8>>,
    edges: &[&Edge],
    image_params: &ImageParams,
) {
    let corner_edges = edges.iter().filter(|e| {
        matches!(
            e.edge_type,
            EdgeType::RightBottom | EdgeType::LeftBottom | EdgeType::RightTop | EdgeType::LeftTop
        )
    });

    for corner_edge in corner_edges {
        let expected_side_edge_type = match corner_edge.edge_type {
            EdgeType::RightBottom | EdgeType::RightTop => EdgeType::Right,
            EdgeType::LeftBottom | EdgeType::LeftTop => EdgeType::Left,
            _ => unreachable!("unexpected edge type for corner edge"),
        };
        let side_edge_opt = edges
            .iter()
            .find(|e| e.edge_type == expected_side_edge_type);
        // No side edge found, nothing to draw (should not happen)
        if let Some(side_edge) = side_edge_opt {
            let line_width_f64 = image_params.line_width as f64;
            let line_width_i32 = image_params.line_width as i32;

            // NOTE: Select y_offset of the corner edge based on the cell width.
            // The hard-coded value `height / 10.0` is based on the assumption that the cell
            // has a 1:1 aspect ratio, and does not work well for non-1:1 ratios.
            let y_offset = if image_params.width == image_params.height {
                image_params.height as f64 / 10.0
            } else {
                image_params.height as f64 / 2.0 - image_params.corner_radius() as f64
            };

            match corner_edge.edge_type {
                EdgeType::RightBottom | EdgeType::LeftBottom => {
                    let start_pos_center = Point::new(
                        (side_edge.pos_x * image_params.width as usize) as f64
                            + (image_params.width as f64 / 2.0),
                        image_params.height as f64 / 2.0,
                    );
                    let end_pos_center = Point::new(
                        (corner_edge.pos_x * image_params.width as usize) as f64
                            + (image_params.width as f64 / 2.0),
                        y_offset,
                    );

                    let line_vec = end_pos_center - start_pos_center;
                    let unit_vec = line_vec.normalize();
                    let normal_vec = unit_vec.perpendicular();

                    let line_start =
                        start_pos_center + unit_vec * (image_params.circle_outer_radius as f64);
                    let line_start_1 = line_start + normal_vec * (line_width_f64 / 2.0);
                    let line_start_2 = line_start - normal_vec * (line_width_f64 / 2.0);

                    let half_width = line_width_f64 / 2.0;
                    let slope = unit_vec.y / unit_vec.x;

                    let vertical_left_x = end_pos_center.x - half_width;
                    let vertical_right_x = end_pos_center.x + half_width;

                    let corner_1 = Point::new(
                        vertical_right_x,
                        line_start_1.y + slope * (vertical_right_x - line_start_1.x),
                    );
                    let corner_2 = Point::new(
                        vertical_left_x,
                        line_start_2.y + slope * (vertical_left_x - line_start_2.x),
                    );

                    let vertices = [line_start_1, corner_1, corner_2, line_start_2];

                    let (min_x, min_y, max_x, max_y) = bounding_box_u32(&vertices);
                    for y in min_y..max_y {
                        for x in min_x..max_x {
                            if x < img_buf.width() && y < img_buf.height() {
                                let p = Point::new(x as f64 + 0.5, y as f64 + 0.5);

                                if p.is_inside_polygon(&vertices) {
                                    let pixel = img_buf.get_pixel_mut(x, y);
                                    let color =
                                        image_params.edge_color(side_edge.associated_line_pos_x);
                                    *pixel = color;
                                }
                            }
                        }
                    }

                    let y_end = corner_1.y.max(corner_2.y) as u32;
                    let end_center_x_i32 = end_pos_center.x as i32;
                    let x_start = end_center_x_i32 - line_width_i32 / 2;
                    for y in 0..y_end {
                        for i in 0..line_width_i32 {
                            let x = (x_start + i) as u32;
                            if x < img_buf.width() && y < img_buf.height() {
                                let pixel = img_buf.get_pixel_mut(x, y);
                                let color =
                                    image_params.edge_color(side_edge.associated_line_pos_x);
                                *pixel = color;
                            }
                        }
                    }
                }
                EdgeType::RightTop | EdgeType::LeftTop => {
                    let start_pos_center = Point::new(
                        (side_edge.pos_x * image_params.width as usize) as f64
                            + (image_params.width as f64 / 2.0),
                        image_params.height as f64 / 2.0,
                    );
                    let end_pos_center = Point::new(
                        (corner_edge.pos_x * image_params.width as usize) as f64
                            + (image_params.width as f64 / 2.0),
                        image_params.height as f64 - y_offset,
                    );

                    let line_vec = end_pos_center - start_pos_center;
                    let unit_vec = line_vec.normalize();
                    let normal_vec = unit_vec.perpendicular();

                    let line_start =
                        start_pos_center + unit_vec * (image_params.circle_outer_radius as f64);
                    let line_start_1 = line_start + normal_vec * (line_width_f64 / 2.0);
                    let line_start_2 = line_start - normal_vec * (line_width_f64 / 2.0);

                    let half_width = line_width_f64 / 2.0;
                    let slope = unit_vec.y / unit_vec.x;

                    let vertical_left_x = end_pos_center.x - half_width;
                    let vertical_right_x = end_pos_center.x + half_width;

                    let corner_1 = Point::new(
                        vertical_left_x,
                        line_start_1.y + slope * (vertical_left_x - line_start_1.x),
                    );
                    let corner_2 = Point::new(
                        vertical_right_x,
                        line_start_2.y + slope * (vertical_right_x - line_start_2.x),
                    );

                    let vertices = [line_start_1, corner_1, corner_2, line_start_2];

                    let (min_x, min_y, max_x, max_y) = bounding_box_u32(&vertices);
                    for y in min_y..max_y {
                        for x in min_x..max_x {
                            if x < img_buf.width() && y < img_buf.height() {
                                let p = Point::new(x as f64 + 0.5, y as f64 + 0.5);

                                if p.is_inside_polygon(&vertices) {
                                    let pixel = img_buf.get_pixel_mut(x, y);
                                    let color =
                                        image_params.edge_color(side_edge.associated_line_pos_x);
                                    *pixel = color;
                                }
                            }
                        }
                    }

                    let y_start = corner_1.y.min(corner_2.y) as u32;
                    let end_center_x_i32 = end_pos_center.x as i32;
                    let x_start = end_center_x_i32 - line_width_i32 / 2;
                    for y in (y_start + 1)..image_params.height as u32 {
                        for i in 0..line_width_i32 {
                            let x = (x_start + i) as u32;
                            if x < img_buf.width() && y < img_buf.height() {
                                let pixel = img_buf.get_pixel_mut(x, y);
                                let color =
                                    image_params.edge_color(side_edge.associated_line_pos_x);
                                *pixel = color;
                            }
                        }
                    }
                }
                _ => unreachable!("unexpected edge type for corner edge"),
            }
        }
    }
}

fn draw_smooth_bezier_segment(
    img_buf: &mut image::ImageBuffer<image::Rgba<u8>, Vec<u8>>,
    segment: &crate::graph::calc::BranchSegment,
    row_y: usize,
    image_params: &ImageParams,
    _cell_count: usize,
    color_override: Option<image::Rgba<u8>>,
) {
    let cell_width = image_params.width as f32;
    let cell_height = image_params.height as f32;
    let image_height = image_params.height as i32;
    // VS Code Git Graph uses thicker curves than standard edges.
    // Minimum radius 2 gives a 5-pixel-wide curve (visible and smooth).
    let radius = ((image_params.line_width as i32).max(2) - 1) / 2;
    let radius = radius.max(2);
    let color = color_override.unwrap_or_else(|| image_params.edge_color(segment.color_index));

    // Absolute pixel coordinates of commit centers.
    // In VS Code Git Graph, curves connect commit centers. The "exits from top/bottom"
    // effect is achieved by control-point overshoot, not by starting from cell edges.
    let p0x = segment.target_pos_x as f32 * cell_width + cell_width / 2.0;
    let p0y = segment.target_pos_y as f32 * cell_height + cell_height / 2.0;
    let p3x = segment.source_pos_x as f32 * cell_width + cell_width / 2.0;
    let p3y = segment.source_pos_y as f32 * cell_height + cell_height / 2.0;

    // S-curve: d scales with total segment height so the Bezier tangent exits
    // each circle vertically for a long visible straight portion before curving.
    // For vertical segments (same column), no curve needed.
    let is_vertical = segment.source_pos_x == segment.target_pos_x;
    let pixel_distance = (p0y - p3y).abs();
    let d = if is_vertical {
        0.0
    } else {
        pixel_distance * 0.9
    };
    let p1x = p0x;
    let p1y = p0y + d; // control point below child center (short overshoot)
    let p2x = p3x;
    let p2y = p3y - d; // control point above parent center (short overshoot)

    // Row boundaries in absolute pixel coordinates
    let row_abs_top = row_y as f32 * cell_height;

    // Sample the global Bézier and clip to this row's pixel range.
    // Pure sampling handles the non-monotone y(t) of the S-curve correctly — no inversion needed.
    let steps = 500;
    let margin = radius + 2;
    let mut prev: Option<(i32, i32)> = None;

    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let mt = 1.0 - t;

        let abs_x =
            mt * mt * mt * p0x + 3.0 * mt * mt * t * p1x + 3.0 * mt * t * t * p2x + t * t * t * p3x;
        let abs_y =
            mt * mt * mt * p0y + 3.0 * mt * mt * t * p1y + 3.0 * mt * t * t * p2y + t * t * t * p3y;

        // Local row coordinates: y=0 at top of this row's image
        let lx = abs_x as i32;
        let ly = (abs_y - row_abs_top) as i32;

        let near = ly >= -margin && ly < image_height + margin;

        if let Some((px, py)) = prev {
            let prev_near = py >= -margin && py < image_height + margin;
            if near || prev_near {
                draw_thick_line(img_buf, px, py, lx, ly, radius, color);
            }
        }
        if ly >= 0 && ly < image_height {
            draw_filled_circle(img_buf, lx, ly, radius, color);
        }

        prev = Some((lx, ly));
    }
}

fn draw_filled_circle(
    img_buf: &mut image::ImageBuffer<image::Rgba<u8>, Vec<u8>>,
    cx: i32,
    cy: i32,
    r: i32,
    color: image::Rgba<u8>,
) {
    for dy in -r..=r {
        for dx in -r..=r {
            if dx * dx + dy * dy <= r * r + r {
                put_pixel_safe(img_buf, cx + dx, cy + dy, color);
            }
        }
    }
}

fn draw_thick_line(
    img_buf: &mut image::ImageBuffer<image::Rgba<u8>, Vec<u8>>,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    radius: i32,
    color: image::Rgba<u8>,
) {
    let dx = (x1 - x0).abs();
    let dy = (y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx - dy;
    let mut x = x0;
    let mut y = y0;

    loop {
        draw_filled_circle(img_buf, x, y, radius, color);
        if x == x1 && y == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 > -dy {
            err -= dy;
            x += sx;
        }
        if e2 < dx {
            err += dx;
            y += sy;
        }
    }
}

fn put_pixel_safe(
    img_buf: &mut image::ImageBuffer<image::Rgba<u8>, Vec<u8>>,
    x: i32,
    y: i32,
    color: image::Rgba<u8>,
) {
    if x >= 0 && x < img_buf.width() as i32 && y >= 0 && y < img_buf.height() as i32 {
        img_buf.put_pixel(x as u32, y as u32, color);
    }
}

fn build_image(img_buf: &[u8], image_width: u32, image_height: u32) -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    image::write_buffer_with_format(
        &mut bytes,
        img_buf,
        image_width,
        image_height,
        image::ColorType::Rgba8,
        image::ImageFormat::Png,
    )
    .unwrap();
    bytes.into_inner()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use image::GenericImage;
    use rstest::rstest;

    use crate::config::GraphColorConfig;

    use super::*;
    use crate::graph::calc::BranchSegment;
    use EdgeType::*;

    const OUTPUT_DIR: &str = "./out/ut/graph/image";

    type TestParam = (usize, Vec<(EdgeType, usize, usize)>);

    fn test_image_params() -> ImageParams {
        ImageParams {
            width: 50,
            height: 56,
            line_width: 5,
            circle_inner_radius: 11,
            circle_outer_radius: 14,
            edge_colors: vec![
                image::Rgba([0x00, 0x7a, 0xcc, 0xff]),
                image::Rgba([0xff, 0x00, 0xff, 0xff]),
            ],
            circle_edge_color: image::Rgba([0xc0, 0xca, 0xf5, 0xff]),
            background_color: image::Rgba([0x20, 0x22, 0x30, 0xff]),
        }
    }

    fn decode_graph_row(row: &GraphRowImage) -> image::RgbaImage {
        image::load_from_memory(&row.bytes).unwrap().to_rgba8()
    }

    #[test]
    fn smooth_branch_segment_on_uncommitted_column_keeps_own_color() {
        let image_params = test_image_params();
        let drawing_pixels = DrawingPixels::new(&image_params);
        let segment = BranchSegment {
            source_pos_x: 0,
            target_pos_x: 0,
            source_pos_y: 1,
            target_pos_y: 0,
            color_index: 0,
            is_branch: true,
            is_uncommitted: false,
        };

        let row = calc_graph_row_image(
            1,
            2,
            &[],
            &image_params,
            &drawing_pixels,
            GraphStyle::Smooth,
            &[segment],
            1,
            false,
            false,
            false,
            image_params.edge_color(1),
            Some((0, 1, image::Rgba([0x80, 0x80, 0x80, 0xff]))),
        );
        let img = decode_graph_row(&row);

        assert_eq!(
            *img.get_pixel(25, 8),
            image_params.edge_color(0),
            "non-uncommitted branches must not be recolored gray just because they share the uncommitted column"
        );
    }

    #[test]
    fn commit_circle_uses_branch_color_instead_of_column_color() {
        let mut image_params = test_image_params();
        image_params.circle_edge_color = image::Rgba([0x00, 0x00, 0x00, 0x00]);
        let drawing_pixels = DrawingPixels::new(&image_params);

        let row = calc_graph_row_image(
            1,
            2,
            &[],
            &image_params,
            &drawing_pixels,
            GraphStyle::Smooth,
            &[],
            0,
            false,
            false,
            false,
            image_params.edge_color(0),
            None,
        );
        let img = decode_graph_row(&row);
        let center_x = image_params.width as u32 + image_params.width as u32 / 2;
        let center_y = image_params.height as u32 / 2;

        assert_eq!(
            *img.get_pixel(center_x, center_y),
            image_params.edge_color(0),
            "commit circles should follow the branch color, not the lane color"
        );
    }

    #[test]
    fn rounded_commit_circle_uses_column_color() {
        let mut image_params = test_image_params();
        image_params.circle_edge_color = image::Rgba([0x00, 0x00, 0x00, 0x00]);
        let drawing_pixels = DrawingPixels::new(&image_params);

        let row = calc_graph_row_image(
            1,
            2,
            &[],
            &image_params,
            &drawing_pixels,
            GraphStyle::Rounded,
            &[],
            0,
            false,
            false,
            false,
            image_params.edge_color(0),
            None,
        );
        let img = decode_graph_row(&row);
        let center_x = image_params.width as u32 + image_params.width as u32 / 2;
        let center_y = image_params.height as u32 / 2;

        assert_eq!(
            *img.get_pixel(center_x, center_y),
            image_params.edge_color(1),
            "rounded nodes should keep legacy lane-based colors"
        );
    }

    #[test]
    fn smooth_hollow_circle_masks_one_pixel_gap_outside_edge() {
        let image_params = test_image_params();
        let drawing_pixels = DrawingPixels::new(&image_params);
        let segment = BranchSegment {
            source_pos_x: 0,
            target_pos_x: 0,
            source_pos_y: 1,
            target_pos_y: 0,
            color_index: 0,
            is_branch: true,
            is_uncommitted: false,
        };

        let row = calc_graph_row_image(
            0,
            1,
            &[],
            &image_params,
            &drawing_pixels,
            GraphStyle::Smooth,
            &[segment],
            0,
            true,
            false,
            false,
            image_params.edge_color(0),
            None,
        );
        let img = decode_graph_row(&row);
        let center_x = image_params.width as u32 / 2;
        let gap_y = image_params.height as u32 / 2 + image_params.circle_outer_radius as u32 + 1;

        assert_eq!(
            *img.get_pixel(center_x, gap_y),
            image_params.background_color,
            "hollow circles should mask a one-pixel background gap outside the circle edge"
        );
    }

    #[test]
    fn smooth_filled_circle_masks_one_pixel_gap_outside_visible_fill() {
        let mut image_params = test_image_params();
        image_params.circle_edge_color = image::Rgba([0x00, 0x00, 0x00, 0x00]);
        let drawing_pixels = DrawingPixels::new(&image_params);
        let segment = BranchSegment {
            source_pos_x: 0,
            target_pos_x: 0,
            source_pos_y: 1,
            target_pos_y: 0,
            color_index: 0,
            is_branch: true,
            is_uncommitted: false,
        };

        let row = calc_graph_row_image(
            0,
            1,
            &[],
            &image_params,
            &drawing_pixels,
            GraphStyle::Smooth,
            &[segment],
            0,
            false,
            false,
            false,
            image_params.edge_color(0),
            None,
        );
        let img = decode_graph_row(&row);
        let center_x = image_params.width as u32 / 2;
        let gap_y = image_params.height as u32 / 2 + image_params.circle_inner_radius as u32 + 1;

        assert_eq!(
            *img.get_pixel(center_x, gap_y),
            image_params.background_color,
            "filled circles should mask the line just outside the visible fill radius"
        );
    }

    #[test]
    fn smooth_uncommitted_branch_segment_below_head_keeps_own_color() {
        let image_params = test_image_params();
        let drawing_pixels = DrawingPixels::new(&image_params);
        let segment = BranchSegment {
            source_pos_x: 0,
            target_pos_x: 0,
            source_pos_y: 2,
            target_pos_y: 1,
            color_index: 0,
            is_branch: true,
            is_uncommitted: true,
        };

        let row = calc_graph_row_image(
            0,
            1,
            &[],
            &image_params,
            &drawing_pixels,
            GraphStyle::Smooth,
            &[segment],
            2,
            false,
            false,
            false,
            image_params.edge_color(0),
            Some((0, 1, image::Rgba([0x80, 0x80, 0x80, 0xff]))),
        );
        let img = decode_graph_row(&row);

        assert_eq!(
            *img.get_pixel(25, 8),
            image_params.edge_color(0),
            "only the uncommitted-to-HEAD range should be recolored gray"
        );
    }

    #[test]
    fn smooth_last_uncommitted_segment_uses_continuation_color_above_head() {
        let image_params = test_image_params();
        let drawing_pixels = DrawingPixels::new(&image_params);
        let uncommitted_segment = BranchSegment {
            source_pos_x: 0,
            target_pos_x: 0,
            source_pos_y: 11,
            target_pos_y: 10,
            color_index: 0,
            is_branch: true,
            is_uncommitted: true,
        };
        let continuation_segment = BranchSegment {
            source_pos_x: 0,
            target_pos_x: 1,
            source_pos_y: 10,
            target_pos_y: 9,
            color_index: 0,
            is_branch: true,
            is_uncommitted: false,
        };

        let row = calc_graph_row_image(
            0,
            2,
            &[],
            &image_params,
            &drawing_pixels,
            GraphStyle::Smooth,
            &[uncommitted_segment, continuation_segment],
            11,
            true,
            false,
            false,
            image_params.edge_color(0),
            Some((0, 11, image::Rgba([0x80, 0x80, 0x80, 0xff]))),
        );
        let img = decode_graph_row(&row);

        assert_eq!(
            *img.get_pixel(25, 8),
            image_params.edge_color(0),
            "the branch continuation above HEAD should color the shared lane segment"
        );
    }

    // Note: The output contents are not verified by the code.

    #[rstest]
    #[case("default_params_rounded", GraphStyle::Rounded)]
    #[case("default_params_angular", GraphStyle::Angular)]
    fn test_calc_graph_row_image_default_params(
        #[case] file_name: &str,
        #[case] graph_style: GraphStyle,
    ) {
        let params = simple_test_params();
        let cell_count = 4;
        let graph_color_config = GraphColorConfig::default();
        let graph_color_set = GraphColorSet::new(&graph_color_config);
        let cell_width_type = CellWidthType::Double;
        let image_params = ImageParams::new(&graph_color_set, cell_width_type);
        let drawing_pixels = DrawingPixels::new(&image_params);

        test_calc_graph_row_image(
            params,
            cell_count,
            image_params,
            drawing_pixels,
            graph_style,
            false,
            file_name,
        );
    }

    #[rstest]
    #[case("wide_image_rounded", GraphStyle::Rounded)]
    #[case("wide_image_angular", GraphStyle::Angular)]
    fn test_calc_graph_row_image_wide_image(
        #[case] file_name: &str,
        #[case] graph_style: GraphStyle,
    ) {
        let params = simple_test_params();
        let cell_count = 4;
        let graph_color_config = GraphColorConfig::default();
        let graph_color_set = GraphColorSet::new(&graph_color_config);
        let cell_width_type = CellWidthType::Double;
        let mut image_params = ImageParams::new(&graph_color_set, cell_width_type);
        image_params.width = 100;
        let drawing_pixels = DrawingPixels::new(&image_params);

        test_calc_graph_row_image(
            params,
            cell_count,
            image_params,
            drawing_pixels,
            graph_style,
            false,
            file_name,
        );
    }

    #[rstest]
    #[case("tall_image_rounded", GraphStyle::Rounded)]
    #[case("tall_image_angular", GraphStyle::Angular)]
    fn test_calc_graph_row_image_tall_image(
        #[case] file_name: &str,
        #[case] graph_style: GraphStyle,
    ) {
        let params = simple_test_params();
        let cell_count = 4;
        let graph_color_config = GraphColorConfig::default();
        let graph_color_set = GraphColorSet::new(&graph_color_config);
        let cell_width_type = CellWidthType::Double;
        let mut image_params = ImageParams::new(&graph_color_set, cell_width_type);
        image_params.height = 100;
        let drawing_pixels = DrawingPixels::new(&image_params);

        test_calc_graph_row_image(
            params,
            cell_count,
            image_params,
            drawing_pixels,
            graph_style,
            false,
            file_name,
        );
    }

    #[rstest]
    #[case("single_cell_width_rounded", GraphStyle::Rounded)]
    #[case("single_cell_width_angular", GraphStyle::Angular)]
    fn test_calc_graph_row_image_single_cell_width(
        #[case] file_name: &str,
        #[case] graph_style: GraphStyle,
    ) {
        let params = simple_test_params();
        let cell_count = 4;
        let graph_color_config = GraphColorConfig::default();
        let graph_color_set = GraphColorSet::new(&graph_color_config);
        let cell_width_type = CellWidthType::Single;
        let image_params = ImageParams::new(&graph_color_set, cell_width_type);
        let drawing_pixels = DrawingPixels::new(&image_params);

        test_calc_graph_row_image(
            params,
            cell_count,
            image_params,
            drawing_pixels,
            graph_style,
            false,
            file_name,
        );
    }

    #[rstest]
    #[case("circle_radius_rounded", GraphStyle::Rounded)]
    #[case("circle_radius_angular", GraphStyle::Angular)]
    fn test_calc_graph_row_image_circle_radius(
        #[case] file_name: &str,
        #[case] graph_style: GraphStyle,
    ) {
        let params = straight_test_params();
        let cell_count = 2;
        let graph_color_config = GraphColorConfig::default();
        let graph_color_set = GraphColorSet::new(&graph_color_config);
        let cell_width_type = CellWidthType::Double;
        let mut image_params = ImageParams::new(&graph_color_set, cell_width_type);
        image_params.circle_inner_radius = 5;
        image_params.circle_outer_radius = 12;
        let drawing_pixels = DrawingPixels::new(&image_params);

        test_calc_graph_row_image(
            params,
            cell_count,
            image_params,
            drawing_pixels,
            graph_style,
            false,
            file_name,
        );
    }

    #[rstest]
    #[case("line_width_rounded", GraphStyle::Rounded)]
    #[case("line_width_angular", GraphStyle::Angular)]
    fn test_calc_graph_row_image_line_width(
        #[case] file_name: &str,
        #[case] graph_style: GraphStyle,
    ) {
        let params = straight_test_params();
        let cell_count = 2;
        let graph_color_config = GraphColorConfig::default();
        let graph_color_set = GraphColorSet::new(&graph_color_config);
        let cell_width_type = CellWidthType::Double;
        let mut image_params = ImageParams::new(&graph_color_set, cell_width_type);
        image_params.line_width = 1;
        let drawing_pixels = DrawingPixels::new(&image_params);

        test_calc_graph_row_image(
            params,
            cell_count,
            image_params,
            drawing_pixels,
            graph_style,
            false,
            file_name,
        );
    }

    #[rstest]
    #[case("color_rounded", GraphStyle::Rounded)]
    #[case("color_angular", GraphStyle::Angular)]
    fn test_calc_graph_row_image_color(#[case] file_name: &str, #[case] graph_style: GraphStyle) {
        let params = branches_test_params();
        let cell_count = 7;
        let graph_color_config = GraphColorConfig {
            branches: vec![
                "#c8c864".into(),
                "#64c8c8".into(),
                "#646464".into(),
                "#c864c8".into(),
            ],
            edge: "#ffffff".into(),
            background: "#00ff0070".into(),
        };
        let graph_color_set = GraphColorSet::new(&graph_color_config);
        let cell_width_type = CellWidthType::Double;
        let image_params = ImageParams::new(&graph_color_set, cell_width_type);
        let drawing_pixels = DrawingPixels::new(&image_params);

        test_calc_graph_row_image(
            params,
            cell_count,
            image_params,
            drawing_pixels,
            graph_style,
            false,
            file_name,
        );
    }

    #[rustfmt::skip]
    fn simple_test_params() -> Vec<TestParam> {
        vec![
            (1, vec![(LeftBottom, 0, 0), (Left, 1, 0), (Down, 1, 1), (Right, 1, 3), (Horizontal, 2, 3), (RightBottom, 3, 3)]),
            (3, vec![(Vertical, 0, 0), (Up, 3, 3), (Down, 3, 3)]),
            (2, vec![(LeftTop, 0, 0), (Horizontal, 1, 0), (Left, 2, 0), (Up, 2, 2), (Right, 2, 3), (RightTop, 3, 3)]),
        ]
    }

    #[rustfmt::skip]
    fn straight_test_params() -> Vec<TestParam> {
        vec![
            (0, vec![(Up, 0, 0), (Down, 0, 0)]),
            (0, vec![(Up, 0, 0), (Down, 0, 0), (Right, 0, 1), (RightBottom, 1, 1)]),
            (1, vec![(Vertical, 0, 0), (Up, 1, 1), (Down, 1, 1)]),
            (0, vec![(Up, 0, 0), (Down, 0, 0), (Right, 0, 1), (RightTop, 1, 1)]),
        ]
    }

    #[rustfmt::skip]
    fn branches_test_params() -> Vec<TestParam> {
        vec![
            (0, vec![(Up, 0, 0), (Down, 0, 0),
                    (Right, 0, 1), (RightBottom, 1, 1),
                    (Right, 0, 2), (Horizontal, 1, 2), (RightBottom, 2, 2),
                    (Right, 0, 3), (Horizontal, 1, 3), (Horizontal, 2, 3), (RightBottom, 3, 3),
                    (Right, 0, 4), (Horizontal, 1, 4), (Horizontal, 2, 4), (Horizontal, 3, 4), (RightBottom, 4, 4),
                    (Right, 0, 5), (Horizontal, 1, 5), (Horizontal, 2, 5), (Horizontal, 3, 5), (Horizontal, 4, 5), (RightBottom, 5, 5),
                    (Right, 0, 6), (Horizontal, 1, 6), (Horizontal, 2, 6), (Horizontal, 3, 6), (Horizontal, 4, 6), (Horizontal, 5, 6), (RightBottom, 6, 6)]),
            (6, vec![(Vertical, 0, 0), (Vertical, 1, 1), (Vertical, 2, 2), (Vertical, 3, 3), (Vertical, 4, 4), (Vertical, 5, 5), (Down, 6, 6), (Up, 6, 6)]),
        ]
    }

    fn test_calc_graph_row_image(
        params: Vec<TestParam>,
        cell_count: usize,
        image_params: ImageParams,
        drawing_pixels: DrawingPixels,
        graph_style: GraphStyle,
        head: bool,
        file_name: &str,
    ) {
        let graph_row_images: Vec<GraphRowImage> = params
            .into_iter()
            .map(|(commit_pos_x, edges)| {
                let edges: Vec<Edge> = edges
                    .into_iter()
                    .map(|t| Edge::new(t.0, t.1, t.2))
                    .collect();
                calc_graph_row_image(
                    commit_pos_x,
                    cell_count,
                    &edges,
                    &image_params,
                    &drawing_pixels,
                    graph_style,
                    &[],
                    0,
                    head,
                    false,
                    false,
                    image::Rgba([0xc0, 0xca, 0xf5, 0xff]),
                    None,
                )
            })
            .collect();

        save_image(&graph_row_images, &image_params, cell_count, file_name);
    }

    fn save_image(
        graph_row_images: &[GraphRowImage],
        image_params: &ImageParams,
        cell_count: usize,
        file_name: &str,
    ) {
        let rows_len = graph_row_images.len() as u32;
        let image_width = image_params.width as u32 * cell_count as u32;
        let image_height = image_params.height as u32 * rows_len;

        let mut img_buf: image::ImageBuffer<image::Rgba<u8>, Vec<u8>> =
            image::ImageBuffer::new(image_width, image_height);

        for (i, graph_row_image) in graph_row_images.iter().enumerate() {
            let image = image::load_from_memory(&graph_row_image.bytes).unwrap();
            let y = image_params.height as u32 * (rows_len - (i as u32) - 1);
            img_buf.copy_from(&image, 0, y).unwrap();

            for x in 0..cell_count {
                let x_offset = x as u32 * image_params.width as u32;
                let y_offset = y;
                draw_border(&mut img_buf, image_params, x_offset, y_offset);
            }
        }

        create_output_dirs(OUTPUT_DIR);
        let file_name = format!("{OUTPUT_DIR}/{file_name}.png");
        image::save_buffer(
            file_name,
            &img_buf,
            image_width,
            image_height,
            image::ColorType::Rgba8,
        )
        .unwrap();
    }

    fn draw_border(
        img_buf: &mut image::ImageBuffer<image::Rgba<u8>, Vec<u8>>,
        image_params: &ImageParams,
        x_offset: u32,
        y_offset: u32,
    ) {
        for x in 0..image_params.width {
            for y in 0..image_params.height {
                if x == 0 || x == image_params.width - 1 || y == 0 || y == image_params.height - 1 {
                    img_buf.put_pixel(
                        x as u32 + x_offset,
                        y as u32 + y_offset,
                        image::Rgba([255, 0, 0, 50]),
                    );
                }
            }
        }
    }

    fn create_output_dirs(path: &str) {
        let path = Path::new(path);
        std::fs::create_dir_all(path).unwrap();
    }
}
