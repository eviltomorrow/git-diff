use similar::{DiffTag, TextDiff};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    Equal,
    Delete,
    Insert,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cell {
    pub num: u64,
    pub text: String,
    pub kind: LineKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AlignedRow {
    pub original: Option<Cell>,
    pub changed: Option<Cell>,
}

fn to_text(content: Option<&[u8]>) -> String {
    match content {
        None => String::new(),
        Some(bytes) => String::from_utf8_lossy(bytes).into_owned(),
    }
}

fn to_lines(content: Option<&[u8]>) -> Vec<String> {
    match content {
        None => Vec::new(),
        Some(bytes) => String::from_utf8_lossy(bytes)
            .lines()
            .map(|l| l.to_string())
            .collect(),
    }
}

pub fn align_rows(original: Option<&[u8]>, changed: Option<&[u8]>) -> Vec<AlignedRow> {
    let old_text = to_text(original);
    let new_text = to_text(changed);
    let old_lines = to_lines(original);
    let new_lines = to_lines(changed);
    let diff = TextDiff::from_lines(&old_text, &new_text);
    let mut rows = Vec::new();
    let mut old_num: u64 = 0;
    let mut new_num: u64 = 0;
    for op in diff.ops() {
        let old_range = op.old_range();
        let new_range = op.new_range();
        match op.tag() {
                DiffTag::Equal => {
                    for (o, n) in old_lines[old_range].iter().zip(&new_lines[new_range]) {
                        old_num += 1;
                        new_num += 1;
                        rows.push(AlignedRow {
                            original: Some(Cell {
                                num: old_num,
                                text: o.clone(),
                                kind: LineKind::Equal,
                            }),
                            changed: Some(Cell {
                                num: new_num,
                                text: n.clone(),
                                kind: LineKind::Equal,
                            }),
                        });
                    }
                }
                DiffTag::Delete => {
                    for o in &old_lines[old_range] {
                        old_num += 1;
                        rows.push(AlignedRow {
                            original: Some(Cell {
                                num: old_num,
                                text: o.clone(),
                                kind: LineKind::Delete,
                            }),
                            changed: None,
                        });
                    }
                }
                DiffTag::Insert => {
                    for n in &new_lines[new_range] {
                        new_num += 1;
                        rows.push(AlignedRow {
                            original: None,
                            changed: Some(Cell {
                                num: new_num,
                                text: n.clone(),
                                kind: LineKind::Insert,
                            }),
                        });
                    }
                }
DiffTag::Replace => {
                    let old_slice = &old_lines[old_range];
                    let new_slice = &new_lines[new_range];
                    let pairs = old_slice.len().min(new_slice.len());
                    for k in 0..pairs {
                        old_num += 1;
                        new_num += 1;
                        rows.push(AlignedRow {
                            original: Some(Cell {
                                num: old_num,
                                text: old_slice[k].clone(),
                                kind: LineKind::Delete,
                            }),
                            changed: Some(Cell {
                                num: new_num,
                                text: new_slice[k].clone(),
                                kind: LineKind::Insert,
                            }),
                        });
                    }
                    for o in &old_slice[pairs..] {
                        old_num += 1;
                        rows.push(AlignedRow {
                            original: Some(Cell {
                                num: old_num,
                                text: o.clone(),
                                kind: LineKind::Delete,
                            }),
                            changed: None,
                        });
                    }
                    for n in &new_slice[pairs..] {
                        new_num += 1;
                        rows.push(AlignedRow {
                            original: None,
                            changed: Some(Cell {
                                num: new_num,
                                text: n.clone(),
                                kind: LineKind::Insert,
                            }),
                        });
                    }
                }
        }
    }
    rows
}

fn is_change(row: &AlignedRow) -> bool {
    row.original.as_ref().map(|c| c.kind != LineKind::Equal).unwrap_or(false)
        || row.changed.as_ref().map(|c| c.kind != LineKind::Equal).unwrap_or(false)
}

pub fn hunk_starts(rows: &[AlignedRow]) -> Vec<usize> {
    let mut starts = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        if is_change(row) {
            let prev_is_change = i > 0 && is_change(&rows[i - 1]);
            if !prev_is_change {
                starts.push(i);
            }
        }
    }
    starts
}

