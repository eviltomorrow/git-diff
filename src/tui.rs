use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
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

/// Default share of the width given to the file list, in percent.
const DEFAULT_LIST_PCT: u16 = 26;
/// Default share of the diff area given to the original (left) pane.
const DEFAULT_DIFF_PCT: u16 = 50;
/// Clamp ranges for the two draggable dividers, so a pane can never vanish.
const LIST_MIN_PCT: u16 = 12;
const LIST_MAX_PCT: u16 = 60;
const DIFF_MIN_PCT: u16 = 20;
const DIFF_MAX_PCT: u16 = 80;
/// Per-frame step of the file-list stat-column reveal (~150ms at 60fps).
const STATS_REVEAL_STEP: f32 = 0.12;

/// Which divider a drag is moving.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragTarget {
    ListSplit,
    DiffSplit,
}

/// Thin shell: owns the terminal and the pure [`Controller`]; rendering is
/// read-only over controller state (viewports are synced via
/// [`Controller::update_viewports`] before drawing).
pub struct App<'a> {
    ctrl: Controller<'a>,
    /// The file-list and diff panel rects from the last render, used to map
    /// mouse clicks onto the focused panel.
    list_rect: Rect,
    diff_rect: Rect,
    /// Middle band holding both panels; the file-list divider spans it.
    panels_rect: Rect,
    /// Absolute screen columns of the two draggable dividers, captured from
    /// the last render so hit-testing matches what the user sees.
    list_divider_x: u16,
    diff_divider_x: u16,
    list_pct: u16,
    diff_pct: u16,
    /// Divider under the pointer (drawn as a highlight) and the one currently
    /// being dragged, if any.
    hover: Option<DragTarget>,
    drag: Option<DragTarget>,
    /// A drag changed a pane ratio at least once; on release the screen is
    /// cleared once if the panes contain wide glyphs (clearing on every drag
    /// step would flicker).
    drag_resized: bool,
    /// Whether the file-list stat columns should be shown (`true`) or hidden
    /// (`false`) at the current width. Kept separate from the animated
    /// reveal so a few columns of hysteresis can damp divider jitter.
    stats_visible: bool,
    /// Animated reveal of the stat columns: 0.0 = hidden, 1.0 = full group.
    stats_reveal: f32,
    /// First render snaps the reveal to its target instead of animating in.
    stats_initialized: bool,
    /// Set when the selected diff changed (file switch, sort, filter, mode):
    /// the next frame must be a full terminal redraw. ratatui's incremental
    /// diff never emits cells that trail a double-width (CJK) glyph, so when a
    /// wide glyph moves to a spot where the previous frame had other content,
    /// the old character can linger on screen. A full redraw (clear + redraw)
    /// wipes those cells first.
    full_redraw: bool,
}

/// Which region of the screen may have changed between two frames. A full
/// terminal redraw is only needed when a region containing wide (CJK)
/// glyphs actually moved: ratatui's incremental diff never clears the cell
/// that trails a double-width glyph, so only clearing the whole screen can
/// remove it. Pure-ASCII scrolling needs no full redraw, which avoids the
/// whole-screen flash some terminals show on every keypress.
#[derive(Clone, PartialEq, Eq)]
pub struct NavKey {
    diff_file: Option<String>,
    cursor: usize,
    list_scroll: usize,
    diff_cursor: usize,
    diff_vscroll: usize,
    diff_hscroll: usize,
    diff_rows_len: usize,
    mode: ComparisonMode,
    selected_commit: Option<String>,
    files: Vec<ChangedFile>,
    collapsed: Vec<String>,
    sort: SortMode,
    filter: Option<String>,
    fold_unchanged: bool,
    hunk_idx: usize,
    hunk_count: usize,
    search: Option<String>,
    goto: Option<String>,
    ignore_whitespace: bool,
    status: String,
    loading: bool,
    overlay: u64,
}

impl<'a> App<'a> {
    pub fn new(
        facade: GitFacade<'a>,
        repo_path: PathBuf,
        has_commits: bool,
        initial_commit: Option<String>,
    ) -> Result<Self> {
        let ctrl = Controller::new(facade, repo_path, has_commits, initial_commit)?;
        Ok(Self {
            ctrl,
            list_rect: Rect::default(),
            diff_rect: Rect::default(),
            panels_rect: Rect::default(),
            list_divider_x: 0,
            diff_divider_x: 0,
            list_pct: DEFAULT_LIST_PCT,
            diff_pct: DEFAULT_DIFF_PCT,
            hover: None,
            drag: None,
            drag_resized: false,
            stats_visible: false,
            stats_reveal: 0.0,
            stats_initialized: false,
            full_redraw: true,
        })
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        let before = self.nav_key();
        self.ctrl.handle_key(key);
        self.sync_full_redraw(before);
    }

    pub fn nav_key(&self) -> NavKey {
        let c = &self.ctrl;
        let mut collapsed: Vec<String> = c.collapsed.iter().cloned().collect();
        collapsed.sort();
        NavKey {
            diff_file: c.diff_file.as_ref().map(|f| f.path.clone()),
            cursor: c.cursor,
            list_scroll: c.list_scroll,
            diff_cursor: c.diff_cursor,
            diff_vscroll: c.diff_vscroll,
            diff_hscroll: c.diff_hscroll,
            diff_rows_len: c.diff_rows.len(),
            mode: c.mode,
            selected_commit: c.selected_commit.clone(),
            files: c.files.clone(),
            collapsed,
            sort: c.sort,
            filter: c.filter.clone(),
            fold_unchanged: c.fold_unchanged,
            hunk_idx: c.hunk_idx,
            hunk_count: c.hunk_count,
            search: c.search.clone(),
            goto: c.goto.clone(),
            ignore_whitespace: c.ignore_whitespace,
            status: c.status.clone(),
            loading: c.loading,
            overlay: overlay_key(c),
        }
    }

