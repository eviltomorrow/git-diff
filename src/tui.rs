use std::path::PathBuf;

use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use ratatui::{Frame, Terminal};
use unicode_width::UnicodeWidthStr;

use crate::align::{Cell as AlignCell, LineKind};
use crate::controller::{Controller, Focus, Overlay};
use crate::git::{GitFacade, GitRunner};
use crate::model::{ChangedFile, CommitEntry, ComparisonMode, Status};
use crate::styles;
use crate::tree::{SortMode, VisibleRow};

const LIST_RATIO: u16 = 26;

/// Thin shell: owns the terminal and the pure [`Controller`]; rendering is
/// read-only over controller state (viewports are synced via
/// [`Controller::update_viewports`] before drawing).
pub struct App<'a> {
    ctrl: Controller<'a>,
}

impl<'a> App<'a> {
    pub fn new(
        facade: GitFacade<'a>,
        repo_path: PathBuf,
        has_commits: bool,
        initial_commit: Option<String>,
    ) -> Result<Self> {
        let ctrl = Controller::new(facade, repo_path, has_commits, initial_commit)?;
        Ok(Self { ctrl })
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        self.ctrl.handle_key(key);
    }

    pub fn render(&mut self, f: &mut Frame) {
        let area = f.area();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),
                Constraint::Min(2),
                Constraint::Length(1),
            ])
            .split(area);

        // sync viewports before drawing so navigation clamps use real sizes
        let panels = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(LIST_RATIO), Constraint::Percentage(100 - LIST_RATIO)])
            .split(chunks[1]);
        let list_height = panels[0].height.saturating_sub(2) as usize;
        let diff_height = panels[1].height.saturating_sub(3) as usize;
        let diff_width = (panels[1].width.saturating_div(2)).saturating_sub(10) as usize;
        let max_line_w = self
            .ctrl
            .diff_rows
            .iter()
            .filter_map(|r| {
                r.original
                    .as_ref()
                    .or(r.changed.as_ref())
                    .map(|c| UnicodeWidthStr::width(c.text.as_str()))
            })
            .max()
            .unwrap_or(0);
        self.ctrl.update_viewports(list_height, diff_height, diff_width, max_line_w);

        render_header(f, chunks[0], &self.ctrl);
        render_filelist(f, panels[0], &mut self.ctrl);
        render_diffview(f, panels[1], &mut self.ctrl);
        render_statusbar(f, chunks[2], &self.ctrl);

        match &self.ctrl.overlay {
            Some(Overlay::CommitPicker { commits, cursor }) => {
                render_commit_picker(f, area, commits, *cursor);
            }
            Some(Overlay::Help) => render_help(f, area),
            None => {}
        }
    }
}

// ---------------------------------------------------------------- header

fn render_header(f: &mut Frame, area: Rect, ctrl: &Controller<'_>) {
    let path_str = ctrl.repo_path.display().to_string();
    let mode_desc = match ctrl.mode {
        ComparisonMode::CommitVsHead => {
            let commit = ctrl.selected_commit.as_deref().unwrap_or("?");
            format!("HEAD ↔ {}", commit)
        }
        m => format!("比较模式: {}", m.label()),
    };
    let line = Line::from(vec![
        Span::raw(" "),
        Span::styled(mode_desc, styles::header_style()),
        Span::styled(format!("  {}", path_str), Style::default().fg(Color::DarkGray)),
    ]);
    f.render_widget(Paragraph::new(line), Rect { x: area.x, y: area.y, width: area.width, height: 1 });
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "─".repeat(area.width as usize),
            styles::separator_style(),
        ))),
        Rect { x: area.x, y: area.y + 1, width: area.width, height: 1 },
    );
}

// ------------------------------------------------------------- file list

