# Spec: git-diff TUI

Status: ready-for-agent

## Problem Statement

开发者想快速查看一个 git 仓库里"当前改了什么"，但不想在终端和编辑器之间来回切换、也不想开重的 GUI diff 工具。现有工具要么只给文本 diff（难扫），要么需要离开终端，要么功能太重。

## Solution

一个纯查看型的 TUI 工具：左侧是变更文件列表，右侧是原始侧/变更侧并排对齐的对比视图（VSCode diff view 风格）。支持四种比较模式，通过快捷键切换；支持从当前分支的 commit 列表选择一次提交，对比 HEAD 与它的差异。纯读操作，不提供任何 stage/commit/checkout 等写操作。

## User Stories

1. As a developer, I want to launch the tool in a git repo and immediately see the list of changed files for the default comparison mode, so that I can see what's different without running git commands manually.
2. As a developer, I want the file list to show each changed file with a status letter and add/delete line counts, so that I can gauge the scope of each change at a glance.
3. As a developer, I want the file list sorted by path, so that I can find a file predictably each time.
4. As a developer, I want status letters colored (M yellow, A/U green, D red, R cyan), so that I can quickly tell what kind of change each file is.
5. As a developer, I want to move the cursor through the file list with ↑/↓, so that I can browse through all changed files.
6. As a developer, I want the right-hand diff to switch to the file under the cursor immediately, so that I can scan changes quickly without extra keystrokes.
7. As a developer, I want the selected list row highlighted with a background color, so that I can track my position in the list.
8. As a developer, I want the default comparison mode to be mode A (working tree ↔ HEAD), so that I can see all uncommitted changes when I open the tool.
9. As a developer, I want mode A to include staged, unstaged, and untracked changes, so that "what I haven't committed" is a complete view.
10. As a developer, I want mode A to show untracked files as additions (U status, +N -0), so that they read consistently with added files.
11. As a developer, I want to press 1/2/3 to switch between modes A/B/C (working↔HEAD / staged↔HEAD / staged↔working), so that I can inspect different git states without leaving the tool.
12. As a developer, I want untracked files to appear only in mode A, so that staged/working comparisons stay faithful to git's own semantics.
13. As a developer, I want switching modes to immediately reload the list and show the diff for the current file (falling back to the first file if the current one no longer appears), so that the view never goes stale or empty.
14. As a developer, I want to press `l` to open a commit picker listing the current branch's commits (short hash, title, date, author), so that I can pick a commit to compare against.
15. As a developer, I want selecting a commit in the picker to switch to mode D (HEAD ↔ selected commit), so that I can review what changed between HEAD and that commit.
16. As a developer, I want to press `l` again while in mode D to reopen the picker and choose a different commit, so that I can iterate over commits easily.
17. As a developer, I want the picker to allow selecting HEAD itself, showing a "no differences" placeholder, so that there's no special-cased disabled entry.
18. As a developer, I want mode D to show only differences between the two commits, excluding untracked and staged state, so that the comparison stays purely between commits.
19. As a developer, I want the right-hand view to be a side-by-side aligned comparison (original side vs changed side) with line numbers on both sides, so that I can see exactly which lines changed and where they sit.
20. As a developer, I want changed lines highlighted at line level (additions in one color, deletions in another), so that I can spot changes fast.
21. As a developer, I want the two panes to scroll together, so that corresponding lines stay aligned while I scroll.
22. As a developer, I want a shared scrollbar for the diff area, so that I can tell how much content remains.
23. As a developer, I want long lines to be horizontally scrollable with Ctrl+←/→ (both panes moving together), so that line alignment is preserved and I can still read the full line.
24. As a developer, I want to jump to the next/previous change hunk with n/N, so that I can move between edit locations without reading every line.
25. As a developer, I want each diff pane to carry a header labeling which ref it shows (e.g. HEAD / working tree / staged / selected commit), so that I never misread which side is which.
26. As a developer, I want the top of the UI to show the current comparison mode and repo path, so that I always know which comparison I'm looking at.
27. As a developer, I want the status bar to show the current mode, current file's add/delete counts, and key hints, so that I have context and available actions in one place.
28. As a developer, I want added, deleted, and untracked files to show a "(no original content)" / "(no changed content)" placeholder on their empty side, so that half-empty views are clearly intentional.
29. As a developer, I want binary files to show a placeholder instead of attempting to render content, so that the tool never garbles output.
30. As a developer, I want to press `/` to filter the file list by path prefix, with the filter shown inline in the status bar, so that I can find a file in a long list.
31. As a developer, I want to press Esc to cancel the filter and restore the full list, keeping my cursor position sensible.
32. As a developer, I want to press `r` to refresh the change list while keeping the current mode and cursor (falling back to the first file if the current one disappears), so that I can pick up newly-made edits without losing my place.
33. As a developer, I want to press `?` to open a help overlay listing all keys, so that I can discover the interface.
34. As a developer, I want to see an empty-state message in the list and diff area when there are no changes, so that a clean repo reads clearly rather than looking broken.
35. As a developer, I want launching the tool in a non-git directory to fail fast with a clear error and non-zero exit, so that I'm not left in a broken TUI.
36. As a developer, I want launching the tool in a repo with no commits to treat mode A as "everything is untracked", so that I can still review my work in a brand-new repo.
37. As a developer, I want `q` and Ctrl+C to both exit cleanly restoring the terminal, so that my shell never gets left in a broken raw-mode state.
38. As a developer, I want rename detection enabled so renames show as R status with the diff rendered normally and the title showing the old name, so that renames read clearly.
39. As a developer, I want the file list to show only the new name for a renamed file, keeping the list single-column and sortable, so that the list stays tidy.
40. As a developer, I want files larger than a threshold (≈50k lines) to degrade to a plain full-text view without alignment highlighting, so that huge files don't freeze the UI.
41. As a developer, I want the tool to work from any subdirectory by resolving to the repo root, so that I get a whole-repo view no matter where I launch it.
42. As a developer, I want refresh failures (e.g. repo state changed) to keep the current view and surface an error in the status bar rather than exit, so that a transient failure never discards what I'm reading.

