use std::path::PathBuf;

use crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use git_diff::git::{GitFacade, GitRunner};
use git_diff::tui::App;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

/// Empty repo: the panes still render (placeholders), which is all the
/// divider geometry needs.
struct EmptyRunner;

impl GitRunner for EmptyRunner {
    fn run(&self, _args: &[&str]) -> anyhow::Result<String> {
        Ok(String::new())
    }
}

fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

fn setup() -> App<'static> {
    let facade = GitFacade::new(&EmptyRunner, &PathBuf::from("/tmp/opencode/does-not-exist"));
    App::new(
        facade,
        PathBuf::from("/tmp/opencode/does-not-exist"),
        true,
        None,
    )
    .unwrap()
}

fn draw<'a>(app: &mut App<'a>, terminal: &mut Terminal<TestBackend>) {
    terminal.draw(|f| app.render(f)).unwrap();
}

/// Columns where a vertical border (`│`) is drawn on the given row.
fn border_columns(terminal: &Terminal<TestBackend>, row: u16) -> Vec<u16> {
    let buf = terminal.backend().buffer();
    (0..buf.area.width)
        .filter(|&x| buf.cell((x, row)).is_some_and(|c| c.symbol() == "│"))
        .collect()
}

#[test]
fn file_list_keeps_a_fixed_width_when_dragged() {
    let mut app = setup();
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    draw(&mut app, &mut terminal);

    // fixed 44-column list: its right border is at x=43, the diff starts at 44
    let before = border_columns(&terminal, 10);
    assert!(
        before.contains(&43) && before.contains(&44),
        "initial seams: {before:?}"
    );

    // pressing the list's right edge must focus the list, not start a resize,
    // and dragging right must leave the pane width untouched
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 43, 10));
    app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 60, 10));
    app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 60, 10));
    draw(&mut app, &mut terminal);

    let after = border_columns(&terminal, 10);
    assert!(
        after.contains(&43) && after.contains(&44),
        "list width must stay fixed: {after:?}"
    );
    assert!(
        !after.contains(&59) && !after.contains(&60),
        "no seam at the drag target: {after:?}"
    );
    assert_eq!(app.ctrl().focus, git_diff::controller::Focus::FileList);
}

#[test]
fn dragging_diff_divider_moves_the_inner_seam() {
    let mut app = setup();
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    draw(&mut app, &mut terminal);

    // fixed 44-wide list, diff area 56 wide split 50/50 -> inner seam at 71/72
    let before = border_columns(&terminal, 10);
    assert!(
        before.contains(&71) && before.contains(&72),
        "initial seams: {before:?}"
    );

    assert!(app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 72, 10)));
    assert!(app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 90, 10)));
    assert!(app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 90, 10)));
    draw(&mut app, &mut terminal);

    let after = border_columns(&terminal, 10);
    // clamped at 80% of the 56-column diff area -> inner seam around x=88/89
    assert!(
        after.contains(&88) || after.contains(&89),
        "inner seam should move right: {after:?}"
    );
    assert!(
        !after.contains(&71),
        "old seam column must be inside the pane now: {after:?}"
    );
}

#[test]
fn diff_divider_drag_clamps_to_keep_both_panes_usable() {
    let mut app = setup();
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    draw(&mut app, &mut terminal);

    // try to collapse the original side by dragging the seam far left
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 72, 10));
    app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 0, 10));
    app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 0, 10));
    draw(&mut app, &mut terminal);

    // clamped to DIFF_MIN_PCT = 20 -> both panes stay usable, list untouched
    let bars = border_columns(&terminal, 10);
    assert!(
        bars.contains(&43) && bars.contains(&44),
        "list stays fixed: {bars:?}"
    );
    assert!(
        bars.contains(&54) || bars.contains(&55),
        "diff divider should clamp at 20%: {bars:?}"
    );
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

#[test]
fn b_toggles_the_file_list_sidebar() {
    let mut app = setup();
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    draw(&mut app, &mut terminal);

    // shown: fixed 44-column list ends at x=43
    assert!(border_columns(&terminal, 10).contains(&43));

    app.handle_key(key(KeyCode::Char('b')));
    assert!(!app.ctrl().sidebar_visible);
    assert_eq!(app.ctrl().focus, git_diff::controller::Focus::Diff);
    draw(&mut app, &mut terminal);
    let hidden = border_columns(&terminal, 10);
    assert!(!hidden.contains(&43), "list must be gone: {hidden:?}");
    // full-width diff split 50/50 -> seams at 0/49 and 50/99
    assert!(
        hidden.contains(&49) && hidden.contains(&50),
        "diff should span the whole width: {hidden:?}"
    );

    app.handle_key(key(KeyCode::Char('b')));
    assert!(app.ctrl().sidebar_visible);
    draw(&mut app, &mut terminal);
    let shown = border_columns(&terminal, 10);
    assert!(
        shown.contains(&43) && shown.contains(&44),
        "list should come back: {shown:?}"
    );
}