fn render_filelist(f: &mut Frame, area: Rect, ctrl: &mut Controller<'_>) {
    let is_active = ctrl.focus == Focus::FileList;
    let border_fg = if is_active { styles::ACTIVE_BORDER } else { styles::INACTIVE_BORDER };
    let block = Block::default()
        .title(format!(" 变更文件 ({}) ", ctrl.files.len()))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_fg))
        .border_type(BorderType::Rounded);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let rows = ctrl.visible_rows();
    if rows.is_empty() {
        let msg = Line::from(Span::styled("(no changes)", Style::default().fg(styles::DIM)));
        let y = inner.y + inner.height.saturating_div(2);
        f.render_widget(
            Paragraph::new(msg).alignment(ratatui::layout::Alignment::Center),
            Rect { x: inner.x, y, width: inner.width, height: 1 },
        );
        return;
    }

    const MARKER_W: usize = 2;
    const STATUS_W: usize = 6;
    const PLUS_W: usize = 4;
    const MINUS_W: usize = 4;
    const GAP: usize = 1;
    const SCROLL_W: usize = 1;
    let list_height = inner.height.saturating_sub(2) as usize;
    let scrollbar_needed = rows.len() > list_height;
    let name_w = inner
        .width
        .saturating_sub(
            (MARKER_W + STATUS_W + PLUS_W + MINUS_W + 3 * GAP + SCROLL_W * usize::from(scrollbar_needed)) as u16,
        ) as usize;

    let header = Line::from(vec![
        Span::raw(" ".repeat(MARKER_W)),
        Span::styled(pad_right("Name", name_w), Style::default().fg(styles::HEADER_FG).add_modifier(Modifier::BOLD)),
        Span::raw(" "),
        Span::styled(pad_left("+", PLUS_W), Style::default().fg(styles::HEADER_FG).add_modifier(Modifier::BOLD)),
        Span::raw(" "),
        Span::styled(pad_left("-", MINUS_W), Style::default().fg(styles::HEADER_FG).add_modifier(Modifier::BOLD)),
        Span::raw(" "),
        Span::styled(pad_left("Status", STATUS_W), Style::default().fg(styles::HEADER_FG).add_modifier(Modifier::BOLD)),
    ]);
    f.render_widget(header, Rect { x: inner.x, y: inner.y, width: inner.width, height: 1 });
    f.render_widget(
        Line::from(Span::styled("─".repeat(inner.width as usize), Style::default().fg(styles::DIM))),
        Rect { x: inner.x, y: inner.y + 1, width: inner.width, height: 1 },
    );

    if ctrl.cursor >= rows.len() {
        ctrl.cursor = rows.len().saturating_sub(1);
    }
    let visible_start = ctrl.list_scroll.min(rows.len().saturating_sub(list_height));
    if ctrl.cursor < visible_start {
        ctrl.list_scroll = ctrl.cursor;
    } else if ctrl.cursor >= visible_start + list_height {
        ctrl.list_scroll = ctrl.cursor + 1 - list_height;
    }

    for i in 0..list_height {
        let idx = visible_start + i;
        if idx >= rows.len() {
            break;
        }
        let is_selected = idx == ctrl.cursor;
        render_list_row(f, inner, &rows[idx], i as u16, is_selected, name_w);
    }

    if scrollbar_needed {
        let scroll_area = Rect {
            x: inner.x + inner.width.saturating_sub(SCROLL_W as u16 + 1),
            y: inner.y + 2,
            width: 1,
            height: list_height as u16,
        };
        render_list_scrollbar(f, scroll_area, rows.len(), list_height, ctrl.list_scroll);
    }
}

fn render_list_scrollbar(f: &mut Frame, area: Rect, total: usize, visible: usize, start: usize) {
    let h = area.height as usize;
    let pos = if total <= visible {
        0.0
    } else {
        (start as f64) / (total - visible) as f64
    };
    let size = (visible as f64 / total as f64 * h as f64).max(1.0);
    let track_start = (pos * (h as f64 - size)).round() as usize;
    for i in 0..h {
        let ch = if (i as f64) >= track_start as f64 && (i as f64) < track_start as f64 + size {
            "█"
        } else {
            "░"
        };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(ch, Style::default().fg(styles::DIM)))),
            Rect { x: area.x, y: area.y + i as u16, width: 1, height: 1 },
        );
    }
}

