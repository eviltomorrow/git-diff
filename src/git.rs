use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, Context};

use crate::model::{ChangedFile, CommitEntry, ComparisonMode, FileSides, SidePair, SideRef, Status};

pub trait GitRunner {
    fn run(&self, args: &[&str]) -> anyhow::Result<String>;
}

pub struct SystemRunner {
    root: PathBuf,
}

impl SystemRunner {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
}

impl GitRunner for SystemRunner {
    fn run(&self, args: &[&str]) -> anyhow::Result<String> {
        let out = Command::new("git")
            .current_dir(&self.root)
            .args(args)
            .output()
            .context("failed to run git")?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(anyhow!("{}", clean_git_error(&stderr)));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

/// Turns git's stderr into a short, friendly message for the status bar:
/// takes the first non-empty line and strips git's own error prefix
/// (`fatal: `, `error: `, and their localized forms), so e.g.
/// `fatal: 有歧义的参数 'd1d413e'` shows as `有歧义的参数 'd1d413e'`.
fn clean_git_error(stderr: &str) -> &str {
    let first = stderr
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    const PREFIXES: [&str; 4] = ["fatal: ", "error: ", "致命错误：", "错误："];
    for prefix in PREFIXES {
        if let Some(rest) = first.strip_prefix(prefix) {
            return rest;
        }
    }
    first
}

pub struct GitFacade<'a> {
    runner: &'a dyn GitRunner,
    root: PathBuf,
}

/// One row of `git diff --numstat`: the new path, optional old path for
/// renames, added/deleted line counts, and whether the file is binary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NumstatRow {
    pub path: String,
    pub old_path: Option<String>,
    pub added: u64,
    pub deleted: u64,
    pub binary: bool,
}

/// One row of `git diff --name-status`: the status, path, and old path for
/// renames.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusRow {
    pub status: Status,
    pub path: String,
    pub old_path: Option<String>,
}

/// Expands one side of git's brace-abbreviated rename path back to the real
/// path. With rename detection, `git diff --numstat` shortens the common
/// prefix/suffix of a rename with braces, e.g. `{a/b => c/d}/e` (directory
/// rename) or `a/{b => c}` (file rename inside a directory). `new` selects the
/// side after the `=>`; the other side keeps the text before it. Braces that
/// do not enclose a ` => ` are kept literally.
fn rename_side(path: &str, new: bool) -> String {
    let mut out = String::new();
    let mut rest = path;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let arrow = after.find(" => ");
        let close = after.find('}');
        if let Some(arrow) = arrow
            && close.is_some_and(|c| arrow < c)
        {
            let close = arrow + 4 + after[arrow + 4..].find('}').expect("close inside braces");
            let a = &after[..arrow];
            let b = &after[arrow + 4..close];
            out.push_str(if new { b } else { a });
            rest = &after[close + 1..];
        } else {
            out.push('{');
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

pub fn parse_numstat(out: &str, with_rename: bool) -> anyhow::Result<Vec<NumstatRow>> {
    let mut rows = Vec::new();
    for line in out.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let mut it = line.splitn(3, '\t');
        let added = it.next().unwrap_or("");
        let deleted = it.next().unwrap_or("");
        let path = it.next().unwrap_or("");
        let binary = added == "-" || deleted == "-";
        let (added, deleted) = if binary {
            (0, 0)
        } else {
            (
                added.parse().context("bad numstat added")?,
                deleted.parse().context("bad numstat deleted")?,
            )
        };
        let (new_path, old_path) = if with_rename {
            if let Some(idx) = path.find(" => ") {
                if path.contains('{') {
                    (rename_side(path, true), Some(rename_side(path, false)))
                } else {
                    (path[idx + 4..].to_string(), Some(path[..idx].to_string()))
                }
            } else {
                (path.to_string(), None)
            }
        } else {
            (path.to_string(), None)
        };
        rows.push(NumstatRow {
            path: new_path,
            old_path,
            added,
            deleted,
            binary,
        });
    }
    Ok(rows)
}

pub fn parse_name_status(out: &str) -> anyhow::Result<Vec<StatusRow>> {
    let mut rows = Vec::new();
    for line in out.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let mut it = line.splitn(3, '\t');
        let status = it.next().unwrap_or("");
        if status.strip_prefix('R').is_some() {
            let old = it.next().unwrap_or("").to_string();
            let new = it.next().unwrap_or("").to_string();
            rows.push(StatusRow {
                status: Status::Renamed,
                path: new,
                old_path: Some(old),
            });
            continue;
        }
        let path = it.next().unwrap_or("").to_string();
        let status = match status {
            "M" => Status::Modified,
            "A" => Status::Added,
            "D" => Status::Deleted,
            _ => Status::Modified,
        };
        rows.push(StatusRow {
            status,
            path,
            old_path: None,
        });
    }
    Ok(rows)
}

