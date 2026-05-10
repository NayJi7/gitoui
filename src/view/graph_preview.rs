use std::path::PathBuf;

use rustc_hash::FxHashMap;

use crate::{
    color::GraphColorSet,
    git::{Commit, CommitHash, CommitType, Head, Ref, Repository},
    graph::{calc_graph, CellWidthType, GraphImageManager, GraphImageWidthMode, GraphStyle},
    protocol::{ImageProtocol, PreparedImage},
};

#[derive(Debug)]
pub struct GraphPreview {
    pub style: GraphStyle,
    pub rows: Vec<PreparedImage>,
    pub pending_uploads: Vec<String>,
    pub image_ids: Vec<u32>,
}

impl GraphPreview {
    pub fn build(
        style: GraphStyle,
        graph_color_set: &GraphColorSet,
        image_protocol: ImageProtocol,
        bg_rgb: Option<(u8, u8, u8)>,
    ) -> Self {
        // Three commits arranged as:
        //   m1  (main HEAD)        row 0, lane 0
        //   f1  (feature HEAD)     row 1, lane 1   parent = m0
        //   m0  (fork commit)      row 2, lane 0
        let m1 = preview_commit("m1", &["m0"]);
        let f1 = preview_commit("f1", &["m0"]);
        let m0 = preview_commit("m0", &[]);

        let commit_hashes: Vec<CommitHash> = vec![
            m1.commit_hash.clone(),
            f1.commit_hash.clone(),
            m0.commit_hash.clone(),
        ];

        let mut commit_map: FxHashMap<CommitHash, Commit> = FxHashMap::default();
        let mut parents_map: FxHashMap<CommitHash, Vec<CommitHash>> = FxHashMap::default();
        let mut children_map: FxHashMap<CommitHash, Vec<CommitHash>> = FxHashMap::default();

        for c in [&m1, &f1, &m0] {
            parents_map.insert(c.commit_hash.clone(), c.parent_commit_hashes.clone());
            for parent in &c.parent_commit_hashes {
                children_map
                    .entry(parent.clone())
                    .or_default()
                    .push(c.commit_hash.clone());
            }
        }
        for c in [m1, f1, m0] {
            commit_map.insert(c.commit_hash.clone(), c);
        }
        for hash in &commit_hashes {
            parents_map.entry(hash.clone()).or_default();
            children_map.entry(hash.clone()).or_default();
        }

        let ref_map: FxHashMap<CommitHash, Vec<Ref>> = FxHashMap::default();
        let repository = Repository::new(
            PathBuf::from("/dev/null"),
            commit_map,
            parents_map,
            children_map,
            ref_map,
            Head::None,
            commit_hashes.clone(),
            None,
        );

        let graph = calc_graph(&repository);

        let cell_width_type = CellWidthType::Double;
        let mut manager = GraphImageManager::new(
            &graph,
            graph_color_set,
            cell_width_type,
            style,
            GraphImageWidthMode::Fixed,
            image_protocol,
        );
        if let Some((r, g, b)) = bg_rgb {
            manager.update_background_color(r, g, b);
        }

        let mut rows = Vec::with_capacity(graph.commits.len());
        for commit in &graph.commits {
            manager.ensure_uploaded(&commit.commit_hash);
            if let Some(img) = manager.prepared_image(&commit.commit_hash) {
                rows.push(img.clone());
            }
        }

        let pending_uploads = manager.drain_pending_uploads();
        let image_ids: Vec<u32> = manager.image_ids().iter().copied().collect();

        GraphPreview {
            style,
            rows,
            pending_uploads,
            image_ids,
        }
    }
}

fn preview_commit(hash: &str, parents: &[&str]) -> Commit {
    Commit {
        commit_hash: hash.into(),
        parent_commit_hashes: parents.iter().map(|p| (*p).into()).collect(),
        commit_type: CommitType::Commit,
        ..Commit::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::GraphColorConfig;

    #[test]
    fn build_preview_for_each_style_produces_three_rows() {
        let color_set = GraphColorSet::new(&GraphColorConfig::default());
        for style in [
            GraphStyle::Smooth,
            GraphStyle::Rounded,
            GraphStyle::Angular,
        ] {
            let preview = GraphPreview::build(style, &color_set, ImageProtocol::Iterm2, None);
            assert_eq!(preview.rows.len(), 3, "style={:?}", style);
            assert_eq!(preview.style, style);
        }
    }
}
