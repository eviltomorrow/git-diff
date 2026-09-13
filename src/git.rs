use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, Context};

use crate::model::{ChangedFile, CommitEntry, ComparisonMode, FileSides, Status};

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
            return Err(anyhow!("git {:?} failed: {}", args, stderr.trim()));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

pub struct GitFacade<'a> {
    runner: &'a dyn GitRunner,
    root: PathBuf,
}

type NumstatRow = (String, Option<String>, u64, u64, bool);

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
                let old = path[..idx].to_string();
                let new = path[idx + 4..].to_string();
                (new, Some(old))
            } else {
                (path.to_string(), None)
            }
        } else {
            (path.to_string(), None)
        };
        rows.push((new_path, old_path, added, deleted, binary));
    }
    Ok(rows)
}

pub fn parse_name_status(out: &str) -> anyhow::Result<Vec<(Status, String, Option<String>)>> {
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
            rows.push((Status::Renamed, new, Some(old)));
            continue;
        }
        let path = it.next().unwrap_or("").to_string();
        let status = match status {
            "M" => Status::Modified,
            "A" => Status::Added,
            "D" => Status::Deleted,
            _ => Status::Modified,
        };
        rows.push((status, path, None));
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
            .map(|(s, p, old)| (p, (s, old)))
            .collect();

        let mut files = Vec::new();
        for (path, numstat_old, added, deleted, is_binary) in rows {
            let (status, name_status_old) = status_by_path
                .get(&path)
                .cloned()
                .unwrap_or((Status::Modified, None));
            files.push(ChangedFile {
                status,
                path,
                old_path: name_status_old.or(numstat_old),
                added,
                deleted,
                is_binary,
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
            "--pretty=format:%h|%s|%ad|%an",
            "--date=short",
        ])?;
        parse_log(&out)
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

    pub fn file_sides(&self, mode: ComparisonMode, file: &ChangedFile) -> anyhow::Result<FileSides> {
        let origin_path = file.old_path.as_deref().unwrap_or(&file.path);
        match mode {
            ComparisonMode::CommitVsHead => Err(anyhow!("use file_sides_between")),
            _ => {
                let no_origin = file.status == Status::Added || file.status == Status::Untracked;
                let no_changed = file.status == Status::Deleted;
                let (original, changed) = match mode {
                    ComparisonMode::WorkingVsHead => (
                        if no_origin { None } else { self.fetch_ref("HEAD", origin_path)? },
                        if no_changed { None } else { self.read_worktree(&file.path).ok() },
                    ),
                    ComparisonMode::StagedVsHead => (
                        if no_origin { None } else { self.fetch_ref("HEAD", origin_path)? },
                        if no_changed { None } else { self.fetch_ref("", &file.path)? },
                    ),
                    ComparisonMode::StagedVsWorking => (
                        if no_origin { None } else { self.fetch_ref("", origin_path)? },
                        if no_changed { None } else { self.read_worktree(&file.path).ok() },
                    ),
                    ComparisonMode::CommitVsHead => unreachable!(),
                };
                Ok(FileSides {
                    original,
                    changed,
                })
            }
        }
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