pub fn plain_rows(original: Option<&[u8]>, changed: Option<&[u8]>) -> Vec<AlignedRow> {
    let old_lines = to_lines(original);
    let new_lines = to_lines(changed);
    let mut rows = Vec::new();
    for (i, l) in old_lines.iter().enumerate() {
        rows.push(AlignedRow {
            original: Some(Cell {
                num: (i + 1) as u64,
                text: l.clone(),
                kind: LineKind::Equal,
            }),
            changed: None,
        });
    }
    for (i, l) in new_lines.iter().enumerate() {
        rows.push(AlignedRow {
            original: None,
            changed: Some(Cell {
                num: (i + 1) as u64,
                text: l.clone(),
                kind: LineKind::Equal,
            }),
        });
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_lines_align_both_sides() {
        let rows = align_rows(Some(b"a\nb\n"), Some(b"a\nb\n"));
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.original.is_some() && r.changed.is_some()));
        assert_eq!(rows[0].original.as_ref().unwrap().num, 1);
        assert_eq!(rows[0].changed.as_ref().unwrap().num, 1);
        assert_eq!(rows[1].original.as_ref().unwrap().num, 2);
    }

    #[test]
    fn insert_lines_have_no_original() {
        let rows = align_rows(Some(b"a\n"), Some(b"a\nb\n"));
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].original, None);
        assert_eq!(rows[1].changed.as_ref().unwrap().text, "b");
        assert_eq!(rows[1].changed.as_ref().unwrap().kind, LineKind::Insert);
    }

    #[test]
    fn delete_lines_have_no_changed() {
        let rows = align_rows(Some(b"a\nb\n"), Some(b"a\n"));
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].changed, None);
        assert_eq!(rows[1].original.as_ref().unwrap().text, "b");
        assert_eq!(rows[1].original.as_ref().unwrap().kind, LineKind::Delete);
    }

    #[test]
    fn modified_line_aligns_both_sides() {
        let rows = align_rows(Some(b"foo()\n"), Some(b"bar()\n"));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].original.as_ref().unwrap().text, "foo()");
        assert_eq!(rows[0].original.as_ref().unwrap().kind, LineKind::Delete);
        assert_eq!(rows[0].changed.as_ref().unwrap().text, "bar()");
        assert_eq!(rows[0].changed.as_ref().unwrap().kind, LineKind::Insert);
        assert_eq!(rows[0].original.as_ref().unwrap().num, 1);
        assert_eq!(rows[0].changed.as_ref().unwrap().num, 1);
    }

    #[test]
    fn replaced_block_pairs_then_spills() {
        // 2 old lines replaced by 3 new: 2 aligned pairs + 1 extra insert
        let rows = align_rows(Some(b"x\ny\nz\n"), Some(b"a\nb\nc\nd\n"));
        // x→a, y→b paired; z deleted; c,d inserted => need to check content
        let origs: Vec<(u64, &str, LineKind)> = rows
            .iter()
            .filter_map(|r| r.original.as_ref().map(|c| (c.num, c.text.as_str(), c.kind)))
            .collect();
        let news: Vec<(u64, &str, LineKind)> = rows
            .iter()
            .filter_map(|r| r.changed.as_ref().map(|c| (c.num, c.text.as_str(), c.kind)))
            .collect();
        // x,y,z all on original side (x,y paired, z deleted)
        assert_eq!(origs.len(), 3);
        assert_eq!(origs[0], (1, "x", LineKind::Delete));
        assert_eq!(origs[1], (2, "y", LineKind::Delete));
        assert_eq!(origs[2], (3, "z", LineKind::Delete));
        // a,b paired on changed side; c,d inserted
        assert_eq!(news.len(), 4);
        assert_eq!(news[0], (1, "a", LineKind::Insert));
        assert_eq!(news[1], (2, "b", LineKind::Insert));
        assert_eq!(news[2], (3, "c", LineKind::Insert));
        assert_eq!(news[3], (4, "d", LineKind::Insert));
        // paired rows carry both sides
        assert!(rows[0].original.is_some() && rows[0].changed.is_some());
        assert!(rows[1].original.is_some() && rows[1].changed.is_some());
    }

    #[test]
    fn empty_sides_produce_no_rows() {
        assert!(!align_rows(None, Some(b"a\n")).is_empty());
        assert!(!align_rows(Some(b"a\n"), None).is_empty());
        assert!(align_rows(None, None).is_empty());
    }

    #[test]
    fn hunk_starts_finds_change_groups() {
        let rows = align_rows(
            Some(b"a\nb\nfoo()\nc\n"),
            Some(b"a\nb\nbar()\nc\n"),
        );
        let starts = hunk_starts(&rows);
        assert_eq!(starts.len(), 1);
        assert_eq!(rows[starts[0]].original.as_ref().unwrap().text, "foo()");
    }

    #[test]
    fn line_numbers_increment_through_changes() {
        let rows = align_rows(
            Some(b"a\nb\nc\n"),
            Some(b"a\nx\nc\n"),
        );
        let originals: Vec<u64> = rows.iter().filter_map(|r| r.original.as_ref().map(|c| c.num)).collect();
        let changed: Vec<u64> = rows.iter().filter_map(|r| r.changed.as_ref().map(|c| c.num)).collect();
        assert_eq!(originals, vec![1, 2, 3]);
        assert_eq!(changed, vec![1, 2, 3]);
    }

    #[test]
    fn multi_hunk_offsets_are_correct() {
        let rows = align_rows(
            Some(b"a\nb\nc\nd\ne\nf\n"),
            Some(b"a\nB\nc\nd\nE\nf\n"),
        );
        let starts = hunk_starts(&rows);
        assert_eq!(starts.len(), 2);
    }
}