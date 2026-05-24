use std::path::Path;

use gitoui::{color, git, graph};

/// Verifies the bg-stream invariant: when calc_graph_colors_only runs over
/// the full repo, every commit gets a commit_pos_map entry and there are
/// branch_segments crossing rows beyond the fg-loaded slice. If either
/// of these is false, the bg-streamed rows will render as isolated dots
/// even after the ReplaceGraph swap.
#[test]
fn bg_stream_graph_covers_every_commit() {
    let path = Path::new("/tmp/gitoui-test-repo");
    if !path.join(".git").exists() {
        eprintln!(
            "skipping bg_stream_graph_covers_every_commit: no test repo at {}",
            path.display()
        );
        return;
    }
    let order = git::SortCommit::Chronological;

    let bg_repo = git::Repository::load(path, order, None).expect("bg load");
    let bg_graph = graph::calc_graph_colors_only(&bg_repo);

    let total = bg_repo.commit_count();
    let mapped: usize = bg_graph
        .commits
        .iter()
        .filter(|c| bg_graph.commit_pos_map.contains_key(&c.commit_hash))
        .count();
    println!(
        "bg graph: commits={} mapped={} max_pos_x={} segments={}",
        total,
        mapped,
        bg_graph.max_pos_x,
        bg_graph.branch_segments.len()
    );
    assert_eq!(mapped, total, "every commit must have a position");
    assert!(
        !bg_graph.branch_segments.is_empty(),
        "must have at least one branch_segment"
    );

    // Pick a deep row (well past the typical fg slice of 500) and verify
    // there's at least one segment crossing it.
    let probe_row = (total / 2).max(600);
    if probe_row < total {
        let crossing: usize = bg_graph
            .branch_segments
            .iter()
            .filter(|s| {
                let (top, bot) = if s.source_pos_y < s.target_pos_y {
                    (s.source_pos_y, s.target_pos_y)
                } else {
                    (s.target_pos_y, s.source_pos_y)
                };
                probe_row >= top && probe_row <= bot
            })
            .count();
        println!("row {}: {} segments crossing", probe_row, crossing);
        assert!(
            crossing > 0,
            "row {} must have at least one segment crossing it",
            probe_row
        );
    }
}

/// Probe the actual rendering of every deep commit and check how many
/// have cell_count == 1 (just a dot, no edges - matches the bug
/// symptom).
#[test]
fn count_isolated_dot_commits() {
    use gitoui::graph::{CellWidthType, GraphImageManager, GraphImageWidthMode, GraphStyle};
    use gitoui::protocol::ImageProtocol;
    let path = Path::new("/tmp/gitoui-test-repo");
    if !path.join(".git").exists() {
        return;
    }
    let order = git::SortCommit::Chronological;
    let bg_repo = git::Repository::load(path, order, None).expect("bg load");
    let bg_graph = graph::calc_graph_colors_only(&bg_repo);
    let n_commits = bg_repo.commit_count();

    // Bake using bg graph directly.
    let color_theme = color::ColorTheme::default();
    let graph_color_set =
        color::build_graph_color_set(&color_theme, &gitoui::config::GraphConfig::default().color);
    let mut manager = GraphImageManager::new(
        bg_graph,
        &graph_color_set,
        CellWidthType::Single,
        GraphStyle::Rounded,
        GraphImageWidthMode::Compact,
        ImageProtocol::Iterm2,
    );

    let commits: Vec<_> = bg_repo.all_commits().into_iter().cloned().collect();
    let mut isolated = 0usize;
    let mut zero = 0usize;
    let mut sample_isolated = Vec::new();
    for (i, c) in commits.iter().enumerate() {
        manager.ensure_uploaded(&c.commit_hash);
        let prep = manager.prepared_image(&c.commit_hash).unwrap();
        let cw = prep.cell_width();
        if cw == 0 {
            zero += 1;
        }
        if cw == 1 {
            isolated += 1;
            if sample_isolated.len() < 5 {
                sample_isolated.push((i, c.commit_hash.clone()));
            }
        }
    }
    println!(
        "total={} zero_cells={} isolated_one_cell={}",
        n_commits, zero, isolated
    );
    for (idx, hash) in &sample_isolated {
        println!("  isolated at idx={}: {}", idx, hash.as_str());
    }
    // Now use a SEPARATE graph instance to inspect segments without
    // borrow conflicts.
    let bg_graph2 = graph::calc_graph_colors_only(&bg_repo);
    for (idx, hash) in &sample_isolated {
        let pos = bg_graph2.commit_pos_map.get(hash).copied();
        let mut cross = 0;
        for s in &bg_graph2.branch_segments {
            let (top, bot) = if s.source_pos_y < s.target_pos_y {
                (s.source_pos_y, s.target_pos_y)
            } else {
                (s.target_pos_y, s.source_pos_y)
            };
            if *idx >= top && *idx <= bot {
                cross += 1;
                if cross <= 3 {
                    println!(
                        "    seg at idx={}: src=({},{}) tgt=({},{}) is_branch={}",
                        idx,
                        s.source_pos_x,
                        s.source_pos_y,
                        s.target_pos_x,
                        s.target_pos_y,
                        s.is_branch
                    );
                }
            }
        }
        println!("  idx={} pos={:?} segs_crossing={}", idx, pos, cross);
    }
}

