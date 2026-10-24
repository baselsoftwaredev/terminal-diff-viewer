use std::collections::{BTreeMap, HashSet};

#[derive(Clone, Debug)]
pub struct Row {
    pub path: String,
    pub label: String,
    pub depth: usize,
    pub file: Option<usize>,
}

#[derive(Default)]
struct Node {
    children: BTreeMap<String, Node>,
    file: Option<usize>,
}

pub fn rows<'a>(paths: impl Iterator<Item = &'a str>, collapsed: &HashSet<String>) -> Vec<Row> {
    let mut root = Node::default();
    for (index, path) in paths.enumerate() {
        let mut node = &mut root;
        for part in path.split('/') {
            node = node.children.entry(part.to_owned()).or_default();
        }
        node.file = Some(index);
    }
    fn visit(
        node: &Node,
        prefix: &str,
        depth: usize,
        collapsed: &HashSet<String>,
        rows: &mut Vec<Row>,
    ) {
        // Directories first, then files; each group is alphabetical.
        for directory in [true, false] {
            for (label, child) in &node.children {
                if (directory && child.children.is_empty()) || (!directory && child.file.is_none())
                {
                    continue;
                }
                let path = if prefix.is_empty() {
                    label.clone()
                } else {
                    format!("{prefix}/{label}")
                };
                rows.push(Row {
                    path: path.clone(),
                    label: label.clone(),
                    depth,
                    file: if directory { None } else { child.file },
                });
                if directory && !collapsed.contains(&path) {
                    visit(child, &path, depth + 1, collapsed, rows);
                }
            }
        }
    }
    let mut rows = Vec::new();
    visit(&root, "", 0, collapsed, &mut rows);
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deleted_file_can_be_replaced_by_a_directory() {
        let paths = ["entry", "entry/child.rs"];
        let tree = rows(paths.iter().copied(), &HashSet::new());
        assert_eq!(tree.len(), 3);
        assert_eq!(tree[0].file, None);
        assert_eq!(tree[1].file, Some(1));
        assert_eq!(tree[2].file, Some(0));
    }

    #[test]
    fn nested_folders_sort_and_keep_original_file_indices() {
        let paths = ["z.rs", "src/nested/b.rs", "src/a.rs", "other/c.rs"];
        let open = rows(paths.iter().copied(), &HashSet::new());
        assert_eq!(
            open.iter().map(|r| r.path.as_str()).collect::<Vec<_>>(),
            [
                "other",
                "other/c.rs",
                "src",
                "src/nested",
                "src/nested/b.rs",
                "src/a.rs",
                "z.rs"
            ]
        );
        assert_eq!(open[4].file, Some(1));
        let closed = rows(paths.iter().copied(), &HashSet::from(["src".into()]));
        assert_eq!(
            closed.iter().map(|r| r.path.as_str()).collect::<Vec<_>>(),
            ["other", "other/c.rs", "src", "z.rs"]
        );
    }
}
