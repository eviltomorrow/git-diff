use std::collections::HashSet;
use std::path::PathBuf;

use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::align::{align_rows, hunk_starts, AlignedRow, LineKind};
use crate::git::{GitFacade, GitRunner};
use crate::model::{ChangedFile, CommitEntry, ComparisonMode};
use crate::styles;
use crate::tree::{self, VisibleRow};

const LIST_RATIO: u16 = 30;

enum Overlay {
    CommitPicker { commits: Vec<CommitEntry>, cursor: usize },
    Help,
}

pub struct App<'a> {
    facade: GitFacade<'a>,
    repo_path: PathBuf,
    has_commits: bool,
    mode: ComparisonMode,
    selected_commit: Option<String>,
    files: Vec<ChangedFile>,
    collapsed: HashSet<String>,
    cursor: usize,
    list_scroll: usize,
    diff_rows: Vec<AlignedRow>,
    diff_file: Option<ChangedFile>,
    diff_vscroll: usize,
    diff_hscroll: usize,
    hunk_idx: usize,
    filter: Option<String>,
    overlay: Option<Overlay>,
    loading: bool,
    status: String,
}

impl<'a> App<'a> {
    pub fn new(facade: GitFacade<'a>, repo_path: PathBuf, has_commits: bool) -> Result<Self> {
        let mut app = Self {
            facade,
            repo_path,
            has_commits,
            mode: ComparisonMode::WorkingVsHead,
            selected_commit: None,
            files: Vec::new(),
            collapsed: HashSet::new(),
            cursor: 0,
            list_scroll: 0,
            diff_rows: Vec::new(),
            diff_file: None,
            diff_vscroll: 0,
            diff_hscroll: 0,
            hunk_idx: 0,
            filter: None,
            overlay: None,
            loading: false,
            status: String::new(),
        };
        app.reload()?;
        Ok(app)
    }

    pub fn reload(&mut self) -> Result<()> {
        self.loading = true;
        self.status = "loading...".into();
        let prev = self
            .files
            .get(self.cursor_index_into_files())
            .map(|f| f.path.clone());
        let result = match self.mode {
            ComparisonMode::CommitVsHead => {
                if let Some(commit) = &self.selected_commit {
                    self.facade.changed_files_between(commit)
                } else {
                    Ok(Vec::new())
                }
            }
            _ => self.facade.changed_files(self.mode),
        };
        self.loading = false;
        match result {
            Ok(files) => {
                self.files = files;
                if let Some(prev) = prev {
                    self.cursor = self
                        .files
                        .iter()
                        .position(|f| f.path == prev)
                        .unwrap_or(0);
                } else {
                    self.cursor = 0;
                }
                self.list_scroll = 0;
                self.collapsed.clear();
                self.status.clear();
                self.load_diff();
                Ok(())
            }
            Err(e) => {
                self.status = format!("刷新失败: {}", e);
                Ok(())
            }
        }
    }

    fn cursor_index_into_files(&self) -> usize {
        let rows = self.visible_rows();
        if let Some(VisibleRow::File { file, .. }) = rows.get(self.cursor).cloned() {
            self.files.iter().position(|f| f.path == file.path).unwrap_or(0)
        } else {
            0
        }
    }