fn render_list_row(f: &mut Frame, inner: Rect, row: &VisibleRow, y: u16, is_selected: bool, name_w: usize) {
    const STATUS_W: usize = 6;
    const PLUS_W: usize = 4;
    const MINUS_W: usize = 4;

    let selected_style = Style::default()
        .fg(Color::White)
        .add_modifier(Modifier::BOLD)
        .bg(styles::SELECT_ROW_BG);
    let row_style = if is_selected { selected_style } else { Style::default() };
    let width = inner.width as usize;

    let marker = if is_selected {
        Span::styled("▌ ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))
    } else {
        Span::raw("  ")
    };

    let line = match row {
        VisibleRow::Dir { collapsed, path, added, deleted, guide, depth, .. } => {
            let collapse = if *collapsed { "▸" } else { "▾" };
            // only the top-level directories get the folder emoji; nested
            // directories rely on the tree guide + collapse arrow alone
            let icon = if *depth == 0 { "📁 " } else { "" };
            let guide_w = UnicodeWidthStr::width(guide.as_str());
            let name_avail = name_w.saturating_sub(guide_w);
            Line::from(vec![
                marker,
                // relationship lines stay thin (dim, not bold) even when
                // the row is selected
                Span::styled(guide.to_string(), guide_style()),
                Span::styled(
                    pad_right(&truncate(&format!("{} {}{}", collapse, icon, short_name(path)), name_avail), name_avail),
                    row_style.fg(Color::Yellow),
                ),
                Span::raw(" "),
                Span::styled(pad_left(&format!("+{}", added), PLUS_W), amount_style(*added, true, is_selected)),
                Span::raw(" "),
                Span::styled(pad_left(&format!("-{}", deleted), MINUS_W), amount_style(*deleted, false, is_selected)),
                Span::raw(" "),
                Span::styled(pad_left("", STATUS_W), row_style),
            ])
        }
        VisibleRow::File { file, guide, .. } => {
            let status_style = if is_selected {
                Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(styles::status_fg(file.status))
            };
            let status = file.status.letter();
            let name = file.path.rsplit('/').next().unwrap_or(&file.path);
            let plus = format!("+{}", file.added);
            let minus = format!("-{}", file.deleted);
            let guide_w = UnicodeWidthStr::width(guide.as_str());
            let name_avail = name_w.saturating_sub(guide_w);
            Line::from(vec![
                marker,
                Span::styled(guide.to_string(), guide_style()),
                Span::styled(
                    pad_right(&truncate(name, name_avail), name_avail),
                    if is_selected { row_style.fg(Color::White) } else { Style::default().fg(Color::White) },
                ),
                Span::raw(" "),
                Span::styled(pad_left(&plus, PLUS_W), amount_style(file.added, true, is_selected)),
                Span::raw(" "),
                Span::styled(pad_left(&minus, MINUS_W), amount_style(file.deleted, false, is_selected)),
                Span::raw(" "),
                Span::styled(pad_left(status, STATUS_W), status_style),
            ])
        }
    };
    f.render_widget(line, Rect { x: inner.x, y: inner.y + 2 + y, width: width as u16, height: 1 });
}

/// Relationship lines (├─ └─ │) are always thin: dim, never bold.
fn guide_style() -> Style {
    Style::default().fg(styles::DIM)
}

/// +N / -N columns: zero is dim, small is plain green/red, larger gets
/// brighter, and huge counts turn yellow for attention.
fn amount_style(n: u64, positive: bool, is_selected: bool) -> Style {
    if is_selected {
        return Style::default().fg(Color::White).add_modifier(Modifier::BOLD);
    }
    let base = match n {
        0 => styles::DIM,
        1..=9 => {
            if positive { Color::Green } else { Color::Red }
        }
        10..=99 => {
            if positive { Color::Rgb(90, 220, 90) } else { Color::Rgb(255, 90, 90) }
        }
        _ => Color::Yellow,
    };
    Style::default().fg(base)
}

// ----------------------------------------------------------------- diff

fn render_diffview(f: &mut Frame, area: Rect, ctrl: &mut Controller<'_>) {
    let file = ctrl.diff_file.clone();
    let (orig_label, changed_label) = ctrl.side_labels();
    let left = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    let scrollbar_needed = ctrl.diff_rows.len() > (area.height.saturating_sub(2)) as usize;

    match file {
        Some(file) => {
            let orig_title = pane_title(&orig_label, &file);
            let changed_title = pane_title(&changed_label, &file);
            if file.is_binary {
                render_placeholder_pane(f, left[0], &orig_title, "(binary file)", ctrl);
                render_placeholder_pane(f, left[1], &changed_title, "(binary file)", ctrl);
                return;
            }
            let orig_empty = ctrl.diff_rows.iter().all(|r| r.original.is_none());
            let changed_empty = ctrl.diff_rows.iter().all(|r| r.changed.is_none());
            if orig_empty {
                render_placeholder_pane(f, left[0], &orig_title, "(no original content)", ctrl);
            } else {
                render_diff_pane(f, left[0], &orig_title, true, ctrl);
            }
            if changed_empty {
                render_placeholder_pane(f, left[1], &changed_title, "(no changed content)", ctrl);
            } else {
                render_diff_pane(f, left[1], &changed_title, false, ctrl);
            }
        }
        None => {
            let msg = if ctrl.files.is_empty() {
                if ctrl.mode == ComparisonMode::CommitVsHead {
                    "无差异"
                } else if !ctrl.has_commits && ctrl.mode != ComparisonMode::WorkingVsHead {
                    "无可用对比（仓库还没有 commit）"
                } else {
                    "(no changes)"
                }
            } else {
                "(select a file)"
            };
            render_placeholder_pane(f, left[0], &orig_label, msg, ctrl);
            render_placeholder_pane(f, left[1], &changed_label, msg, ctrl);
        }
    }

    if scrollbar_needed {
        render_scrollbar(f, left[1], ctrl);
    }
}

fn pane_title(label: &str, file: &ChangedFile) -> String {
    let rename = file.old_path.as_deref().unwrap_or("");
    if !rename.is_empty() && file.status == Status::Renamed {
        format!("{} · {} → {}", label, rename, file.path)
    } else {
        format!("{} · {}", label, file.path)
    }
}

fn render_diff_pane(f: &mut Frame, area: Rect, label: &str, is_original: bool, ctrl: &Controller<'_>) {
    let is_active = ctrl.focus == Focus::Diff;
    let border_fg = if is_active { styles::ACTIVE_BORDER } else { styles::INACTIVE_BORDER };
    let block = Block::default()
        .title(format!(" {} ", label))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_fg))
        .border_type(BorderType::Rounded);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let height = inner.height.saturating_sub(1) as usize;
    let content_w = inner.width.saturating_sub(10) as usize;
    let row_rect_w = inner.width.saturating_sub(2);

    let mut real = ctrl.diff_vscroll;
    let mut screen = 0usize;
    while screen < height && real < ctrl.diff_rows.len() {
        // fold long unchanged runs (unless cursor is inside)
        if ctrl.fold_unchanged && ctrl.is_unchanged_row(real) {
            let mut start = real;
            while start > 0 && ctrl.is_unchanged_row(start - 1) {
                start -= 1;
            }
            let mut end = real;
            while end < ctrl.diff_rows.len() && ctrl.is_unchanged_row(end) {
                end += 1;
            }
            let run_len = end - start;
            let cursor_in_run = ctrl.diff_cursor >= start && ctrl.diff_cursor < end;
            if run_len > crate::controller::FOLD_MIN && !cursor_in_run {
                render_diff_row(f, inner, content_w, start, screen, is_original, ctrl);
                screen += 1;
                if screen >= height {
                    break;
                }
                let marker = format!("⋯ {} 行未改动", run_len - 2);
                let line = Line::from(vec![
                    Span::styled(format!("{:>3} ", ""), Style::default().fg(styles::DIM)),
                    Span::styled(" ", Style::default().fg(styles::DIM)),
                    Span::styled(marker, Style::default().fg(styles::DIM)),
                ]);
                f.render_widget(
                    line,
                    Rect { x: inner.x + 1, y: inner.y + 1 + screen as u16, width: row_rect_w, height: 1 },
                );
                screen += 1;
                if screen >= height {
                    break;
                }
                render_diff_row(f, inner, content_w, end - 1, screen, is_original, ctrl);
                screen += 1;
                real = end;
                continue;
            }
        }
        render_diff_row(f, inner, content_w, real, screen, is_original, ctrl);
        screen += 1;
        real += 1;
    }
}

