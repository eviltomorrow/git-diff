use similar::{ChangeTag, DiffTag, TextDiff};
use unicode_width::UnicodeWidthStr;

/// Tab stop used when expanding tabs in displayed diff lines. A literal tab
/// byte sent to the terminal is expanded by the terminal to the next tab stop,
/// which is far wider than the single cell ratatui's buffer accounts for, so
/// long lines overflow the pane and leave ghost residue on file switch. We
/// therefore expand tabs to spaces up front so the computed display width
/// matches the terminal's.
pub const TAB_STOP: usize = 8;

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
    /// Optional inline fragments for Delete/Insert cells on a paired replace:
    /// `(emphasized, text)` where emphasized means "changed within the line".
    pub inline: Option<Vec<(bool, String)>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AlignedRow {
    pub original: Option<Cell>,
    pub changed: Option<Cell>,
}

/// Inline fragments for a changed line: `(emphasized, text)`.
pub type InlineFragments = Vec<(bool, String)>;

fn to_text(content: Option<&[u8]>) -> String {
    match content {
        None => String::new(),
        Some(bytes) => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// Replaces every tab with spaces up to the next `TAB_STOP`-column tab stop,
/// so the display width of a line (as measured by [`UnicodeWidthStr`]) matches
/// what a terminal actually renders. Keeps the line count unchanged, so diff
/// ops that index into the line arrays stay valid.
fn expand_tabs(line: &str) -> String {
    if !line.contains('\t') {
        return line.to_string();
    }
    let mut out = String::with_capacity(line.len() + TAB_STOP);
    let mut col = 0usize;
    for c in line.chars() {
        if c == '\t' {
            let pad = TAB_STOP - (col % TAB_STOP);
            out.push_str(&" ".repeat(pad));
            col += pad;
        } else {
            out.push(c);
            col += UnicodeWidthStr::width(c.to_string().as_str());
        }
    }
    out
}

fn to_lines(content: Option<&[u8]>) -> Vec<String> {
    match content {
        None => Vec::new(),
        Some(bytes) => String::from_utf8_lossy(bytes)
            .lines()
            .map(expand_tabs)
            .collect(),
    }
}

/// Strips trailing whitespace from every line without changing the line
/// count, so diff ops still index into the original line arrays.
fn trim_end_all(text: &str) -> String {
    text.lines()
        .map(|l| l.trim_end_matches([' ', '\t']))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Aligns two file contents into side-by-side rows.
///
/// When `ignore_whitespace` is true, trailing whitespace is stripped from
/// lines *before* diffing, but the original line text is kept for display.
pub fn align_rows(
    original: Option<&[u8]>,
    changed: Option<&[u8]>,
    ignore_whitespace: bool,
) -> Vec<AlignedRow> {
    let old_text = to_text(original);
    let new_text = to_text(changed);
    let old_lines = to_lines(original);
    let new_lines = to_lines(changed);
    let diff_text_old = if ignore_whitespace { trim_end_all(&old_text) } else { old_text.clone() };
    let diff_text_new = if ignore_whitespace { trim_end_all(&new_text) } else { new_text.clone() };
    let diff = TextDiff::from_lines(&diff_text_old, &diff_text_new);
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
                                inline: None,
                            }),
                            changed: Some(Cell {
                                num: new_num,
                                text: n.clone(),
                                kind: LineKind::Equal,
                                inline: None,
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
                                inline: None,
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
                                inline: None,
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
                        let (old_inline, new_inline) =
                            inline_fragments(&old_slice[k], &new_slice[k]);
                        rows.push(AlignedRow {
                            original: Some(Cell {
                                num: old_num,
                                text: old_slice[k].clone(),
                                kind: LineKind::Delete,
                                inline: old_inline,
                            }),
                            changed: Some(Cell {
                                num: new_num,
                                text: new_slice[k].clone(),
                                kind: LineKind::Insert,
                                inline: new_inline,
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
                                inline: None,
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
                                inline: None,
                            }),
                        });
                    }
                }
        }
    }
    rows
}