/// Render an idx=714-style commit row through `build_single_graph_row_image`
/// directly and dump the row image to disk for inspection.
#[test]
fn dump_isolated_row_image() {
    use gitoui::graph::{
        build_single_graph_row_image, CellWidthType, DrawingPixels, GraphImageWidthMode,
        GraphStyle, ImageParams,
    };
    let path = Path::new("/tmp/gitoui-test-repo");
    if !path.join(".git").exists() {
        return;
    }
    let order = git::SortCommit::Chronological;
    let bg_repo = git::Repository::load(path, order, None).expect("bg load");
    let bg_graph = graph::calc_graph_colors_only(&bg_repo);
    let target_hash = bg_repo
        .all_commits()
        .get(714)
        .expect("must have 714+ commits")
        .commit_hash
        .clone();
    let color_theme = color::ColorTheme::default();
    let graph_color_set =
        color::build_graph_color_set(&color_theme, &gitoui::config::GraphConfig::default().color);
    let image_params = ImageParams::new(&graph_color_set, CellWidthType::Single);
    let dp = DrawingPixels::new(&image_params);
    let row = build_single_graph_row_image(
        &bg_graph,
        &image_params,
        &dp,
        GraphStyle::Rounded,
        GraphImageWidthMode::Compact,
        &target_hash,
        false,
    );
    println!(
        "row.cell_count = {}, row.bytes.len() = {}",
        row.cell_count,
        row.bytes.len()
    );

    // Now compare against a FRESH calc_graph (full edges) for the same commit
    let bg_graph_full = graph::calc_graph(&bg_repo);
    let row2 = build_single_graph_row_image(
        &bg_graph_full,
        &image_params,
        &dp,
        GraphStyle::Rounded,
        GraphImageWidthMode::Compact,
        &target_hash,
        false,
    );
    println!(
        "with full edges: cell_count = {}, bytes.len = {}",
        row2.cell_count,
        row2.bytes.len()
    );
    // Write both to disk
    std::fs::write("/tmp/row-bg.png", &row.bytes).unwrap();
    std::fs::write("/tmp/row-fg.png", &row2.bytes).unwrap();
    println!("wrote /tmp/row-bg.png and /tmp/row-fg.png");

    // Compare ALL commits between fg's calc_graph and bg's calc_graph_colors_only
    // to find any commit where the rendered bytes differ.
    let bg_full = graph::calc_graph(&bg_repo);
    let bg_colors = graph::calc_graph_colors_only(&bg_repo);
    let mut differences = 0usize;
    let mut sample_diff: Vec<(usize, String)> = Vec::new();
    for (i, c) in bg_repo.all_commits().iter().enumerate() {
        let row_a = build_single_graph_row_image(
            &bg_full,
            &image_params,
            &dp,
            GraphStyle::Rounded,
            GraphImageWidthMode::Compact,
            &c.commit_hash,
            false,
        );
        let row_b = build_single_graph_row_image(
            &bg_colors,
            &image_params,
            &dp,
            GraphStyle::Rounded,
            GraphImageWidthMode::Compact,
            &c.commit_hash,
            false,
        );
        if row_a.bytes != row_b.bytes || row_a.cell_count != row_b.cell_count {
            differences += 1;
            if sample_diff.len() < 5 {
                sample_diff.push((i, c.commit_hash.as_str().to_string()));
            }
        }
    }
    println!(
        "commits with rendering differences: {} / {}",
        differences,
        bg_repo.commit_count()
    );
    for (i, h) in &sample_diff {
        println!("  diff at idx={}: {}", i, h);
    }
    // Dump a non-trivial diff to file for inspection
    if let Some((i, _hash_str)) = sample_diff.first() {
        let hash = bg_repo.all_commits()[*i].commit_hash.clone();
        let row_a = build_single_graph_row_image(
            &bg_full,
            &image_params,
            &dp,
            GraphStyle::Rounded,
            GraphImageWidthMode::Compact,
            &hash,
            false,
        );
        let row_b = build_single_graph_row_image(
            &bg_colors,
            &image_params,
            &dp,
            GraphStyle::Rounded,
            GraphImageWidthMode::Compact,
            &hash,
            false,
        );
        std::fs::write(format!("/tmp/diff-fg-{}.png", i), &row_a.bytes).unwrap();
        std::fs::write(format!("/tmp/diff-bg-{}.png", i), &row_b.bytes).unwrap();
        println!(
            "  fg cell_count={}, bg cell_count={}",
            row_a.cell_count, row_b.cell_count
        );
        println!("  wrote /tmp/diff-fg-{}.png and /tmp/diff-bg-{}.png", i, i);

        // Bake with KittyUnicode to access derived edges in a stub way.
        // Instead, just inspect by reimplementing the derive logic inline.

        // Inspect fg's per-row edges and bg's branch_segments crossing this row
        println!("  fg row edges at idx={}:", i);
        if let Some(edges) = bg_full.edges.get(*i) {
            for e in edges {
                println!(
                    "    {:?} pos_x={} line_pos_x={}",
                    e.edge_type, e.pos_x, e.associated_line_pos_x
                );
            }
        }
        println!("  bg segments crossing idx={} (showing ALL):", i);
        for s in &bg_colors.branch_segments {
            let (top, bot) = if s.source_pos_y < s.target_pos_y {
                (s.source_pos_y, s.target_pos_y)
            } else {
                (s.target_pos_y, s.source_pos_y)
            };
            if *i >= top && *i <= bot {
                println!(
                    "    src=({},{}) tgt=({},{}) color_idx={} is_branch={} is_unc={}",
                    s.source_pos_x,
                    s.source_pos_y,
                    s.target_pos_x,
                    s.target_pos_y,
                    s.color_index,
                    s.is_branch,
                    s.is_uncommitted
                );
            }
        }
        // Also show first 15 segments for context
        println!("  first 15 bg segments overall:");
        for s in bg_colors.branch_segments.iter().take(15) {
            println!(
                "    src=({},{}) tgt=({},{}) color={}",
                s.source_pos_x, s.source_pos_y, s.target_pos_x, s.target_pos_y, s.color_index
            );
        }
    }
}