pub fn parse_log(out: &str) -> anyhow::Result<Vec<CommitEntry>> {
    let mut commits = Vec::new();
    for line in out.lines() {
        let mut it = line.splitn(4, '|');
        let short_hash = it.next().unwrap_or("").to_string();
        let title = it.next().unwrap_or("").to_string();
        let date = it.next().unwrap_or("").to_string();
        let author = it.next().unwrap_or("").to_string();
        commits.push(CommitEntry {
            short_hash,
            title,
            date,
            author,
        });
    }
    Ok(commits)
}

impl<'a> GitFacade<'a> {
    pub fn new(runner: &'a dyn GitRunner, root: &Path) -> Self {
        Self {
            runner,
            root: root.to_path_buf(),
        }
    }

    fn run(&self, args: &[&str]) -> anyhow::Result<String> {
        self.runner.run(args)
    }

    fn changed_files_with(&self, numstat_args: &[&str], status_args: &[&str]) -> anyhow::Result<Vec<ChangedFile>> {
        let numstat = self.run(numstat_args)?;
        let statuses = self.run(status_args)?;
        let rows = parse_numstat(&numstat, true)?;
        let status_rows = parse_name_status(&statuses)?;

        let status_by_path: std::collections::HashMap<String, (Status, Option<String>)> = status_rows
            .into_iter()
            .map(|s| (s.path, (s.status, s.old_path)))
            .collect();

        let mut files = Vec::new();
        for row in rows {
            let (status, name_status_old) = status_by_path
                .get(&row.path)
                .cloned()
                .unwrap_or((Status::Modified, None));
            files.push(ChangedFile {
                status,
                path: row.path,
                old_path: name_status_old.or(row.old_path),
                added: row.added,
                deleted: row.deleted,
                is_binary: row.binary,
            });
        }
        Ok(files)
    }

    pub fn changed_files(&self, mode: ComparisonMode) -> anyhow::Result<Vec<ChangedFile>> {
        let files = match mode {
            ComparisonMode::WorkingVsHead => self.changed_files_with(
                &["diff", "--numstat", "-M", "HEAD"],
                &["diff", "--name-status", "-M", "HEAD"],
            )?,
            ComparisonMode::StagedVsHead => self.changed_files_with(
                &["diff", "--cached", "--numstat", "-M", "HEAD"],
                &["diff", "--cached", "--name-status", "-M", "HEAD"],
            )?,
            ComparisonMode::StagedVsWorking => self.changed_files_with(
                &["diff", "--numstat", "-M"],
                &["diff", "--name-status", "-M"],
            )?,
            ComparisonMode::CommitVsHead => {
                return Err(anyhow!("changed_files does not accept CommitVsHead; use changed_files_between"))
            }
        };

        Ok(files)
    }

    pub fn untracked_files(&self) -> anyhow::Result<Vec<ChangedFile>> {
        let untracked = self.run(&["ls-files", "--others", "--exclude-standard"])?;
        let mut files = Vec::new();
        for path in untracked.lines() {
            if path.trim().is_empty() {
                continue;
            }
            let added = self
                .read_worktree(path)
                .map(|c| count_lines(&c) as u64)
                .unwrap_or(0);
            files.push(ChangedFile {
                status: Status::Untracked,
                path: path.to_string(),
                old_path: None,
                added,
                deleted: 0,
                is_binary: false,
            });
        }
        Ok(files)
    }

