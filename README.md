# git-diff

一个终端里的 git 差异查看器（TUI）。左侧是变更文件列表（可折叠树形），右侧是原始侧 / 变更侧并排对齐的对比视图。

## 安装

```bash
cargo build --release
# 可选：部署到 Applications
just deploy
```

## 使用

```bash
git-diff                # 默认模式 A：工作区 ↔ HEAD
git-diff <commit-id>    # 直接进入模式 D：HEAD ↔ 该 commit
```

在任意 git 仓库内运行即可，工具会自动定位到仓库根目录。

## 比较模式

| 键 | 模式 | 对比内容 |
|----|------|----------|
| `1` | A | 工作区 ↔ HEAD（未提交的全部改动） |
| `2` | B | 暂存区 ↔ HEAD |
| `3` | C | 暂存区 ↔ 工作区 |
| `4` | D | HEAD ↔ 选中的 commit |

## 快捷键

| 键 | 功能 |
|----|------|
| `Tab` | 切换焦点（文件列表 / 对比区） |
| `Enter` | 查看选中文件对比（文件列表焦点） |
| `↑↓` | 移动光标 / 逐行滚动（对比区焦点） |
| `PgUp/PgDn` | 翻页 |
| `1 / 2 / 3 / 4` | 切换比较模式 |
| `l` | 换一个 commit |
| `/` | 过滤文件列表（输入后 Enter 确认，Esc 取消） |
| `n / m` | 下一个 / 上一个 hunk |
| `z` | 折叠 / 展开未改动大段 |
| `r` | 刷新 |
| `?` | 帮助 |
| `q` / `Ctrl+C` | 退出 |

## 功能特性

- 并排 diff 视图：行号、`-`/`+` 标记、增删背景高亮
- 行内字符级高亮：改动行内精确标出变化的字符片段
- 可折叠树形文件列表：目录折叠、路径过滤
- 大段未改动区域折叠（`z`）
- 状态栏显示当前模式、hunk 进度、光标位置
- 各比较模式独立记忆光标、折叠、diff 滚动状态

## 开发

```bash
just check    # 快速编译检查
just test     # 运行测试
just lint     # clippy（deny warnings）
just ci       # check + test + lint + fmt-check
```