/// Computes inline (word-level) emphasis fragments for a paired old/new line.
/// Returns `(old_fragments, new_fragments)` where each fragment is
/// `(emphasized, text)`.
fn inline_fragments(old: &str, new: &str) -> (Option<InlineFragments>, Option<InlineFragments>) {
    let diff = TextDiff::from_lines(old, new);
    let mut old_frags: InlineFragments = Vec::new();
    let mut new_frags: InlineFragments = Vec::new();
    let mut used_old = false;
    let mut used_new = false;
    for op in diff.ops() {
        for change in diff.iter_inline_changes(op) {
            let mut frags: Vec<(bool, String)> = Vec::new();
            for (emph, val) in change.iter_strings_lossy() {
                frags.push((emph, val.into_owned()));
            }
            match change.tag() {
                ChangeTag::Equal => {
                    if !frags.is_empty() {
                        old_frags.extend(frags.clone());
                        new_frags.extend(frags);
                    }
                }
                ChangeTag::Delete => {
                    if !frags.is_empty() {
                        old_frags.extend(frags);
                        used_old = true;
                    }
                }
                ChangeTag::Insert => {
                    if !frags.is_empty() {
                        new_frags.extend(frags);
                        used_new = true;
                    }
                }
            }
        }
    }
    let old_inline = if used_old { Some(old_frags) } else { None };
    let new_inline = if used_new { Some(new_frags) } else { None };
    (old_inline, new_inline)
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

/// Returns the 1-based index of the hunk containing `cursor_row`,
/// or 0 when the cursor is above the first hunk.
pub fn hunk_index(starts: &[usize], cursor_row: usize) -> usize {
    starts
        .iter()
        .rposition(|&s| s <= cursor_row)
        .map(|i| i + 1)
        .unwrap_or(0)
}

/// A maximal run of unchanged rows (both sides present and Equal).
/// `start` is inclusive, `end` is exclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FoldRun {
    pub start: usize,
    pub end: usize,
}

/// Returns the runs of consecutive unchanged rows. Rows are "unchanged" only
/// when both the original and changed sides are present and Equal, so runs
/// never overlap hunks.
pub fn fold_runs(rows: &[AlignedRow]) -> Vec<FoldRun> {
    let mut runs = Vec::new();
    let mut i = 0;
    while i < rows.len() {
        if is_unchanged(&rows[i]) {
            let start = i;
            while i < rows.len() && is_unchanged(&rows[i]) {
                i += 1;
            }
            runs.push(FoldRun { start, end: i });
        } else {
            i += 1;
        }
    }
    runs
}