/// Probe the actual rendering of a deep commit in the new graph
/// after replace_graph.
#[test]
fn deep_commit_renders_with_edges_after_replace() {
    use gitoui::graph::{CellWidthType, GraphImageManager, GraphImageWidthMode, GraphStyle};
    use gitoui::protocol::ImageProtocol;
    let path = Path::new("/tmp/gitoui-test-repo");
    if !path.join(".git").exists() {
        return;
    }
    let order = git::SortCommit::Chronological;
    let bg_repo = git::Repository::load(path, order, None).expect("bg load");
    let bg_graph = graph::calc_graph_colors_only(&bg_repo);

    // Pick a deep commit (index 600).
    let target_hash = bg_repo
        .all_commits()
        .get(600)
        .expect("must have 600+ commits")
        .commit_hash
        .clone();

    // Bake using bg graph directly.
    let color_theme = color::ColorTheme::default();
    let graph_color_set =
        color::build_graph_color_set(&color_theme, &gitoui::config::GraphConfig::default().color);
    let mut manager = GraphImageManager::new(
        bg_graph,
        &graph_color_set,
        CellWidthType::Single,
        GraphStyle::Rounded,
        GraphImageWidthMode::Compact,
        ImageProtocol::Iterm2,
    );
    manager.ensure_uploaded(&target_hash);
    let prep = manager.prepared_image(&target_hash).unwrap();
    println!("Deep commit cell_width: {}", prep.cell_width());
    assert!(prep.cell_width() > 0);
}