#[allow(clippy::too_many_arguments)]
fn render_diff_row(
    f: &mut Frame,
    inner: Rect,
    content_w: usize,
    real: usize,
    screen: usize,
    is_original: bool,
    ctrl: &Controller<'_>,
) {
    let row = &ctrl.diff_rows[real];
    let cell = if is_original { &row.original } else { &row.changed };
    let Some(cell) = cell else { return };
    let kind = cell.kind;
    let marker = match kind {
        LineKind::Delete => "-",
        LineKind::Insert => "+",
        LineKind::Equal => " ",
    };
    let (fg, bg, emph_bg) = match kind {
        LineKind::Delete => (Color::White, styles::DEL_BG, styles::INLINE_DEL_BG),
        LineKind::Insert => (Color::White, styles::ADD_BG, styles::INLINE_ADD_BG),
        LineKind::Equal => (Color::White, Color::Reset, Color::Reset),
    };
    let is_cursor = ctrl.focus == Focus::Diff && real == ctrl.diff_cursor;
    let mut spans = content_spans(cell, fg, bg, emph_bg, content_w, ctrl.diff_hscroll);
    let line_num = format!("{:>3} ", cell.num);
    let marker_color = match kind {
        LineKind::Delete => Color::Red,
        LineKind::Insert => Color::Green,
        LineKind::Equal => Color::DarkGray,
    };
    if is_cursor {
        spans.insert(0, Span::styled(line_num, Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)));
        spans.insert(1, Span::styled(marker, Style::default().fg(marker_color).add_modifier(Modifier::BOLD).bg(styles::CURSOR_BG)));
        // give every content span the cursor background
        for s in spans.iter_mut().skip(2) {
            let st = s.style;
            let content = s.content.clone();
            *s = Span::styled(content, st.bg(styles::CURSOR_BG));
        }
    } else {
        spans.insert(0, Span::styled(line_num, Style::default().fg(styles::DIM)));
        spans.insert(1, Span::styled(marker, Style::default().fg(marker_color).add_modifier(Modifier::BOLD)));
    }
    let line = Line::from(spans);
    let y = inner.y + 1 + screen as u16;
    f.render_widget(line, Rect { x: inner.x + 1, y, width: inner.width.saturating_sub(2), height: 1 });
}

/// Builds content spans for a diff cell, handling inline fragments,
/// horizontal scroll, truncation and trailing-whitespace highlight.
fn content_spans(
    cell: &AlignCell,
    fg: Color,
    bg: Color,
    emph_bg: Color,
    avail: usize,
    hscroll: usize,
) -> Vec<Span<'static>> {
    let frags: Vec<(bool, String)> = match &cell.inline {
        Some(inline) => inline.iter().map(|(e, t)| (*e, t.clone())).collect(),
        None => vec![(false, cell.text.clone())],
    };
    let mut skip = hscroll;
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;
    for (emph, text) in frags {
        let tw = UnicodeWidthStr::width(text.as_str());
        if tw <= skip {
            skip -= tw;
            continue;
        }
        let seg = &text[char_offset_at_width(&text, skip)..];
        skip = 0;
        let seg_w = UnicodeWidthStr::width(seg);
        let remaining = avail.saturating_sub(used);
        let style = if emph {
            Style::default().fg(fg)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
                .bg(emph_bg)
        } else {
            Style::default().fg(fg).bg(bg)
        };
        if seg_w > remaining {
            out.push(Span::styled(truncate(seg, remaining), style));
            break;
        }
        out.push(Span::styled(seg.to_string(), style));
        used += seg_w;
        if used >= avail {
            break;
        }
    }
    // highlight trailing whitespace in the final rendered text
    if let Some(last) = out.last_mut() {
        let text = last.content.as_ref();
        let ws_start = text.trim_end_matches([' ', '\t']).len();
        if ws_start < text.len() {
            let clean: String = text[..ws_start].to_string();
            let ws: String = text[ws_start..].to_string();
            let base = last.style;
            last.content = clean.into();
            out.push(Span::styled(ws, base.bg(styles::TRAILING_WS_BG)));
        }
    }
    out
}

