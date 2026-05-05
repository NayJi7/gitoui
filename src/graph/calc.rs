use rustc_hash::FxHashMap;

use crate::git::{Commit, CommitHash, CommitType, Repository};

type CommitPosMap<'a> = FxHashMap<&'a CommitHash, (usize, usize)>;

#[derive(Debug, Clone)]
pub struct BranchSegment {
    pub source_pos_x: usize,
    pub target_pos_x: usize,
    pub source_pos_y: usize,
    pub target_pos_y: usize,
    pub color_index: usize,
    pub is_branch: bool,
}

#[derive(Debug)]
pub struct Graph<'a> {
    pub commits: Vec<&'a Commit>,
    pub commit_pos_map: CommitPosMap<'a>,
    pub edges: Vec<Vec<Edge>>,
    pub max_pos_x: usize,
    pub branch_segments: Vec<BranchSegment>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Edge {
    pub edge_type: EdgeType,
    pub pos_x: usize,
    pub associated_line_pos_x: usize,
}

impl Edge {
    pub fn new(edge_type: EdgeType, pos_x: usize, line_pos_x: usize) -> Self {
        Self {
            edge_type,
            pos_x,
            associated_line_pos_x: line_pos_x,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub enum EdgeType {
    Vertical,    // │
    Horizontal,  // ─
    Up,          // ╵
    Down,        // ╷
    Left,        // ╴
    Right,       // ╶
    RightTop,    // ╮
    RightBottom, // ╯
    LeftTop,     // ╭
    LeftBottom,  // ╰
}

impl EdgeType {
    pub fn is_vertically_related(&self) -> bool {
        matches!(self, EdgeType::Vertical | EdgeType::Up | EdgeType::Down)
    }
}

// ── Internal layout types ──────────────────────────────────────────────────

#[derive(Clone, Copy, Debug)]
struct Pt { x: usize, y: usize }

#[derive(Debug)]
struct Connection {
    x: usize,
    connects_to: usize,   // vertex id (index into vertices); usize::MAX = no vertex
    on_branch: usize,     // branch id (index into branches)
}

struct LayoutVertex {
    id: usize,
    x: Option<usize>,           // None = not yet on a branch
    next_x: usize,
    connections: Vec<Connection>,
    parent_ids: Vec<usize>,     // indices into vertices; usize::MAX = parent not in graph
    child_ids: Vec<usize>,
    next_parent: usize,         // index into parent_ids currently being processed
    branch_id: Option<usize>,   // index into branches
    is_committed: bool,
}

impl LayoutVertex {
    fn point(&self) -> Pt { Pt { x: self.x.unwrap_or(0), y: self.id } }
    fn next_point(&self) -> Pt { Pt { x: self.next_x, y: self.id } }
    fn not_on_branch(&self) -> bool { self.branch_id.is_none() }
    fn is_merge(&self) -> bool { self.parent_ids.len() > 1 }

    fn add_to_branch(&mut self, branch_id: usize, x: usize) {
        if self.branch_id.is_none() {
            self.branch_id = Some(branch_id);
            self.x = Some(x);
        }
    }

    fn register_unavailable_point(&mut self, x: usize, connects_to: usize, on_branch: usize) {
        if x == self.next_x {
            self.connections.push(Connection { x, connects_to, on_branch });
            self.next_x += 1;
        }
    }

    fn get_point_connecting_to(&self, target_id: usize, branch_id: usize) -> Option<Pt> {
        self.connections.iter()
            .find(|c| c.connects_to == target_id && c.on_branch == branch_id)
            .map(|c| Pt { x: c.x, y: self.id })
    }

    fn register_parent_processed(&mut self) {
        self.next_parent += 1;
    }
}

#[derive(Debug)]
struct LayoutBranch {
    colour: usize,
    lines: Vec<(Pt, Pt)>,   // (p1=newer, p2=older) in grid coords
    end: usize,
}

impl LayoutBranch {
    fn new(colour: usize) -> Self {
        Self { colour, lines: Vec::new(), end: 0 }
    }
    fn add_line(&mut self, p1: Pt, p2: Pt) {
        self.lines.push((p1, p2));
    }
    fn set_end(&mut self, end: usize) {
        self.end = end;
    }
}

fn load_commits<'a>(
    commits: &[&'a Commit],
    _repository: &Repository,
) -> Vec<LayoutVertex> {
    // Build hash → index map
    let mut hash_to_id: FxHashMap<&CommitHash, usize> = FxHashMap::default();
    for (i, c) in commits.iter().enumerate() {
        hash_to_id.insert(&c.commit_hash, i);
    }

    let n = commits.len();

    // Create vertex for each commit
    let mut vertices: Vec<LayoutVertex> = commits
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let is_committed = !matches!(c.commit_type, CommitType::Uncommitted);
            LayoutVertex {
                id: i,
                x: None,
                next_x: 0,
                connections: Vec::new(),
                parent_ids: Vec::new(),
                child_ids: Vec::new(),
                next_parent: 0,
                branch_id: None,
                is_committed,
            }
        })
        .collect();

    // Wire parent/child links
    for i in 0..n {
        let parent_ids: Vec<usize> = commits[i]
            .parent_commit_hashes
            .iter()
            .map(|ph| hash_to_id.get(ph).copied().unwrap_or(usize::MAX))
            .collect();
        for &pid in &parent_ids {
            if pid != usize::MAX {
                vertices[pid].child_ids.push(i);
            }
        }
        vertices[i].parent_ids = parent_ids;
    }

    vertices
}

fn get_available_colour(start_at: usize, available_colours: &[usize]) -> usize {
    for (colour, &end) in available_colours.iter().enumerate() {
        if end <= start_at {
            return colour;
        }
    }
    available_colours.len() // allocate new colour index
}

fn determine_path(
    start_at: usize,
    vertices: &mut Vec<LayoutVertex>,
    branches: &mut Vec<LayoutBranch>,
    available_colours: &mut Vec<usize>,
) {
    // Extract info from start_at vertex before any mutation
    let (parent_id, v_not_on_branch, v_is_merge, v_on_branch) = {
        let v = &vertices[start_at];
        let pid = v.parent_ids.get(v.next_parent).copied().unwrap_or(usize::MAX);
        (pid, v.not_on_branch(), v.is_merge(), !v.not_on_branch())
    };

    // No parent in graph — mark processed and return
    if parent_id == usize::MAX {
        vertices[start_at].register_parent_processed();
        return;
    }

    let last_pt_start = if v_not_on_branch {
        vertices[start_at].next_point()
    } else {
        vertices[start_at].point()
    };

    let parent_on_branch = !vertices[parent_id].not_on_branch();

    if v_is_merge && v_on_branch && parent_on_branch {
        // ── MERGE PATH ──────────────────────────────────────────────────────
        // Route along the parent's existing branch
        let parent_branch_id = vertices[parent_id].branch_id.unwrap();
        let mut last_pt = last_pt_start;
        let mut found_point_to_parent = false;

        let mut i = start_at + 1;
        while i < vertices.len() {
            // Check if this vertex has a reserved connection point toward parent
            let conn_pt = { vertices[i].get_point_connecting_to(parent_id, parent_branch_id) };
            if conn_pt.is_some() {
                found_point_to_parent = true;
            }
            let cur_pt = conn_pt.unwrap_or_else(|| { vertices[i].next_point() });

            branches[parent_branch_id].add_line(last_pt, cur_pt);
            { vertices[i].register_unavailable_point(cur_pt.x, parent_id, parent_branch_id); }
            last_pt = cur_pt;

            if found_point_to_parent {
                vertices[start_at].register_parent_processed();
                break;
            }
            i += 1;
        }
        if !found_point_to_parent {
            // Parent connection point not found (shouldn't happen in a consistent graph)
            vertices[start_at].register_parent_processed();
        }
    } else {
        // ── NORMAL PATH ─────────────────────────────────────────────────────
        // Create a new branch and trace it from start_at down to parent
        let colour = get_available_colour(start_at, available_colours);
        while available_colours.len() <= colour {
            available_colours.push(0);
        }

        let branch_id = branches.len();
        branches.push(LayoutBranch::new(colour));

        // Place start_at vertex on this branch
        let last_pt_x = last_pt_start.x;
        vertices[start_at].add_to_branch(branch_id, last_pt_x);
        // No-op if vertex already on a branch (next_x already advanced past last_pt_x)
        vertices[start_at].register_unavailable_point(last_pt_x, start_at, branch_id);

        let mut last_pt = last_pt_start;
        let mut cur_vertex_id = start_at;
        let mut cur_parent_id = parent_id;

        let mut i = start_at + 1;
        while i < vertices.len() {
            // Determine where this row's connection point is
            let parent_already_placed = !vertices[cur_parent_id].not_on_branch();
            let cur_pt = if i == cur_parent_id && parent_already_placed {
                vertices[i].point()
            } else {
                vertices[i].next_point()
            };

            branches[branch_id].add_line(last_pt, cur_pt);
            vertices[i].register_unavailable_point(cur_pt.x, cur_parent_id, branch_id);
            last_pt = cur_pt;

            if i == cur_parent_id {
                vertices[cur_vertex_id].register_parent_processed();
                let parent_was_on_branch = !vertices[i].not_on_branch();
                vertices[i].add_to_branch(branch_id, cur_pt.x);
                cur_vertex_id = i;

                // Get next parent of the newly adopted vertex
                let next_pid = {
                    let v = &vertices[cur_vertex_id];
                    v.parent_ids.get(v.next_parent).copied().unwrap_or(usize::MAX)
                };

                if next_pid == usize::MAX || parent_was_on_branch {
                    // No more parents to trace, or parent was already placed
                    if next_pid == usize::MAX {
                        vertices[cur_vertex_id].register_parent_processed();
                    }
                    cur_parent_id = usize::MAX;
                    break;
                }
                cur_parent_id = next_pid;
            }
            i += 1;
        }

        // Handle case where we ran off the end (parent not in graph)
        if i == vertices.len() && cur_parent_id != usize::MAX {
            vertices[cur_vertex_id].register_parent_processed();
        }

        branches[branch_id].set_end(i);
        available_colours[colour] = i;
    }
}

pub fn calc_graph(repository: &Repository) -> Graph<'_> {
    let commits = repository.all_commits();
    let n = commits.len();

