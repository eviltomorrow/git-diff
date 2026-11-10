use std::path::PathBuf;

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
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
    MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE }
}

fn setup() -> App<'static> {
    let facade = GitFacade::new(&EmptyRunner, &PathBuf::from("/tmp/opencode/does-not-exist"));
    App::new(facade, PathBuf::from("/tmp/opencode/does-not-exist"), true, None).unwrap()
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
fn dragging_list_divider_moves_the_seam() {
    let mut app = setup();
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    draw(&mut app, &mut terminal);

    let before = border_columns(&terminal, 10);
    assert!(before.contains(&25) && before.contains(&26), "initial seams: {before:?}");
    assert!(!before.contains(&40), "x=40 starts outside the list pane");

    // press on the seam at x=26 and drag it to x=40
    assert!(app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 26, 10)));
    assert!(app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 40, 10)));
    assert!(app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 40, 10)));
    draw(&mut app, &mut terminal);

    let after = border_columns(&terminal, 10);
    assert!(after.contains(&39) && after.contains(&40), "seams after drag: {after:?}");

    // a resize must not steal focus from the file list
    assert_eq!(app.ctrl().focus, git_diff::controller::Focus::FileList);
}

#[test]
fn dragging_diff_divider_moves_the_inner_seam() {
    let mut app = setup();
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    draw(&mut app, &mut terminal);

    // default: list 26 wide, diff area 74 wide split 50/50 -> inner seam at 63
    let before = border_columns(&terminal, 10);
    assert!(before.contains(&62) && before.contains(&63), "initial seams: {before:?}");

    assert!(app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 63, 10)));
    assert!(app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 80, 10)));
    assert!(app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 80, 10)));
    draw(&mut app, &mut terminal);

    let after = border_columns(&terminal, 10);
    // 72% of the 74-column diff area lands on column 79 (or 80 with rounding)
    assert!(
        after.contains(&79) || after.contains(&80),
        "inner seam should move right: {after:?}"
    );
    assert!(!after.contains(&62), "old seam column must be inside the pane now: {after:?}");
}

#[test]
fn divider_drag_clamps_to_keep_both_panes_usable() {
    let mut app = setup();
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    draw(&mut app, &mut terminal);

    // try to collapse the list by dragging the seam far left
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 26, 10));
    app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 0, 10));
    app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 0, 10));
    draw(&mut app, &mut terminal);

    // clamped to LIST_MIN_PCT = 12 -> list pane still 12 columns wide
    let bars = border_columns(&terminal, 10);
    assert!(bars.contains(&12), "list divider should clamp at 12%: {bars:?}");
}
