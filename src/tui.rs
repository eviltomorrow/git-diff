use std::collections::HashSet;
use std::path::PathBuf;

use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::align::{align_rows, hunk_starts, plain_rows, AlignedRow, LineKind};
use crate::git::{GitFacade, GitRunner};
use crate::model::{ChangedFile, CommitEntry, ComparisonMode, Status};
use crate::styles;
use crate::tree::{self, VisibleRow};

const LIST_RATIO: u16 = 26;
const LINE_LIMIT: usize = 50_000;

fn line_count(content: &[u8]) -> usize {
    content.iter().filter(|&&b| b == b'\n').count() + 1
}

enum Overlay {
    CommitPicker { commits: Vec<CommitEntry>, cursor: usize },
    Help,
}

#[derive(Clone, Copy, PartialEq)]
enum Focus {
    FileList,
    Diff,
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
    focus: Focus,
    diff_viewport: usize,
    diff_hviewport: usize,
    diff_max_line_w: usize,
    list_viewport: usize,
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
            focus: Focus::FileList,
            diff_viewport: 0,
            diff_hviewport: 0,
            diff_max_line_w: 0,
            list_viewport: 0,
            loading: false,
            status: String::new(),
        };
        app.reload(false)?;
        Ok(app)
    }

    pub fn reload(&mut self, keep_state: bool) -> Result<()> {
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
            ComparisonMode::WorkingVsHead if !self.has_commits => {
                self.facade.untracked_files()
            }
            ComparisonMode::WorkingVsHead => {
                let mut files = self.facade.changed_files(self.mode)?;
                let untracked = self.facade.untracked_files()?;
                files.extend(untracked);
                Ok(files)
            }
            _ => self.facade.changed_files(self.mode),
        };
        self.loading = false;
        match result {
            Ok(files) => {
                self.files = files;
                if keep_state {
                    self.restore_cursor(prev.as_deref());
                } else {
                    self.cursor = 0;
                    self.list_scroll = 0;
                    self.collapsed.clear();
                }
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

    fn restore_cursor(&mut self, prev: Option<&str>) {
        let Some(prev) = prev else {
            self.cursor = 0;
            self.list_scroll = 0;
            return;
        };
        let rows = self.visible_rows();
        match rows.iter().position(|r| match r {
            VisibleRow::File { file, .. } => file.path == prev,
            _ => false,
        }) {
            Some(idx) => {
                self.cursor = idx;
                self.list_scroll = idx.saturating_sub(10);
            }
            None => {
                self.cursor = 0;
                self.list_scroll = 0;
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
                        VisibleRow::File { file, .. } => file.path.starts_with(f),
                        VisibleRow::Dir { path, .. } => path.starts_with(f),
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
                let _ = self.reload(true);
            }
            (KeyCode::Char('/'), KeyModifiers::NONE) => {
                self.filter = Some(String::new());
                self.cursor = 0;
            }
            (KeyCode::Char('?'), KeyModifiers::NONE) => {
                self.overlay = Some(Overlay::Help);
            }
            (KeyCode::Tab, _) => self.toggle_focus(),
            (KeyCode::Char('n'), KeyModifiers::NONE) => self.jump_hunk(1),
            (KeyCode::Char('N'), KeyModifiers::NONE) => self.jump_hunk(-1),
            (KeyCode::Right, KeyModifiers::CONTROL) => self.scroll_horizontal(1),
            (KeyCode::Left, KeyModifiers::CONTROL) => self.scroll_horizontal(-1),
            (KeyCode::Char('k'), KeyModifiers::CONTROL) => self.scroll_diff(-1),
            (KeyCode::Char('j'), KeyModifiers::CONTROL) => self.scroll_diff(1),
            (KeyCode::Up, _) => match self.focus {
                Focus::FileList => self.move_cursor(-1),
                Focus::Diff => self.scroll_diff(-1),
            },
            (KeyCode::Down, _) => match self.focus {
                Focus::FileList => self.move_cursor(1),
                Focus::Diff => self.scroll_diff(1),
            },
            (KeyCode::Right, _) => match self.focus {
                Focus::FileList => self.expand_dir(),
                Focus::Diff => self.scroll_horizontal(1),
            },
            (KeyCode::Left, _) => match self.focus {
                Focus::FileList => self.collapse_dir(),
                Focus::Diff => self.scroll_horizontal(-1),
            },
            (KeyCode::PageUp, _) => match self.focus {
                Focus::FileList => self.move_cursor(-(self.list_page() as isize)),
                Focus::Diff => self.scroll_diff(-(self.diff_page() as isize)),
            },
            (KeyCode::PageDown, _) => match self.focus {
                Focus::FileList => self.move_cursor(self.list_page() as isize),
                Focus::Diff => self.scroll_diff(self.diff_page() as isize),
            },
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
            let _ = self.reload(false);
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
        let _ = self.reload(false);
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

    fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::FileList => Focus::Diff,
            Focus::Diff => Focus::FileList,
        };
    }

    fn list_page(&self) -> usize {
        self.list_viewport.max(1)
    }

    fn diff_page(&self) -> usize {
        self.diff_viewport.max(1)
    }

    fn scroll_diff(&mut self, delta: isize) {
        let viewport = self.diff_viewport.max(1);
        let max = self.diff_rows.len().saturating_sub(viewport);
        let new = (self.diff_vscroll as isize + delta).clamp(0, max as isize);
        self.diff_vscroll = new as usize;
        self.sync_hunk_idx();
    }

    fn scroll_horizontal(&mut self, delta: isize) {
        let viewport_w = self.diff_hviewport.max(1);
        let max_w = self.diff_max_line_w.max(viewport_w);
        let max = max_w.saturating_sub(viewport_w);
        let new = (self.diff_hscroll as isize + delta * 8).clamp(0, max as isize);
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
                        let big = sides.original.as_deref().map(line_count).unwrap_or(0) > LINE_LIMIT
                            || sides.changed.as_deref().map(line_count).unwrap_or(0) > LINE_LIMIT;
                        self.diff_rows = if file.is_binary {
                            Vec::new()
                        } else if big {
                            plain_rows(sides.original.as_deref(), sides.changed.as_deref())
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
        let is_active = self.focus == Focus::FileList;
        let border_fg = if is_active { styles::ACTIVE_BORDER } else { styles::INACTIVE_BORDER };
        let block = Block::default()
            .title(format!(" 变更文件 ({}) ", self.files.len()))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_fg))
            .border_type(if is_active { BorderType::Plain } else { BorderType::Rounded });
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

        const MARKER_W: usize = 2;
        const STATUS_W: usize = 6;
        const PLUS_W: usize = 4;
        const MINUS_W: usize = 4;
        const GAP: usize = 1;
        let name_w = inner
            .width
            .saturating_sub((MARKER_W + STATUS_W + PLUS_W + MINUS_W + 3 * GAP) as u16) as usize;

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

        let list_height = inner.height.saturating_sub(2) as usize;
        self.list_viewport = list_height;
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
            self.render_list_row(f, inner, &rows[idx], i as u16, is_selected, name_w);
        }
    }

    fn render_list_row(&mut self, f: &mut Frame, inner: Rect, row: &VisibleRow, y: u16, is_selected: bool, name_w: usize) {
        const STATUS_W: usize = 6;
        const PLUS_W: usize = 4;
        const MINUS_W: usize = 4;

        let selected_style = Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD);
        let row_style = if is_selected { selected_style } else { Style::default() };
        let indent = "  ".repeat(row_depth(row));
        let width = inner.width as usize;

        let marker = if is_selected {
            Span::styled("▌ ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))
        } else {
            Span::raw("  ")
        };

        let line = match row {
            VisibleRow::Dir { collapsed, path, .. } => {
                let collapse = if *collapsed { "▸" } else { "▾" };
                Line::from(vec![
                    marker,
                    Span::styled(
                        pad_right(&truncate(&format!("{}{}📁 {}", collapse, indent, short_name(path)), name_w), name_w),
                        if is_selected { row_style.fg(Color::Yellow) } else { Style::default().fg(styles::DIR_FG) },
                    ),
                    Span::raw(" "),
                    Span::styled(pad_left("", PLUS_W), Style::default().fg(styles::DIM)),
                    Span::raw(" "),
                    Span::styled(pad_left("", MINUS_W), Style::default().fg(styles::DIM)),
                    Span::raw(" "),
                    Span::styled(pad_left("", STATUS_W), Style::default()),
                ])
            }
            VisibleRow::File { file, .. } => {
                let status_style = if is_selected {
                    Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(styles::status_fg(file.status))
                };
                let status = file.status.letter();
                let name = file.path.rsplit('/').next().unwrap_or(&file.path);
                let plus = format!("+{}", file.added);
                let minus = format!("-{}", file.deleted);
                Line::from(vec![
                    marker,
                    Span::styled(
                        pad_right(&truncate(&format!("{}{}", indent, name), name_w), name_w),
                        if is_selected { row_style.fg(Color::White) } else { Style::default().fg(Color::White) },
                    ),
                    Span::raw(" "),
                    Span::styled(
                        pad_left(&plus, PLUS_W),
                        if is_selected { row_style } else { Style::default().fg(styles::STATUS_OK) },
                    ),
                    Span::raw(" "),
                    Span::styled(
                        pad_left(&minus, MINUS_W),
                        if is_selected { row_style } else { Style::default().fg(styles::STATUS_ERR) },
                    ),
                    Span::raw(" "),
                    Span::styled(pad_left(status, STATUS_W), status_style),
                ])
            }
        };
        f.render_widget(line, Rect { x: inner.x, y: inner.y + 2 + y, width: width as u16, height: 1 });
    }

    fn render_diffview(&mut self, frame: &mut Frame, area: Rect) {
        let file = self.diff_file.clone();
        let (orig_label, changed_label) = self.side_labels();
        self.diff_viewport = area.height.saturating_sub(2) as usize;
        let left = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(area);

        self.diff_max_line_w = self
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
        self.diff_hviewport = left[0].width.saturating_sub(10) as usize;

        let scrollbar_needed = self.diff_rows.len() > (area.height.saturating_sub(2)) as usize;

        match file {
            Some(f) => {
                let orig_title = self.pane_title(&orig_label, &f, true);
                let changed_title = self.pane_title(&changed_label, &f, false);
                if f.is_binary {
                    self.render_placeholder_pane_with_title(frame, left[0], &orig_title, "(binary file)");
                    self.render_placeholder_pane_with_title(frame, left[1], &changed_title, "(binary file)");
                    return;
                }
                let orig_empty = self.diff_rows.iter().all(|r| r.original.is_none());
                let changed_empty = self.diff_rows.iter().all(|r| r.changed.is_none());
                if orig_empty {
                    self.render_placeholder_pane_with_title(frame, left[0], &orig_title, "(no original content)");
                } else {
                    self.render_diff_pane(frame, left[0], &orig_title, true);
                }
                if changed_empty {
                    self.render_placeholder_pane_with_title(frame, left[1], &changed_title, "(no changed content)");
                } else {
                    self.render_diff_pane(frame, left[1], &changed_title, false);
                }
            }
            None => {
                let msg = if self.files.is_empty() {
                    if self.mode == ComparisonMode::CommitVsHead {
                        "无差异"
                    } else if !self.has_commits && self.mode != ComparisonMode::WorkingVsHead {
                        "无可用对比（仓库还没有 commit）"
                    } else {
                        "(no changes)"
                    }
                } else {
                    "(select a file)"
                };
                self.render_placeholder_pane_with_title(frame, left[0], &orig_label, msg);
                self.render_placeholder_pane_with_title(frame, left[1], &changed_label, msg);
            }
        }

        if scrollbar_needed {
            self.render_scrollbar(frame, area);
        }
    }

    fn pane_title(&self, label: &str, file: &ChangedFile, _is_original: bool) -> String {
        let rename = file.old_path.as_deref().unwrap_or("");
        if !rename.is_empty() && file.status == Status::Renamed {
            format!("{} · {} → {}", label, rename, file.path)
        } else {
            format!("{} · {}", label, file.path)
        }
    }

    fn render_diff_pane(&mut self, frame: &mut Frame, area: Rect, label: &str, is_original: bool) {
        let is_active = self.focus == Focus::Diff;
        let border_fg = if is_active { styles::ACTIVE_BORDER } else { styles::INACTIVE_BORDER };
        let block = Block::default()
            .title(format!(" {} ", label))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_fg))
            .border_type(if is_active { BorderType::Plain } else { BorderType::Rounded });
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
            let avail = inner.width.saturating_sub(8) as usize;
            let content = slice_after_hscroll(&cell.text, self.diff_hscroll);
            let content = truncate(&content, avail);
            let line = Line::from(vec![
                Span::styled(format!("{:>3} ", cell.num), Style::default().fg(styles::DIM)),
                Span::styled(marker, Style::default().fg(fg).add_modifier(Modifier::BOLD)),
                Span::styled(format!(" {}", content), Style::default().fg(fg).bg(bg)),
            ]);
            frame.render_widget(line, Rect { x: inner.x + 1, y: inner.y + 1 + i as u16, width: inner.width.saturating_sub(2), height: 1 });
        }
    }

    fn render_scrollbar(&mut self, frame: &mut Frame, area: Rect) {
        let inner_height = area.height.saturating_sub(2) as usize;
        if self.diff_rows.len() <= inner_height {
            return;
        }
        let total = self.diff_rows.len();
        let viewport = inner_height.max(1);
        let pos = (self.diff_vscroll as f64 / total as f64) * inner_height as f64;
        let size = (viewport as f64 / total as f64 * inner_height as f64).max(1.0);
        let track = area.y + 1;
        let x = area.x + area.width.saturating_sub(1);
        for i in 0..inner_height {
            let bar_y = track + i as u16;
            let is_bar = (bar_y as f64) >= pos && (bar_y as f64) < pos + size;
            let ch = if is_bar { "█" } else { "░" };
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(ch, Style::default().fg(styles::DIM)))),
                Rect { x, y: bar_y, width: 1, height: 1 },
            );
        }
    }

    fn render_placeholder_pane_with_title(&mut self, f: &mut Frame, area: Rect, label: &str, msg: &str) {
        let is_active = self.focus == Focus::Diff;
        let border_fg = if is_active { styles::ACTIVE_BORDER } else { styles::INACTIVE_BORDER };
        let block = Block::default()
            .title(format!(" {} ", label))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_fg))
            .border_type(if is_active { BorderType::Plain } else { BorderType::Rounded });
        let inner = block.inner(area);
        f.render_widget(block, area);
        let msg = Line::from(Span::styled(msg, Style::default().fg(styles::DIM)));
        let y = inner.y + inner.height.saturating_div(2);
        f.render_widget(msg, Rect { x: inner.x, y, width: inner.width, height: 1 });
    }

    fn render_statusbar(&mut self, f: &mut Frame, area: Rect) {
        let mut spans: Vec<Span> = Vec::new();
        let mode_tag = match self.mode {
            ComparisonMode::WorkingVsHead => "[模式A]",
            ComparisonMode::StagedVsHead => "[模式B]",
            ComparisonMode::StagedVsWorking => "[模式C]",
            ComparisonMode::CommitVsHead => "[模式D]",
        };
        spans.push(Span::styled(mode_tag, Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)));
        spans.push(Span::raw(" "));
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
            key_hint("Tab"),
            key_hint("↑↓"),
            key_hint("PgUp/PgDn"),
            key_hint("l"),
            key_hint("1/2/3"),
            key_hint("/"),
            key_hint("n/N"),
            key_hint("r"),
            key_hint("?"),
            key_hint("q"),
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

    fn clear_area(frame: &mut Frame, area: Rect) {
    frame.render_widget(Clear, area);
}

