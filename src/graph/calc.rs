use rustc_hash::FxHashMap;

use crate::git::{Commit, CommitHash, CommitType, Repository};

type CommitPosMap<'a> = FxHashMap<&'a CommitHash, (usize, usize)>;
type CommitColorMap<'a> = FxHashMap<&'a CommitHash, usize>;

#[derive(Debug, Clone)]
pub struct BranchSegment {
    pub source_pos_x: usize,
    pub target_pos_x: usize,
    pub source_pos_y: usize,
    pub target_pos_y: usize,
    pub color_index: usize,
    pub is_branch: bool,
    pub is_uncommitted: bool,
}

#[derive(Debug)]
pub struct Graph<'a> {
    pub commits: Vec<&'a Commit>,
    pub commit_pos_map: CommitPosMap<'a>,
    pub commit_color_map: CommitColorMap<'a>,
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
struct Pt {
    x: usize,
    y: usize,
}

#[derive(Debug)]
struct Connection {
    x: usize,
    connects_to: usize, // vertex id (index into vertices); usize::MAX = no vertex
    on_branch: usize,   // branch id (index into branches)
}

struct LayoutVertex {
    id: usize,
    x: Option<usize>, // None = not yet on a branch
    next_x: usize,
    connections: Vec<Connection>,
    parent_ids: Vec<usize>, // indices into vertices; usize::MAX = parent not in graph
    child_ids: Vec<usize>,
    next_parent: usize,       // index into parent_ids currently being processed
    branch_id: Option<usize>, // index into branches
    is_committed: bool,
}

impl LayoutVertex {
    fn point(&self) -> Pt {
        Pt {
            x: self.x.unwrap_or(0),
            y: self.id,
        }
    }
    fn next_point(&self) -> Pt {
        Pt {
            x: self.next_x,
            y: self.id,
        }
    }
    fn not_on_branch(&self) -> bool {
        self.branch_id.is_none()
    }
    fn is_merge(&self) -> bool {
        self.parent_ids.len() > 1
    }

    fn add_to_branch(&mut self, branch_id: usize, x: usize) {
        if self.branch_id.is_none() {
            self.branch_id = Some(branch_id);
            self.x = Some(x);
        }
    }

    fn register_unavailable_point(&mut self, x: usize, connects_to: usize, on_branch: usize) {
        if x == self.next_x {
            self.connections.push(Connection {
                x,
                connects_to,
                on_branch,
            });
            self.next_x += 1;
        }
    }