    /// After handling a key / mouse event or drawing, call with the `nav_key()`
    /// captured just before. Requests a full redraw only when a region that
    /// actually contains wide (CJK) glyphs moved.
    pub fn sync_full_redraw(&mut self, before: NavKey) {
        let now = self.nav_key();
        if now == before {
            return;
        }
        // overlays cover the whole panel area -> always redraw fully
        if now.overlay != before.overlay {
            self.full_redraw = true;
            return;
        }
        let diff_moved = now.diff_cursor != before.diff_cursor
            || now.diff_vscroll != before.diff_vscroll
            || now.diff_hscroll != before.diff_hscroll
            || now.diff_rows_len != before.diff_rows_len;
        let list_moved = now.cursor != before.cursor
            || now.list_scroll != before.list_scroll
            || now.collapsed != before.collapsed
            || now.sort != before.sort
            || now.filter != before.filter;
        let switched = now.diff_file != before.diff_file
            || now.mode != before.mode
            || now.files != before.files
            || now.selected_commit != before.selected_commit;
        let other_moved = now.fold_unchanged != before.fold_unchanged
            || now.search != before.search
            || now.goto != before.goto
            || now.ignore_whitespace != before.ignore_whitespace
            || now.status != before.status
            || now.loading != before.loading;
        // the header's "行" wide glyph shifts when the line numbers change digit
        // count (e.g. 9 -> 10), so a diff cursor move can need a full redraw
        // even for ASCII content
        let stats_shifted = digits(now.diff_cursor + 1) != digits(before.diff_cursor + 1)
            || digits(now.diff_rows_len) != digits(before.diff_rows_len)
            || now.hunk_idx != before.hunk_idx
            || now.hunk_count != before.hunk_count;

        let mut need = false;
        if diff_moved {
            need |= self.diff_has_wide() || stats_shifted;
        }
        if list_moved {
            need |= self.list_has_wide();
        }
        if switched || other_moved {
            need |= self.any_wide();
        }
        if need {
            self.full_redraw = true;
        }
    }

    fn diff_has_wide(&self) -> bool {
        use unicode_width::UnicodeWidthChar;
        self.ctrl.diff_rows.iter().any(|r| {
            r.original
                .as_ref()
                .is_some_and(|x| x.text.chars().any(|ch| UnicodeWidthChar::width(ch).unwrap_or(0) > 1))
                || r.changed
                    .as_ref()
                    .is_some_and(|x| x.text.chars().any(|ch| UnicodeWidthChar::width(ch).unwrap_or(0) > 1))
        })
    }

    fn list_has_wide(&self) -> bool {
        use unicode_width::UnicodeWidthChar;
        self.ctrl.visible_rows().iter().any(|r| {
            let name = match r {
                crate::tree::VisibleRow::File { file, .. } => &file.path,
                crate::tree::VisibleRow::Dir { path, .. } => path,
            };
            name.chars().any(|ch| UnicodeWidthChar::width(ch).unwrap_or(0) > 1)
        })
    }

    fn any_wide(&self) -> bool {
        self.diff_has_wide()
            || self.list_has_wide()
            || self
                .ctrl
                .diff_file
                .as_ref()
                .is_some_and(|f| wide_str(&f.path))
    }

    /// Whether the next render must be a full redraw; consume the flag.
    pub fn take_full_redraw(&mut self) -> bool {
        std::mem::take(&mut self.full_redraw)
    }

