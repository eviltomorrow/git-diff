use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use git_diff::controller::Controller;
use git_diff::git::{GitFacade, GitRunner};
use git_diff::tree::VisibleRow;

/// Rich fake repo: modified, added, deleted, binary, nested paths, plus a file
/// with a large unchanged middle (for fold scenarios).
struct RichRunner;

impl GitRunner for RichRunner {
    fn run(&self, args: &[&str]) -> anyhow::Result<String> {
        match args {
            ["diff", "--numstat", "-M", "HEAD"] => Ok(
                "10\t0\tsrc/big.rs\n1\t0\tsrc/deep/small.rs\n2\t1\ttop.rs\n-\t-\timg.bin\n0\t3\tgone.txt\n1\t0\tadded.rs\n".into(),
            ),
            ["diff", "--name-status", "-M", "HEAD"] => Ok(
                "M\tsrc/big.rs\nM\tsrc/deep/small.rs\nM\ttop.rs\nM\timg.bin\nD\tgone.txt\nA\tadded.rs\n".into(),
            ),
            ["ls-files", "--others", "--exclude-standard"] => Ok(String::new()),
            ["show", "HEAD:src/big.rs"] => Ok(gen_lines(40)),
            ["show", "HEAD:src/deep/small.rs"] => Ok("a\nb\nc\n".into()),
            ["show", "HEAD:top.rs"] => Ok("t1\nt2\nt3\n".into()),
            ["show", "HEAD:gone.txt"] => Ok("g1\ng2\ng3\n".into()),
            _ => Ok(String::new()),
        }
    }
}

fn gen_lines(n: usize) -> String {
    (0..n)
        .map(|i| format!("line {i}").to_string())
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn setup() -> Controller<'static> {
    let facade = GitFacade::new(&RichRunner, &PathBuf::from("/tmp"));
    Controller::new(facade, PathBuf::from("/tmp"), true, None).unwrap()
}

/// The diff panel must always show the file currently highlighted in the list
/// (or nothing when a directory/no file is selected).
fn assert_diff_matches_selection(ctrl: &Controller) {
    let rows = ctrl.visible_rows();
    let selected = rows.get(ctrl.cursor).and_then(|r| match r {
        VisibleRow::File { file, .. } => Some(file.path.as_str()),
        _ => None,
    });
    let shown = ctrl.diff_file.as_ref().map(|f| f.path.as_str());
    assert_eq!(
        shown, selected,
        "diff panel ({shown:?}) must match the list selection ({selected:?})"
    );
}

#[test]
fn diff_follows_selection_through_all_scenarios() {
    let mut ctrl = setup();
    assert_diff_matches_selection(&ctrl);

    // walk the whole list with Up/Down
    let count = ctrl.visible_rows().len();
    for _ in 0..count {
        ctrl.handle_key(key(KeyCode::Down));
        assert_diff_matches_selection(&ctrl);
    }
    for _ in 0..count {
        ctrl.handle_key(key(KeyCode::Up));
        assert_diff_matches_selection(&ctrl);
    }

    // page through
    ctrl.handle_key(key(KeyCode::PageDown));
    assert_diff_matches_selection(&ctrl);
    ctrl.handle_key(key(KeyCode::PageUp));
    assert_diff_matches_selection(&ctrl);

    // sort cycles must keep the panel in sync
    ctrl.handle_key(key(KeyCode::Char('s')));
    assert_diff_matches_selection(&ctrl);
    ctrl.handle_key(key(KeyCode::Char('s')));
    assert_diff_matches_selection(&ctrl);
    ctrl.handle_key(key(KeyCode::Char('s')));
    assert_diff_matches_selection(&ctrl);

    // collapse / expand all dirs
    ctrl.handle_key(key(KeyCode::Char('[')));
    assert_diff_matches_selection(&ctrl);
    ctrl.handle_key(key(KeyCode::Char(']')));
    assert_diff_matches_selection(&ctrl);

    // filter start, type, backspace, cancel
    ctrl.handle_key(key(KeyCode::Char('/')));
    for ch in ['s', 'r', 'c'] {
        ctrl.handle_key(key(KeyCode::Char(ch)));
        assert_diff_matches_selection(&ctrl);
    }
    ctrl.handle_key(key(KeyCode::Backspace));
    assert_diff_matches_selection(&ctrl);
    ctrl.handle_key(key(KeyCode::Esc));
    assert_diff_matches_selection(&ctrl);

    // filter to a match and confirm with Enter
    ctrl.handle_key(key(KeyCode::Char('/')));
    for ch in ['t', 'o', 'p'] {
        ctrl.handle_key(key(KeyCode::Char(ch)));
    }
    ctrl.handle_key(key(KeyCode::Enter));
    assert_diff_matches_selection(&ctrl);

    // mode switches (B empty, C, back to A)
    ctrl.handle_key(key(KeyCode::Char('2')));
    assert_diff_matches_selection(&ctrl);
    ctrl.handle_key(key(KeyCode::Char('3')));
    assert_diff_matches_selection(&ctrl);
    ctrl.handle_key(key(KeyCode::Char('1')));
    assert_diff_matches_selection(&ctrl);
}
