use std::cmp::Ordering;
use std::collections::HashSet;

use crate::model::ChangedFile;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TreeNode {
    Dir {
        name: String,
        children: Vec<TreeNode>,
        /// Sum of `added` across the subtree.
        added: u64,
        /// Sum of `deleted` across the subtree.
        deleted: u64,
    },
    File(ChangedFile),
}

/// How sibling rows are ordered in the file list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortMode {
    /// By path (directories first, then files alphabetically).
    Path,
    /// By status letter, then path.
    Status,
    /// By added-line count descending, then path.
    Added,
}

pub fn build_tree(files: &[ChangedFile], sort: SortMode) -> Vec<TreeNode> {
    let mut root: Vec<TreeNode> = Vec::new();
    for file in files {
        insert_file(&mut root, &file.path, file.clone());
    }
    aggregate(&mut root);
    sort_tree(&mut root, sort);
    root
}

/// Wrap a forest of top-level dirs under a single root directory representing
/// the current directory. The root carries the aggregated counts.
pub fn wrap_root(children: Vec<TreeNode>, name: String) -> TreeNode {
    let mut added = 0u64;
    let mut deleted = 0u64;
    for child in &children {
        match child {
            TreeNode::Dir { added: a, deleted: d, .. } => {
                added += a;
                deleted += d;
            }
            TreeNode::File(f) => {
                added += f.added;
                deleted += f.deleted;
            }
        }
    }
    TreeNode::Dir {
        name,
        children,
        added,
        deleted,
    }
}

fn insert_file(nodes: &mut Vec<TreeNode>, path: &str, file: ChangedFile) {
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() == 1 {
        nodes.push(TreeNode::File(file));
        return;
    }
    let dir = parts[0].to_string();
    let rest = parts[1..].join("/");
    for node in nodes.iter_mut() {
        if let TreeNode::Dir { name, children, .. } = node
            && *name == dir
        {
            insert_file(children, &rest, file);
            return;
        }
    }
    let mut children = Vec::new();
    insert_file(&mut children, &rest, file);
    nodes.push(TreeNode::Dir {
        name: dir,
        children,
        added: 0,
        deleted: 0,
    });
}

/// Fill each Dir's `added`/`deleted` with the subtree sums.
fn aggregate(nodes: &mut [TreeNode]) {
    for node in nodes.iter_mut() {
        if let TreeNode::Dir { children, added, deleted, .. } = node {
            aggregate(children);
            let mut a = 0u64;
            let mut d = 0u64;
            for child in children.iter() {
                match child {
                    TreeNode::Dir { added: ca, deleted: cd, .. } => {
                        a += ca;
                        d += cd;
                    }
                    TreeNode::File(f) => {
                        a += f.added;
                        d += f.deleted;
                    }
                }
            }
            *added = a;
            *deleted = d;
        }
    }
}

fn status_rank(s: crate::model::Status) -> u8 {
    match s {
        crate::model::Status::Added => 0,
        crate::model::Status::Modified => 1,
        crate::model::Status::Renamed => 2,
        crate::model::Status::Deleted => 3,
        crate::model::Status::Untracked => 4,
    }
}

