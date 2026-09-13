use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use git_diff::controller::{Controller, Focus, Overlay};
use git_diff::git::{GitFacade, GitRunner};
use git_diff::model::ComparisonMode;

/// Fake git that returns a canned file list and blank content for any ref.
struct FakeRunner;

impl GitRunner for FakeRunner {
    fn run(&self, args: &[&str]) -> anyhow::Result<String> {
        match args {
            ["diff", "--numstat", "-M", "HEAD"] => Ok("2\t1\tmain.rs\n1\t0\tlib.rs\n".into()),
            ["diff", "--name-status", "-M", "HEAD"] => Ok("M\tmain.rs\nM\tlib.rs\n".into()),
            ["diff", "--cached", "--numstat", "-M", "HEAD"] => Ok(String::new()),
            ["diff", "--cached", "--name-status", "-M", "HEAD"] => Ok(String::new()),
            ["diff", "--numstat", "-M"] => Ok(String::new()),
            ["diff", "--name-status", "-M"] => Ok(String::new()),
            ["ls-files", "--others", "--exclude-standard"] => Ok(String::new()),
            ["log", "-n", "200", "--pretty=format:%h|%s|%ad|%an", "--date=short"] => Ok("abc1234|Add feature|2026-01-01|Alice\n".into()),
            ["show", "HEAD:main.rs"] => Ok("line1\nline2\nline3\n".into()),
            ["show", "HEAD:lib.rs"] => Ok("x\ny\n".into()),
            _ => Ok(String::new()),
        }
    }
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn setup() -> Controller<'static> {
    let facade = GitFacade::new(&FakeRunner, &PathBuf::from("/tmp"));
    Controller::new(facade, PathBuf::from("/tmp"), true, None).unwrap()
}

/// Fake git returning nested paths so the tree has dirs.
struct NestedRunner;

impl GitRunner for NestedRunner {
    fn run(&self, args: &[&str]) -> anyhow::Result<String> {
        match args {
            ["diff", "--numstat", "-M", "HEAD"] => Ok(
                "1\t0\tsrc/main.rs\n1\t0\tsrc/deep/lib.rs\n1\t0\ttop.rs\n".into(),
            ),
            ["diff", "--name-status", "-M", "HEAD"] => Ok(
                "M\tsrc/main.rs\nM\tsrc/deep/lib.rs\nM\ttop.rs\n".into(),
            ),
            ["ls-files", "--others", "--exclude-standard"] => Ok(String::new()),
            _ => Ok(String::new()),
        }
    }
}

fn setup_nested() -> Controller<'static> {
    let facade = GitFacade::new(&NestedRunner, &PathBuf::from("/tmp"));
    Controller::new(facade, PathBuf::from("/tmp"), true, None).unwrap()
}

#[test]
fn loads_mode_a_files_and_selects_first() {
    let ctrl = setup();
    assert_eq!(ctrl.mode, ComparisonMode::WorkingVsHead);
    assert_eq!(ctrl.files.len(), 2);
    assert_eq!(ctrl.files[0].path, "main.rs");
    // cursor 0 is the wrapped repo-root dir, not a file
    assert_eq!(ctrl.cursor, 0);
    match &ctrl.visible_rows()[0] {
        git_diff::tree::VisibleRow::Dir { path, depth, .. } => {
            assert_eq!(depth, &0);
            assert!(!path.is_empty());
        }
        _ => panic!("expected root dir row"),
    }
}

#[test]
fn down_moves_cursor_and_loads_diff() {
    let mut ctrl = setup();
    // rows: root dir(0), lib.rs(1), main.rs(2)
    ctrl.handle_key(key(KeyCode::Down));
    assert_eq!(ctrl.cursor, 1);
    assert_eq!(ctrl.diff_file.as_ref().unwrap().path, "lib.rs");
    ctrl.handle_key(key(KeyCode::Down));
    assert_eq!(ctrl.cursor, 2);
    assert_eq!(ctrl.diff_file.as_ref().unwrap().path, "main.rs");
    // clamps at bottom
    ctrl.handle_key(key(KeyCode::Down));
    assert_eq!(ctrl.cursor, 2);
}