    pub fn changed_files_between(&self, commit: &str) -> anyhow::Result<Vec<ChangedFile>> {
        let numstat_args = ["diff", "--numstat", "-M", commit, "HEAD"];
        let status_args = ["diff", "--name-status", "-M", commit, "HEAD"];
        self.changed_files_with(&numstat_args, &status_args)
    }

    pub fn commits(&self) -> anyhow::Result<Vec<CommitEntry>> {
        let out = self.run(&[
            "log",
            "-n",
            "200",
            "--pretty=format:%h|%s|%ad|%an",
            "--date=short",
        ])?;
        parse_log(&out)
    }

    /// The current branch name (e.g. `main`), or `None` on a detached HEAD.
    pub fn current_branch(&self) -> anyhow::Result<Option<String>> {
        match self.run(&["symbolic-ref", "--short", "HEAD"]) {
            Ok(out) => {
                let name = out.trim();
                Ok(if name.is_empty() { None } else { Some(name.to_string()) })
            }
            Err(_) => Ok(None),
        }
    }

    fn fetch_ref(&self, rev: &str, path: &str) -> anyhow::Result<Option<Vec<u8>>> {
        let arg = format!("{}:{}", rev, path);
        match self.run(&["show", &arg]) {
            Ok(out) => Ok(Some(out.into_bytes())),
            Err(_) => Ok(None),
        }
    }

    fn read_worktree(&self, path: &str) -> anyhow::Result<Vec<u8>> {
        std::fs::read(self.root.join(path))
            .with_context(|| format!("failed to read {}", path))
    }

    fn fetch_side(&self, side: &SideRef, path: &str) -> anyhow::Result<Option<Vec<u8>>> {
    match side {
        SideRef::Worktree => Ok(self.read_worktree(path).ok()),
        SideRef::Index => self.fetch_ref("", path),
        SideRef::Head => self.fetch_ref("HEAD", path),
        SideRef::Commit(c) => self.fetch_ref(c, path),
    }
}

pub fn file_sides(&self, mode: ComparisonMode, file: &ChangedFile) -> anyhow::Result<FileSides> {
    if mode == ComparisonMode::CommitVsHead {
        return Err(anyhow!("use file_sides_between"));
    }
    let pair = SidePair::for_mode(mode, None);
    let origin_path = file.old_path.as_deref().unwrap_or(&file.path);
    let no_origin = file.status == Status::Added || file.status == Status::Untracked;
    let no_changed = file.status == Status::Deleted;
    let original = if no_origin { None } else { self.fetch_side(&pair.original, origin_path)? };
    let changed = if no_changed { None } else { self.fetch_side(&pair.changed, &file.path)? };
    Ok(FileSides { original, changed })
}

pub fn file_sides_between(&self, commit: &str, file: &ChangedFile) -> anyhow::Result<FileSides> {
    let origin_path = file.old_path.as_deref().unwrap_or(&file.path);
    let original = if file.status == Status::Added || file.status == Status::Untracked {
        None
    } else {
        self.fetch_ref(commit, origin_path)?
    };
    let changed = if file.status == Status::Deleted {
        None
    } else {
        self.fetch_ref("HEAD", &file.path)?
    };
    Ok(FileSides { original, changed })
}
}

pub fn count_lines(content: &[u8]) -> usize {
    content
        .split(|&b| b == b'\n')
        .filter(|l| !l.is_empty())
        .count()
}

#[cfg(test)]
mod tests {
    use super::count_lines;

    #[test]
    fn count_lines_counts_nonempty() {
        assert_eq!(count_lines(b"a\nb\nc\n"), 3);
        assert_eq!(count_lines(b"a\nb\n"), 2);
        assert_eq!(count_lines(b"abc"), 1);
        assert_eq!(count_lines(b""), 0);
    }
}