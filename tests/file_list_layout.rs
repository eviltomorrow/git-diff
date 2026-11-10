use std::path::PathBuf;

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use git_diff::git::{GitFacade, GitRunner};
use git_diff::tui::App;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

struct OneFileRunner;

impl GitRunner for OneFileRunner {
    fn run(&self, args: &[&str]) -> anyhow::Result<String> {
        match args {
            ["diff", "--numstat", "-M", "HEAD"] => Ok("3\t1\thello.rs\n".into()),
            ["diff", "--name-status", "-M", "HEAD"] => Ok("M\thello.rs\n".into()),
            ["show", "HEAD:hello.rs"] => Ok("fn main() {}\n".into()),
            _ => Ok(String::new()),
        }
    }
}

fn setup(width: u16) -> (App<'static>, Terminal<TestBackend>) {
    let facade = GitFacade::new(&OneFileRunner, &PathBuf::from("/tmp/opencode/does-not-exist"));
    let app = App::new(facade, PathBuf::from("/tmp/opencode/does-not-exist"), true, None).unwrap();
    (app, Terminal::new(TestBackend::new(width, 30)).unwrap())
}

fn draw(app: &mut App<'static>, terminal: &mut Terminal<TestBackend>) {
    terminal.draw(|f| app.render(f)).unwrap();
}

/// The first file row (the root directory sits one row above it).
fn list_row_text(terminal: &Terminal<TestBackend>) -> String {
    let buf = terminal.backend().buffer();
    // the list pane's right border on the header row bounds the row content
    let right = (1..buf.area.width)
        .find(|&x| buf.cell((x, 3)).is_some_and(|c| c.symbol() == "│"))
        .unwrap_or(buf.area.width);
    (1..right)
        .filter_map(|x| buf.cell((x, 6)).map(|c| c.symbol().to_string()))
        .collect::<String>()
        .trim_end()
        .to_string()
}

/// The list divider starts on the pane's right border; pressing there is
/// inside the one-column hit slop.
fn press_divider(app: &mut App<'static>, terminal: &Terminal<TestBackend>) -> u16 {
    let buf = terminal.backend().buffer();
    let x = (1..buf.area.width)
        .find(|&x| buf.cell((x, 3)).is_some_and(|c| c.symbol() == "│"))
        .unwrap_or(buf.area.width);
    let down = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: x,
        row: 10,
        modifiers: KeyModifiers::NONE,
    };
    app.handle_mouse(down);
    x
}

fn drag_to(app: &mut App<'static>, x: u16) {
    let drag = MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: x,
        row: 10,
        modifiers: KeyModifiers::NONE,
    };
    app.handle_mouse(drag);
}

fn release(app: &mut App<'static>, x: u16) {
    let up = MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: x,
        row: 10,
        modifiers: KeyModifiers::NONE,
    };
    app.handle_mouse(up);
}

#[test]
fn narrow_list_keeps_only_the_filename() {
    // 80 cols -> 26% list panel ~= 21 cols, 19 inner: stats are hidden
    let (mut app, mut terminal) = setup(80);
    draw(&mut app, &mut terminal);
    let row = list_row_text(&terminal);
    assert!(row.contains("hello.rs"), "filename should survive: {row:?}");
    assert!(row.ends_with("hello.rs"), "name is the last field: {row:?}");
    assert!(!row.contains('+'), "added column hidden: {row:?}");
    assert!(!row.contains('-'), "deleted column hidden: {row:?}");
    assert!(!row.contains('M'), "status hidden: {row:?}");
}

#[test]
fn wide_list_shows_stats_and_status() {
    // 200 cols -> 26% list panel = 52 cols, 50 inner: full columns
    let (mut app, mut terminal) = setup(200);
    draw(&mut app, &mut terminal);
    let row = list_row_text(&terminal);
    assert!(row.contains("hello.rs"), "filename: {row:?}");
    assert!(row.contains("+3"), "added column: {row:?}");
    assert!(row.contains("-1"), "deleted column: {row:?}");
    assert!(row.ends_with('M'), "status letter: {row:?}");
}

#[test]
fn narrowing_drops_stat_columns_one_segment_at_a_time() {
    let (mut app, mut terminal) = setup(200);
    draw(&mut app, &mut terminal);
    assert!(list_row_text(&terminal).contains("+3"));

    press_divider(&mut app, &terminal);
    // 36-col list pane -> still all columns
    drag_to(&mut app, 36);
    draw(&mut app, &mut terminal);
    assert!(list_row_text(&terminal).contains("+3"));

    // 30-col pane -> the +N segment is gone, -M and Status remain
    drag_to(&mut app, 30);
    draw(&mut app, &mut terminal);
    let row = list_row_text(&terminal);
    assert!(!row.contains("+3"), "+N hides first: {row:?}");
    assert!(row.contains("-1"), "-M still visible: {row:?}");
    assert!(row.ends_with('M'), "status still visible: {row:?}");

    // 24-col pane -> only Status remains
    drag_to(&mut app, 24);
    draw(&mut app, &mut terminal);
    let row = list_row_text(&terminal);
    assert!(!row.contains("+3") && !row.contains("-1"), "stats gone: {row:?}");
    assert!(row.ends_with('M'), "status last: {row:?}");
    release(&mut app, 24);
}

#[test]
fn widening_slides_segments_back_in_over_ticks() {
    let (mut app, mut terminal) = setup(80);
    draw(&mut app, &mut terminal);
    assert!(!list_row_text(&terminal).contains('M'));

    press_divider(&mut app, &terminal);
    drag_to(&mut app, 48); // 60% of 80 = 48 cols, 46 inner: full group
    release(&mut app, 48);
    draw(&mut app, &mut terminal);
    // target is now "visible" but the reveal animation starts at zero
    assert!(!list_row_text(&terminal).contains('M'));

    // after 4 ticks (~0.48) only Status is fully revealed
    for _ in 0..4 {
        app.tick();
    }
    draw(&mut app, &mut terminal);
    let row = list_row_text(&terminal);
    assert!(row.ends_with('M'), "status arrives first: {row:?}");
    assert!(!row.contains("-1"), "-M not yet: {row:?}");

    // after 6 ticks (~0.72) -M joins
    for _ in 0..2 {
        app.tick();
    }
    draw(&mut app, &mut terminal);
    let row = list_row_text(&terminal);
    assert!(row.contains("-1"), "-M slides in: {row:?}");
    assert!(!row.contains("+3"), "+N still hidden: {row:?}");

    // finish the slide: +N joins
    for _ in 0..4 {
        app.tick();
    }
    draw(&mut app, &mut terminal);
    let row = list_row_text(&terminal);
    assert!(row.contains("+3") && row.contains("-1") && row.ends_with('M'), "full group: {row:?}");
}