#[test]
fn mode_switch_preserves_navigation_per_mode() {
    let mut ctrl = setup();
    // move to the second file in mode A
    ctrl.handle_key(key(KeyCode::Down));
    assert_eq!(ctrl.cursor, 1);
    // switch to mode B (empty file set)
    ctrl.handle_key(key(KeyCode::Char('2')));
    assert_eq!(ctrl.mode, ComparisonMode::StagedVsHead);
    assert!(ctrl.files.is_empty());
    // switch back to mode A: cursor should be restored to the second file
    ctrl.handle_key(key(KeyCode::Char('1')));
    assert_eq!(ctrl.mode, ComparisonMode::WorkingVsHead);
    assert_eq!(ctrl.cursor, 1, "mode A nav should be restored");
}

#[test]
fn filtering_limits_rows_and_enter_focuses_diff() {
    let mut ctrl = setup();
    ctrl.handle_key(key(KeyCode::Char('/')));
    for ch in ['m', 'a', 'i', 'n'] {
        ctrl.handle_key(key(KeyCode::Char(ch)));
    }
    let rows = ctrl.visible_rows();
    // root dir row + the matched file
    assert_eq!(rows.len(), 2, "filter should keep root dir + main.rs");
    match &rows[1] {
        git_diff::tree::VisibleRow::File { file, .. } => assert_eq!(file.path, "main.rs"),
        _ => panic!("expected file row"),
    }
    assert_eq!(ctrl.focus, Focus::FileList);
    ctrl.handle_key(key(KeyCode::Enter));
    assert_eq!(ctrl.filter, None);
    assert_eq!(ctrl.focus, Focus::Diff);
}

#[test]
fn overlay_help_ignores_navigation() {
    let mut ctrl = setup();
    ctrl.handle_key(key(KeyCode::Char('?')));
    assert!(matches!(ctrl.overlay, Some(Overlay::Help)));
    // navigation keys must not fire while an overlay is open
    ctrl.handle_key(key(KeyCode::Down));
    assert_eq!(ctrl.cursor, 0);
    ctrl.handle_key(key(KeyCode::Esc));
    assert!(ctrl.overlay.is_none());
}

#[test]
fn mode_d_opens_picker_then_loads_commit() {
    let mut ctrl = setup();
    ctrl.handle_key(key(KeyCode::Char('4')));
    assert!(matches!(ctrl.overlay, Some(Overlay::CommitPicker { .. })));
    ctrl.handle_key(key(KeyCode::Enter));
    assert_eq!(ctrl.mode, ComparisonMode::CommitVsHead);
    assert_eq!(ctrl.selected_commit.as_deref(), Some("abc1234"));
    assert!(ctrl.overlay.is_none());
}

#[test]
fn filter_negation_excludes_matches() {
    let mut ctrl = setup();
    ctrl.handle_key(key(KeyCode::Char('/')));
    for ch in ['!', 'm', 'a', 'i', 'n'] {
        ctrl.handle_key(key(KeyCode::Char(ch)));
    }
    let rows = ctrl.visible_rows();
    // root dir row + the surviving file
    assert_eq!(rows.len(), 2, "!main should keep root dir + lib.rs");
    match &rows[1] {
        git_diff::tree::VisibleRow::File { file, .. } => assert_eq!(file.path, "lib.rs"),
        _ => panic!("expected file row"),
    }
}

#[test]
fn sort_cycles_through_modes() {
    use git_diff::tree::SortMode;
    let mut ctrl = setup();
    assert_eq!(ctrl.sort, SortMode::Path);
    ctrl.handle_key(key(KeyCode::Char('s')));
    assert_eq!(ctrl.sort, SortMode::Status);
    ctrl.handle_key(key(KeyCode::Char('s')));
    assert_eq!(ctrl.sort, SortMode::Added);
    ctrl.handle_key(key(KeyCode::Char('s')));
    assert_eq!(ctrl.sort, SortMode::Path);
}

#[test]
fn collapse_all_then_expand_all() {
    let mut ctrl = setup_nested();
    // dirs visible initially: src, src/deep, top.rs
    let before = ctrl.visible_rows();
    assert!(before.iter().any(|r| matches!(r, git_diff::tree::VisibleRow::Dir { .. })));
    ctrl.handle_key(key(KeyCode::Char('[')));
    let collapsed = ctrl.visible_rows();
    // only root dirs remain; nested files hidden
    assert!(collapsed.iter().any(|r| matches!(r, git_diff::tree::VisibleRow::Dir { .. })));
    assert!(!collapsed
        .iter()
        .any(|r| matches!(r, git_diff::tree::VisibleRow::File { file, .. } if file.path.contains('/'))));
    ctrl.handle_key(key(KeyCode::Char(']')));
    let expanded = ctrl.visible_rows();
    assert!(expanded.len() >= before.len());
}