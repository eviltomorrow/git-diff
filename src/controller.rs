use std::collections::HashSet;
use std::path::PathBuf;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::align::{align_rows, hunk_index, hunk_starts, plain_rows, AlignedRow, LineKind};
use crate::git::GitFacade;
use crate::model::{ChangedFile, CommitEntry, ComparisonMode};
use crate::tree::{self, SortMode, TreeNode, VisibleRow};

pub const LINE_LIMIT: usize = 50_000;
pub const FOLD_MIN: usize = 10;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Focus {
    FileList,
    Diff,
}

pub enum Overlay {
    CommitPicker { commits: Vec<CommitEntry>, cursor: usize },
    Help,
}

/// Per-mode navigation snapshot. Owns the field list and the capture/apply
/// behaviour, so adding a navigation field means one edit here plus its
/// capture/apply arms — the mapping never lives in two places.
#[derive(Clone, Default)]
pub struct ModeState {
    /// Path of the selected visible row (file or directory). Indexes are
    /// meaningless across modes since the file sets differ.
    selected_path: Option<String>,
    list_scroll: usize,
    collapsed: HashSet<String>,
    selected_commit: Option<String>,
    diff_vscroll: usize,
    diff_hscroll: usize,
    diff_cursor: usize,
}

impl ModeState {
    fn capture(&self, ctrl: &Controller<'_>) -> ModeState {
        let selected_path = ctrl.visible_rows().get(ctrl.cursor).map(|r| match r {
            VisibleRow::File { file, .. } => file.path.clone(),
            VisibleRow::Dir { path, .. } => path.clone(),
        });
        ModeState {
            selected_path,
            list_scroll: ctrl.list_scroll,
            collapsed: ctrl.collapsed.clone(),
            selected_commit: ctrl.selected_commit.clone(),
            diff_vscroll: ctrl.diff_vscroll,
            diff_hscroll: ctrl.diff_hscroll,
            diff_cursor: ctrl.diff_cursor,
        }
    }

    fn apply(&self, ctrl: &mut Controller<'_>) {
        ctrl.collapsed = self.collapsed.clone();
        let saved_path = self.selected_path.clone();
        if let Some(path) = saved_path {
            let rows = ctrl.visible_rows();
            if let Some(ri) = rows.iter().position(|r| match r {
                VisibleRow::File { file, .. } => file.path == path,
                VisibleRow::Dir { path: p, .. } => *p == path,
            }) {
                ctrl.cursor = ri;
                ctrl.list_scroll = self.list_scroll;
            }
        }
        ctrl.diff_vscroll = self.diff_vscroll;
        ctrl.diff_hscroll = self.diff_hscroll;
        ctrl.diff_cursor = self.diff_cursor;
    }
}

pub struct Controller<'a> {
    pub facade: GitFacade<'a>,
    pub repo_path: PathBuf,
    pub has_commits: bool,
    pub mode: ComparisonMode,
    pub selected_commit: Option<String>,
    mode_state: [ModeState; 4],
    pub files: Vec<ChangedFile>,
    /// Cached file tree, rebuilt only when `files` changes (on reload).
    tree_cache: Vec<TreeNode>,
    pub collapsed: HashSet<String>,
    pub cursor: usize,
    pub list_scroll: usize,
    pub diff_rows: Vec<AlignedRow>,
    pub diff_file: Option<ChangedFile>,
    pub diff_vscroll: usize,
    pub diff_hscroll: usize,
    pub diff_cursor: usize,
    pub fold_unchanged: bool,
    pub hunk_idx: usize,
    pub hunk_count: usize,
    pub filter: Option<String>,
    /// Sibling ordering for the file list.
    pub sort: SortMode,
    pub overlay: Option<Overlay>,
    pub focus: Focus,
    pub diff_viewport: usize,
    pub diff_hviewport: usize,
    pub diff_max_line_w: usize,
    pub list_viewport: usize,
    pub loading: bool,
    pub status: String,
}

