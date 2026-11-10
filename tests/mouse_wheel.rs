use std::path::PathBuf;

use crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind,
};
use git_diff::controller::Focus;
use git_diff::git::{GitFacade, GitRunner};
use git_diff::tui::App;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

/// Several files, each with 40 very long lines, so both vertical/horizontal
/// diff scrolling and file-list scrolling have room to move.
struct LongFileRunner;

const FILES: usize = 8;

fn long_content() -> String {
    (1..=40)
        .map(|i| format!("line {i} {}", "x".repeat(180)))
        .collect::<Vec<_>>()
        .join("\n")
}

impl GitRunner for LongFileRunner {
    fn run(&self, args: &[&str]) -> anyhow::Result<String> {
        match args {
            ["diff", "--numstat", "-M", "HEAD"] => Ok((0..FILES)
                .map(|i| format!("40\t0\tbig{i}.rs\n"))
                .collect::<Vec<_>>()
                .join("")),
            ["diff", "--name-status", "-M", "HEAD"] => Ok((0..FILES)
                .map(|i| format!("M\tbig{i}.rs\n"))
                .collect::<Vec<_>>()
                .join("")),
            _ if args.len() == 2 && args[0] == "show" && args[1].starts_with("HEAD:big") => {
                Ok(long_content())
            }
            _ => Ok(String::new()),
        }
    }
}

fn setup() -> (App<'static>, Terminal<TestBackend>) {
    let root = PathBuf::from("/tmp/opencode/does-not-exist");
    let facade = GitFacade::new(&LongFileRunner, &root);
    let app = App::new(facade, root, true, None).unwrap();
    (app, Terminal::new(TestBackend::new(100, 30)).unwrap())
}

fn draw(app: &mut App<'static>, terminal: &mut Terminal<TestBackend>) {
    terminal.draw(|f| app.render(f)).unwrap();
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn wheel(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

/// Select the changed file (row 0 is the repo-root directory) and draw once so
/// the controller knows the diff viewport size.
fn setup_with_diff() -> (App<'static>, Terminal<TestBackend>) {
    let (mut app, mut terminal) = setup();
    app.handle_key(key(KeyCode::Down));
    draw(&mut app, &mut terminal);
    assert!(app.ctrl().diff_rows.len() >= 30, "fixture must produce a tall diff");
    (app, terminal)
}

#[test]
fn wheel_over_diff_scrolls_vertically() {
    let (mut app, _terminal) = setup_with_diff();
    assert_eq!(app.ctrl().diff_cursor, 0);

    // one notch down moves the shared cursor/viewport by three rows
    assert!(app.handle_mouse(wheel(MouseEventKind::ScrollDown, 80, 10)));
    assert_eq!(app.ctrl().diff_cursor, 3);
    assert_eq!(app.ctrl().focus, Focus::Diff);

    assert!(app.handle_mouse(wheel(MouseEventKind::ScrollUp, 80, 10)));
    assert_eq!(app.ctrl().diff_cursor, 0);

    // keep wheeling past the end: clamped to the last row, viewport follows
    for _ in 0..40 {
        app.handle_mouse(wheel(MouseEventKind::ScrollDown, 80, 10));
    }
    assert_eq!(app.ctrl().diff_cursor, app.ctrl().diff_rows.len() - 1);
    assert!(app.ctrl().diff_vscroll > 0, "viewport should have followed");
}

#[test]
fn wheel_over_file_list_moves_the_selection() {
    let (mut app, _terminal) = setup_with_diff();
    assert_eq!(app.ctrl().cursor, 1); // root dir is row 0, first file is row 1

    // one notch down moves the cursored file three rows down and previews it
    assert!(app.handle_mouse(wheel(MouseEventKind::ScrollDown, 10, 10)));
    assert_eq!(app.ctrl().cursor, 4);
    assert_eq!(app.ctrl().focus, Focus::FileList);
    assert_eq!(
        app.ctrl().diff_file.as_ref().map(|f| f.path.as_str()),
        Some("big3.rs")
    );

    assert!(app.handle_mouse(wheel(MouseEventKind::ScrollUp, 10, 10)));
    assert_eq!(app.ctrl().cursor, 1);
    assert_eq!(
        app.ctrl().diff_file.as_ref().map(|f| f.path.as_str()),
        Some("big0.rs")
    );
    // list wheel must not scroll the diff viewport
    assert_eq!(app.ctrl().diff_vscroll, 0);
}

#[test]
fn horizontal_wheel_scrolls_both_panes() {
    let (mut app, _terminal) = setup_with_diff();
    assert_eq!(app.ctrl().diff_hscroll, 0);

    assert!(app.handle_mouse(wheel(MouseEventKind::ScrollRight, 80, 10)));
    assert_eq!(app.ctrl().diff_hscroll, 8);

    assert!(app.handle_mouse(wheel(MouseEventKind::ScrollLeft, 80, 10)));
    assert_eq!(app.ctrl().diff_hscroll, 0);
}