    fn visible_rows(&self) -> Vec<VisibleRow> {
        let filter = self.filter.as_deref();
        let collapsed = if filter.is_some() {
            &HashSet::new()
        } else {
            &self.collapsed
        };
        let tree = tree::build_tree(&self.files);
        let all = tree::visible_rows(&tree, collapsed, "");
        if let Some(f) = filter {
            if f.is_empty() {
                all
            } else {
                all.into_iter()
                    .filter(|r| match r {
                        VisibleRow::File { file, .. } => file.path.contains(f),
                        VisibleRow::Dir { path, .. } => path.contains(f),
                    })
                    .collect()
            }
        } else {
            all
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        if self.loading {
            return;
        }
        if self.overlay.is_some() {
            self.handle_overlay_key(key);
            return;
        }
        match (key.code, key.modifiers) {
            (KeyCode::Char('q'), KeyModifiers::NONE) => {
                // handled by caller to exit
            }
            (KeyCode::Char('l'), KeyModifiers::NONE) => {
                if self.has_commits {
                    match self.facade.commits() {
                        Ok(commits) => {
                            self.overlay = Some(Overlay::CommitPicker { commits, cursor: 0 });
                        }
                        Err(e) => self.status = format!("加载 commit 失败: {}", e),
                    }
                } else {
                    self.status = "仓库还没有 commit".into();
                }
            }
            (KeyCode::Char('1'), KeyModifiers::NONE) => self.set_mode(ComparisonMode::WorkingVsHead),
            (KeyCode::Char('2'), KeyModifiers::NONE) => self.set_mode(ComparisonMode::StagedVsHead),
            (KeyCode::Char('3'), KeyModifiers::NONE) => self.set_mode(ComparisonMode::StagedVsWorking),
            (KeyCode::Char('r'), KeyModifiers::NONE) => {
                let _ = self.reload();
            }
            (KeyCode::Char('/'), KeyModifiers::NONE) => {
                self.filter = Some(String::new());
            }
            (KeyCode::Char('?'), KeyModifiers::NONE) => {
                self.overlay = Some(Overlay::Help);
            }
            (KeyCode::Char('n'), KeyModifiers::NONE) => self.jump_hunk(1),
            (KeyCode::Char('N'), KeyModifiers::NONE) => self.jump_hunk(-1),
            (KeyCode::Right, KeyModifiers::CONTROL) => self.scroll_horizontal(1),
            (KeyCode::Left, KeyModifiers::CONTROL) => self.scroll_horizontal(-1),
            (KeyCode::Up, _) => self.move_cursor(-1),
            (KeyCode::Down, _) => self.move_cursor(1),
            (KeyCode::Right, _) => self.expand_dir(),
            (KeyCode::Left, _) => self.collapse_dir(),
            (KeyCode::PageUp, _) | (KeyCode::Char('k'), KeyModifiers::CONTROL) => {
                self.scroll_diff(-1)
            }
            (KeyCode::PageDown, _) | (KeyCode::Char('j'), KeyModifiers::CONTROL) => {
                self.scroll_diff(1)
            }
            (KeyCode::Char(c), KeyModifiers::NONE) => {
                if let Some(filter) = &mut self.filter {
                    filter.push(c);
                    self.cursor = 0;
                }
            }
            (KeyCode::Backspace, _) => {
                if let Some(filter) = &mut self.filter {
                    filter.pop();
                    self.cursor = 0;
                }
            }
            (KeyCode::Esc, _) => {
                self.filter = None;
                self.cursor = 0;
            }
            _ => {}
        }
    }

    fn handle_overlay_key(&mut self, key: KeyEvent) {
        let action = match &mut self.overlay {
            Some(Overlay::CommitPicker { commits, cursor }) => match key.code {
                KeyCode::Up => {
                    if *cursor > 0 {
                        *cursor -= 1;
                    }
                    None
                }
                KeyCode::Down => {
                    if *cursor + 1 < commits.len() {
                        *cursor += 1;
                    }
                    None
                }
                KeyCode::Enter => Some(commits.get(*cursor).cloned()),
                KeyCode::Esc => None,
                _ => None,
            },
            Some(Overlay::Help) => None,
            None => None,
        };
        if let Some(selected) = action.flatten() {
            self.selected_commit = Some(selected.short_hash.clone());
            self.mode = ComparisonMode::CommitVsHead;
            self.overlay = None;
            self.filter = None;
            let _ = self.reload();
        } else if matches!(key.code, KeyCode::Esc) || key.code == KeyCode::Char('?') {
            self.overlay = None;
        }
    }

    fn set_mode(&mut self, mode: ComparisonMode) {
        if self.mode == mode {
            return;
        }
        self.mode = mode;
        self.selected_commit = None;
        self.filter = None;
        let _ = self.reload();
    }

    fn move_cursor(&mut self, delta: isize) {
        let rows = self.visible_rows();
        if rows.is_empty() {
            return;
        }
        let len = rows.len() as isize;
        let mut new = self.cursor as isize + delta;
        new = new.clamp(0, len - 1);
        if new != self.cursor as isize {
            self.cursor = new as usize;
            self.load_diff();
        }
    }

    fn expand_dir(&mut self) {
        let rows = self.visible_rows();
        if let Some(VisibleRow::Dir { path, .. }) = rows.get(self.cursor) {
            self.collapsed.remove(path);
        }
    }

    fn collapse_dir(&mut self) {
        let rows = self.visible_rows();
        let current = rows.get(self.cursor).cloned();
        let collapse_path = match current {
            Some(VisibleRow::Dir { path, .. }) => Some(path),
            Some(VisibleRow::File { file, .. }) => {
                file.path.rsplit_once('/').map(|(d, _)| d.to_string())
            }
            None => None,
        };
        if let Some(path) = collapse_path {
            self.collapsed.insert(path.clone());
            let rows = self.visible_rows();
            if let Some(idx) = rows.iter().position(|r| match r {
                VisibleRow::Dir { path: p, .. } => *p == path,
                _ => false,
            }) {
                self.cursor = idx;
                self.load_diff();
            }
        }
    }

    fn scroll_diff(&mut self, delta: isize) {
        let max = self.diff_rows.len().saturating_sub(1);
        let new = (self.diff_vscroll as isize + delta).clamp(0, max as isize);
        self.diff_vscroll = new as usize;
        self.sync_hunk_idx();
    }

    fn scroll_horizontal(&mut self, delta: isize) {
        let new = (self.diff_hscroll as isize + delta * 8).max(0);
        self.diff_hscroll = new as usize;
    }

    fn jump_hunk(&mut self, dir: isize) {
        let starts = hunk_starts(&self.diff_rows);
        if starts.is_empty() {
            return;
        }
        let cur = self.diff_vscroll;
        let target = if dir > 0 {
            starts.iter().find(|&&s| s > cur).copied().unwrap_or(cur)
        } else {
            starts.iter().rev().find(|&&s| s < cur).copied().unwrap_or(cur)
        };
        if target != cur {
            self.diff_vscroll = target;
            self.sync_hunk_idx();
        }
    }

    fn sync_hunk_idx(&mut self) {
        let starts = hunk_starts(&self.diff_rows);
        self.hunk_idx = starts
            .iter()
            .position(|&s| s <= self.diff_vscroll)
            .map(|i| i + 1)
            .unwrap_or(0);
    }

    fn load_diff(&mut self) {
        let file: Option<ChangedFile> = match self.visible_rows().get(self.cursor).cloned() {
            Some(VisibleRow::File { file, .. }) => Some(file),
            _ => None,
        };
        match file {
            Some(file) => {
                let sides = match self.mode {
                    ComparisonMode::CommitVsHead => {
                        if let Some(commit) = &self.selected_commit {
                            self.facade.file_sides_between(commit, &file)
                        } else {
                            Ok(crate::model::FileSides { original: None, changed: None })
                        }
                    }
                    _ => self.facade.file_sides(self.mode, &file),
                };
                match sides {
                    Ok(sides) => {
                        self.diff_rows = if file.is_binary {
                            Vec::new()
                        } else {
                            align_rows(sides.original.as_deref(), sides.changed.as_deref())
                        };
                        self.diff_file = Some(file);
                        self.diff_vscroll = 0;
                        self.diff_hscroll = 0;
                        self.sync_hunk_idx();
                    }
                    Err(e) => {
                        self.status = format!("加载对比失败: {}", e);
                        self.diff_file = None;
                    }
                }
            }
            None => {
                self.diff_file = None;
                self.diff_rows = Vec::new();
            }
        }
    }

    pub fn quit(&self) -> bool {
        false
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
        self.render_header(f, chunks[0]);
        let panels = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(LIST_RATIO), Constraint::Percentage(100 - LIST_RATIO)])
            .split(chunks[1]);
        self.render_filelist(f, panels[0]);
        self.render_diffview(f, panels[1]);
        self.render_statusbar(f, chunks[2]);
        if self.overlay.is_some() {
            let (commits, cursor) = match &self.overlay {
                Some(Overlay::CommitPicker { commits, cursor }) => (commits.clone(), *cursor),
                _ => (Vec::new(), 0),
            };
            let is_help = matches!(self.overlay, Some(Overlay::Help));
            if !is_help {
                self.render_commit_picker(f, area, &commits, cursor);
            } else {
                self.render_help(f, area);
            }
        }
    }

    fn render_header(&mut self, f: &mut Frame, area: Rect) {
        let path_str = self.repo_path.display().to_string();
        let mode_desc = match self.mode {
            ComparisonMode::CommitVsHead => {
                let commit = self.selected_commit.as_deref().unwrap_or("?");
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

    fn render_filelist(&mut self, f: &mut Frame, area: Rect) {
        let block = Block::default()
            .title(format!(" 变更文件 ({}) ", self.files.len()))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(styles::ACTIVE_BORDER))
            .border_type(BorderType::Rounded);
        let inner = block.inner(area);
        f.render_widget(block, area);

        let rows = self.visible_rows();
        if rows.is_empty() {
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    "  (no changes)",
                    Style::default().fg(styles::DIM),
                ))),
                Rect { x: inner.x, y: inner.y + 1, width: inner.width, height: 1 },
            );
            return;
        }

        let header = Line::from(vec![
            Span::styled(pad_right("Status", 6), Style::default().fg(styles::HEADER_FG).add_modifier(Modifier::BOLD)),
            Span::styled(pad_right("Name", 24), Style::default().fg(styles::HEADER_FG).add_modifier(Modifier::BOLD)),
            Span::raw(" "),
            Span::styled(pad_left("+", 4), Style::default().fg(styles::HEADER_FG).add_modifier(Modifier::BOLD)),
            Span::raw(" "),
            Span::styled(pad_left("-", 4), Style::default().fg(styles::HEADER_FG).add_modifier(Modifier::BOLD)),
        ]);
        f.render_widget(header, Rect { x: inner.x, y: inner.y, width: inner.width, height: 1 });
        f.render_widget(
            Line::from(Span::styled("─".repeat(inner.width as usize), Style::default().fg(styles::DIM))),
            Rect { x: inner.x, y: inner.y + 1, width: inner.width, height: 1 },
        );

        let list_height = inner.height.saturating_sub(2) as usize;
        if self.cursor >= rows.len() {
            self.cursor = rows.len().saturating_sub(1);
        }
        let visible_start = self.list_scroll.min(rows.len().saturating_sub(list_height));
        if self.cursor < visible_start {
            self.list_scroll = self.cursor;
        } else if self.cursor >= visible_start + list_height {
            self.list_scroll = self.cursor + 1 - list_height;
        }

        for i in 0..list_height {
            let idx = visible_start + i;
            if idx >= rows.len() {
                break;
            }
            let is_selected = idx == self.cursor;
            self.render_list_row(f, inner, &rows[idx], i as u16, is_selected);
        }
    }

    fn render_list_row(&mut self, f: &mut Frame, inner: Rect, row: &VisibleRow, y: u16, is_selected: bool) {
        let selected_style = Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD)
            .bg(styles::SELECTED_BG);
        let row_style = if is_selected { selected_style } else { Style::default() };
        let indent = "  ".repeat(row_depth(row));
        let width = inner.width.saturating_sub(2) as usize;
        let line = match row {
            VisibleRow::Dir { collapsed, path, .. } => {
                let marker = if *collapsed { "▸" } else { "▾" };
                Line::from(vec![
                    Span::styled(format!("{} {} 📁 {}", marker, indent, short_name(path)), row_style.fg(styles::DIR_FG)),
                ])
            }
            VisibleRow::File { file, .. } => {
                let status_style = if is_selected {
                    Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(styles::status_fg(file.status))
                };
                let marker = if is_selected { "▶" } else { " " };
                let status = file.status.letter();
                let name = file.path.rsplit('/').next().unwrap_or(&file.path);
                let plus = format!("+{}", file.added);
                let minus = format!("-{}", file.deleted);
                Line::from(vec![
                    Span::styled(marker, row_style),
                    Span::raw(" "),
                    Span::styled(pad_right(status, 4), status_style),
                    Span::raw(" "),
                    Span::styled(truncate(&format!("{}{}", indent, name), width.saturating_sub(20)), row_style.fg(Color::White)),
                    Span::styled(format!("{:>4}", plus), if is_selected { row_style } else { Style::default().fg(styles::STATUS_OK) }),
                    Span::styled(format!(" {:>4}", minus), if is_selected { row_style } else { Style::default().fg(styles::STATUS_ERR) }),
                ])
            }
        };
        f.render_widget(line, Rect { x: inner.x + 1, y: inner.y + 2 + y, width: width as u16, height: 1 });
    }

    fn render_diffview(&mut self, frame: &mut Frame, area: Rect) {
        let file = self.diff_file.clone();
        let (orig_label, changed_label) = self.side_labels();
        let left = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(area);

        match file {
            Some(f) => {
                if f.is_binary {
                    self.render_placeholder_pane(frame, left[0], &orig_label, "(binary file)");
                    self.render_placeholder_pane(frame, left[1], &changed_label, "(binary file)");
                    return;
                }
                let orig_empty = self.diff_rows.iter().all(|r| r.original.is_none());
                let changed_empty = self.diff_rows.iter().all(|r| r.changed.is_none());
                if orig_empty {
                    self.render_placeholder_pane(frame, left[0], &orig_label, "(no original content)");
                } else {
                    self.render_diff_pane(frame, left[0], &orig_label, true, false);
                }
                if changed_empty {
                    self.render_placeholder_pane(frame, left[1], &changed_label, "(no changed content)");
                } else {
                    self.render_diff_pane(frame, left[1], &changed_label, false, false);
                }
            }
            None => {
                let msg = if self.files.is_empty() {
                    "(no changes)"
                } else {
                    "(select a file)"
                };
                self.render_placeholder_pane_with_title(frame, left[0], &orig_label, msg);
                self.render_placeholder_pane_with_title(frame, left[1], &changed_label, msg);
            }
        }
    }

    fn render_diff_pane(&mut self, frame: &mut Frame, area: Rect, label: &str, is_original: bool, _: bool) {
        let block = Block::default()
            .title(format!(" {} ", label))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(styles::INACTIVE_BORDER));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let height = inner.height.saturating_sub(1) as usize;
        let start = self.diff_vscroll;
        for i in 0..height {
            let idx = start + i;
            if idx >= self.diff_rows.len() {
                break;
            }
            let row = &self.diff_rows[idx];
            let cell = if is_original { &row.original } else { &row.changed };
            let cell = match cell {
                Some(c) => c,
                None => continue,
            };
            let kind = cell.kind;
            let marker = match kind {
                LineKind::Delete => "-",
                LineKind::Insert => "+",
                LineKind::Equal => " ",
            };
            let (fg, bg) = match kind {
                LineKind::Delete => (Color::Red, styles::DEL_BG),
                LineKind::Insert => (Color::Green, styles::ADD_BG),
                LineKind::Equal => (Color::White, Color::Reset),
            };
            let content = truncate(&cell.text, inner.width.saturating_sub(8) as usize);
            let line = Line::from(vec![
                Span::styled(format!("{:>3} ", cell.num), Style::default().fg(styles::DIM)),
                Span::styled(marker, Style::default().fg(fg).add_modifier(Modifier::BOLD)),
                Span::styled(format!(" {}", content), Style::default().fg(fg).bg(bg)),
            ]);
            frame.render_widget(line, Rect { x: inner.x + 1, y: inner.y + 1 + i as u16, width: inner.width.saturating_sub(2), height: 1 });
        }
        self.render_scrollbar(frame, area, inner);
    }

    fn render_scrollbar(&mut self, frame: &mut Frame, area: Rect, inner: Rect) {
        if self.diff_rows.len() <= inner.height as usize {
            return;
        }
        let total = self.diff_rows.len();
        let viewport = inner.height.saturating_sub(1) as usize;
        let pos = (self.diff_vscroll as f64 / total as f64) * inner.height as f64;
        let size = (viewport as f64 / total as f64 * inner.height as f64).max(1.0);
        let track = inner.y + 1;
        let x = area.x + area.width.saturating_sub(1);
        for i in 0..inner.height {
            let bar_y = track + i;
            let is_bar = (bar_y as f64) >= pos && (bar_y as f64) < pos + size;
            let ch = if is_bar { "█" } else { "░" };
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(ch, Style::default().fg(styles::DIM)))),
                Rect { x, y: bar_y, width: 1, height: 1 },
            );
        }
    }

    fn render_placeholder_pane(&mut self, frame: &mut Frame, area: Rect, label: &str, msg: &str) {
        self.render_placeholder_pane_with_title(frame, area, label, msg);
    }

    fn render_placeholder_pane_with_title(&mut self, f: &mut Frame, area: Rect, label: &str, msg: &str) {
        let block = Block::default()
            .title(format!(" {} ", label))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(styles::INACTIVE_BORDER));
        let inner = block.inner(area);
        f.render_widget(block, area);
        let msg = Line::from(Span::styled(msg, Style::default().fg(styles::DIM)));
        let y = inner.y + inner.height.saturating_div(2);
        f.render_widget(msg, Rect { x: inner.x, y, width: inner.width, height: 1 });
    }

    fn render_statusbar(&mut self, f: &mut Frame, area: Rect) {
        let mut spans: Vec<Span> = Vec::new();
        if self.loading {
            spans.push(Span::styled("loading...", Style::default().fg(Color::Yellow)));
        } else if !self.status.is_empty() {
            spans.push(Span::styled(&self.status, Style::default().fg(Color::Red)));
        } else if let Some(f) = &self.diff_file {
            spans.push(Span::styled(
                format!("{}  ", f.path),
                Style::default().fg(Color::White),
            ));
            spans.push(Span::styled(
                format!("+{} -{}", f.added, f.deleted),
                Style::default().fg(Color::DarkGray),
            ));
        } else {
            spans.push(Span::styled("select a file", Style::default().fg(styles::DIM)));
        }

        let mut hint: Vec<Span> = vec![
            key_hint("↑↓", "移动"),
            key_hint("→←", "展开/折叠"),
            key_hint("l", "commit"),
            key_hint("1/2/3", "模式"),
            key_hint("/", "过滤"),
            key_hint("n/N", "hunk"),
            key_hint("r", "刷新"),
            key_hint("?", "帮助"),
            key_hint("q", "退出"),
        ];
        if let Some(f) = &self.filter {
            let f = f.clone();
            hint.push(Span::styled(format!(" filter: {}", f), Style::default().fg(Color::Cyan)));
        }
        spans.push(Span::styled(" │ ", styles::status_sep_style()));
        let w = area.width as usize;
        let left_width: usize = spans.iter().map(|s| s.width()).sum();
        let right_width: usize = hint.iter().map(|s| s.width()).sum();
        if left_width + right_width + 2 < w {
            spans.push(Span::raw(" ".repeat(w - left_width - right_width - 2)));
        }
        spans.extend(hint);
        f.render_widget(Line::from(spans), area);
    }

    fn render_commit_picker(&mut self, f: &mut Frame, area: Rect, commits: &[CommitEntry], cursor: usize) {
        let width = 60u16.min(area.width.saturating_sub(4));
        let height = (commits.len() as u16 + 4).min(area.height.saturating_sub(4)).max(6);
        let x = area.x + area.width.saturating_div(2) - width.saturating_div(2);
        let y = area.y + area.height.saturating_div(2) - height.saturating_div(2);
        let panel = Rect { x, y, width, height };
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
        let visible = (inner.height.saturating_sub(2)) as usize;
        let start = cursor.saturating_sub(visible.saturating_sub(1));
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
            let line = Line::from(vec![
                Span::styled(marker, style),
                Span::styled(format!(" {} ", c.short_hash), style.fg(Color::Cyan)),
                Span::styled(truncate(&c.title, inner.width.saturating_sub(42) as usize), style),
                Span::styled(format!(" {:>12} {}", c.date, c.author), style.fg(styles::DIM)),
            ]);
            f.render_widget(line, Rect { x: inner.x + 1, y: inner.y + 1 + i as u16, width: inner.width.saturating_sub(2), height: 1 });
        }
        f.render_widget(
            Paragraph::new(Line::from(Span::styled("  ↑↓ 选择   Enter 确认   Esc 关闭", Style::default().fg(styles::DIM)))),
            Rect { x: inner.x, y: inner.y + inner.height.saturating_sub(2), width: inner.width, height: 1 },
        );
    }

    fn render_help(&mut self, f: &mut Frame, area: Rect) {
        let width = 46u16.min(area.width.saturating_sub(4));
        let height = 16u16.min(area.height.saturating_sub(2));
        let x = area.x + area.width.saturating_div(2) - width.saturating_div(2);
        let y = area.y + area.height.saturating_div(2) - height.saturating_div(2);
        let panel = Rect { x, y, width, height };
        let block = Block::default()
            .title(" 帮助 ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .border_type(BorderType::Rounded);
        let inner = block.inner(panel);
        f.render_widget(block, panel);
        let help_lines = [
            ("↑↓", "移动光标"),
            ("→ / ←", "展开 / 折叠目录"),
            ("Ctrl+↑↓ / PgUp/PgDn", "滚动对比区"),
            ("Ctrl+←→", "水平滚动对比区"),
            ("n / N", "跳转下一个/上一个 hunk"),
            ("l", "打开 commit 选择器 (模式D)"),
            ("1 / 2 / 3", "切换比较模式 A/B/C"),
            ("/", "过滤文件列表"),
            ("r", "刷新"),
            ("?", "本帮助"),
            ("q / Ctrl+C", "退出"),
        ];
        for (i, (k, d)) in help_lines.iter().enumerate() {
            let line = Line::from(vec![
                Span::styled(pad_right(k, 22), styles::key_style()),
                Span::styled(*d, Style::default().fg(Color::White)),
            ]);
            f.render_widget(line, Rect { x: inner.x + 1, y: inner.y + 1 + i as u16, width: inner.width.saturating_sub(2), height: 1 });
        }
    }

    fn side_labels(&self) -> (String, String) {
        match self.mode {
            ComparisonMode::WorkingVsHead => ("HEAD".into(), "工作区".into()),
            ComparisonMode::StagedVsHead => ("HEAD".into(), "暂存区".into()),
            ComparisonMode::StagedVsWorking => ("暂存区".into(), "工作区".into()),
            ComparisonMode::CommitVsHead => (
                self.selected_commit.clone().unwrap_or_else(|| "commit".into()),
                "HEAD".into(),
            ),
        }
    }
}

fn row_depth(row: &VisibleRow) -> usize {
    match row {
        VisibleRow::Dir { depth, .. } => *depth,
        VisibleRow::File { depth, .. } => *depth,
    }
}

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

fn key_hint<'a>(key: &'a str, _desc: &'a str) -> Span<'a> {
    Span::styled(format!("[{}]", key), styles::key_style())
        .to_owned()
}

pub fn run<R: GitRunner>(runner: &R, root: PathBuf, has_commits: bool) -> Result<()> {
    let facade = GitFacade::new(runner, &root);
    let mut app = App::new(facade, root, has_commits)?;
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &mut app);
    ratatui::restore();
    result
}

fn event_loop(terminal: &mut ratatui::DefaultTerminal, app: &mut App<'_>) -> Result<()> {
    loop {
        terminal.draw(|f| app.render(f))?;
        let event = crossterm::event::read()?;
        if let Event::Key(key) = event {
            if key.code == KeyCode::Char('q') && key.modifiers == KeyModifiers::NONE && app.overlay.is_none() {
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