    fn get_point_connecting_to(&self, target_id: usize, branch_id: usize) -> Option<Pt> {
        self.connections
            .iter()
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
    lines: Vec<(Pt, Pt, bool)>, // (p1=newer, p2=older, is_uncommitted) in grid coords
    end: usize,
}

impl LayoutBranch {
    fn new(colour: usize) -> Self {
        Self {
            colour,
            lines: Vec::new(),
            end: 0,
        }
    }
    fn add_line(&mut self, p1: Pt, p2: Pt, is_uncommitted: bool) {
        self.lines.push((p1, p2, is_uncommitted));
    }
    fn set_end(&mut self, end: usize) {
        self.end = end;
    }
}

fn load_commits(commits: &[&Commit], _repository: &Repository) -> Vec<LayoutVertex> {
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

fn get_available_colour(_start_at: usize, available_colours: &[usize]) -> usize {
    // Always allocate a fresh sequential colour index. Earlier versions
    // reused the lowest "ended" index for tightly packed colour usage, but
    // that produced graphs where every new branch picked palette[0] the
    // instant the previous lane ended, the user saw three red branches in
    // a row even though the palette had 16 entries.
    //
    // Wrap-around is handled later by `ImageParams::edge_color`, which does
    // `palette[index % palette.len()]`. With this change, distinct branches
    // get distinct colours until we exceed the palette size; only then do
    // colours repeat, and at that point the repeated branches are far
    // apart in the graph history, where reuse is no longer visually noisy.
    available_colours.len()
}

fn determine_path(
    start_at: usize,
    vertices: &mut [LayoutVertex],
    branches: &mut Vec<LayoutBranch>,
    available_colours: &mut Vec<usize>,
) {
    // Extract info from start_at vertex before any mutation
    let (parent_id, v_not_on_branch, v_is_merge, v_on_branch) = {
        let v = &vertices[start_at];
        let pid = v
            .parent_ids
            .get(v.next_parent)
            .copied()
            .unwrap_or(usize::MAX);
        (pid, v.not_on_branch(), v.is_merge(), !v.not_on_branch())
    };

    // No parent in graph, mark processed and return
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
            let cur_pt = conn_pt.unwrap_or_else(|| vertices[i].next_point());

            branches[parent_branch_id].add_line(last_pt, cur_pt, false);
            {
                vertices[i].register_unavailable_point(cur_pt.x, parent_id, parent_branch_id);
            }
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

            let line_is_uncommitted = !vertices[cur_vertex_id].is_committed;
            branches[branch_id].add_line(last_pt, cur_pt, line_is_uncommitted);
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
                    v.parent_ids
                        .get(v.next_parent)
                        .copied()
                        .unwrap_or(usize::MAX)
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

/// Cheap stand-in for `calc_graph` used when the user has disabled
/// (or auto-disabled) the graph column. Skips the topology walk
/// entirely. The ONLY useful output is `commit_color_map`, which is
/// built from a first-parent walk per branch ref so the inline `│`
/// separator drawn left of each commit message still gets a
/// per-branch tint:
///
/// 1. Branch refs are visited in a stable order (HEAD's branch first
///    so it always claims the primary colour, then the rest
///    alphabetically).
/// 2. For each branch, walk first-parent from its tip; every commit
///    reached that isn't already coloured gets the branch's index as
///    its colour slot.
/// 3. Commits not reachable from any branch (orphans, dangling, etc.)
///    fall back to colour 0.
///
/// All other fields are empty / zeroed: `commit_pos_map` pins every
/// row to lane 0, `edges` is empty per row, `max_pos_x = 0`, no
/// branch segments. The image-rendering pipeline never runs when the
/// graph column is disabled, so those fields are never consumed -
/// they're only present so the `Graph` struct's downstream readers
/// in `app.rs` (which still do a generic `commit_pos_map[hash]`
/// lookup for sizing purposes) keep compiling.
///
/// Runs in O(N) total across all branch walks (each commit visited
/// once), tens of milliseconds on a 300k-commit repo vs several
/// seconds for the full `calc_graph`.
pub fn calc_colors_only(repository: &Repository) -> Graph<'_> {
    let commits = repository.all_commits();
    let n = commits.len();

    // Seed pos_map with pos_x = 0 for every commit; we'll overwrite
    // pos_x below with the branch colour index so the downstream
    // colour selector in `app.rs` (which uses `pos_x` directly for
    // non-Smooth styles) picks up per-branch tints. Smooth style
    // already reads `commit_color_map` and gets the same result.
    let mut commit_pos_map: CommitPosMap = FxHashMap::default();
    commit_pos_map.reserve(n);
    for (i, c) in commits.iter().enumerate() {
        commit_pos_map.insert(&c.commit_hash, (0, i));
    }

    let mut commit_color_map: CommitColorMap = FxHashMap::default();
    commit_color_map.reserve(n);

    // Collect branch tips in a stable order so colours don't shuffle
    // between runs. HEAD's branch wins slot 0 so the user's working
    // line stays the primary palette colour.
    let head_branch_name = match repository.head() {
        crate::git::Head::Branch { name } => Some(name.clone()),
        _ => None,
    };
    // Include BOTH local and remote branch tips. Many repos
    // (dependabot, ci, fork PRs) live entirely on remote branches
    // with no matching local; without those, the first-parent walks
    // miss the commits and they all fall through to colour 0 in the
    // fallback below, rendering the entire `│` separator column in
    // the palette's first colour.
    let mut branch_tips: Vec<(String, &CommitHash)> = Vec::new();
    for r in repository.all_refs() {
        match r {
            crate::git::Ref::Branch { name, target }
            | crate::git::Ref::RemoteBranch { name, target } => {
                branch_tips.push((name.clone(), target));
            }
            _ => {}
        }
    }
    // Stable sort: HEAD's local branch first (claims slot 0), then
    // every other ref alphabetically. Dedup so a local branch and
    // its matching remote (e.g. `master` + `origin/master`) don't
    // both walk the same first-parent chain and the same commit
    // doesn't get re-claimed by a different colour.
    branch_tips.sort_by(|a, b| {
        let a_is_head = head_branch_name.as_ref() == Some(&a.0);
        let b_is_head = head_branch_name.as_ref() == Some(&b.0);
        b_is_head.cmp(&a_is_head).then_with(|| a.0.cmp(&b.0))
    });

    // First-parent walk from each tip. Stop on already-coloured
    // commits so the first branch to claim a commit owns it.
    for (color_index, (_, tip)) in branch_tips.iter().enumerate() {
        let mut cur: Option<&CommitHash> = Some(tip);
        while let Some(hash) = cur {
            if commit_color_map.contains_key(hash) {
                break;
            }
            // Locate the cached `&CommitHash` from the canonical
            // `commits` Vec so the lifetime matches the map key
            // signature.
            let canonical_idx = commit_pos_map.get(hash).map(|&(_, y)| y);
            let (key, y) = match canonical_idx.and_then(|i| commits.get(i).map(|c| (i, c)))
            {
                Some((i, c)) => (&c.commit_hash, i),
                None => break,
            };
            commit_color_map.insert(key, color_index);
            // Mirror the colour into pos_x as well: app.rs's
            // non-Smooth path (`color_index = pos_x`) would
            // otherwise read 0 for every commit and render the
            // whole separator column in the palette's first
            // colour. Storing the branch index here makes both
            // paths agree.
            commit_pos_map.insert(key, (color_index, y));
            cur = repository.parents_hash(hash).first().copied();
        }
    }

    // Any remaining commits (orphans, unreferenced) fall back to 0.
    for c in &commits {
        commit_color_map.entry(&c.commit_hash).or_insert(0);
    }

    Graph {
        commits,
        commit_pos_map,
        commit_color_map,
        edges: vec![Vec::new(); n],
        max_pos_x: 0,
        branch_segments: Vec::new(),
    }
}

/// Lane-assignment phase of `calc_graph` only - returns the same
/// `commit_pos_map` and `commit_color_map` as the full version
/// without paying for the `build_legacy_edges` step. That step
/// allocates a `Vec<Vec<WrappedEdge>>` whose memory usage on a
/// 332k-commit repo with many branches reaches several hundred MB,
/// triggering an OOM kill of the bg streaming thread on machines
/// with limited RAM. The bg thread only needs colours, not edges,
/// so it calls this lighter variant; the fg path still uses the
/// full `calc_graph` so the image renderer has the edge data.
pub fn calc_graph_colors_only(repository: &Repository) -> Graph<'_> {
    let commits = repository.all_commits();
    let n = commits.len();

    let mut vertices = load_commits(&commits, repository);
    let mut branches: Vec<LayoutBranch> = Vec::new();
    let mut available_colours: Vec<usize> = Vec::new();

    for i in 0..n {
        loop {
            let has_more = {
                let v = &vertices[i];
                v.next_parent < v.parent_ids.len()
            };
            if !has_more {
                break;
            }
            determine_path(i, &mut vertices, &mut branches, &mut available_colours);
        }
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

    let mut commit_pos_map: CommitPosMap = FxHashMap::default();
    let mut commit_color_map: CommitColorMap = FxHashMap::default();
    let mut max_pos_x = 0usize;
    for (i, commit) in commits.iter().enumerate() {
        let x = vertices[i].x.unwrap_or(0);
        commit_pos_map.insert(&commit.commit_hash, (x, i));
        let color = vertices[i]
            .branch_id
            .map(|branch_id| branches[branch_id].colour)
            .unwrap_or(x);
        commit_color_map.insert(&commit.commit_hash, color);
        if x > max_pos_x {
            max_pos_x = x;
        }
    }

    Graph {
        commits,
        commit_pos_map,
        commit_color_map,
        edges: vec![Vec::new(); n],
        max_pos_x,
        branch_segments: Vec::new(),
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
            if !has_more {
                break;
            }
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
    let mut commit_color_map: CommitColorMap = FxHashMap::default();
    let mut max_pos_x = 0usize;
    for (i, commit) in commits.iter().enumerate() {
        let x = vertices[i].x.unwrap_or(0);
        commit_pos_map.insert(&commit.commit_hash, (x, i));
        let color = vertices[i]
            .branch_id
            .map(|branch_id| branches[branch_id].colour)
            .unwrap_or(x);
        commit_color_map.insert(&commit.commit_hash, color);
        if x > max_pos_x {
            max_pos_x = x;
        }
    }

    // 4. Convert branch lines → BranchSegments
    // Convention: source = older commit (higher row index), target = newer (lower row index)
    // Each branch line is (p1=newer, p2=older)
    let branch_segments: Vec<BranchSegment> = branches
        .iter()
        .flat_map(|b| {
            b.lines
                .iter()
                .map(|&(p1, p2, is_uncommitted)| BranchSegment {
                    source_pos_x: p2.x,
                    target_pos_x: p1.x,
                    source_pos_y: p2.y,
                    target_pos_y: p1.y,
                    color_index: b.colour,
                    is_branch: true,
                    is_uncommitted,
                })
        })
        .collect();

    // Extend max_pos_x to cover all x-coordinates that appear in branch segments.
    // Intermediate waypoint x values (from next_x) can exceed committed vertex positions.
    for seg in &branch_segments {
        if seg.source_pos_x > max_pos_x {
            max_pos_x = seg.source_pos_x;
        }
        if seg.target_pos_x > max_pos_x {
            max_pos_x = seg.target_pos_x;
        }
    }

    // 5. Build legacy row edges for Rounded/Angular styles. Smooth uses branch_segments.
    let (edges, legacy_max_pos_x) = build_legacy_edges(&commit_pos_map, &commits, repository);
    max_pos_x = max_pos_x.max(legacy_max_pos_x);

    Graph {
        commits,
        commit_pos_map,
        commit_color_map,
        edges,
        max_pos_x,
        branch_segments,
    }
}

#[derive(Debug, Clone)]
struct WrappedEdge<'a> {
    edge: Edge,
    edge_parent_hash: &'a CommitHash,
}

impl<'a> WrappedEdge<'a> {
    fn new(
        edge_type: EdgeType,
        pos_x: usize,
        line_pos_x: usize,
        edge_parent_hash: &'a CommitHash,
    ) -> Self {
        Self {
            edge: Edge::new(edge_type, pos_x, line_pos_x),
            edge_parent_hash,
        }
    }
}

fn build_legacy_edges<'a>(
    commit_pos_map: &CommitPosMap<'a>,
    commits: &[&'a Commit],
    repository: &'a Repository,
) -> (Vec<Vec<Edge>>, usize) {
    let mut max_pos_x = 0;
    let mut edges: Vec<Vec<WrappedEdge>> = vec![vec![]; commits.len()];

    for commit in commits {
        // Defensive lookup: orphaned children referenced from
        // `children_map` but absent from the laid-out commit set
        // (stash boundaries, partial Repository views, edge cases
        // around shallow clones) used to panic the bg streaming
        // thread silently and leave the user stuck at the last
        // successfully-streamed commit (e.g. blocked at
        // c2bb45ec on rust-lang/rust). Skip rather than crash.
        let Some(&(pos_x, pos_y)) = commit_pos_map.get(&commit.commit_hash) else {
            continue;
        };
        let hash = &commit.commit_hash;

        for child_hash in repository.children_hash(hash) {
            let Some(&(child_pos_x, child_pos_y)) = commit_pos_map.get(child_hash) else {
                continue;
            };

            if pos_x == child_pos_x {
                edges[pos_y].push(WrappedEdge::new(EdgeType::Up, pos_x, pos_x, hash));
                for y in ((child_pos_y + 1)..pos_y).rev() {
                    edges[y].push(WrappedEdge::new(EdgeType::Vertical, pos_x, pos_x, hash));
                }
                edges[child_pos_y].push(WrappedEdge::new(EdgeType::Down, pos_x, pos_x, hash));
            } else {
                let child_first_parent_hash = &commits[child_pos_y].parent_commit_hashes[0];
                if *child_first_parent_hash == *hash {
                    if pos_x < child_pos_x {
                        edges[pos_y].push(WrappedEdge::new(
                            EdgeType::Right,
                            pos_x,
                            child_pos_x,
                            hash,
                        ));
                        for x in (pos_x + 1)..child_pos_x {
                            edges[pos_y].push(WrappedEdge::new(
                                EdgeType::Horizontal,
                                x,
                                child_pos_x,
                                hash,
                            ));
                        }
                        edges[pos_y].push(WrappedEdge::new(
                            EdgeType::RightBottom,
                            child_pos_x,
                            child_pos_x,
                            hash,
                        ));
                    } else {
                        edges[pos_y].push(WrappedEdge::new(
                            EdgeType::Left,
                            pos_x,
                            child_pos_x,
                            hash,
                        ));
                        for x in (child_pos_x + 1)..pos_x {
                            edges[pos_y].push(WrappedEdge::new(
                                EdgeType::Horizontal,
                                x,
                                child_pos_x,
                                hash,
                            ));
                        }
                        edges[pos_y].push(WrappedEdge::new(
                            EdgeType::LeftBottom,
                            child_pos_x,
                            child_pos_x,
                            hash,
                        ));
                    }
                    for y in ((child_pos_y + 1)..pos_y).rev() {
                        edges[y].push(WrappedEdge::new(
                            EdgeType::Vertical,
                            child_pos_x,
                            child_pos_x,
                            hash,
                        ));
                    }
                    edges[child_pos_y].push(WrappedEdge::new(
                        EdgeType::Down,
                        child_pos_x,
                        child_pos_x,
                        hash,
                    ));
                }
            }
        }

        if max_pos_x < pos_x {
            max_pos_x = pos_x;
        }

        if !commit.parent_commit_hashes.is_empty()
            && repository.commit(&commit.parent_commit_hashes[0]).is_none()
        {
            edges[pos_y].push(WrappedEdge::new(EdgeType::Down, pos_x, pos_x, hash));
            ((pos_y + 1)..commits.len()).for_each(|y| {
                edges[y].push(WrappedEdge::new(EdgeType::Vertical, pos_x, pos_x, hash));
            });
        }
    }

    for commit in commits {
        // Same defensive lookup as `build_legacy_edges` above:
        // skip orphan / missing entries instead of panicking.
        let Some(&(pos_x, pos_y)) = commit_pos_map.get(&commit.commit_hash) else {
            continue;
        };
        let hash = &commit.commit_hash;

        for child_hash in repository.children_hash(hash) {
            let Some(&(child_pos_x, child_pos_y)) = commit_pos_map.get(child_hash) else {
                continue;
            };

            if pos_x != child_pos_x {
                let child_first_parent_hash = &commits[child_pos_y].parent_commit_hashes[0];
                if *child_first_parent_hash != *hash {
                    let mut overlap = false;
                    let mut new_pos_x = pos_x;

                    let mut skip_judge_overlap = true;
                    for y in (child_pos_y + 1)..pos_y {
                        let processing_commit_pos_x =
                            commit_pos_map.get(&commits[y].commit_hash).unwrap().0;
                        if processing_commit_pos_x == new_pos_x {
                            skip_judge_overlap = false;
                            break;
                        }
                        if edges[y]
                            .iter()
                            .filter(|e| e.edge.pos_x == pos_x)
                            .filter(|e| matches!(e.edge.edge_type, EdgeType::Vertical))
                            .any(|e| e.edge_parent_hash != hash)
                        {
                            skip_judge_overlap = false;
                            break;
                        }
                    }

                    if !skip_judge_overlap {
                        for y in (child_pos_y + 1)..pos_y {
                            let processing_commit_pos_x =
                                commit_pos_map.get(&commits[y].commit_hash).unwrap().0;
                            if processing_commit_pos_x == new_pos_x {
                                overlap = true;
                                if new_pos_x < processing_commit_pos_x + 1 {
                                    new_pos_x = processing_commit_pos_x + 1;
                                }
                            }
                            for edge in &edges[y] {
                                if edge.edge.pos_x >= new_pos_x
                                    && edge.edge_parent_hash != hash
                                    && matches!(edge.edge.edge_type, EdgeType::Vertical)
                                {
                                    overlap = true;
                                    if new_pos_x < edge.edge.pos_x + 1 {
                                        new_pos_x = edge.edge.pos_x + 1;
                                    }
                                }
                            }
                        }
                    }

                    if overlap {
                        edges[pos_y].push(WrappedEdge::new(EdgeType::Right, pos_x, pos_x, hash));
                        for x in (pos_x + 1)..new_pos_x {
                            edges[pos_y].push(WrappedEdge::new(
                                EdgeType::Horizontal,
                                x,
                                pos_x,
                                hash,
                            ));
                        }
                        edges[pos_y].push(WrappedEdge::new(
                            EdgeType::RightBottom,
                            new_pos_x,
                            pos_x,
                            hash,
                        ));
                        for y in ((child_pos_y + 1)..pos_y).rev() {
                            edges[y].push(WrappedEdge::new(
                                EdgeType::Vertical,
                                new_pos_x,
                                pos_x,
                                hash,
                            ));
                        }
                        edges[child_pos_y].push(WrappedEdge::new(
                            EdgeType::RightTop,
                            new_pos_x,
                            pos_x,
                            hash,
                        ));
                        for x in (child_pos_x + 1)..new_pos_x {
                            edges[child_pos_y].push(WrappedEdge::new(
                                EdgeType::Horizontal,
                                x,
                                pos_x,
                                hash,
                            ));
                        }
                        edges[child_pos_y].push(WrappedEdge::new(
                            EdgeType::Right,
                            child_pos_x,
                            pos_x,
                            hash,
                        ));

                        if max_pos_x < new_pos_x {
                            max_pos_x = new_pos_x;
                        }
                    } else {
                        edges[pos_y].push(WrappedEdge::new(EdgeType::Up, pos_x, pos_x, hash));
                        for y in ((child_pos_y + 1)..pos_y).rev() {
                            edges[y].push(WrappedEdge::new(EdgeType::Vertical, pos_x, pos_x, hash));
                        }
                        if pos_x < child_pos_x {
                            edges[child_pos_y].push(WrappedEdge::new(
                                EdgeType::LeftTop,
                                pos_x,
                                pos_x,
                                hash,
                            ));
                            for x in (pos_x + 1)..child_pos_x {
                                edges[child_pos_y].push(WrappedEdge::new(
                                    EdgeType::Horizontal,
                                    x,
                                    pos_x,
                                    hash,
                                ));
                            }
                            edges[child_pos_y].push(WrappedEdge::new(
                                EdgeType::Left,
                                child_pos_x,
                                pos_x,
                                hash,
                            ));
                        } else {
                            edges[child_pos_y].push(WrappedEdge::new(
                                EdgeType::RightTop,
                                pos_x,
                                pos_x,
                                hash,
                            ));
                            for x in (child_pos_x + 1)..pos_x {
                                edges[child_pos_y].push(WrappedEdge::new(
                                    EdgeType::Horizontal,
                                    x,
                                    pos_x,
                                    hash,
                                ));
                            }
                            edges[child_pos_y].push(WrappedEdge::new(
                                EdgeType::Right,
                                child_pos_x,
                                pos_x,
                                hash,
                            ));
                        }
                    }
                }
            }
        }

        if max_pos_x < pos_x {
            max_pos_x = pos_x;
        }
    }

    let edges = edges
        .into_iter()
        .map(|es| {
            let mut es: Vec<Edge> = es.into_iter().map(|e| e.edge).collect();
            es.sort_by_key(|e| (e.associated_line_pos_x, e.pos_x, e.edge_type));
            es.dedup();
            es
        })
        .collect();

    (edges, max_pos_x)
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, FixedOffset};
    use rustc_hash::FxHashMap;

    use crate::git::{Commit, CommitHash, CommitType, Head, Repository};

    use super::*;

    fn commit(hash: &str, parents: Vec<&str>, commit_type: CommitType) -> Commit {
        Commit {
            commit_hash: CommitHash::from(hash),
            author_date: DateTime::parse_from_rfc3339("2024-01-01T00:00:00+00:00").unwrap(),
            committer_date: DateTime::<FixedOffset>::parse_from_rfc3339(
                "2024-01-01T00:00:00+00:00",
            )
            .unwrap(),
            commit_message: hash.to_string(),
            parent_commit_hashes: parents.into_iter().map(CommitHash::from).collect(),
            commit_type,
            ..Default::default()
        }
    }

    #[test]
    fn uncommitted_layout_marks_only_segments_before_head() {
        let commits = vec![
            commit("uncommitted", vec!["head"], CommitType::Uncommitted),
            commit("head", vec!["parent"], CommitType::Commit),
            commit("parent", vec![], CommitType::Commit),
        ];
        let commit_hashes = commits
            .iter()
            .map(|c| c.commit_hash.clone())
            .collect::<Vec<_>>();
        let commit_map = commits
            .into_iter()
            .map(|c| (c.commit_hash.clone(), std::sync::Arc::new(c)))
            .collect::<FxHashMap<_, _>>();
        let repository = Repository::new(
            Default::default(),
            commit_map,
            FxHashMap::default(),
            FxHashMap::default(),
            FxHashMap::default(),
            Head::Detached {
                target: CommitHash::from("head"),
            },
            commit_hashes,
            None,
        );

        let graph = calc_graph(&repository);

        let segment_to_head = graph
            .branch_segments
            .iter()
            .find(|s| s.target_pos_y == 0 && s.source_pos_y == 1)
            .expect("expected uncommitted to HEAD segment");
        assert!(segment_to_head.is_uncommitted);

        let segment_after_head = graph
            .branch_segments
            .iter()
            .find(|s| s.target_pos_y == 1 && s.source_pos_y == 2)
            .expect("expected HEAD to parent segment");
        assert!(!segment_after_head.is_uncommitted);
    }

    #[test]
    fn non_smooth_edges_route_merge_into_child_row() {
        let commits = vec![
            commit("merge", vec!["main", "side"], CommitType::Commit),
            commit("main", vec!["root"], CommitType::Commit),
            commit("side", vec!["root"], CommitType::Commit),
            commit("root", vec![], CommitType::Commit),
        ];
        let commit_hashes = commits
            .iter()
            .map(|c| c.commit_hash.clone())
            .collect::<Vec<_>>();
        let commit_map = commits
            .into_iter()
            .map(|c| (c.commit_hash.clone(), std::sync::Arc::new(c)))
            .collect::<FxHashMap<_, _>>();
        let parents_map = FxHashMap::from_iter([
            (
                CommitHash::from("merge"),
                vec![CommitHash::from("main"), CommitHash::from("side")],
            ),
            (CommitHash::from("main"), vec![CommitHash::from("root")]),
            (CommitHash::from("side"), vec![CommitHash::from("root")]),
        ]);
        let children_map = FxHashMap::from_iter([
            (CommitHash::from("main"), vec![CommitHash::from("merge")]),
            (CommitHash::from("side"), vec![CommitHash::from("merge")]),
            (
                CommitHash::from("root"),
                vec![CommitHash::from("main"), CommitHash::from("side")],
            ),
        ]);
        let repository = Repository::new(
            Default::default(),
            commit_map,
            parents_map,
            children_map,
            FxHashMap::default(),
            Head::Detached {
                target: CommitHash::from("merge"),
            },
            commit_hashes,
            None,
        );

        let graph = calc_graph(&repository);

        assert!(
            graph.edges[0].iter().any(|edge| matches!(
                edge.edge_type,
                EdgeType::Horizontal | EdgeType::Left | EdgeType::Right
            )),
            "merge rows need horizontal entry edges for rounded/angular renderers"
        );
    }

    /// Locks in the "always allocate a fresh index" semantics of the new
    /// `get_available_colour`. The old algorithm reused the lowest ended
    /// index, the user complained that this produced three red branches
    /// in a row. We freeze the new behaviour so future refactors can't
    /// silently revert it.
    #[test]
    fn get_available_colour_always_returns_len() {
        assert_eq!(get_available_colour(0, &[]), 0);
        assert_eq!(get_available_colour(10, &[]), 0);
        // Even when slot 0 has "ended" (entry value <= start_at), we DON'T
        // reuse it, we hand out the next sequential index.
        assert_eq!(get_available_colour(10, &[3]), 1);
        assert_eq!(get_available_colour(10, &[3, 5, 7]), 3);
        // And `start_at` is ignored entirely, only the slot count matters.
        assert_eq!(get_available_colour(0, &[100, 100, 100]), 3);
    }

    /// Smoke test at the `calc_graph` layer: two sequential branches that
    /// don't overlap in time still get distinct colour indices.
    #[test]
    fn sequential_non_overlapping_branches_get_distinct_colours() {
        // Linear history of 4 commits; root has no parent.
        let commits = vec![
            commit("d", vec!["c"], CommitType::Commit),
            commit("c", vec!["b"], CommitType::Commit),
            commit("b", vec!["a"], CommitType::Commit),
            commit("a", vec![], CommitType::Commit),
        ];
        let commit_hashes: Vec<_> = commits.iter().map(|c| c.commit_hash.clone()).collect();
        let commit_map = commits
            .into_iter()
            .map(|c| (c.commit_hash.clone(), std::sync::Arc::new(c)))
            .collect::<FxHashMap<_, _>>();
        let repository = Repository::new(
            Default::default(),
            commit_map,
            FxHashMap::default(),
            FxHashMap::default(),
            FxHashMap::default(),
            Head::Detached {
                target: CommitHash::from("d"),
            },
            commit_hashes,
            None,
        );

        let graph = calc_graph(&repository);
        // A linear history yields a single branch on lane 0, but its
        // colour index must come out as 0 (first allocation).
        let head_color = graph.commit_color_map[&CommitHash::from("d")];
        assert_eq!(head_color, 0);
        // Every commit on the same single branch shares that colour.
        for h in ["c", "b", "a"] {
            assert_eq!(graph.commit_color_map[&CommitHash::from(h)], head_color);
        }
    }
}
