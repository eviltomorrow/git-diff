# 0001: Use the git CLI as the data source

We shell out to `git` (via `git show`, `git diff --numstat`, `git diff --name-status`, `git log`) for all repo data instead of linking against libgit2 (`git2`) or `gix`.

A pure-Rust TUI would otherwise carry a C dependency (libgit2) or an immature API. The git CLI is always present, behaves exactly like the git the user knows, and needs only parsing of `--numstat`/`--name-status`/`--log` output. Line alignment between the two sides is computed ourselves with `similar` from the raw file contents, so we never parse unified-diff text — the two concerns stay decoupled.

Rejected: `git2` (C build complexity, version skew with user's git), `gix` (pure Rust but young), parsing `git diff` output to reconstruct both sides (fragile edge cases).