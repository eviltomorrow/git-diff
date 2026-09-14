use std::collections::HashMap;
use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use git_diff::git::{GitFacade, GitRunner};
use git_diff::tui::App;
use ratatui::Terminal;
use ratatui::backend::{Backend, TestBackend};
use ratatui::buffer::Cell;

struct LoggingBackend {
    inner: TestBackend,
    display: HashMap<(u16, u16), String>,
    log: Vec<String>,
}
impl LoggingBackend {
    fn new(w: u16, h: u16) -> Self {
        Self {
            inner: TestBackend::new(w, h),
            display: HashMap::new(),
            log: Vec::new(),
        }
    }
    fn display_cell(&self, x: u16, y: u16) -> Option<String> {
        self.display.get(&(x, y)).cloned()
    }
    fn clear_display(&mut self) {
        self.display.clear();
    }
}
impl Backend for LoggingBackend {
    fn draw<'a, I>(&mut self, content: I) -> std::io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        for (x, y, cell) in content {
            let sym = cell.symbol();
            let prev = self.display.insert((x, y), sym.to_string());
            if prev != Some(sym.to_string()) {
                self.log.push(format!("({x},{y})={sym:?}"));
            }
        }
        self.inner.draw(std::iter::empty())
    }
    fn hide_cursor(&mut self) -> std::io::Result<()> {
        self.inner.hide_cursor()
    }
    fn show_cursor(&mut self) -> std::io::Result<()> {
        self.inner.show_cursor()
    }
    fn get_cursor_position(&mut self) -> std::io::Result<ratatui::layout::Position> {
        self.inner.get_cursor_position()
    }
    fn set_cursor_position<P: Into<ratatui::layout::Position>>(
        &mut self,
        position: P,
    ) -> std::io::Result<()> {
        self.inner.set_cursor_position(position)
    }
    fn clear(&mut self) -> std::io::Result<()> {
        self.inner.clear()
    }
    fn size(&self) -> std::io::Result<ratatui::layout::Size> {
        self.inner.size()
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
    fn window_size(&mut self) -> std::io::Result<ratatui::backend::WindowSize> {
        self.inner.window_size()
    }
}

