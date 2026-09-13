use std::path::PathBuf;
use std::process::Command;

use anyhow::bail;

use git_diff::git::SystemRunner;
use git_diff::tui;

fn repo_root() -> anyhow::Result<PathBuf> {
    let out = Command::new("git")
        .arg("rev-parse")
        .arg("--show-toplevel")
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let path = String::from_utf8_lossy(&o.stdout);
            Ok(PathBuf::from(path.trim()))
        }
        _ => bail!("error: not a git repository"),
    }
}

fn has_commits(root: &PathBuf) -> bool {
    Command::new("git")
        .current_dir(root)
        .args(["rev-parse", "--verify", "HEAD"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn main() -> anyhow::Result<()> {
    let root = repo_root()?;
    let commits = has_commits(&root);
    let runner = SystemRunner::new(root.clone());
    tui::run(&runner, root, commits)
}