fn render_scrollbar(f: &mut Frame, pane: Rect, ctrl: &Controller<'_>) {
    let total = ctrl.diff_rows.len();
    let viewport = ctrl.diff_viewport.max(1);
    if total <= viewport {
        return;
    }
    // draw inside the right pane, one column left of its right border
    let x = pane.x + pane.width.saturating_sub(3);
    let track_h = (pane.height.saturating_sub(2)) as usize;
    let max_scroll = total.saturating_sub(viewport);
    let ratio = ctrl.diff_vscroll as f64 / max_scroll as f64;
    let thumb_h = (viewport as f64 / total as f64 * track_h as f64).max(1.0);
    let top = (ratio * (track_h as f64 - thumb_h)).round() as usize;
    for i in 0..track_h {
        let bar_y = pane.y + 1 + i as u16;
        let is_bar = (i as f64) >= top as f64 && (i as f64) < top as f64 + thumb_h;
        let ch = if is_bar { "█" } else { "░" };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(ch, Style::default().fg(styles::DIM)))),
            Rect { x, y: bar_y, width: 1, height: 1 },
        );
    }
}

fn render_placeholder_pane(f: &mut Frame, area: Rect, label: &str, msg: &str, ctrl: &Controller<'_>) {
    let is_active = ctrl.focus == Focus::Diff;
    let border_fg = if is_active { styles::ACTIVE_BORDER } else { styles::INACTIVE_BORDER };
    let block = Block::default()
        .title(format!(" {} ", label))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_fg))
        .border_type(BorderType::Rounded);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let msg = Line::from(Span::styled(msg, Style::default().fg(styles::DIM)));
    let y = inner.y + inner.height.saturating_div(2);
    f.render_widget(
        Paragraph::new(msg).alignment(ratatui::layout::Alignment::Center),
        Rect { x: inner.x, y, width: inner.width, height: 1 },
    );
}

// ------------------------------------------------------------- statusbar

fn render_statusbar(f: &mut Frame, area: Rect, ctrl: &Controller<'_>) {
    let mut left: Vec<Span> = Vec::new();
    let mode_tag = match ctrl.mode {
        ComparisonMode::WorkingVsHead => "[模式A]",
        ComparisonMode::StagedVsHead => "[模式B]",
        ComparisonMode::StagedVsWorking => "[模式C]",
        ComparisonMode::CommitVsHead => "[模式D]",
    };
    left.push(Span::styled(mode_tag, Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)));
    left.push(Span::raw(" "));
    if ctrl.loading {
        left.push(Span::styled("loading...", Style::default().fg(Color::Yellow)));
    } else if !ctrl.status.is_empty() {
        left.push(Span::styled(&ctrl.status, Style::default().fg(Color::Red)));
    } else if let Some(file) = &ctrl.diff_file {
        left.push(Span::styled(
            format!("{}  ", file.path),
            Style::default().fg(Color::White),
        ));
        left.push(Span::styled(
            format!("+{} -{}", file.added, file.deleted),
            Style::default().fg(Color::DarkGray),
        ));
        if ctrl.hunk_count > 0 {
            left.push(Span::styled(" │ ", styles::status_sep_style()));
            left.push(Span::styled(
                format!("hunk {}/{}", ctrl.hunk_idx, ctrl.hunk_count),
                Style::default().fg(Color::Cyan),
            ));
        }
        if !ctrl.diff_rows.is_empty() {
            left.push(Span::styled(" │ ", styles::status_sep_style()));
            left.push(Span::styled(
                format!("行 {}/{}", ctrl.diff_cursor + 1, ctrl.diff_rows.len()),
                Style::default().fg(Color::DarkGray),
            ));
        }
        if ctrl.fold_unchanged {
            left.push(Span::styled(" │ ", styles::status_sep_style()));
            left.push(Span::styled("折叠", Style::default().fg(Color::Yellow)));
        }
        if ctrl.sort != SortMode::Path {
            left.push(Span::styled(" │ ", styles::status_sep_style()));
            let label = match ctrl.sort {
                SortMode::Path => "路径",
                SortMode::Status => "按状态",
                SortMode::Added => "按增行",
            };
            left.push(Span::styled(
                format!("排序: {}", label),
                Style::default().fg(Color::Magenta),
            ));
        }
    } else {
        left.push(Span::styled("select a file", Style::default().fg(styles::DIM)));
    }

    // right-aligned hint block
    let mut right: Vec<Span> = Vec::new();
    if let Some(f) = &ctrl.filter {
        // while filtering, show the input after the status info
        left.push(Span::styled(" │ ", styles::status_sep_style()));
        left.push(Span::styled(
            format!("/ {}", f),
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ));
        right.push(Span::styled("│ [Esc]", styles::key_style()));
        right.push(Span::styled("取消", Style::default().fg(styles::DIM)));
    } else {
        let hints: [(&str, &str); 12] = [
            ("Tab", "焦点"),
            ("Enter", "查看对比"),
            ("↑↓", "移动"),
            ("Pg", "翻页"),
            ("1/2/3/4", "模式"),
            ("l", "换一个 commit"),
            ("/", "过滤"),
            ("s", "排序"),
            ("n/m", "hunk"),
            ("z", "折叠"),
            ("?", "帮助"),
            ("q", "退出"),
        ];
        for (k, d) in hints.iter() {
            right.push(Span::styled(format!("[{}]", k), styles::key_style()));
            right.push(Span::styled(format!("{} ", d), Style::default().fg(styles::DIM)));
        }
    }

    let mut spans = left;
    let left_w: usize = spans.iter().map(|s| s.width()).sum();
    let right_w: usize = right.iter().map(|s| s.width()).sum();
    let w = area.width as usize;
    if left_w + right_w + 2 < w {
        spans.push(Span::raw(" ".repeat(w - left_w - right_w - 2)));
    } else if !right.is_empty() {
        spans.push(Span::raw(" "));
    }
    spans.extend(right);
    f.render_widget(Line::from(spans), area);
}

