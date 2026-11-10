use std::path::PathBuf;

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
    let facade = GitFacade::new(
        &OneFileRunner,
        &PathBuf::from("/tmp/opencode/does-not-exist"),
    );
    let app = App::new(
        facade,
        PathBuf::from("/tmp/opencode/does-not-exist"),
        true,
        None,
    )
    .unwrap();
    (app, Terminal::new(TestBackend::new(width, 30)).unwrap())
}

fn draw(app: &mut App<'static>, terminal: &mut Terminal<TestBackend>) {
    terminal.draw(|f| app.render(f)).unwrap();
}

/// Column of the list pane's right border on the header row.
fn list_right_border(terminal: &Terminal<TestBackend>) -> u16 {
    let buf = terminal.backend().buffer();
    (1..buf.area.width)
        .find(|&x| buf.cell((x, 3)).is_some_and(|c| c.symbol() == "│"))
        .unwrap_or(buf.area.width)
}

/// The first file row (the root directory sits one row above it).
fn list_row_text(terminal: &Terminal<TestBackend>) -> String {
    let buf = terminal.backend().buffer();
    let right = list_right_border(terminal);
    (1..right)
        .filter_map(|x| buf.cell((x, 6)).map(|c| c.symbol().to_string()))
        .collect::<String>()
        .trim_end()
        .to_string()
}

#[test]
fn list_width_is_fixed_regardless_of_terminal_width() {
    // the list is a fixed 44-column pane, so 100- and 200-column terminals
    // both end it at the same screen column (very narrow terminals let it
    // shrink so the diff area keeps its minimum)
    for width in [100, 200] {
        let (mut app, mut terminal) = setup(width);
        draw(&mut app, &mut terminal);
        assert_eq!(
            list_right_border(&terminal),
            43,
            "list width at {width} columns"
        );
    }
}

#[test]
fn fixed_width_list_shows_stats_and_status() {
    for width in [80, 200] {
        let (mut app, mut terminal) = setup(width);
        draw(&mut app, &mut terminal);
        let row = list_row_text(&terminal);
        assert!(
            row.contains("hello.rs"),
            "filename at {width} cols: {row:?}"
        );
        assert!(row.contains("+3"), "added column at {width} cols: {row:?}");
        assert!(
            row.contains("-1"),
            "deleted column at {width} cols: {row:?}"
        );
        assert!(row.ends_with('M'), "status letter at {width} cols: {row:?}");
    }
}