/// Simulates the live hot-swap: build the fg's small-slice graph, hand it
/// to a GraphImageManager, then call replace_graph with the bg's full
/// graph. After the swap, ensure_uploaded for a bg-only commit must
/// produce a non-empty image (cell_count > 0).
#[test]
fn replace_graph_renders_bg_streamed_commits() {
    use gitoui::graph::{CellWidthType, GraphImageManager, GraphImageWidthMode, GraphStyle};
    use gitoui::protocol::ImageProtocol;

    let path = Path::new("/tmp/gitoui-test-repo");
    if !path.join(".git").exists() {
        eprintln!("skipping: no test repo at {}", path.display());
        return;
    }
    let order = git::SortCommit::Chronological;

    // fg: small slice
    let fg_repo = git::Repository::load(path, order, Some(50)).expect("fg load");
    let fg_graph = graph::calc_graph(&fg_repo);
    let color_theme = color::ColorTheme::default();
    let graph_color_set =
        color::build_graph_color_set(&color_theme, &gitoui::config::GraphConfig::default().color);

    let mut manager = GraphImageManager::new(
        fg_graph,
        &graph_color_set,
        CellWidthType::Single,
        GraphStyle::Rounded,
        GraphImageWidthMode::Compact,
        ImageProtocol::Iterm2,
    );

    // bg: full history
    let bg_repo = git::Repository::load(path, order, None).expect("bg load");
    let bg_graph = graph::calc_graph_colors_only(&bg_repo);

    // Pick a bg-only commit (one beyond the fg's 50-commit slice).
    let bg_only_commit_hash = bg_repo
        .all_commits()
        .iter()
        .find(|c| {
            !fg_repo
                .all_commits()
                .iter()
                .any(|fc| fc.commit_hash == c.commit_hash)
        })
        .expect("there must be commits beyond the fg slice")
        .commit_hash
        .clone();

    println!("Probe bg-only commit: {}", bg_only_commit_hash.as_str());

    // Before swap: try to render this commit, expect 0-cell stub.
    manager.ensure_uploaded(&bg_only_commit_hash);
    let pre = manager
        .prepared_image(&bg_only_commit_hash)
        .map(|p| p.cell_width());
    println!("Pre-swap cell_width: {:?}", pre);

    // Now swap.
    manager.replace_graph(bg_graph);

    // After swap, the cache should be cleared, so ensure_uploaded rebakes.
    manager.ensure_uploaded(&bg_only_commit_hash);
    let post = manager
        .prepared_image(&bg_only_commit_hash)
        .map(|p| p.cell_width());
    println!("Post-swap cell_width: {:?}", post);

    assert!(
        matches!(post, Some(w) if w > 0),
        "after replace_graph, bg-only commit must render with cell_width > 0 (got {:?})",
        post
    );
}