// ------------------------------------------------------------- overlays

fn clear_area(f: &mut Frame, area: Rect) {
    f.render_widget(Clear, area);
}

fn render_commit_picker(f: &mut Frame, area: Rect, commits: &[CommitEntry], cursor: usize) {
    const MAX_ROWS: usize = 20;
    const SCROLL_W: usize = 2;
    const MARKER_W: usize = 2;
    const HASH_W: usize = 9;
    const DATE_W: usize = 18;
    let max_text = commits
        .iter()
        .map(|c| UnicodeWidthStr::width(c.title.as_str()))
        .max()
        .unwrap_or(0);
    let width = ((max_text + MARKER_W + HASH_W + DATE_W + SCROLL_W) as u16)
        .min(area.width.saturating_sub(4))
        .max(40);
    let height = ((commits.len().min(MAX_ROWS) as u16) + 5)
        .min(area.height.saturating_sub(4))
        .max(7);
    let x = area.x + area.width.saturating_div(2) - width.saturating_div(2);
    let y = area.y + area.height.saturating_div(2) - height.saturating_div(2);
    let panel = Rect { x, y, width, height };
    clear_area(f, panel);
    let block = Block::default()
        .title(" 选择 commit (HEAD ↔ 选中) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .border_type(BorderType::Rounded);
    let inner = block.inner(panel);
    f.render_widget(block, panel);
    if commits.is_empty() {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled("(no commits)", Style::default().fg(styles::DIM)))),
            Rect { x: inner.x, y: inner.y + 1, width: inner.width, height: 1 },
        );
        return;
    }
    let visible = (inner.height.saturating_sub(3)) as usize;
    let max_start = commits.len().saturating_sub(visible);
    let start = cursor.saturating_sub(visible.saturating_sub(1)).min(max_start);
    let has_scroll = commits.len() > visible;
    let content_w = inner.width.saturating_sub(2) as usize;
    let title_w = content_w.saturating_sub(MARKER_W + HASH_W + DATE_W + SCROLL_W);
    for i in 0..visible {
        let idx = start + i;
        if idx >= commits.len() {
            break;
        }
        let c = &commits[idx];
        let selected = idx == cursor;
        let style = if selected {
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(styles::SELECTED_BG)
        } else {
            Style::default()
        };
        let marker = if selected { "▶" } else { " " };
        let date_author = format!(" {:>12} {}", c.date, c.author);
        let date_author = truncate(&date_author, DATE_W);
        let line = Line::from(vec![
            Span::styled(marker, style),
            Span::styled(format!(" {} ", c.short_hash), style.fg(Color::Cyan)),
            Span::styled(pad_right(&truncate(&c.title, title_w), title_w), style),
            Span::styled(pad_right(&date_author, DATE_W), style.fg(styles::DIM)),
            Span::styled(" ".repeat(SCROLL_W), style),
        ]);
        f.render_widget(line, Rect { x: inner.x + 1, y: inner.y + 1 + i as u16, width: inner.width.saturating_sub(2), height: 1 });
    }
    if has_scroll {
        let scroll_area = Rect {
            x: inner.x + inner.width.saturating_sub(SCROLL_W as u16),
            y: inner.y + 1,
            width: SCROLL_W as u16,
            height: inner.height.saturating_sub(3),
        };
        render_commit_scrollbar(f, scroll_area, commits.len(), visible, start);
    }
    let hint = "↑↓ 选择   Enter 确认   Esc 关闭";
    let hint_w = UnicodeWidthStr::width(hint);
    let pad = inner.width.saturating_sub(hint_w as u16 + 1);
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!("{}{}", " ".repeat(pad as usize), hint),
            Style::default().fg(styles::DIM),
        ))),
        Rect { x: inner.x, y: inner.y + inner.height.saturating_sub(2), width: inner.width, height: 1 },
    );
}