    /// Read-only access to the controller (used by tests to inspect state).
    pub fn ctrl(&self) -> &Controller<'a> {
        &self.ctrl
    }

    /// Clicking inside a panel focuses it (a Tab variant); pressing on a
    /// divider and dragging resizes the panes. Returns `true` when the event
    /// changed visible state, so the event loop can skip redrawing on plain
    /// pointer motion.
    pub fn handle_mouse(&mut self, mouse: MouseEvent) -> bool {
        // overlays own the screen; pane gestures underneath are meaningless
        if self.ctrl.overlay.is_some() {
            return false;
        }

        let before_hover = self.hover;
        let before_drag = self.drag;
        let before_focus = self.ctrl.focus;
        let before_list = self.list_pct;
        let before_diff = self.diff_pct;

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                // a missed button-up (e.g. released outside the terminal)
                // must not leave a stale drag active
                self.drag = None;
                self.drag_resized = false;
                if let Some(target) = self.divider_at(mouse.column, mouse.row) {
                    self.drag = Some(target);
                    self.hover = Some(target);
                } else {
                    let in_list = rect_contains(self.list_rect, mouse.column, mouse.row);
                    let in_diff = rect_contains(self.diff_rect, mouse.column, mouse.row);
                    match (in_list, in_diff) {
                        (true, _) => self.ctrl.set_focus(Focus::FileList),
                        (false, true) => self.ctrl.set_focus(Focus::Diff),
                        _ => {}
                    }
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(target) = self.drag {
                    self.apply_drag(target, mouse.column);
                    self.hover = Some(target);
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if self.drag.take().is_some() {
                    self.hover = self.divider_at(mouse.column, mouse.row);
                    // One clear after the final layout settles removes any
                    // cells left trailing double-width glyphs while dragging.
                    if std::mem::take(&mut self.drag_resized)
                        && (self.diff_has_wide() || self.list_has_wide())
                    {
                        self.full_redraw = true;
                    }
                }
            }
            MouseEventKind::Moved => {
                self.hover = self.divider_at(mouse.column, mouse.row);
            }
            _ => {}
        }

        let resized = before_list != self.list_pct || before_diff != self.diff_pct;
        if resized {
            self.drag_resized = true;
        }

        before_hover != self.hover
            || before_drag != self.drag
            || before_focus != self.ctrl.focus
            || resized
    }

    /// The divider under a screen position, widened by one column on either
    /// side because a single-column target is hard to hit.
    fn divider_at(&self, x: u16, y: u16) -> Option<DragTarget> {
        if self.panels_rect.width == 0
            || y < self.panels_rect.y
            || y >= self.panels_rect.y.saturating_add(self.panels_rect.height)
        {
            return None;
        }
        if x.abs_diff(self.list_divider_x) <= 1 {
            return Some(DragTarget::ListSplit);
        }
        if x.abs_diff(self.diff_divider_x) <= 1 {
            return Some(DragTarget::DiffSplit);
        }
        None
    }

    /// Move a divider so it sits under the pointer column, clamped to the
    /// pane's allowed share of the width.
    fn apply_drag(&mut self, target: DragTarget, x: u16) {
        match target {
            DragTarget::ListSplit => {
                self.list_pct = pct_at(x, self.panels_rect, LIST_MIN_PCT, LIST_MAX_PCT);
            }
            DragTarget::DiffSplit => {
                self.diff_pct = pct_at(x, self.diff_rect, DIFF_MIN_PCT, DIFF_MAX_PCT);
            }
        }
    }

    /// Whether a full redraw was requested since the last one (without
    /// consuming it). The event loop uses this to honour a redraw request
    /// made by `sync_full_redraw` right after a frame.
    pub fn full_redraw_pending(&self) -> bool {
        self.full_redraw
    }

    /// Update the stat-column target for the current list width. A couple of
    /// columns of hysteresis keep a divider sitting exactly on the threshold
    /// from toggling the columns on every mouse event.
    fn update_stats_visibility(&mut self, width: usize) {
        const SHOW_AT: usize = ListLayout::FULL_MIN_WIDTH;
        const HIDE_AT: usize = SHOW_AT - 2;
        if !self.stats_initialized {
            self.stats_visible = width >= SHOW_AT;
            self.stats_reveal = if self.stats_visible { 1.0 } else { 0.0 };
            self.stats_initialized = true;
        } else if self.stats_visible {
            if width < HIDE_AT {
                self.stats_visible = false;
            }
        } else if width >= SHOW_AT {
            self.stats_visible = true;
        }
    }

    /// Advance the stat-column reveal animation by one frame. Returns true
    /// when the reveal changed and a redraw is needed.
    pub fn tick(&mut self) -> bool {
        let target = if self.stats_visible { 1.0 } else { 0.0 };
        if (self.stats_reveal - target).abs() < f32::EPSILON {
            return false;
        }
        self.stats_reveal = if self.stats_reveal < target {
            (self.stats_reveal + STATS_REVEAL_STEP).min(target)
        } else {
            (self.stats_reveal - STATS_REVEAL_STEP).max(target)
        };
        if (self.stats_reveal - target).abs() < f32::EPSILON && self.list_has_wide() {
            // the name column stopped resizing; clear any cells left trailing
            // wide glyphs during the animation
            self.full_redraw = true;
        }
        true
    }

    /// Whether the reveal animation still has frames to run.
    pub fn is_animating(&self) -> bool {
        let target = if self.stats_visible { 1.0 } else { 0.0 };
        (self.stats_reveal - target).abs() >= f32::EPSILON
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
            .constraints([Constraint::Percentage(self.list_pct), Constraint::Percentage(100 - self.list_pct)])
            .split(chunks[1]);
        // the diff area is split again; both halves are passed to the renderer
        // so the draggable divider and hit-testing share one geometry
        let diff_cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(self.diff_pct), Constraint::Percentage(100 - self.diff_pct)])
            .split(panels[1]);
        let list_height = panels[0].height.saturating_sub(2) as usize;
        let diff_height = panels[1].height.saturating_sub(3) as usize;
        // horizontal scrolling is shared, so the narrower pane bounds it
        let diff_width = diff_cols[0]
            .width
            .min(diff_cols[1].width)
            .saturating_sub(10) as usize;
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
        self.panels_rect = chunks[1];
        self.list_rect = panels[0];
        self.diff_rect = panels[1];
        self.list_divider_x = panels[0].x.saturating_add(panels[0].width);
        self.diff_divider_x = diff_cols[1].x;
        // keep the stat-column target in sync with the list's inner width
        self.update_stats_visibility(panels[0].width.saturating_sub(2) as usize);
        let reveal = self.stats_reveal;

        render_header(f, chunks[0], &self.ctrl);
        render_filelist(f, panels[0], &mut self.ctrl, reveal);
        render_diffview(f, diff_cols[0], diff_cols[1], &mut self.ctrl);
        render_statusbar(f, chunks[2], &self.ctrl);
        if let Some(target) = self.drag.or(self.hover) {
            let (x, band) = match target {
                DragTarget::ListSplit => (self.list_divider_x, self.panels_rect),
                DragTarget::DiffSplit => (self.diff_divider_x, self.diff_rect),
            };
            render_divider_highlight(f, x, band.y, band.height, self.drag == Some(target));
        }

        match &self.ctrl.overlay {
            Some(Overlay::CommitPicker { commits, cursor, preview_scroll }) => {
                let preview = self
                    .ctrl
                    .commit_messages
                    .get(&commits[*cursor].short_hash)
                    .map(String::as_str);
                // preview rows = panel inner height minus header + hint lines,
                // used as the PgUp/PgDn scroll page
                let panel_h = commit_picker_height(commits.len(), area.height);
                self.ctrl.commit_preview_viewport = panel_h.saturating_sub(5) as usize;
                let max_scroll = render_commit_picker(f, area, commits, *cursor, preview, *preview_scroll);
                self.ctrl.commit_preview_max_scroll = max_scroll;
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
    let mut spans: Vec<Span<'static>> = vec![
        Span::raw(" "),
        Span::styled(mode_desc, styles::header_style()),
    ];
    if let Some(branch) = &ctrl.branch {
        spans.push(Span::styled(format!("  [{}]", branch), Style::default().fg(Color::Cyan)));
    }
    spans.push(Span::styled(format!("  {}", path_str), Style::default().fg(Color::DarkGray)));
    // diff position summary, right-aligned in the top row
    let stats = diff_status(ctrl);
    let w = area.width as usize;
    if stats.is_empty() {
        f.render_widget(Paragraph::new(Line::from(spans)), Rect { x: area.x, y: area.y, width: area.width, height: 1 });
    } else {
        // Right-align the stats text to the very edge of the row. It slides
        // horizontally when its length changes, but the event loop forces a
        // full redraw on any navigation change, so the wide "行" glyph never
        // leaves residue behind.
        let stats_w = UnicodeWidthStr::width(stats.as_str());
        let avail = w.saturating_sub(stats_w);
        let mut out = truncate_spans(spans, avail);
        let used: usize = out.iter().map(|s| s.width()).sum();
        if used < avail {
            out.push(Span::raw(" ".repeat(avail - used)));
        }
        out.push(Span::styled(stats, Style::default().fg(Color::DarkGray)));
        f.render_widget(Paragraph::new(Line::from(out)), Rect { x: area.x, y: area.y, width: area.width, height: 1 });
    }
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "─".repeat(area.width as usize),
            styles::separator_style(),
        ))),
        Rect { x: area.x, y: area.y + 1, width: area.width, height: 1 },
    );
}

// ------------------------------------------------------------- file list

