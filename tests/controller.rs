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

#[test]
fn loads_mode_a_files_and_selects_first() {
    let ctrl = setup();
    assert_eq!(ctrl.mode, ComparisonMode::WorkingVsHead);
    assert_eq!(ctrl.files.len(), 2);
    assert_eq!(ctrl.files[0].path, "main.rs");
    assert_eq!(ctrl.cursor, 0);
    assert!(ctrl.diff_file.is_some());
}

#[test]
fn down_moves_cursor_and_loads_diff() {
    let mut ctrl = setup();
    // tree sorts by path: row0 = lib.rs, row1 = main.rs
    ctrl.handle_key(key(KeyCode::Down));
    assert_eq!(ctrl.cursor, 1);
    assert_eq!(ctrl.diff_file.as_ref().unwrap().path, "main.rs");
    // clamps at bottom
    ctrl.handle_key(key(KeyCode::Down));
    assert_eq!(ctrl.cursor, 1);
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
    assert_eq!(rows.len(), 1, "filter should keep only main.rs");
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