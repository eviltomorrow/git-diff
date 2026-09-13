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
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            let hint = stderr.trim();
            if hint.is_empty() {
                bail!("当前目录不是 git 仓库\n提示：请先进入一个 git 仓库，或在其中执行 `git init`")
            } else {
                bail!("当前目录不是 git 仓库（{}）\n提示：请先进入一个 git 仓库，或在其中执行 `git init`", hint)
            }
        }
        Err(e) => bail!("无法运行 git：{}\n提示：请确认已安装 git 并配置 PATH", e),
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
    let arg = std::env::args().nth(1);
    let root = match repo_root() {
        Ok(r) => r,
        Err(e) => {
            // friendly, stacktrace-free exit for common setup mistakes
            eprintln!("{}", e);
            std::process::exit(1);
        }
    };
    let commits = has_commits(&root);
    let runner = SystemRunner::new(root.clone());
    tui::run(&runner, root, commits, arg)
}