fn render_filelist(f: &mut Frame, area: Rect, ctrl: &mut Controller<'_>, reveal: f32) {
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

    let cols = ListLayout::for_width(inner.width as usize, reveal);
    let list_height = inner.height.saturating_sub(2) as usize;

    let header_style = Style::default().fg(styles::HEADER_FG).add_modifier(Modifier::BOLD);
    let mut header = vec![
        Span::raw(" ".repeat(ListLayout::MARKER_W)),
        Span::styled(pad_right("Name", cols.name), header_style),
        Span::raw(" ".repeat(cols.pad())),
    ];
    if cols.plus_cols() > 0 {
        header.push(Span::raw(" "));
        header.push(Span::styled(pad_left("+", ListLayout::PLUS_W), header_style));
    }
    if cols.minus_cols() > 0 {
        header.push(Span::raw(" "));
        header.push(Span::styled(pad_left("-", ListLayout::MINUS_W), header_style));
    }
    if cols.status_cols() > 0 {
        header.push(Span::raw(" "));
        header.push(Span::styled(pad_left("Status", ListLayout::STATUS_W), header_style));
    }
    f.render_widget(Line::from(header), Rect { x: inner.x, y: inner.y, width: inner.width, height: 1 });
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
        render_list_row(f, inner, &rows[idx], i as u16, is_selected, cols);
    }
}

/// Column widths for one file-list row. The stat group is revealed
/// progressively: as the pane widens, `Status`, then `-M`, then `+N` slide in
/// from the right, while the filename column narrows in step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ListLayout {
    name: usize,
    /// Columns reserved at the right edge for the stat group (0..=17).
    reserved: usize,
}

impl ListLayout {
    const MARKER_W: usize = 2;
    const GAP_W: usize = 1;
    const PLUS_W: usize = 4;
    const MINUS_W: usize = 4;
    const STATUS_W: usize = 6;
    /// Gap + "+N" + gap + "-M" + gap + "Status".
    const RESERVED_MAX: usize = 3 * Self::GAP_W + Self::PLUS_W + Self::MINUS_W + Self::STATUS_W;
    /// Never squeeze the filename below this, even mid-animation.
    const MIN_NAME_W: usize = 12;
    /// Inner width at which the whole stat group fits beside a usable name.
    const FULL_MIN_WIDTH: usize = Self::MARKER_W + Self::RESERVED_MAX + Self::MIN_NAME_W;

    fn for_width(width: usize, reveal: f32) -> Self {
        let want = ((Self::RESERVED_MAX as f32) * reveal.clamp(0.0, 1.0)).round() as usize;
        // mid-hide on a narrow pane, cap the reserve so the name stays readable
        let cap = width.saturating_sub(Self::MARKER_W + Self::MIN_NAME_W);
        let reserved = want.min(cap);
        Self {
            name: width.saturating_sub(Self::MARKER_W + reserved),
            reserved,
        }
    }

    /// Width including its leading gap, or 0 while the segment is clipped out.
    fn status_cols(&self) -> usize {
        if self.reserved >= Self::GAP_W + Self::STATUS_W {
            Self::GAP_W + Self::STATUS_W
        } else {
            0
        }
    }

    fn minus_cols(&self) -> usize {
        if self.reserved >= 2 * Self::GAP_W + Self::STATUS_W + Self::MINUS_W {
            Self::GAP_W + Self::MINUS_W
        } else {
            0
        }
    }

    fn plus_cols(&self) -> usize {
        if self.reserved >= Self::RESERVED_MAX {
            Self::GAP_W + Self::PLUS_W
        } else {
            0
        }
    }

    fn stats_width(&self) -> usize {
        self.plus_cols() + self.minus_cols() + self.status_cols()
    }

    /// Blank columns between the name and the first visible stat segment.
    fn pad(&self) -> usize {
        self.reserved - self.stats_width()
    }
}