fn sort_tree(nodes: &mut [TreeNode], sort: SortMode) {
    nodes.sort_by(|a, b| match (a, b) {
        (TreeNode::Dir { name: an, .. }, TreeNode::Dir { name: bn, .. }) => an.cmp(bn),
        (TreeNode::Dir { .. }, TreeNode::File(_)) => Ordering::Less,
        (TreeNode::File(_), TreeNode::Dir { .. }) => Ordering::Greater,
        (TreeNode::File(fa), TreeNode::File(fb)) => match sort {
            SortMode::Path => fa.path.cmp(&fb.path),
            SortMode::Status => status_rank(fa.status)
                .cmp(&status_rank(fb.status))
                .then_with(|| fa.path.cmp(&fb.path)),
            SortMode::Added => fb.added
                .cmp(&fa.added)
                .then_with(|| fa.path.cmp(&fb.path)),
        },
    });
    for node in nodes.iter_mut() {
        if let TreeNode::Dir { children, .. } = node {
            sort_tree(children, sort);
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VisibleRow {
    Dir {
        path: String,
        depth: usize,
        collapsed: bool,
        added: u64,
        deleted: u64,
        /// Tree-guide prefix to draw before the name (e.g. `"  ├─"`).
        guide: String,
    },
    File {
        file: ChangedFile,
        depth: usize,
        /// Tree-guide prefix to draw before the name (e.g. `"  ├─"`).
        guide: String,
    },
}

pub fn visible_rows(nodes: &[TreeNode], collapsed: &HashSet<String>) -> Vec<VisibleRow> {
    let mut rows = Vec::new();
    walk(nodes, collapsed, "", "", 0, &mut rows);
    rows
}

fn walk(
    nodes: &[TreeNode],
    collapsed: &HashSet<String>,
    prefix: &str,
    path_prefix: &str,
    depth: usize,
    out: &mut Vec<VisibleRow>,
) {
    let count = nodes.len();
    for (i, node) in nodes.iter().enumerate() {
        let is_last = i + 1 == count;
        // the top-most node (the repo-root wrapper) has no parent, so it
        // gets no connector; its children start the guide lines.
        let guide = if depth == 0 {
            prefix.to_string()
        } else {
            let connector = if is_last { "└─" } else { "├─" };
            format!("{}{}", prefix, connector)
        };
        match node {
            TreeNode::Dir { name, children, added, deleted } => {
                let path = if path_prefix.is_empty() {
                    name.clone()
                } else {
                    format!("{}/{}", path_prefix, name)
                };
                let is_collapsed = collapsed.contains(&path);
                out.push(VisibleRow::Dir {
                    path: path.clone(),
                    depth,
                    collapsed: is_collapsed,
                    added: *added,
                    deleted: *deleted,
                    guide,
                });
                if !is_collapsed {
                    let child_prefix = format!("{}{}", prefix, if is_last { "  " } else { "│ " });
                    walk(children, collapsed, &child_prefix, &path, depth + 1, out);
                }
            }
            TreeNode::File(f) => out.push(VisibleRow::File {
                file: f.clone(),
                depth,
                guide,
            }),
        }
    }
}

/// All directory paths in the tree, in walk order.
pub fn all_dir_paths(nodes: &[TreeNode]) -> Vec<String> {
    let mut paths = Vec::new();
    collect_dirs(nodes, "", &mut paths);
    paths
}

fn collect_dirs(nodes: &[TreeNode], prefix: &str, out: &mut Vec<String>) {
    for node in nodes {
        if let TreeNode::Dir { name, children, .. } = node {
            let path = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{}/{}", prefix, name)
            };
            out.push(path.clone());
            collect_dirs(children, &path, out);
        }
    }
}

/// Rows for a filtered view: keeps files matching `pred` and keeps any
/// directory that has a matching descendant (empty dirs are hidden). Guide
/// lines are recomputed over the surviving siblings.
pub fn filtered_rows(
    nodes: &[TreeNode],
    pred: &dyn Fn(&ChangedFile) -> bool,
) -> Vec<VisibleRow> {
    fn keep_any(nodes: &[TreeNode], pred: &dyn Fn(&ChangedFile) -> bool) -> bool {
        nodes.iter().any(|n| match n {
            TreeNode::Dir { children, .. } => keep_any(children, pred),
            TreeNode::File(f) => pred(f),
        })
    }

    fn walk(
        nodes: &[TreeNode],
        pred: &dyn Fn(&ChangedFile) -> bool,
        prefix: &str,
        path_prefix: &str,
        depth: usize,
        out: &mut Vec<VisibleRow>,
    ) {
        let kept: Vec<&TreeNode> = nodes
            .iter()
            .filter(|n| match n {
                TreeNode::Dir { children, .. } => keep_any(children, pred),
                TreeNode::File(f) => pred(f),
            })
            .collect();
        let count = kept.len();
        for (i, node) in kept.iter().enumerate() {
            let is_last = i + 1 == count;
            // the top-most node has no parent, so no connector at depth 0
            let guide = if depth == 0 {
                prefix.to_string()
            } else {
                let connector = if is_last { "└─" } else { "├─" };
                format!("{}{}", prefix, connector)
            };
            match node {
                TreeNode::Dir { name, children, added, deleted } => {
                    let path = if path_prefix.is_empty() {
                        name.clone()
                    } else {
                        format!("{}/{}", path_prefix, name)
                    };
                    out.push(VisibleRow::Dir {
                        path: path.clone(),
                        depth,
                        collapsed: false,
                        added: *added,
                        deleted: *deleted,
                        guide,
                    });
                    let child_prefix = format!("{}{}", prefix, if is_last { "  " } else { "│ " });
                    walk(children, pred, &child_prefix, &path, depth + 1, out);
                }
                TreeNode::File(f) => out.push(VisibleRow::File {
                    file: f.clone(),
                    depth,
                    guide,
                }),
            }
        }
    }

    let mut rows = Vec::new();
    walk(nodes, pred, "", "", 0, &mut rows);
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Status;

    pub fn file_path_at_row(row: &VisibleRow) -> Option<&str> {
        match row {
            VisibleRow::File { file, .. } => Some(&file.path),
            _ => None,
        }
    }

    pub fn is_dir_row(row: &VisibleRow) -> bool {
        matches!(row, VisibleRow::Dir { .. })
    }

    pub fn count_changed(nodes: &[TreeNode]) -> usize {
        nodes
            .iter()
            .map(|n| match n {
                TreeNode::Dir { children, .. } => count_changed(children),
                TreeNode::File(_) => 1,
            })
            .sum()
    }

    fn f(status: Status, path: &str) -> ChangedFile {
        ChangedFile {
            status,
            path: path.to_string(),
            old_path: None,
            added: 1,
            deleted: 0,
            is_binary: false,
        }
    }

    #[test]
    fn builds_nested_tree_with_dirs_first() {
        let files = vec![
            f(Status::Modified, "src/main.rs"),
            f(Status::Added, "notes.md"),
            f(Status::Deleted, "src/deep/lib.rs"),
        ];
        let tree = build_tree(&files, SortMode::Path);
        assert_eq!(tree.len(), 2);
        match &tree[0] {
            TreeNode::Dir { name, .. } => assert_eq!(name, "src"),
            _ => panic!("expected dir first"),
        }
        assert_eq!(tree[1], TreeNode::File(f(Status::Added, "notes.md")));
    }

    #[test]
    fn visible_rows_respect_collapse() {
        let files = vec![f(Status::Modified, "src/a.rs"), f(Status::Modified, "src/b.rs")];
        let tree = build_tree(&files, SortMode::Path);
        let mut collapsed = HashSet::new();
        let rows = visible_rows(&tree, &collapsed);
        assert_eq!(rows.len(), 3);
        assert!(is_dir_row(&rows[0]));
        assert!(file_path_at_row(&rows[1]).is_some());
        collapsed.insert("src".to_string());
        let rows = visible_rows(&tree, &collapsed);
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn counts_all_files() {
        let files = vec![f(Status::Modified, "src/a.rs"), f(Status::Modified, "src/b.rs"), f(Status::Added, "c.rs")];
        let tree = build_tree(&files, SortMode::Path);
        assert_eq!(count_changed(&tree), 3);
    }

    #[test]
    fn aggregates_dir_added_deleted() {
        let files = vec![
            f(Status::Modified, "src/main.rs"),
            f(Status::Added, "src/notes.md"),
        ];
        let tree = build_tree(&files, SortMode::Path);
        match &tree[0] {
            TreeNode::Dir { name, added, deleted, .. } => {
                assert_eq!(name, "src");
                assert_eq!(*added, 2);
                assert_eq!(*deleted, 0);
            }
            _ => panic!("expected dir"),
        }
    }

    #[test]
    fn sort_by_added_desc() {
        let mut m = f(Status::Modified, "z.rs");
        m.added = 5;
        let mut n = f(Status::Modified, "a.rs");
        n.added = 50;
        let files = vec![m, n];
        let tree = build_tree(&files, SortMode::Added);
        match &tree[0] {
            TreeNode::File(f0) => assert_eq!(f0.path, "a.rs"),
            _ => panic!("expected file"),
        }
    }

    #[test]
    fn filtered_rows_hide_empty_dirs_and_keep_guides() {
        let files = vec![
            f(Status::Modified, "src/keep.rs"),
            f(Status::Modified, "src/drop.rs"),
            f(Status::Added, "top.rs"),
        ];
        let tree = build_tree(&files, SortMode::Path);
        let pred = |f: &ChangedFile| f.path.contains("keep");
        let rows = filtered_rows(&tree, &pred);
        let paths: Vec<&str> = rows
            .iter()
            .map(|r| match r {
                VisibleRow::File { file, .. } => file.path.as_str(),
                VisibleRow::Dir { path, .. } => path.as_str(),
            })
            .collect();
        assert_eq!(paths, vec!["src", "src/keep.rs"]);
    }

    #[test]
    fn all_dir_paths_lists_every_dir() {
        let files = vec![f(Status::Modified, "src/a.rs"), f(Status::Modified, "src/deep/b.rs")];
        let tree = build_tree(&files, SortMode::Path);
        assert_eq!(all_dir_paths(&tree), vec!["src", "src/deep"]);
    }
}