    // 1. Build layout vertices
    let mut vertices = load_commits(&commits, repository);

    // 2. Run determine_path for every vertex that has unprocessed parents
    let mut branches: Vec<LayoutBranch> = Vec::new();
    let mut available_colours: Vec<usize> = Vec::new();

    for i in 0..n {
        loop {
            let has_more = {
                let v = &vertices[i];
                v.next_parent < v.parent_ids.len()
            };
            if !has_more { break; }
            determine_path(i, &mut vertices, &mut branches, &mut available_colours);
        }
        // Place root commits (no parents processed yet) that still have no branch
        if vertices[i].not_on_branch() {
            let colour = get_available_colour(i, &available_colours);
            while available_colours.len() <= colour {
                available_colours.push(0);
            }
            let branch_id = branches.len();
            branches.push(LayoutBranch::new(colour));
            let x = vertices[i].next_x;
            vertices[i].add_to_branch(branch_id, x);
            vertices[i].register_unavailable_point(x, i, branch_id);
            branches[branch_id].set_end(i + 1);
            available_colours[colour] = i + 1;
        }
    }

    // 3. Build commit_pos_map (one (x, y) per commit)
    let mut commit_pos_map: CommitPosMap = FxHashMap::default();
    let mut max_pos_x = 0usize;
    for (i, commit) in commits.iter().enumerate() {
        let x = vertices[i].x.unwrap_or(0);
        commit_pos_map.insert(&commit.commit_hash, (x, i));
        if x > max_pos_x { max_pos_x = x; }
    }