fn render_list_row(f: &mut Frame, inner: Rect, row: &VisibleRow, y: u16, is_selected: bool, cols: ListLayout) {
    let selected_style = Style::default()
        .fg(Color::White)
        .add_modifier(Modifier::BOLD);
    let row_style = if is_selected { selected_style } else { Style::default() };
    let width = inner.width as usize;

    let marker = if is_selected {
        Span::styled("▌ ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))
    } else {
        Span::raw("  ")
    };

    // stat segments slide in from the right; any not-yet-revealed columns are
    // blank padding so the filename column shrinks smoothly
    let stat_spans = |plus: &str, minus: &str, plus_style: Style, minus_style: Style, status: &str, status_style: Style| {
        let mut spans = vec![Span::raw(" ".repeat(cols.pad()))];
        if cols.plus_cols() > 0 {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(pad_left(plus, ListLayout::PLUS_W), plus_style));
        }
        if cols.minus_cols() > 0 {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(pad_left(minus, ListLayout::MINUS_W), minus_style));
        }
        if cols.status_cols() > 0 {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(pad_left(status, ListLayout::STATUS_W), status_style));
        }
        spans
    };

    let line = match row {
        VisibleRow::Dir { collapsed, path, added, deleted, guide, depth, .. } => {
            let is_root = *depth == 0;
            // the root (current directory) is display-only: no collapse arrow,
            // just the folder emoji. child dirs get the ▾/▸ collapse arrow.
            let icon = if is_root { "📁 " } else if *collapsed { "▸ " } else { "▾ " };
            // keep the indentation inside the name column; never let deep
            // guides push the stats out of alignment
            let guide = truncate(guide, cols.name);
            let name_avail = cols.name.saturating_sub(UnicodeWidthStr::width(guide.as_str()));
            let mut spans = vec![
                marker,
                // relationship lines stay thin (dim, not bold) even when
                // the row is selected
                Span::styled(guide, guide_style()),
                Span::styled(
                    pad_right(&truncate(&format!("{}{}", icon, short_name(path)), name_avail), name_avail),
                    row_style.fg(Color::Yellow),
                ),
            ];
            spans.extend(stat_spans(
                &format!("+{}", added),
                &format!("-{}", deleted),
                amount_style(*added, true, is_selected),
                amount_style(*deleted, false, is_selected),
                "",
                row_style,
            ));
            Line::from(spans)
        }
        VisibleRow::File { file, guide, .. } => {
            let status_style = if is_selected {
                Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(styles::status_fg(file.status))
            };
            let name = file.path.rsplit('/').next().unwrap_or(&file.path);
            let guide = truncate(guide, cols.name);
            let name_avail = cols.name.saturating_sub(UnicodeWidthStr::width(guide.as_str()));
            let mut spans = vec![
                marker,
                Span::styled(guide, guide_style()),
                Span::styled(
                    pad_right(&truncate(name, name_avail), name_avail),
                    if is_selected { row_style.fg(Color::White) } else { Style::default().fg(Color::White) },
                ),
            ];
            spans.extend(stat_spans(
                &format!("+{}", file.added),
                &format!("-{}", file.deleted),
                amount_style(file.added, true, is_selected),
                amount_style(file.deleted, false, is_selected),
                file.status.letter(),
                status_style,
            ));
            Line::from(spans)
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

fn render_diffview(f: &mut Frame, orig_area: Rect, changed_area: Rect, ctrl: &mut Controller<'_>) {
    // Clear both panes up front so every cell is re-evaluated when the
    // selected file changes. This defends against ghost residue from the
    // previous file lingering at cells that a normal redraw would skip (e.g.
    // cells that trail a double-width glyph).
    f.render_widget(Clear, orig_area);
    f.render_widget(Clear, changed_area);
    let file = ctrl.diff_file.clone();
    let (orig_label, changed_label) = ctrl.side_labels();

    let scrollbar_needed = ctrl.diff_rows.len() > (changed_area.height.saturating_sub(2)) as usize;

    match file {
        Some(file) => {
            let orig_title = pane_title(&orig_label, &file);
            let changed_title = pane_title(&changed_label, &file);
            if file.is_binary {
                render_placeholder_pane(f, orig_area, &orig_title, "(binary file)", ctrl);
                render_placeholder_pane(f, changed_area, &changed_title, "(binary file)", ctrl);
                return;
            }
            let orig_empty = ctrl.diff_rows.iter().all(|r| r.original.is_none());
            let changed_empty = ctrl.diff_rows.iter().all(|r| r.changed.is_none());
            if orig_empty {
                render_placeholder_pane(f, orig_area, &orig_title, "(no original content)", ctrl);
            } else {
                render_diff_pane(f, orig_area, &orig_title, true, ctrl);
            }
            if changed_empty {
                render_placeholder_pane(f, changed_area, &changed_title, "(no changed content)", ctrl);
            } else {
                render_diff_pane(f, changed_area, &changed_title, false, ctrl);
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
            render_placeholder_pane(f, orig_area, &orig_label, msg, ctrl);
            render_placeholder_pane(f, changed_area, &changed_label, msg, ctrl);
        }
    }

    if scrollbar_needed {
        render_scrollbar(f, changed_area, ctrl);
    }
}

/// Bright vertical bar over a divider while it is hovered or dragged, so the
/// resizable seam is discoverable.
fn render_divider_highlight(f: &mut Frame, x: u16, y: u16, height: u16, dragging: bool) {
    // Leave the pane's top/bottom border rows intact so rounded corners stay
    // rounded; the highlight only spans the vertical edges.
    if height <= 2 {
        return;
    }
    let fg = if dragging { styles::ACTIVE_BORDER } else { styles::HINT };
    let style = Style::default().fg(fg).add_modifier(Modifier::BOLD);
    for i in 1..height - 1 {
        let cell = Rect { x, y: y.saturating_add(i), width: 1, height: 1 };
        f.render_widget(Paragraph::new("│").style(style), cell);
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

/// Compact diff-position summary shown right-aligned in the top header row.
fn diff_status(ctrl: &Controller<'_>) -> String {
    if ctrl.diff_rows.is_empty() {
        return String::new();
    }
    let line = format!("行 {}/{}", ctrl.diff_cursor + 1, ctrl.diff_rows.len());
    if ctrl.hunk_count > 0 {
        format!("{line} hunk {}/{}", ctrl.hunk_idx, ctrl.hunk_count)
    } else {
        line
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
    let mut spans = content_spans(cell, fg, bg, emph_bg, content_w, ctrl.diff_hscroll, ctrl.search.as_deref());
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
    search: Option<&str>,
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
    // highlight search matches within the rendered text
    if let Some(query) = search
        && !query.is_empty()
        && !out.is_empty()
    {
        let q = query.to_lowercase();
        let mut expanded: Vec<Span<'static>> = Vec::new();
        for span in out {
            let text = span.content.as_ref();
            let lower = text.to_lowercase();
            let mut start = 0;
            let mut rest = lower.as_str();
            let base = span.style;
            loop {
                match rest.find(&q) {
                    Some(pos) => {
                        let match_start = start + pos;
                        let match_end = match_start + q.len();
                        if match_start > start {
                            expanded.push(Span::styled(
                                text[start..match_start].to_string(),
                                base,
                            ));
                        }
                        expanded.push(Span::styled(
                            text[match_start..match_end].to_string(),
                            base.bg(styles::SEARCH_BG),
                        ));
                        start = match_end;
                        rest = &lower[match_end..];
                        // avoid matching empty query at end
                        if match_end >= lower.len() {
                            break;
                        }
                    }
                    None => {
                        if start < text.len() {
                            expanded.push(Span::styled(text[start..].to_string(), base));
                        }
                        break;
                    }
                }
            }
        }
        out = expanded;
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
        if ctrl.ignore_whitespace {
            left.push(Span::styled(" │ ", styles::status_sep_style()));
            left.push(Span::styled("忽略空白", Style::default().fg(Color::Yellow)));
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
    } else if let Some(g) = &ctrl.goto {
        left.push(Span::styled(" │ ", styles::status_sep_style()));
        left.push(Span::styled(
            format!("跳到行: {}", g),
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ));
        right.push(Span::styled("│ [Enter]跳转 [Esc]取消", styles::key_style()));
    } else if let Some(q) = &ctrl.search {
        left.push(Span::styled(" │ ", styles::status_sep_style()));
        left.push(Span::styled(
            format!("搜索: {}", q),
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ));
        right.push(Span::styled("│ [Esc]结束", styles::key_style()));
    } else {
        let hints: [(&str, &str); 11] = [
            ("Tab", "焦点"),
            ("1-4", "模式"),
            ("↑↓", "移动"),
            ("Pg", "翻页"),
            ("g/f", "跳转/搜索"),
            ("w", "忽略空白"),
            ("s", "排序"),
            ("n/m", "hunk"),
            ("z", "折叠"),
            ("?", "帮助"),
            ("q", "退出"),
        ];
        for (k, d) in hints.iter() {
            right.push(Span::styled(format!("[{}]", k), styles::key_style()));
            right.push(Span::styled(*d, Style::default().fg(styles::DIM)));
        }
    }

    let spans = left;
    let right_w: usize = right.iter().map(|s| s.width()).sum();
    let w = area.width as usize;
    if right_w == 0 {
        f.render_widget(Line::from(spans), area);
        return;
    }
    // The right hint block is always anchored at the right edge. When the left
    // content is too wide to fit beside it, we truncate the left content instead
    // of letting the hints shift left: shifting would slide the wide (CJK) glyphs
    // in the hints over cells that previously held other characters, and the
    // ratatui diff skips the cells that trail a double-width glyph, so those old
    // cells would linger on screen as ghost residue.
    let right_x = w.saturating_sub(right_w + 1).max(1);
    let avail = right_x.saturating_sub(1);
    let mut out = truncate_spans(spans, avail);
    let used: usize = out.iter().map(|s| s.width()).sum();
    if used < avail {
        out.push(Span::raw(" ".repeat(avail - used)));
    }
    out.extend(right);
    f.render_widget(Line::from(out), area);
}

/// Copy `spans`, truncating so the total width is at most `avail`; a truncated
/// tail is replaced with `…`. Styles are preserved.
fn truncate_spans<'a>(spans: Vec<Span<'a>>, avail: usize) -> Vec<Span<'a>> {
    let mut out: Vec<Span<'a>> = Vec::new();
    let mut used = 0usize;
    for s in spans {
        if used >= avail {
            break;
        }
        let tw = s.width();
        if used + tw <= avail {
            out.push(s);
            used += tw;
        } else {
            let need = avail - used;
            let text = s.content.as_ref();
            out.push(Span::styled(truncate(text, need), s.style));
            used = avail;
        }
    }
    out
}

// ------------------------------------------------------------- overlays

fn clear_area(f: &mut Frame, area: Rect) {
    f.render_widget(Clear, area);
}

/// Widest the author / refs columns may grow before truncation.
const MAX_AUTHOR_W: usize = 24;
const MAX_REFS_W: usize = 16;

/// Height of the commit picker panel for a given commit count / screen height.
/// The renderer and the controller both need it (the latter for the preview's
/// PgUp/PgDn page size), so it lives in one place.
fn commit_picker_height(commits_len: usize, area_height: u16) -> u16 {
    const MAX_ROWS: usize = 20;
    ((commits_len.min(MAX_ROWS) as u16) + 5)
        .min(area_height.saturating_sub(4))
        .max(7)
}

fn render_commit_picker(
    f: &mut Frame,
    area: Rect,
    commits: &[CommitEntry],
    cursor: usize,
    preview: Option<&str>,
    preview_scroll: usize,
) -> usize {
    const SCROLL_W: usize = 2;
    const MARKER_W: usize = 2;
    const HASH_W: usize = 9;
    const DATE_W: usize = 11;
    const GAP: usize = 1;
    const MIN_PREVIEW_W: usize = 28;

    let max_text = commits
        .iter()
        .map(|c| UnicodeWidthStr::width(c.title.as_str()))
        .max()
        .unwrap_or(0);
    let max_author = commits
        .iter()
        .map(|c| UnicodeWidthStr::width(c.author.as_str()))
        .max()
        .unwrap_or(0)
        .min(MAX_AUTHOR_W);
    let max_refs = commits
        .iter()
        .map(|c| UnicodeWidthStr::width(c.refs.as_str()))
        .max()
        .unwrap_or(0)
        .min(MAX_REFS_W);

    // list columns plus a right-hand preview pane
    let list_w = MARKER_W + HASH_W + DATE_W + max_author + max_refs + SCROLL_W + max_text;
    let width = ((list_w + GAP + MIN_PREVIEW_W + 2) as u16)
        .min(area.width.saturating_sub(4))
        .max(60);
    let height = commit_picker_height(commits.len(), area.height);
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
        return 0;
    }

    // split the inner area: commit list on the left, message preview on the right
    let inner_w = inner.width as usize;
    let preview_w = (inner_w / 3)
        .clamp(MIN_PREVIEW_W, 64)
        .min(inner_w.saturating_sub(50));
    let left_w = inner_w.saturating_sub(preview_w + GAP);
    let list_rect = Rect { x: inner.x, y: inner.y, width: left_w as u16, height: inner.height };
    render_commit_list(f, list_rect, commits, cursor);

    // vertical separator between the two panes
    let sep_x = inner.x + left_w as u16;
    for i in 0..inner.height {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled("│", Style::default().fg(styles::DIM)))),
            Rect { x: sep_x, y: inner.y + i, width: 1, height: 1 },
        );
    }

    let preview_rect = Rect {
        x: inner.x + left_w as u16 + GAP as u16,
        y: inner.y,
        width: preview_w as u16,
        height: inner.height,
    };
    let max_scroll = render_commit_preview(f, preview_rect, commits, cursor, preview, preview_scroll);

    let hint = "↑↓ 选择  PgUp/PgDn 预览  Enter 确认  Esc 关闭";
    let hint_w = UnicodeWidthStr::width(hint);
    let pad = inner.width.saturating_sub(hint_w as u16 + 1);
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!("{}{}", " ".repeat(pad as usize), hint),
            Style::default().fg(styles::DIM),
        ))),
        Rect { x: inner.x, y: inner.y + inner.height.saturating_sub(2), width: inner.width, height: 1 },
    );
    max_scroll
}

