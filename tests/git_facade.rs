use git_diff::model::*;
use git_diff::git::{GitFacade, GitRunner, parse_numstat, parse_name_status, parse_log};

struct FakeRunner {
    responses: Vec<(&'static [&'static str], &'static str)>,
}

impl FakeRunner {
    fn new(responses: Vec<(&'static [&'static str], &'static str)>) -> Self {
        Self { responses }
    }
}

impl GitRunner for FakeRunner {
    fn run(&self, args: &[&str]) -> anyhow::Result<String> {
        for (key, out) in &self.responses {
            if *key == args {
                return Ok(out.to_string());
            }
        }
        Ok(String::new())
    }
}

#[test]
fn parse_numstat_basic() {
    let out = "2\t1\tsrc/main.rs\n30\t0\tsrc/lib.rs\n0\t5\told.txt\n";
    let rows = parse_numstat(out, false).unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0], (String::from("src/main.rs"), None, 2, 1, false));
    assert_eq!(rows[1], (String::from("src/lib.rs"), None, 30, 0, false));
    assert_eq!(rows[2], (String::from("old.txt"), None, 0, 5, false));
}

#[test]
fn parse_numstat_rename() {
    let out = "1\t1\tsrc/a.rs => src/b.rs\n";
    let rows = parse_numstat(out, true).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0], (String::from("src/b.rs"), Some(String::from("src/a.rs")), 1, 1, false));
}

#[test]
fn parse_numstat_binary() {
    let out = "-\t-\tbinary.png\n";
    let rows = parse_numstat(out, false).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].4);
}

#[test]
fn parse_name_status_basic() {
    let out = "M\tsrc/main.rs\nA\tsrc/lib.rs\nD\told.txt\n";
    let rows = parse_name_status(out).unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0], (Status::Modified, String::from("src/main.rs"), None));
    assert_eq!(rows[1], (Status::Added, String::from("src/lib.rs"), None));
    assert_eq!(rows[2], (Status::Deleted, String::from("old.txt"), None));
}

#[test]
fn parse_name_status_rename() {
    let out = "R100\tsrc/a.rs\tsrc/b.rs\nR075\tsrc/c.rs\tsrc/d.rs\n";
    let rows = parse_name_status(out).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0], (Status::Renamed, String::from("src/b.rs"), Some(String::from("src/a.rs"))));
    assert_eq!(rows[1], (Status::Renamed, String::from("src/d.rs"), Some(String::from("src/c.rs"))));
}

#[test]
fn parse_log_format() {
    let out = "a1b2c3d|Add refund logic|2026-09-10|Alice\nf4e5d6c|Fix login crash|2026-09-05|Bob\n";
    let commits = parse_log(out).unwrap();
    assert_eq!(commits.len(), 2);
    assert_eq!(commits[0].short_hash, "a1b2c3d");
    assert_eq!(commits[0].title, "Add refund logic");
    assert_eq!(commits[0].date, "2026-09-10");
    assert_eq!(commits[0].author, "Alice");
}

#[test]
fn changed_files_mode_a_lists_modified() {
    let runner = FakeRunner::new(vec![
        (
            &["diff", "--numstat", "-M", "HEAD"],
            "2\t1\tsrc/main.rs\n-\t-\tbin.dat\n",
        ),
        (
            &["diff", "--name-status", "-M", "HEAD"],
            "M\tsrc/main.rs\n",
        ),
    ]);
    let root = std::env::temp_dir();
    let facade = GitFacade::new(&runner, &root);
    let files = facade.changed_files(ComparisonMode::WorkingVsHead).unwrap();
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].status, Status::Modified);
    assert_eq!(files[0].path, "src/main.rs");
    assert_eq!(files[0].added, 2);
    assert_eq!(files[1].status, Status::Modified);
    assert_eq!(files[1].path, "bin.dat");
    assert!(files[1].is_binary);
}

#[test]
fn untracked_files_lists_and_counts_untracked() {
    let runner = FakeRunner::new(vec![(
        &["ls-files", "--others", "--exclude-standard"],
        "notes.md\n",
    )]);
    let root = std::env::temp_dir();
    let notes = root.join("notes.md");
    std::fs::write(&notes, "a\nb\nc\n").unwrap();
    let facade = GitFacade::new(&runner, &root);
    let files = facade.untracked_files().unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].status, Status::Untracked);
    assert_eq!(files[0].path, "notes.md");
    assert_eq!(files[0].added, 3);
    let _ = std::fs::remove_file(&notes);
}

#[test]
fn changed_files_mode_b_uses_cached() {
    let runner = FakeRunner::new(vec![
        (
            &["diff", "--cached", "--numstat", "-M", "HEAD"],
            "2\t1\tsrc/main.rs\n",
        ),
        (
            &["diff", "--cached", "--name-status", "-M", "HEAD"],
            "M\tsrc/main.rs\n",
        ),
    ]);
    let facade = GitFacade::new(&runner, &std::env::temp_dir());
    let files = facade.changed_files(ComparisonMode::StagedVsHead).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "src/main.rs");
}

#[test]
fn changed_files_mode_c_uses_working_vs_index() {
    let runner = FakeRunner::new(vec![
        (
            &["diff", "--numstat", "-M"],
            "2\t1\tsrc/main.rs\n",
        ),
        (
            &["diff", "--name-status", "-M"],
            "M\tsrc/main.rs\n",
        ),
    ]);
    let facade = GitFacade::new(&runner, &std::env::temp_dir());
    let files = facade.changed_files(ComparisonMode::StagedVsWorking).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "src/main.rs");
}

#[test]
fn changed_files_mode_d_uses_commit() {
    let runner = FakeRunner::new(vec![
        (
            &["diff", "--numstat", "-M", "abc1234", "HEAD"],
            "2\t1\tsrc/main.rs\n",
        ),
        (
            &["diff", "--name-status", "-M", "abc1234", "HEAD"],
            "M\tsrc/main.rs\n",
        ),
    ]);
    let facade = GitFacade::new(&runner, &std::env::temp_dir());
    let files = facade.changed_files_between("abc1234").unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "src/main.rs");
}

#[test]
fn empty_output_means_no_changes() {
    let runner = FakeRunner::new(vec![]);
    let facade = GitFacade::new(&runner, &std::env::temp_dir());
    let files = facade.changed_files(ComparisonMode::WorkingVsHead).unwrap();
    assert!(files.is_empty());
}