fn render_commit_picker(&mut self, f: &mut Frame, area: Rect, commits: &[CommitEntry], cursor: usize) {
        let width = 60u16.min(area.width.saturating_sub(4));
        let height = (commits.len() as u16 + 5).min(area.height.saturating_sub(4)).max(7);
        let x = area.x + area.width.saturating_div(2) - width.saturating_div(2);
        let y = area.y + area.height.saturating_div(2) - height.saturating_div(2);
        let panel = Rect { x, y, width, height };
        Self::clear_area(f, panel);
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
        let height = 19u16.min(area.height.saturating_sub(2));
        let x = area.x + area.width.saturating_div(2) - width.saturating_div(2);
        let y = area.y + area.height.saturating_div(2) - height.saturating_div(2);
        let panel = Rect { x, y, width, height };
        Self::clear_area(f, panel);
        let block = Block::default()
            .title(" 帮助 ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .border_type(BorderType::Rounded);
        let inner = block.inner(panel);
        f.render_widget(block, panel);
        let help_lines = [
            ("Tab", "切换焦点（文件列表 / 对比区）"),
            ("文件列表焦点:", "操作变更文件列表"),
            ("↑↓", "移动光标"),
            ("→ / ←", "展开 / 折叠目录"),
            ("PgUp/PgDn", "列表翻页"),
            ("对比区焦点:", "操作对比视图"),
            ("↑↓", "逐行滚动对比区"),
            ("PgUp/PgDn", "按页滚动对比区"),
            ("→ / ←", "水平滚动对比区"),
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

fn slice_after_hscroll(s: &str, hscroll: usize) -> String {
    if hscroll == 0 {
        return s.to_string();
    }
    let mut acc = 0;
    let mut out = String::new();
    for c in s.chars() {
        let cw = UnicodeWidthStr::width(c.to_string().as_str());
        if acc >= hscroll {
            out.push(c);
        }
        acc += cw;
    }
    out
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

fn key_hint<'a>(key: &'a str) -> Span<'a> {
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