fn render_commit_scrollbar(f: &mut Frame, area: Rect, total: usize, visible: usize, start: usize) {
    let h = area.height as usize;
    let pos = if total <= visible {
        0.0
    } else {
        (start as f64) / (total - visible) as f64
    };
    let size = (visible as f64 / total as f64 * h as f64).max(1.0);
    let track_start = (pos * (h as f64 - size)).round() as usize;
    for i in 0..h {
        let ch = if (i as f64) >= track_start as f64 && (i as f64) < track_start as f64 + size {
            "█"
        } else {
            "░"
        };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(ch, Style::default().fg(styles::DIM)))),
            Rect { x: area.x, y: area.y + i as u16, width: 1, height: 1 },
        );
    }
}

fn render_help(f: &mut Frame, area: Rect) {
    const KEY_GAP: usize = 6;
    const INNER_PAD: u16 = 2;
    struct Group {
        title: &'static str,
        rows: &'static [(&'static str, &'static str)],
    }
    let groups = [
        Group {
            title: "通用",
            rows: &[
                ("Tab", "切换焦点（文件列表 / 对比区）"),
                ("4", "进入模式D（上次 commit 对比 / 选择 commit）"),
                ("1 / 2 / 3 / 4", "切换比较模式 A/B/C/D"),
                ("r", "刷新"),
                ("?", "本帮助"),
                ("q / Ctrl+C", "退出"),
            ],
        },
        Group {
            title: "文件列表",
            rows: &[
                ("↑↓", "移动光标"),
                ("Enter", "查看选中文件对比"),
                ("→ / ←", "展开 / 折叠目录"),
                ("PgUp/PgDn", "列表翻页"),
                ("[ / ]", "折叠 / 展开全部目录"),
                ("s", "切换排序（路径 / 状态 / 增行数）"),
                ("/", "过滤（! 前缀反向匹配）"),
            ],
        },
        Group {
            title: "对比区",
            rows: &[
                ("↑↓", "移动当前行"),
                ("PgUp/PgDn", "按页滚动"),
                ("→ / ←", "水平滚动"),
                ("n / m", "跳转下一个/上一个 hunk"),
                ("z", "折叠/展开未改动段"),
            ],
        },
    ];
    let key_w = groups
        .iter()
        .flat_map(|g| g.rows.iter())
        .map(|(k, _)| UnicodeWidthStr::width(*k))
        .max()
        .unwrap_or(0)
        + KEY_GAP;
    let desc_w = groups
        .iter()
        .flat_map(|g| g.rows.iter())
        .map(|(_, d)| UnicodeWidthStr::width(*d))
        .max()
        .unwrap_or(0);
    let total_rows: usize = groups.iter().map(|g| g.rows.len() + 1).sum::<usize>() + (groups.len() - 1);
    let row_w = 2 + key_w + desc_w;
    let width = ((row_w + INNER_PAD as usize * 2 + 4) as u16)
        .min(area.width.saturating_sub(4))
        .max(34);
    let height = (total_rows as u16 + 4).min(area.height.saturating_sub(2));
    let x = area.x + area.width.saturating_div(2) - width.saturating_div(2);
    let y = area.y + area.height.saturating_div(2) - height.saturating_div(2);
    let panel = Rect { x, y, width, height };
    clear_area(f, panel);
    let block = Block::default()
        .title(format!(" {} ", "⌨ 帮助"))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(styles::ACTIVE_BORDER))
        .border_type(BorderType::Rounded);
    let inner = block.inner(panel);
    f.render_widget(block, panel);

    let mut row_idx = 0usize;
    let group_count = groups.len();
    for (gi, group) in groups.iter().enumerate() {
        // group header
        let header = Line::from(vec![
            Span::styled(
                format!(" {} ", group.title),
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ),
        ]);
        f.render_widget(
            header,
            Rect { x: inner.x + 1, y: inner.y + 1 + row_idx as u16, width: inner.width.saturating_sub(2), height: 1 },
        );
        row_idx += 1;
        for (k, d) in group.rows.iter() {
            let line = Line::from(vec![
                Span::styled("  ", Style::default().fg(styles::DIM)),
                Span::styled(pad_right(k, key_w), styles::key_style()),
                Span::styled(*d, Style::default().fg(Color::White)),
            ]);
            let rect = Rect { x: inner.x + 1, y: inner.y + 1 + row_idx as u16, width: inner.width.saturating_sub(2), height: 1 };
            f.render_widget(line, rect);
            row_idx += 1;
        }
        // blank separator line between groups
        if gi + 1 < group_count {
            row_idx += 1;
        }
    }
}