    // 4. Convert branch lines → BranchSegments
    // Convention: source = older commit (higher row index), target = newer (lower row index)
    // Each branch line is (p1=newer, p2=older)
    let branch_segments: Vec<BranchSegment> = branches.iter().flat_map(|b| {
        b.lines.iter().map(|&(p1, p2)| BranchSegment {
            source_pos_x: p2.x,
            target_pos_x: p1.x,
            source_pos_y: p2.y,
            target_pos_y: p1.y,
            color_index: b.colour,
            is_branch: true,
        })
    }).collect();

    // Extend max_pos_x to cover all x-coordinates that appear in branch segments.
    // Intermediate waypoint x values (from next_x) can exceed committed vertex positions.
    for seg in &branch_segments {
        if seg.source_pos_x > max_pos_x { max_pos_x = seg.source_pos_x; }
        if seg.target_pos_x > max_pos_x { max_pos_x = seg.target_pos_x; }
    }

    // 5. Build edges for Rounded/Angular styles
    let edges = build_edges(n, &branch_segments);

    Graph {
        commits,
        commit_pos_map,
        edges,
        max_pos_x,
        branch_segments,
    }
}

fn build_edges(commits_len: usize, branch_segments: &[BranchSegment]) -> Vec<Vec<Edge>> {
    let mut edges: Vec<Vec<Edge>> = vec![vec![]; commits_len];

    for seg in branch_segments {
        // source = older commit (higher row index), target = newer (lower row index)
        let (src_x, src_y) = (seg.source_pos_x, seg.source_pos_y);
        let (tgt_x, tgt_y) = (seg.target_pos_x, seg.target_pos_y);
        let color_x = seg.color_index;

        if src_y >= commits_len || tgt_y >= commits_len {
            continue; // safety: skip out-of-bound segments
        }

        if src_x == tgt_x {
            // Straight vertical connection
            if src_y < commits_len { edges[src_y].push(Edge::new(EdgeType::Up, src_x, color_x)); }
            for y in (tgt_y + 1)..src_y {
                edges[y].push(Edge::new(EdgeType::Vertical, src_x, color_x));
            }
            if tgt_y < commits_len { edges[tgt_y].push(Edge::new(EdgeType::Down, src_x, color_x)); }
        } else {
            // Diagonal: horizontal exit at source row, vertical on target column, entry at target row
            if src_x > tgt_x {
                // Source is to the right → branch going left-up
                edges[src_y].push(Edge::new(EdgeType::Left, src_x, color_x));
                for x in (tgt_x + 1)..src_x {
                    edges[src_y].push(Edge::new(EdgeType::Horizontal, x, color_x));
                }
                edges[src_y].push(Edge::new(EdgeType::LeftBottom, tgt_x, color_x));
            } else {
                // Source is to the left → branch going right-up
                edges[src_y].push(Edge::new(EdgeType::Right, src_x, color_x));
                for x in (src_x + 1)..tgt_x {
                    edges[src_y].push(Edge::new(EdgeType::Horizontal, x, color_x));
                }
                edges[src_y].push(Edge::new(EdgeType::RightBottom, tgt_x, color_x));
            }
            // Vertical segment on target column between the two commits
            for y in (tgt_y + 1)..src_y {
                edges[y].push(Edge::new(EdgeType::Vertical, tgt_x, color_x));
            }
            // Entry at target row
            if tgt_y < commits_len {
                edges[tgt_y].push(Edge::new(EdgeType::Down, tgt_x, color_x));
            }
        }
    }

    // Dedup each row
    for row in &mut edges {
        row.sort_by_key(|e| (e.pos_x, e.edge_type));
        row.dedup();
    }

    edges
}