/// The commit list pane: marker, hash, title, refs, date and author columns.
/// The author gets its own column sized to the longest name so it is never
/// crammed into the date's width and truncated.
fn render_commit_list(f: &mut Frame, rect: Rect, commits: &[CommitEntry], cursor: usize) {
    const SCROLL_W: usize = 2;
    const MARKER_W: usize = 2;
    const HASH_W: usize = 9;
    const DATE_W: usize = 11;

    let max_author = commits
        .iter()
        .map(|c| UnicodeWidthStr::width(c.author.as_str()))
        .max()
        .unwrap_or(0)
        .min(MAX_AUTHOR_W);
    let max_refs = commits
        .iter()
        .map(|c| UnicodeWidthStr::width(c.refs.as_str()))
        .max()
        .unwrap_or(0)
        .min(MAX_REFS_W);

    let fixed = MARKER_W + HASH_W + DATE_W + max_author + max_refs + SCROLL_W;
    let title_w = (rect.width as usize).saturating_sub(fixed + 1);

    let visible = (rect.height.saturating_sub(3)) as usize;
    let max_start = commits.len().saturating_sub(visible);
    let start = cursor.saturating_sub(visible.saturating_sub(1)).min(max_start);

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
            Span::styled(pad_right(&truncate(&c.title, title_w), title_w), style),
            Span::styled(pad_right(&truncate(&c.refs, max_refs), max_refs), style.fg(Color::Magenta)),
            Span::styled(pad_right(&c.date, DATE_W), style.fg(styles::DIM)),
            Span::styled(pad_right(&truncate(&c.author, max_author), max_author), style.fg(styles::DIM)),
            Span::styled(" ".repeat(SCROLL_W), style),
        ]);
        f.render_widget(line, Rect { x: rect.x + 1, y: rect.y + 1 + i as u16, width: rect.width.saturating_sub(2), height: 1 });
    }

    if commits.len() > visible {
        let scroll_area = Rect {
            x: rect.x + rect.width.saturating_sub(SCROLL_W as u16),
            y: rect.y + 1,
            width: SCROLL_W as u16,
            height: rect.height.saturating_sub(3),
        };
        render_commit_scrollbar(f, scroll_area, commits.len(), visible, start);
    }
}