impl<'a> Controller<'a> {
    pub fn new(
        facade: GitFacade<'a>,
        repo_path: PathBuf,
        has_commits: bool,
        initial_commit: Option<String>,
    ) -> Result<Self> {
        let mut ctrl = Self {
            facade,
            repo_path,
            has_commits,
            mode: if initial_commit.is_some() {
                ComparisonMode::CommitVsHead
            } else {
                ComparisonMode::WorkingVsHead
            },
            selected_commit: initial_commit.clone(),
            mode_state: std::array::from_fn(|_| ModeState::default()),
            files: Vec::new(),
            tree_cache: Vec::new(),
            collapsed: HashSet::new(),
            cursor: 0,
            list_scroll: 0,
            diff_rows: Vec::new(),
            diff_file: None,
            diff_vscroll: 0,
            diff_hscroll: 0,
            diff_cursor: 0,
            fold_unchanged: false,
            hunk_idx: 0,
            hunk_count: 0,
            filter: None,
            sort: SortMode::Path,
            overlay: None,
            focus: Focus::FileList,
            diff_viewport: 0,
            diff_hviewport: 0,
            diff_max_line_w: 0,
            list_viewport: 0,
            loading: false,
            status: String::new(),
        };
        if let Some(c) = &ctrl.selected_commit {
            ctrl.mode_state[ctrl.mode_index(ComparisonMode::CommitVsHead)].selected_commit = Some(c.clone());
        }
        ctrl.reload(false)?;
        Ok(ctrl)
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
                self.rebuild_tree();
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

    pub fn visible_rows(&self) -> Vec<VisibleRow> {
        let filter = self.filter.as_deref();
        let collapsed = if filter.is_some() {
            &HashSet::new()
        } else {
            &self.collapsed
        };
        match filter.filter(|f| !f.trim().is_empty()) {
            None => tree::visible_rows(&self.tree_cache, collapsed),
            Some(f) => {
                // optional leading `!` inverts the match; otherwise a
                // case-insensitive substring match on the file path. Directories
                // are kept only when a descendant matches (empty dirs hide).
                let (negate, pat) = match f.strip_prefix('!') {
                    Some(rest) => (true, rest),
                    None => (false, f),
                };
                let pat = pat.to_lowercase();
                tree::filtered_rows(&self.tree_cache, &|file: &ChangedFile| {
                    let m = file.path.to_lowercase().contains(&pat);
                    if negate { !m } else { m }
                })
            }
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
        // while filtering, printable characters feed the filter instead of
        // triggering global shortcuts (l/n/z/r/1/2/3...)
        if self.filter.is_some() {
            match (key.code, key.modifiers) {
                (KeyCode::Char(c), KeyModifiers::NONE) => {
                    self.filter.as_mut().unwrap().push(c);
                    self.cursor = 0;
                    return;
                }
                (KeyCode::Backspace, _) => {
                    self.filter.as_mut().unwrap().pop();
                    self.cursor = 0;
                    return;
                }
                (KeyCode::Esc, _) => {
                    self.filter = None;
                    self.cursor = 0;
                    return;
                }
                (KeyCode::Enter, _) => {
                    // confirm the filter result: exit filter mode and focus the
                    // diff panel to view the matched file
                    let filtered = self.visible_rows();
                    let target = filtered
                        .iter()
                        .find_map(|r| match r {
                            VisibleRow::File { file, .. } => Some(file.path.clone()),
                            _ => None,
                        });
                    self.filter = None;
                    if let Some(path) = target {
                        let rows = self.visible_rows();
                        if let Some(fi) = rows.iter().position(|r| match r {
                            VisibleRow::File { file, .. } => file.path == path,
                            _ => false,
                        }) {
                            self.cursor = fi;
                        }
                    }
                    self.load_diff();
                    if self.diff_file.is_some() {
                        self.focus = Focus::Diff;
                    }
                    return;
                }
                _ => {}
            }
        }
        match (key.code, key.modifiers) {
            (KeyCode::Char('q'), KeyModifiers::NONE) => {
                // handled by caller to exit
            }
            (KeyCode::Char('l'), KeyModifiers::NONE) => self.enter_mode_d(),
            (KeyCode::Char('1'), KeyModifiers::NONE) => self.set_mode(ComparisonMode::WorkingVsHead),
            (KeyCode::Char('2'), KeyModifiers::NONE) => self.set_mode(ComparisonMode::StagedVsHead),
            (KeyCode::Char('3'), KeyModifiers::NONE) => self.set_mode(ComparisonMode::StagedVsWorking),
            (KeyCode::Char('4'), KeyModifiers::NONE) => self.enter_mode_d(),
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
            (KeyCode::Enter, _) => {
                // in the file list, Enter switches to the diff panel to view the
                // currently selected file's comparison
                if self.focus == Focus::FileList && self.diff_file.is_some() {
                    self.focus = Focus::Diff;
                }
            }
            (KeyCode::Tab, _) => self.toggle_focus(),
            (KeyCode::Char('n'), KeyModifiers::NONE) => self.jump_hunk(1),
            (KeyCode::Char('m'), KeyModifiers::NONE) => self.jump_hunk(-1),
            (KeyCode::Char('z'), KeyModifiers::NONE) => {
                self.fold_unchanged = !self.fold_unchanged;
            }
            (KeyCode::Char('s'), KeyModifiers::NONE) => self.cycle_sort(),
            (KeyCode::Char('['), KeyModifiers::NONE) => self.collapse_all_dirs(),
            (KeyCode::Char(']'), KeyModifiers::NONE) => self.expand_all_dirs(),
            (KeyCode::Right, KeyModifiers::CONTROL) => self.scroll_horizontal(1),
            (KeyCode::Left, KeyModifiers::CONTROL) => self.scroll_horizontal(-1),
            (KeyCode::Char('k'), KeyModifiers::CONTROL) => self.move_diff_cursor(-1),
            (KeyCode::Char('j'), KeyModifiers::CONTROL) => self.move_diff_cursor(1),
            (KeyCode::Up, _) => match self.focus {
                Focus::FileList => self.move_cursor(-1),
                Focus::Diff => self.move_diff_cursor(-1),
            },
            (KeyCode::Down, _) => match self.focus {
                Focus::FileList => self.move_cursor(1),
                Focus::Diff => self.move_diff_cursor(1),
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
                Focus::Diff => self.move_diff_cursor(-(self.diff_page() as isize)),
            },
            (KeyCode::PageDown, _) => match self.focus {
                Focus::FileList => self.move_cursor(self.list_page() as isize),
                Focus::Diff => self.move_diff_cursor(self.diff_page() as isize),
            },
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
            // save the mode we're leaving so switching back restores it
            self.save_mode_state();
            self.selected_commit = Some(selected.short_hash.clone());
            self.mode = ComparisonMode::CommitVsHead;
            self.overlay = None;
            self.filter = None;
            // remember the chosen commit so `l` can re-enter this comparison
            let idx = self.mode_index(ComparisonMode::CommitVsHead);
            self.mode_state[idx].selected_commit = self.selected_commit.clone();
            let _ = self.reload(false);
        } else if matches!(key.code, KeyCode::Esc) || key.code == KeyCode::Char('?') {
            self.overlay = None;
        }
    }

    /// Enter mode D (HEAD ↔ selected commit). Re-uses the last selected commit
    /// if one exists; otherwise opens the commit picker.
    fn enter_mode_d(&mut self) {
        let idx = self.mode_index(ComparisonMode::CommitVsHead);
        let has_saved = self.mode_state[idx].selected_commit.is_some();
        if self.mode != ComparisonMode::CommitVsHead && has_saved {
            self.save_mode_state();
            self.mode = ComparisonMode::CommitVsHead;
            self.selected_commit = self.mode_state[idx].selected_commit.clone();
            self.filter = None;
            let _ = self.reload(false);
            self.restore_mode_nav(idx);
        } else if self.has_commits {
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

    fn set_mode(&mut self, mode: ComparisonMode) {
        if self.mode == mode {
            return;
        }
        // save the current mode's navigation state
        self.save_mode_state();
        self.mode = mode;
        self.selected_commit = self.mode_state[self.mode_index(mode)].selected_commit.clone();
        self.filter = None;
        let idx = self.mode_index(mode);
        // reload to load the target mode's file set, then restore its state
        let _ = self.reload(false);
        self.restore_mode_nav(idx);
    }

    fn restore_mode_nav(&mut self, idx: usize) {
        self.mode_state[idx].clone().apply(self);
        self.load_diff();
        self.keep_cursor_visible();
        self.sync_hunk_idx();
    }

    fn mode_index(&self, mode: ComparisonMode) -> usize {
        match mode {
            ComparisonMode::WorkingVsHead => 0,
            ComparisonMode::StagedVsHead => 1,
            ComparisonMode::StagedVsWorking => 2,
            ComparisonMode::CommitVsHead => 3,
        }
    }

    fn save_mode_state(&mut self) {
        let idx = self.mode_index(self.mode);
        let snap = ModeState::default();
        self.mode_state[idx] = snap.capture(self);
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

    fn cycle_sort(&mut self) {
        self.sort = match self.sort {
            SortMode::Path => SortMode::Status,
            SortMode::Status => SortMode::Added,
            SortMode::Added => SortMode::Path,
        };
        self.rebuild_tree();
        self.status = match self.sort {
            SortMode::Path => "排序: 路径".into(),
            SortMode::Status => "排序: 状态".into(),
            SortMode::Added => "排序: 增行数".into(),
        };
    }

    /// Rebuild the cached tree from the current files, wrapped under a root
    /// directory named after the repo's own directory.
    fn rebuild_tree(&mut self) {
        let children = tree::build_tree(&self.files, self.sort);
        self.tree_cache = vec![tree::wrap_root(children, self.repo_root_name())];
    }

    fn repo_root_name(&self) -> String {
        self.repo_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.repo_path.display().to_string())
    }

    fn collapse_all_dirs(&mut self) {
        for path in tree::all_dir_paths(&self.tree_cache) {
            self.collapsed.insert(path);
        }
        self.load_diff();
    }

    fn expand_all_dirs(&mut self) {
        self.collapsed.clear();
        self.load_diff();
    }

    fn list_page(&self) -> usize {
        self.list_viewport.max(1)
    }

    fn diff_page(&self) -> usize {
        self.diff_viewport.max(1)
    }

    fn move_diff_cursor(&mut self, delta: isize) {
        if self.diff_rows.is_empty() {
            return;
        }
        let len = self.diff_rows.len() as isize;
        let new = (self.diff_cursor as isize + delta).clamp(0, len - 1) as usize;
        if new != self.diff_cursor {
            self.diff_cursor = new;
        }
        self.keep_cursor_visible();
        self.sync_hunk_idx();
    }

    pub fn is_unchanged_row(&self, idx: usize) -> bool {
        self.diff_rows
            .get(idx)
            .map(|r| {
                r.original.as_ref().map(|c| c.kind == LineKind::Equal).unwrap_or(false)
                    && r.changed.as_ref().map(|c| c.kind == LineKind::Equal).unwrap_or(false)
            })
            .unwrap_or(false)
    }

    fn keep_cursor_visible(&mut self) {
        let viewport = self.diff_viewport.max(1);
        if self.diff_cursor < self.diff_vscroll {
            self.diff_vscroll = self.diff_cursor;
        } else if self.diff_cursor >= self.diff_vscroll + viewport {
            self.diff_vscroll = self.diff_cursor + 1 - viewport;
        }
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
        let cur = self.diff_cursor;
        let target = if dir > 0 {
            starts.iter().find(|&&s| s > cur).copied().unwrap_or(cur)
        } else {
            starts.iter().rev().find(|&&s| s < cur).copied().unwrap_or(cur)
        };
        // hunk navigation targets the diff panel: bring focus there so the
        // cursor highlight is visible even when navigating from the file list
        self.focus = Focus::Diff;
        if target != cur {
            self.diff_cursor = target;
            self.keep_cursor_visible();
            self.sync_hunk_idx();
        }
    }

    fn sync_hunk_idx(&mut self) {
        let starts = hunk_starts(&self.diff_rows);
        self.hunk_count = starts.len();
        self.hunk_idx = hunk_index(&starts, self.diff_cursor);
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
                        self.diff_cursor = 0;
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

    /// Sets the viewport sizes computed from the rendered areas. Navigation
    /// clamps read these; render calls this once per frame so the controller
    /// stays pure (no render side-effects).
    pub fn update_viewports(&mut self, list_height: usize, diff_height: usize, diff_width: usize, max_line_w: usize) {
        self.list_viewport = list_height;
        self.diff_viewport = diff_height;
        self.diff_hviewport = diff_width;
        self.diff_max_line_w = max_line_w;
    }

    pub fn side_labels(&self) -> (String, String) {
        let pair = crate::model::SidePair::for_mode(self.mode, self.selected_commit.as_deref());
        (pair.original.label(), pair.changed.label())
    }
}

pub fn line_count(content: &[u8]) -> usize {
    content.iter().filter(|&&b| b == b'\n').count() + 1
}