## Implementation Decisions

- **Single crate**: one binary using `ratatui` + `crossterm` + `similar`. No workspace split. Layout follows the minishell TUI conventions (top header + separator, left panel, bottom status bar with help hints, `SELECTED_BG`-style selection).
- **Comparison modes**: a mode enum with four variants — A (working tree ↔ HEAD), B (staged ↔ HEAD), C (staged ↔ working tree), D (HEAD ↔ selected commit). Each mode determines which ref the original side and changed side point at. Default is mode A. Keys `1`/`2`/`3`/`l` select modes; `l` opens the commit picker.
- **GitFacade (single seam)**: all git interaction lives in one module behind a command-runner trait. It exposes pure data: changed file list (path, status letter, add/delete counts), per-file side contents, commit list, and binary/untracked detection. Callers never parse git output themselves. Data comes from `git show`, `git diff --numstat`, `git diff --name-status`, `git log`, `git ls-files --others`. Per ADR-0001, we use the git CLI, never libgit2/gix.
- **Untracked handling**: untracked files appear only in mode A, treated as additions (U status, +N -0). Detected via `git ls-files --others --exclude-standard`.
- **Side-by-side diff**: `similar` `TextDiff::from_lines` computes line-level Equal/Delete/Insert groups. Left pane renders original lines (Delete/Equal), right pane renders changed lines (Insert/Equal). Line numbers shown on both sides. Each changed line carries a `-`/`+` prefix marker in a narrow gutter, and changed lines are highlighted with a **background color** (red-tinted background for deletions on the original side, green-tinted background for additions on the changed side), not just foreground color — matching editor-style diff views. No word/char-level highlighting. Files above the line threshold render both sides fully without alignment.
- **Empty-side placeholders**: added/deleted/untracked files get a "(no original content)" or "(no changed content)" placeholder on their empty side. Binary files (numstat shows `-`) get a "(binary file)" placeholder on both sides. Mode D comparing HEAD with itself shows a "no differences" placeholder.
- **Scrolling**: panes scroll in lockstep via one shared offset; `Ctrl+↑/↓` / `PageUp/PageDown` scroll the diff area, `Ctrl+←/→` scroll horizontally (both panes together). `↑/↓` always move the file cursor — there is no panel focus switching. A shared scrollbar renders on the right edge of the diff area. Hunk jumping (`n`/`N`) moves the scroll offset between `similar` change groups.
- **Layout**: left file list 30%, right diff area 70%, fixed ratio. Top: mode description + repo path + separator. Diff panes each have a header naming their ref. Bottom: status bar (mode, current file counts | key hints).
- **File list**: columns are status letter + file name + add/delete counts. The list is a **collapsible tree** (minishell filebrowser style): directories are nodes shown as `📁 <dir>/` with an empty add/delete column; changed files sit indented under their directory. `→` expands a directory, `←` collapses it and moves the cursor to the parent directory row (minishell behavior). Directories sort before files within each level, both alphabetical. Default state is all expanded; `r` refresh keeps the current expand state. Colors: M yellow, A/U green, D red, R cyan, selected row blue background. Renamed files show only the new name in the list; the old name appears in the diff pane header. `/` filters by path prefix with the filter inline in the status bar; while filtering, all directories are temporarily expanded so matches are always visible, and Esc cancels the filter and restores the prior expand state.
- **Commit picker**: lists current branch commits (`short hash`, title, date, author) in a floating overlay that locks all other input while open. `↑/↓` to select, Enter to confirm, Esc to close without switching mode. Selecting HEAD is allowed and yields a no-difference view. Mode D excludes untracked and staged state.
- **Startup/edge behavior**: launch resolves the repo root from any subdirectory (whole-repo view, no directory filtering). Non-git directory → error message + non-zero exit. Empty repo → mode A shows all files as untracked; modes B/C/D show "no comparison available". Startup mode is always A; no persisted state.
- **Refresh & failure**: `r` reloads keeping mode and cursor (fall back to first file). Refresh failures keep the current view and show an error in the status bar. Loading indicator ("loading...") is shown transiently for slower operations (commit picker, first load).
- **Exit**: `q` and Ctrl+C both restore the terminal cleanly (crossterm raw mode disabled via guard/drop).
- **View-only**: per ADR-0002, no write operations are implemented — no stage/unstage, checkout, commit, or config changes.

