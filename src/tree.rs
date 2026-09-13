use crate::model::ChangedFile;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TreeNode {
    Dir { name: String, children: Vec<TreeNode> },
    File(ChangedFile),
}

pub fn build_tree(files: &[ChangedFile]) -> Vec<TreeNode> {
    let mut root: Vec<TreeNode> = Vec::new();
    for file in files {
        insert_file(&mut root, &file.path, file.clone());
    }
    sort_tree(&mut root);
    root
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
        if let TreeNode::Dir { name, children } = node
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
    });
}

fn sort_tree(nodes: &mut [TreeNode]) {
    nodes.sort_by_key(|n| match n {
        TreeNode::Dir { name, .. } => (0, name.clone()),
        TreeNode::File(f) => (1, f.path.clone()),
    });
    for node in nodes.iter_mut() {
        if let TreeNode::Dir { children, .. } = node {
            sort_tree(children);
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VisibleRow {
    Dir { path: String, depth: usize, collapsed: bool },
    File { file: ChangedFile, depth: usize },
}

pub fn visible_rows(nodes: &[TreeNode], collapsed: &std::collections::HashSet<String>, prefix: &str) -> Vec<VisibleRow> {
    let mut rows = Vec::new();
    walk(nodes, collapsed, prefix, 0, &mut rows);
    rows
}

fn walk(
    nodes: &[TreeNode],
    collapsed: &std::collections::HashSet<String>,
    prefix: &str,
    depth: usize,
    out: &mut Vec<VisibleRow>,
) {
    for node in nodes {
        match node {
            TreeNode::Dir { name, children } => {
                let path = if prefix.is_empty() {
                    name.clone()
                } else {
                    format!("{}/{}", prefix, name)
                };
                let is_collapsed = collapsed.contains(&path);
                out.push(VisibleRow::Dir {
                    path: path.clone(),
                    depth,
                    collapsed: is_collapsed,
                });
                if !is_collapsed {
                    walk(children, collapsed, &path, depth + 1, out);
                }
            }
            TreeNode::File(f) => out.push(VisibleRow::File {
                file: f.clone(),
                depth,
            }),
        }
    }
}

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

pub fn rename_target(row: &VisibleRow) -> Option<&ChangedFile> {
    match row {
        VisibleRow::File { file, .. } => Some(file),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Status;

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
        let tree = build_tree(&files);
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
        let tree = build_tree(&files);
        let mut collapsed = std::collections::HashSet::new();
        let rows = visible_rows(&tree, &collapsed, "");
        assert_eq!(rows.len(), 3);
        assert!(is_dir_row(&rows[0]));
        assert!(file_path_at_row(&rows[1]).is_some());
        collapsed.insert("src".to_string());
        let rows = visible_rows(&tree, &collapsed, "");
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn counts_all_files() {
        let files = vec![f(Status::Modified, "src/a.rs"), f(Status::Modified, "src/b.rs"), f(Status::Added, "c.rs")];
        let tree = build_tree(&files);
        assert_eq!(count_changed(&tree), 3);
    }
}