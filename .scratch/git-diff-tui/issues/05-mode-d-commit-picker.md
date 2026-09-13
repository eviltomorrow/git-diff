# 05 — 模式 D commit 选择器

**What to build:** `l` 弹出当前分支 commit 列表浮层，选中一个 commit 后切换为模式 D（HEAD↔选中 commit）对比；再按 `l` 换 commit。允许选中 HEAD 显示"无差异"占位；D 模式排除未跟踪与暂存状态。

**Blocked by:** 02 — 模式 A 并排对比视图

**Status:** ready-for-agent

- [ ] `l` 弹出浮层，列出当前分支 commit：短 hash + 标题 + 日期 + 作者
- [ ] 浮层打开时锁定其他输入；`↑/↓` 选择、Enter 确认、Esc 关闭（不退出 D）
- [ ] 确认后进入模式 D：原始侧 = 选中 commit，变更侧 = HEAD
- [ ] 选中 HEAD 自身时对比区显示 "no differences" 占位
- [ ] D 模式只显示两次提交间的差异，排除未跟踪与暂存区内容
- [ ] 模式 D 下再按 `l` 重开列表更换 commit
- [ ] 顶部标题与两栏栏头显示 HEAD ↔ 选中 commit（短 hash）
- [ ] GitFacade `git log` 输出解析经同一条测试缝用 fixture 覆盖（含日期/作者格式）