struct RealRunner;
impl GitRunner for RealRunner {
    fn run(&self, args: &[&str]) -> anyhow::Result<String> {
        match args {
            ["diff", "--numstat", "-M", "HEAD"] => {
                Ok("3\t2\tmod.rs\n1\t0\tadd.rs\n0\t4\tdel.rs\n2\t1\tcjk.rs\n".into())
            }
            ["diff", "--name-status", "-M", "HEAD"] => {
                Ok("M\tmod.rs\nA\tadd.rs\nD\tdel.rs\nM\tcjk.rs\n".into())
            }
            ["show", "HEAD:mod.rs"] => {
                Ok("fn main() {\n    let x = \"hello\";\n    println!(\"{}\", x);\n}\n".into())
            }
            ["show", "HEAD:cjk.rs"] => {
                Ok("// 这是中文注释\nlet s = \"你好\";\n// 另一行注释\n".into())
            }
            ["show", "HEAD:del.rs"] => Ok("a\nb\nc\nd\n".into()),
            _ => Ok(String::new()),
        }
    }
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn walk_stale(
    app: &mut App<'_>,
    terminal: &mut Terminal<LoggingBackend>,
    moves: &[KeyCode],
) -> Vec<(u16, u16)> {
    let mut prev_text: Vec<(u16, u16)> = {
        let f0 = terminal.draw(|f| app.render(f)).unwrap();
        let mut t = Vec::new();
        for y in 0..f0.buffer.area.height {
            for x in 0..f0.buffer.area.width {
                if let Some(c) = f0.buffer.cell((x, y))
                    && c.symbol() != " "
                {
                    t.push((x, y));
                }
            }
        }
        t
    };
    let mut stale = Vec::new();
    for mc in moves {
        app.handle_key(key(*mc));
        terminal.backend_mut().log.clear();
        let (cur_blank, cur_cells) = {
            let f = terminal.draw(|f| app.render(f)).unwrap();
            let mut blank = std::collections::HashSet::new();
            let mut cells = std::collections::HashMap::new();
            for y in 0..f.buffer.area.height {
                for x in 0..f.buffer.area.width {
                    if let Some(c) = f.buffer.cell((x, y)) {
                        cells.insert((x, y), c.symbol().to_string());
                        if c.symbol() == " " {
                            blank.insert((x, y));
                        }
                    }
                }
            }
            (blank, cells)
        };
        let writes = terminal.backend_mut().log.clone();
        let writes_set: std::collections::HashSet<(u16, u16)> = writes
            .iter()
            .filter_map(|e| {
                let (pos, _) = e.split_once('=')?;
                let inner = pos.trim_start_matches('(').trim_end_matches(')');
                let (x, y) = inner.split_once(',')?;
                Some((x.parse().ok()?, y.parse().ok()?))
            })
            .collect();
        for (x, y) in &prev_text {
            if cur_blank.contains(&(*x, *y)) && !writes_set.contains(&(*x, *y)) {
                stale.push((*x, *y));
            }
        }
        prev_text.clear();
        for ((x, y), s) in cur_cells {
            if s != " " {
                prev_text.push((x, y));
            }
        }
    }
    stale
}

#[test]
fn realistic_content_leaves_no_stale_cells() {
    let facade = GitFacade::new(&RealRunner, &PathBuf::from("/tmp"));
    let mut app = App::new(facade, PathBuf::from("/tmp"), true, None).unwrap();
    let backend = LoggingBackend::new(120, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    // rows: root(0), add.rs(1), cjk.rs(2), del.rs(3), mod.rs(4)
    let moves = vec![
        KeyCode::Down, // add.rs
        KeyCode::Down, // cjk.rs
        KeyCode::Down, // del.rs
        KeyCode::Down, // mod.rs
        KeyCode::Up,   // del.rs
        KeyCode::Up,   // cjk.rs
        KeyCode::Up,   // add.rs
    ];
    let stale = walk_stale(&mut app, &mut terminal, &moves);
    eprintln!("STALE CELLS: {:?}", stale);
    assert!(stale.is_empty(), "stale: {stale:?}");
}

/// A modified file with very long lines containing quotes, so horizontal
/// scrolling is meaningful.
struct LongRunner;
impl GitRunner for LongRunner {
    fn run(&self, args: &[&str]) -> anyhow::Result<String> {
        match args {
            ["diff", "--numstat", "-M", "HEAD"] => Ok("1\t0\tlong.rs\n1\t0\tshort.rs\n".into()),
            ["diff", "--name-status", "-M", "HEAD"] => Ok("M\tlong.rs\nM\tshort.rs\n".into()),
            ["show", "HEAD:long.rs"] => Ok(
                "fn main() { let value = \"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"; println!(\"{}\", value); }\n".into(),
            ),
            ["show", "HEAD:short.rs"] => Ok("short\n".into()),
            _ => Ok(String::new()),
        }
    }
}

#[test]
fn hscroll_then_switch_leaves_no_stale_cells() {
    let facade = GitFacade::new(&LongRunner, &PathBuf::from("/tmp"));
    let mut app = App::new(facade, PathBuf::from("/tmp"), true, None).unwrap();
    let backend = LoggingBackend::new(120, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    // select long.rs
    app.handle_key(key(KeyCode::Down));
    // scroll the diff panel horizontally a few times (needs diff focus)
    app.handle_key(key(KeyCode::Tab)); // focus diff
    for _ in 0..4 {
        app.handle_key(key(KeyCode::Right));
    }
    let stale_h = walk_stale(&mut app, &mut terminal, &[KeyCode::Tab]);
    eprintln!("stale after hscroll switch: {:?}", stale_h);
    assert!(stale_h.is_empty(), "hscroll stale: {stale_h:?}");
}

/// Realistic Go-style files: big_g.go has many rows, small_g.go has few rows.
struct GoRunner;
impl GitRunner for GoRunner {
    fn run(&self, args: &[&str]) -> anyhow::Result<String> {
        match args {
            ["diff", "--numstat", "-M", "HEAD"] => Ok("40\t0\tbig_g.go\n2\t0\tsmall_g.go\n".into()),
            ["diff", "--name-status", "-M", "HEAD"] => Ok("M\tbig_g.go\nM\tsmall_g.go\n".into()),
            ["show", "HEAD:big_g.go"] => Ok(go_lines(40)),
            ["show", "HEAD:small_g.go"] => Ok(go_lines(2)),
            _ => Ok(String::new()),
        }
    }
}

fn go_lines(n: usize) -> String {
    (0..n)
        .map(|i| format!("func worker{i}() error {{\n    value := \"str\"\n    return nil\n}}"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

#[test]
fn big_file_to_small_file_leaves_no_stale_rows() {
    let facade = GitFacade::new(&GoRunner, &PathBuf::from("/tmp"));
    let mut app = App::new(facade, PathBuf::from("/tmp"), true, None).unwrap();
    let backend = LoggingBackend::new(120, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    // rows: root(0), big_g.go(1), small_g.go(2); select big_g.go (40 funcs)
    app.handle_key(key(KeyCode::Down));
    // switch to small_g.go (2 funcs = few rows); the header stats ("行 1/…")
    // shrink and slide right, exercising the wide-glyph redraw path
    let stale = full_redraw_walk(&mut app, &mut terminal, &[KeyCode::Down]);
    assert!(stale.is_empty(), "big->small stale: {stale:?}");
}

/// Walk through `moves` replicating the real event loop: whenever the app asks
/// for a full redraw (or the render mutated navigation state), physically clear
/// the terminal and redraw everything. Returns cells the terminal still shows
/// as non-blank while the current frame has them blank.
fn full_redraw_walk(
    app: &mut App<'_>,
    terminal: &mut Terminal<LoggingBackend>,
    moves: &[KeyCode],
) -> Vec<(u16, u16)> {
    let mut prev_text: Vec<(u16, u16)> = {
        let f0 = terminal.draw(|f| app.render(f)).unwrap();
        let mut t = Vec::new();
        for y in 0..f0.buffer.area.height {
            for x in 0..f0.buffer.area.width {
                if let Some(c) = f0.buffer.cell((x, y))
                    && c.symbol() != " "
                {
                    t.push((x, y));
                }
            }
        }
        t
    };
    let mut stale = Vec::new();
    let mut pending_redraw = true;
    for mc in moves {
        // snapshot nav before draw (mirrors the event loop)
        let nav_before = (
            app.ctrl().diff_file.as_ref().map(|f| f.path.clone()),
            app.ctrl().cursor,
            app.ctrl().list_scroll,
            app.ctrl().diff_cursor,
            app.ctrl().diff_vscroll,
            app.ctrl().diff_hscroll,
        );
        app.handle_key(key(*mc));
        if app.take_full_redraw() {
            pending_redraw = true;
        }
        if pending_redraw {
            terminal.clear().unwrap();
            terminal.swap_buffers();
            terminal.backend_mut().clear_display();
            pending_redraw = false;
        }
        terminal.backend_mut().log.clear();
        let (cur_blank, cur_cells) = {
            let f = terminal.draw(|f| app.render(f)).unwrap();
            let mut blank = std::collections::HashSet::new();
            let mut cells = std::collections::HashMap::new();
            for y in 0..f.buffer.area.height {
                for x in 0..f.buffer.area.width {
                    if let Some(c) = f.buffer.cell((x, y)) {
                        cells.insert((x, y), c.symbol().to_string());
                        if c.symbol() == " " {
                            blank.insert((x, y));
                        }
                    }
                }
            }
            (blank, cells)
        };
        // if the render mutated nav state, a full redraw is needed next frame
        let nav_after = (
            app.ctrl().diff_file.as_ref().map(|f| f.path.clone()),
            app.ctrl().cursor,
            app.ctrl().list_scroll,
            app.ctrl().diff_cursor,
            app.ctrl().diff_vscroll,
            app.ctrl().diff_hscroll,
        );
        if nav_after != nav_before {
            pending_redraw = true;
        }
        let writes = terminal.backend_mut().log.clone();
        let writes_set: std::collections::HashSet<(u16, u16)> = writes
            .iter()
            .filter_map(|e| {
                let (pos, _) = e.split_once('=')?;
                let inner = pos.trim_start_matches('(').trim_end_matches(')');
                let (x, y) = inner.split_once(',')?;
                Some((x.parse().ok()?, y.parse().ok()?))
            })
            .collect();
        let display_nonblank: std::collections::HashSet<(u16, u16)> = {
            let mut s = std::collections::HashSet::new();
            for y in 0..30u16 {
                for x in 0..120u16 {
                    if let Some(c) = terminal.backend().display_cell(x, y)
                        && c != " "
                    {
                        s.insert((x, y));
                    }
                }
            }
            s
        };
        for (x, y) in &prev_text {
            if cur_blank.contains(&(*x, *y))
                && !writes_set.contains(&(*x, *y))
                && display_nonblank.contains(&(*x, *y))
            {
                stale.push((*x, *y));
            }
        }
        prev_text.clear();
        for ((x, y), s) in cur_cells {
            if s != " " {
                prev_text.push((x, y));
            }
        }
    }
    stale
}

/// Chinese file and directory names, plus CJK file content, so switching
/// files / scrolling moves wide (double-width) glyphs.
struct ChineseRunner;
impl GitRunner for ChineseRunner {
    fn run(&self, args: &[&str]) -> anyhow::Result<String> {
        match args {
            ["diff", "--numstat", "-M", "HEAD"] => Ok(
                "1\t0\tsrc/controller.rs\n40\t0\tdocs/各省份配置/福建/note.txt\n2\t0\tdocs/参数-下发格式.txt\n1\t0\tgo.mod\n".into(),
            ),
            ["diff", "--name-status", "-M", "HEAD"] => Ok(
                "M\tsrc/controller.rs\nM\tdocs/各省份配置/福建/note.txt\nM\tdocs/参数-下发格式.txt\nM\tgo.mod\n".into(),
            ),
            ["show", "HEAD:src/controller.rs"] => Ok("use std::fmt;\n".into()),
            ["show", "HEAD:docs/各省份配置/福建/note.txt"] => Ok("查询 BGP-LS 路径\n下发链路调试日志\n".into()),
            ["show", "HEAD:docs/参数-下发格式.txt"] => Ok("参数文本内容\n".into()),
            ["show", "HEAD:go.mod"] => Ok("module example\n\ngo 1.22\n".into()),
            _ => Ok(String::new()),
        }
    }
}

#[test]
fn switching_chinese_content_with_full_redraw_leaves_no_stale_cells() {
    let facade = GitFacade::new(&ChineseRunner, &PathBuf::from("/tmp"));
    let mut app = App::new(facade, PathBuf::from("/tmp"), true, None).unwrap();
    let backend = LoggingBackend::new(120, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    // walk the whole list and back (scrolls the file list; CJK names move)
    let mut moves = vec![KeyCode::Down; 8];
    moves.extend(std::iter::repeat_n(KeyCode::Up, 8));
    let stale = full_redraw_walk(&mut app, &mut terminal, &moves);
    assert!(
        stale.is_empty(),
        "stale cells remain after full-redraw walk: {stale:?}"
    );
}

#[test]
fn file_list_collapse_sort_filter_leaves_no_stale() {
    let facade = GitFacade::new(&ChineseRunner, &PathBuf::from("/tmp"));
    let mut app = App::new(facade, PathBuf::from("/tmp"), true, None).unwrap();
    let backend = LoggingBackend::new(120, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    let moves = vec![
        KeyCode::Down,      // -> docs dir
        KeyCode::Down,      // -> 各省份配置 dir
        KeyCode::Down,      // -> 福建 dir
        KeyCode::Down,      // -> note.txt (Chinese name scrolls list)
        KeyCode::Char('['), // collapse all dirs
        KeyCode::Char(']'), // expand all dirs
        KeyCode::Char('s'), // sort by status
        KeyCode::Char('s'), // sort by added
        KeyCode::Char('s'), // sort by path
        KeyCode::Char('/'), // start filter
        KeyCode::Char('g'), // type filter
        KeyCode::Char('o'), // type filter
        KeyCode::Esc,       // cancel filter
        KeyCode::Down,      // scroll list
        KeyCode::Up,
    ];
    let stale = full_redraw_walk(&mut app, &mut terminal, &moves);
    assert!(stale.is_empty(), "stale after list ops: {stale:?}");
}