/// The preview pane: header (hash, date, author) plus the selected commit's
/// full message, wrapped to the pane width. The subject line is bold, the body
/// is dim; a scrollbar appears when the message is longer than the pane.
fn render_commit_preview(
    f: &mut Frame,
    rect: Rect,
    commits: &[CommitEntry],
    cursor: usize,
    preview: Option<&str>,
    preview_scroll: usize,
) -> usize {
    let c = &commits[cursor];
    let width = rect.width.saturating_sub(2) as usize;
    let header = Line::from(vec![
        Span::styled(format!(" {} ", c.short_hash), Style::default().fg(Color::Cyan)),
        Span::styled(format!(" {}  {}", c.date, c.author), Style::default().fg(styles::DIM)),
    ]);
    f.render_widget(header, Rect { x: rect.x + 1, y: rect.y, width: rect.width.saturating_sub(2), height: 1 });

    let Some(msg) = preview else {
        f.render_widget(
            Line::from(Span::styled(" 加载中…", Style::default().fg(styles::DIM))),
            Rect { x: rect.x + 1, y: rect.y + 1, width: width as u16, height: 1 },
        );
        return 0;
    };

    // wrap the full message; the subject (first line) stays bold
    let mut rows: Vec<(String, bool)> = Vec::new();
    for (i, line) in msg.lines().enumerate() {
        for w in wrap_line(line, width.saturating_sub(2)) {
            rows.push((w, i == 0));
        }
    }
    // rows below the header, above the shared hint line
    let avail = (rect.height.saturating_sub(3)) as usize;
    let max_scroll = rows.len().saturating_sub(avail);
    let start = preview_scroll.min(max_scroll);
    for (i, (text, is_subject)) in rows.iter().enumerate().skip(start).take(avail) {
        let style = if *is_subject {
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(styles::DIM)
        };
        f.render_widget(
            Line::from(Span::styled(text.as_str(), style)),
            Rect { x: rect.x + 1, y: rect.y + 1 + (i - start) as u16, width: width as u16, height: 1 },
        );
    }
    if rows.len() > avail {
        let sb = Rect {
            x: rect.x + rect.width.saturating_sub(1),
            y: rect.y + 1,
            width: 1,
            height: rect.height.saturating_sub(3),
        };
        render_commit_preview_scrollbar(f, sb, rows.len(), avail, start);
    }
    max_scroll
}

fn render_commit_preview_scrollbar(f: &mut Frame, area: Rect, total: usize, visible: usize, start: usize) {
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

/// Char-level wrap of a single line to a display width, keeping wide (CJK)
/// glyphs intact.
fn wrap_line(s: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![s.to_string()];
    }
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0usize;
    for c in s.chars() {
        let cw = UnicodeWidthStr::width(c.to_string().as_str());
        if cur_w > 0 && cur_w + cw > width {
            out.push(std::mem::take(&mut cur));
            cur_w = 0;
        }
        cur.push(c);
        cur_w += cw;
    }
    if cur_w > 0 {
        out.push(cur);
    }
    out
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
                ("鼠标拖动", "调整分隔线宽度（Shift+拖动选择文本）"),
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
                ("g", "输入行号跳转"),
                ("f", "在 diff 内搜索文本"),
                ("w", "忽略 / 恢复空白变化"),
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

/// Whether a string contains any double-width (CJK) character.
fn wide_str(s: &str) -> bool {
    use unicode_width::UnicodeWidthChar;
    s.chars().any(|ch| UnicodeWidthChar::width(ch).unwrap_or(0) > 1)
}

/// Number of decimal digits in `n` (for `0` returns 1).
fn digits(n: usize) -> usize {
    n.max(1).to_string().len()
}

/// Fingerprint of the current overlay state (which panel it covers).
fn overlay_key(c: &Controller<'_>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    match &c.overlay {
        Some(Overlay::Help) => 0u8.hash(&mut h),
        Some(Overlay::CommitPicker { commits, cursor, preview_scroll }) => {
            1u8.hash(&mut h);
            cursor.hash(&mut h);
            preview_scroll.hash(&mut h);
            for e in commits {
                e.short_hash.hash(&mut h);
                e.title.hash(&mut h);
            }
        }
        None => 2u8.hash(&mut h),
    }
    h.finish()
}

/// Whether a point falls inside a rect (inclusive of top-left, exclusive of
/// bottom-right, matching ratatui's Rect semantics).
fn rect_contains(rect: Rect, x: u16, y: u16) -> bool {
    x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height
}

/// Percentage of `area`'s width that lies left of screen column `x`, clamped
/// to `[min, max]`. Used to turn a divider drag into a new pane ratio.
fn pct_at(x: u16, area: Rect, min: u16, max: u16) -> u16 {
    if area.width == 0 {
        return max;
    }
    let rel = x.saturating_sub(area.x).min(area.width.saturating_sub(1));
    let pct = ((rel as u32 * 100) / area.width as u32) as u16;
    pct.clamp(min, max)
}