// ------------------------------------------------------------- helpers

fn short_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn truncate(s: &str, max: usize) -> String {
    let w = UnicodeWidthStr::width(s);
    if w <= max {
        return s.to_string();
    }
    let mut acc = 0;
    let mut out = String::new();
    for c in s.chars() {
        let cw = UnicodeWidthStr::width(c.to_string().as_str());
        if acc + cw > max.saturating_sub(1) {
            break;
        }
        acc += cw;
        out.push(c);
    }
    format!("{}…", out)
}

fn char_offset_at_width(s: &str, width: usize) -> usize {
    if width == 0 {
        return 0;
    }
    let mut acc = 0;
    for (i, c) in s.char_indices() {
        let cw = UnicodeWidthStr::width(c.to_string().as_str());
        if acc + cw > width {
            return i;
        }
        acc += cw;
    }
    s.len()
}

fn pad_right(s: &str, width: usize) -> String {
    let w = UnicodeWidthStr::width(s);
    if w >= width {
        s.to_string()
    } else {
        format!("{}{}", s, " ".repeat(width - w))
    }
}

fn pad_left(s: &str, width: usize) -> String {
    let w = UnicodeWidthStr::width(s);
    if w >= width {
        s.to_string()
    } else {
        format!("{}{}", " ".repeat(width - w), s)
    }
}

pub fn run<R: GitRunner>(
    runner: &R,
    root: PathBuf,
    has_commits: bool,
    initial_commit: Option<String>,
) -> Result<()> {
    let facade = GitFacade::new(runner, &root);
    let mut app = App::new(facade, root, has_commits, initial_commit)?;

    enable_raw_mode()?;
    execute!(std::io::stdout(), EnterAlternateScreen)?;
    let stdout = std::io::stdout();
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.hide_cursor()?;

    let result = event_loop(&mut terminal, &mut app);

    // restore terminal to its original state: leave raw mode and the alternate
    // screen so the shell prompt renders normally after quitting
    disable_raw_mode()?;
    let _ = execute!(std::io::stdout(), LeaveAlternateScreen);
    result
}

fn event_loop(terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>, app: &mut App<'_>) -> Result<()> {
    loop {
        terminal.draw(|f| app.render(f))?;
        let event = crossterm::event::read()?;
        if let Event::Key(key) = event {
            if key.code == KeyCode::Char('q') && key.modifiers == KeyModifiers::NONE && app.ctrl.overlay.is_none() && app.ctrl.filter.is_none() {
                break;
            }
            if key.code == KeyCode::Char('c') && key.modifiers == KeyModifiers::CONTROL {
                break;
            }
            app.handle_key(key);
        }
    }
    Ok(())
}

#[cfg(test)]
mod width_check {
    use unicode_width::UnicodeWidthStr;

    struct Group<'a> {
        rows: &'a [(&'a str, &'a str)],
    }

    fn panel_width(groups: &[Group]) -> (usize, usize, usize) {
        const KEY_GAP: usize = 6;
        const INNER_PAD: usize = 2;
        let key_w = groups
            .iter()
            .flat_map(|g| g.rows.iter())
            .map(|(k, _)| UnicodeWidthStr::width(*k))
            .max()
            .unwrap_or(0)
            + KEY_GAP;
        let desc_w = groups
            .iter()
            .flat_map(|g| g.rows.iter())
            .map(|(_, d)| UnicodeWidthStr::width(*d))
            .max()
            .unwrap_or(0);
        let row_w = 2 + key_w + desc_w;
        let width = row_w + INNER_PAD * 2 + 2;
        (row_w, width, width - 4)
    }

    #[test]
    fn help_rows_fit_within_panel() {
        let groups = [
            Group {
                rows: &[
                    ("Tab", "切换焦点（文件列表 / 对比区）"),
                    ("4", "进入模式D（上次 commit 对比 / 选择 commit）"),
                    ("1 / 2 / 3 / 4", "切换比较模式 A/B/C/D"),
                    ("r", "刷新"),
                    ("?", "本帮助"),
                    ("q / Ctrl+C", "退出"),
                ],
            },
            Group {
                rows: &[
                    ("↑↓", "移动光标"),
                    ("→ / ←", "展开 / 折叠目录"),
                    ("PgUp/PgDn", "列表翻页"),
                    ("[ / ]", "折叠 / 展开全部目录"),
                    ("s", "切换排序（路径 / 状态 / 增行数）"),
                    ("/", "过滤（! 前缀反向匹配）"),
                ],
            },
            Group {
                rows: &[
                    ("↑↓", "逐行滚动"),
                    ("PgUp/PgDn", "按页滚动"),
                    ("→ / ←", "水平滚动"),
                    ("n / m", "跳转下一个/上一个 hunk"),
                ],
            },
        ];
        let (row_w, _width, inner_avail) = panel_width(&groups);
        assert!(row_w <= inner_avail, "rows {} exceed inner {} ", row_w, inner_avail);
    }
}