## Testing Decisions

- **Single seam**: tests target the GitFacade boundary — the pure-data module behind the command-runner trait. The trait is stubbed with fixed git-output fixtures (numstat/name-status/log/ls-files/show output), covering format edge cases: rename rows, untracked rows, binary rows, empty repos, no-newline-at-EOF, deleted/added files.
- **What makes a good test**: only external behavior of the seam — given stubbed git output, the facade returns the correct changed-file list, status letters, add/delete counts, side contents, and commit list. No TUI state or rendering is exercised in tests; no real git is invoked.
- **Which modules are tested**: the GitFacade module (parsing + assembly into pure data). The line-alignment logic built on `similar` is tested via the same seam (fixtures in, aligned group structure out).
- **Prior art**: this repo has no existing tests. The seam pattern (trait-injected command runner returning fixture data) mirrors the minishell codebase's separation of data providers from TUI rendering, and keeps the renderer manually testable.
- **Not tested**: ratatui rendering and key handling are manually tested.

## Out of Scope

- Write operations (stage, unstage, checkout, commit, stash) — ADR-0002.
- Character/word-level inline diff highlighting (future iteration).
- Draggable/resizable panel widths; multi-commit range selection (start..end) in the picker; comparing arbitrary refs other than HEAD↔selected commit.
- Directory/subtree filtering and viewing changes for a single directory.
- Auto-refresh on filesystem change; persisted UI state or last-used mode.
- Support for non-git remotes/workflows (the tool is local-only).
- Rendering tests for the TUI layer.

## Further Notes

- Spec was produced from a grilling/domain-modeling session. The domain glossary lives in `CONTEXT.md` (比较模式, 原始侧, 变更侧, 变更文件, 状态字母, 未跟踪); ADRs in `docs/adr/0001-git-cli-as-data-source.md` and `docs/adr/0002-view-only-tool.md`.
- The minishell codebase (crates/minishell-tui) is the reference for TUI conventions: layout, styles, status-bar help hints, inline input handling, floating overlay panels.
- Line threshold for the no-alignment degrade path is ~50k lines; tune during implementation.