fn short_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn truncate(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
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

    let result = (|| {
        // Enable mouse reporting: clicks (`?1000h`), button-motion (`?1002h`,
        // delivers drag events for the pane dividers), any-motion (`?1003h`,
        // for hover highlight) and SGR coordinates (`?1006h`). Motion tracking
        // means a plain drag goes to the app instead of selecting text; most
        // terminals keep native selection available with Shift + drag.
        write!(std::io::stdout(), "\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1006h")?;
        std::io::stdout().flush()?;
        event_loop(&mut terminal, &mut app)
    })();

    // restore terminal to its original state: leave raw mode and the alternate
    // screen so the shell prompt renders normally after quitting
    let _ = write!(std::io::stdout(), "\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l");
    let _ = std::io::stdout().flush();
    disable_raw_mode()?;
    let _ = execute!(std::io::stdout(), LeaveAlternateScreen);
    result
}

fn event_loop(terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>, app: &mut App<'_>) -> Result<()> {
    // With any-motion mouse tracking enabled, pointer motion produces a
    // constant stream of events. Draw only when the last event actually
    // changed state, so idle motion costs nothing.
    let mut redraw = true;
    loop {
        if redraw {
            // advance the stat-column slide once per frame
            app.tick();
            let nav_before = app.nav_key();
            if app.take_full_redraw() {
                // Force a full terminal redraw: clear the screen, empty both ratatui
                // buffers, then draw. The next flush then writes every cell, so
                // cells that trail a double-width glyph get physically cleared by
                // the terminal instead of lingering with the previous file's content.
                terminal.clear()?;
                terminal.swap_buffers();
            }
            terminal.draw(|f| app.render(f))?;
            // Some renderers adjust navigation state (e.g. keeping the list cursor
            // visible), which means the frame we just drew was one scroll position
            // behind. Re-draw fully next frame so no wide glyph shifts are skipped.
            app.sync_full_redraw(nav_before);
            redraw = app.full_redraw_pending();
        }
        // shorter poll while the reveal animates; a slow poll otherwise so an
        // idle app does not wake up constantly
        let timeout = if app.is_animating() {
            Duration::from_millis(16)
        } else {
            Duration::from_millis(500)
        };
        if !crossterm::event::poll(timeout)? {
            redraw |= app.is_animating();
            continue;
        }
        let event = crossterm::event::read()?;
        match event {
            Event::Key(key) => {
                if key.code == KeyCode::Char('q') && key.modifiers == KeyModifiers::NONE && app.ctrl.overlay.is_none() && app.ctrl.filter.is_none() {
                    break;
                }
                if key.code == KeyCode::Char('c') && key.modifiers == KeyModifiers::CONTROL {
                    break;
                }
                app.handle_key(key);
                redraw = true;
            }
            Event::Mouse(mouse) => redraw |= app.handle_mouse(mouse),
            Event::Resize(_, _) => redraw = true,
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod width_check {
    use super::{ListLayout, pct_at, rect_contains};
    use ratatui::layout::Rect;
    use unicode_width::UnicodeWidthStr;

    fn used_width(cols: ListLayout) -> usize {
        ListLayout::MARKER_W + cols.name + cols.reserved
    }

    #[test]
    fn list_layout_reveals_stats_progressively() {
        for w in [2usize, 3, 12, 20, 30, 31, 60] {
            for reveal in [0.0f32, 0.2, 0.5, 0.8, 1.0] {
                let cols = ListLayout::for_width(w, reveal);
                // every frame must account for exactly the available width
                assert_eq!(used_width(cols), w, "width {w} reveal {reveal} -> {cols:?}");
                assert!(cols.stats_width() <= cols.reserved);
            }
        }
        // hidden: no columns at all, name takes everything but the marker
        let hidden = ListLayout::for_width(31, 0.0);
        assert_eq!(hidden.reserved, 0);
        assert_eq!(hidden.name, 29);
        // full: all three segments, leaving the minimum filename width
        let full = ListLayout::for_width(31, 1.0);
        assert_eq!(full.reserved, ListLayout::RESERVED_MAX);
        assert_eq!((full.plus_cols(), full.minus_cols(), full.status_cols()), (5, 5, 7));
        assert_eq!(full.name, 12);
        // mid-slide: only the segments that fully fit are drawn
        let mid = ListLayout::for_width(60, 0.5);
        assert_eq!(mid.reserved, 9); // 17 * 0.5 rounds to 9
        assert_eq!((mid.plus_cols(), mid.minus_cols(), mid.status_cols()), (0, 0, 7));
        // on a narrow pane the reserve is capped so the name stays readable
        let capped = ListLayout::for_width(20, 1.0);
        assert_eq!(capped.name, ListLayout::MIN_NAME_W);
        assert_eq!(capped.reserved, 6);
    }

    #[test]
    fn truncate_handles_zero_width() {
        assert_eq!(super::truncate("hello.rs", 0), "");
        assert_eq!(super::truncate("hello.rs", 3), "he…");
    }

    #[test]
    fn pct_at_maps_columns_and_clamps() {
        let area = Rect { x: 10, y: 0, width: 100, height: 10 };
        assert_eq!(pct_at(10, area, 0, 100), 0);
        assert_eq!(pct_at(40, area, 0, 100), 30);
        assert_eq!(pct_at(109, area, 0, 100), 99);
        // clamping keeps both panes visible
        assert_eq!(pct_at(10, area, 15, 60), 15);
        assert_eq!(pct_at(109, area, 15, 60), 60);
        assert_eq!(pct_at(200, area, 15, 60), 60);
    }

    #[test]
    fn pct_at_handles_zero_width() {
        let area = Rect { x: 5, y: 0, width: 0, height: 0 };
        assert_eq!(pct_at(5, area, 20, 80), 80);
    }

    #[test]
    fn rect_contains_bounds() {
        let r = Rect { x: 10, y: 5, width: 20, height: 10 };
        assert!(rect_contains(r, 10, 5));
        assert!(rect_contains(r, 29, 14));
        assert!(!rect_contains(r, 30, 14));
        assert!(!rect_contains(r, 10, 15));
        assert!(!rect_contains(r, 9, 5));
    }

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
                    ("鼠标", "点击面板直接获得焦点"),
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
                    ("g", "输入行号跳转"),
                    ("f", "在 diff 内搜索文本"),
                    ("w", "忽略 / 恢复空白变化"),
                ],
            },
        ];
        let (row_w, _width, inner_avail) = panel_width(&groups);
        assert!(row_w <= inner_avail, "rows {} exceed inner {} ", row_w, inner_avail);
    }
}