fn is_unchanged(row: &AlignedRow) -> bool {
    row.original.as_ref().map(|c| c.kind == LineKind::Equal).unwrap_or(false)
        && row.changed.as_ref().map(|c| c.kind == LineKind::Equal).unwrap_or(false)
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
                inline: None,
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
                inline: None,
            }),
        });
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_tabs_pads_to_tab_stop() {
        assert_eq!(expand_tabs("\tfoo"), "        foo");
        assert_eq!(expand_tabs("\t\tfoo"), "                foo");
        assert_eq!(expand_tabs("a\tb"), "a       b");
        assert_eq!(expand_tabs("no tabs"), "no tabs");
        assert_eq!(expand_tabs(""), "");
    }

    #[test]
    fn expand_tabs_keeps_line_count() {
        let lines = to_lines(Some(b"\tfoo\n\t\tbar\n"));
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|l| !l.contains('\t')));
    }

    #[test]
    fn equal_lines_align_both_sides() {
        let rows = align_rows(Some(b"a\nb\n"), Some(b"a\nb\n"), false);
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.original.is_some() && r.changed.is_some()));
        assert_eq!(rows[0].original.as_ref().unwrap().num, 1);
        assert_eq!(rows[0].changed.as_ref().unwrap().num, 1);
        assert_eq!(rows[1].original.as_ref().unwrap().num, 2);
    }

    #[test]
    fn insert_lines_have_no_original() {
        let rows = align_rows(Some(b"a\n"), Some(b"a\nb\n"), false);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].original, None);
        assert_eq!(rows[1].changed.as_ref().unwrap().text, "b");
        assert_eq!(rows[1].changed.as_ref().unwrap().kind, LineKind::Insert);
    }

    #[test]
    fn delete_lines_have_no_changed() {
        let rows = align_rows(Some(b"a\nb\n"), Some(b"a\n"), false);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].changed, None);
        assert_eq!(rows[1].original.as_ref().unwrap().text, "b");
        assert_eq!(rows[1].original.as_ref().unwrap().kind, LineKind::Delete);
    }

    #[test]
    fn modified_line_aligns_both_sides() {
        let rows = align_rows(Some(b"foo()\n"), Some(b"bar()\n"), false);
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
        let rows = align_rows(Some(b"x\ny\nz\n"), Some(b"a\nb\nc\nd\n"), false);
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
        assert!(!align_rows(None, Some(b"a\n"), false).is_empty());
        assert!(!align_rows(Some(b"a\n"), None, false).is_empty());
        assert!(align_rows(None, None, false).is_empty());
    }

    #[test]
    fn hunk_starts_finds_change_groups() {
        let rows = align_rows(
            Some(b"a\nb\nfoo()\nc\n"),
            Some(b"a\nb\nbar()\nc\n"),
            false,
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
            false,
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
            false,
        );
        let starts = hunk_starts(&rows);
        assert_eq!(starts.len(), 2);
    }

    #[test]
    fn hunk_index_picks_last_containing_hunk() {
        let starts = vec![5, 40];
        assert_eq!(hunk_index(&starts, 0), 0);
        assert_eq!(hunk_index(&starts, 5), 1);
        assert_eq!(hunk_index(&starts, 39), 1);
        assert_eq!(hunk_index(&starts, 40), 2);
        assert_eq!(hunk_index(&starts, 59), 2);
        assert_eq!(hunk_index(&[], 10), 0);
    }

    #[test]
    fn fold_runs_find_unchanged_spans() {
        let rows = align_rows(
            Some(b"a\nb\nc\nd\ne\nf\n"),
            Some(b"a\nB\nc\nd\nE\nf\n"),
            false,
        );
        // rows: a(0 equal), B(1 change), c(2), d(3), E(4 change), f(5)
        let runs = fold_runs(&rows);
        assert_eq!(runs, vec![
            FoldRun { start: 0, end: 1 },
            FoldRun { start: 2, end: 4 },
            FoldRun { start: 5, end: 6 },
        ]);
    }

    #[test]
    fn fold_runs_empty_for_all_changed() {
        let rows = align_rows(Some(b"a\nb\n"), Some(b"c\nd\n"), false);
        assert!(fold_runs(&rows).is_empty());
    }

    #[test]
    fn inline_fragments_mark_changed_words() {
        let rows = align_rows(
            Some(b"let x = foo(a, b);\n"),
            Some(b"let x = foo(c, b);\n"),
            false,
        );
        assert_eq!(rows.len(), 1);
        let orig = rows[0].original.as_ref().unwrap();
        let changed = rows[0].changed.as_ref().unwrap();
        let inline_old = orig.inline.as_ref().expect("old inline");
        let inline_new = changed.inline.as_ref().expect("new inline");
        // changed word is emphasized on each side; shared text is not
        let old_emphasized: String = inline_old.iter().filter(|(e, _)| *e).map(|(_, t)| t.as_str()).collect();
        let new_emphasized: String = inline_new.iter().filter(|(e, _)| *e).map(|(_, t)| t.as_str()).collect();
        assert_eq!(old_emphasized, "foo(a,");
        assert_eq!(new_emphasized, "foo(c,");
        // both sides keep the full original/new text across all fragments
        let old_all: String = inline_old.iter().map(|(_, t)| t.as_str()).collect();
        let new_all: String = inline_new.iter().map(|(_, t)| t.as_str()).collect();
        assert_eq!(old_all, "let x = foo(a, b);");
        assert_eq!(new_all, "let x = foo(c, b);");
    }

    #[test]
    fn pure_delete_lines_have_no_inline() {
        let rows = align_rows(Some(b"a\nb\n"), Some(b"a\n"), false);
        assert_eq!(rows[1].original.as_ref().unwrap().inline, None);
    }

    #[test]
    fn equal_lines_have_no_inline() {
        let rows = align_rows(Some(b"a\n"), Some(b"a\n"), false);
        assert_eq!(rows[0].original.as_ref().unwrap().inline, None);
        assert_eq!(rows[0].changed.as_ref().unwrap().inline, None);
    }

    #[test]
    fn ignore_whitespace_treats_trailing_space_changes_as_equal() {
        // "a " vs "a": normally a Replace (1 paired row with change markers);
        // when ignoring whitespace it must become a plain Equal row.
        let normal = align_rows(Some(b"a \n"), Some(b"a\n"), false);
        assert_eq!(normal.len(), 1);
        assert_eq!(normal[0].original.as_ref().unwrap().kind, LineKind::Delete);
        assert_eq!(normal[0].changed.as_ref().unwrap().kind, LineKind::Insert);

        let ws = align_rows(Some(b"a \n"), Some(b"a\n"), true);
        assert_eq!(ws.len(), 1);
        // both sides equal, no change emphasis
        assert_eq!(ws[0].original.as_ref().unwrap().kind, LineKind::Equal);
        assert_eq!(ws[0].changed.as_ref().unwrap().kind, LineKind::Equal);
        assert_eq!(ws[0].original.as_ref().unwrap().inline, None);
        // original text preserved for display despite the normalized diff
        assert_eq!(ws[0].original.as_ref().unwrap().text, "a ");
        assert_eq!(ws[0].changed.as_ref().unwrap().text, "a");
    }

    #[test]
    fn ignore_whitespace_still_diffs_real_changes() {
        let ws = align_rows(Some(b"foo \n"), Some(b"bar\n"), true);
        // real content change is still flagged as a change, not Equal
        assert!(ws.iter().any(|r| r.original.as_ref().is_some_and(|c| c.kind == LineKind::Delete)));
        assert!(ws.iter().any(|r| r.changed.as_ref().is_some_and(|c| c.kind == LineKind::Insert)));
    }
}