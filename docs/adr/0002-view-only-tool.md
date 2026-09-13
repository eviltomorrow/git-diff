# 0002: git-diff is view-only

The tool only reads git state (list, diff, log). It deliberately implements no write operations: no stage/unstage, checkout, commit, or config changes.

Scope discipline: the product is named for diffing and its value is in browsing changes. Staging/committing is a separate concern with real foot-guns (accidental `git add -A`, mid-conflict states), handled well by dedicated tools like lazygit. Reversing this is cheap if a future version wants actions, but the boundary keeps v1 small and safe — this is the explicit no that stops a future contributor from "helpfully" adding stage/commit.

Rejected: staging/checkout keys, an embedded commit workflow.