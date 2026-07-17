# next-hunk

**更快地 review 超大 diff，为 agent 时代而建。**

[English](./README.md) | **中文**

面向超大 changeset 的高性能终端 **review 引擎**。

| 支柱 | 承诺 |
|------|------|
| **性能** | 仅视口渲染、紧凑运行时 IR、硬性 bench 门禁 |
| **规模** | 多文件 review 流，不为每一行建 widget |
| **体验** | 交互式多文件导航、可读布局、可脚本化 CLI |
| **Agent 时代** | 终端原生、可脚本化，面向人类与 coding agent（路线图） |

**二进制体积不是产品目标。** 我们优化的是大 diff 下的延迟与运行时内存，不是「谁更小谁赢」。

## 命名

**next-hunk** —— 按文件与 hunk 导航 review 流。CLI：`next-hunk`。以后可加短别名（如 `nh`）。

## 状态

`v0.4-dev` — 日常 review diff 可用：

- [x] 项目骨架 + 紧凑 unified-diff IR（运行时模型）
- [x] 视口查询（基于文件 span 的二分查找）
- [x] 虚拟化多文件 TUI（ratatui）
- [x] gix 驱动的 worktree / staged / show（+ patch stdin）
- [x] 健壮的 unified 解析（重命名、二进制占位、无换行符、CRLF）
- [x] 基准测试：解析 + 视口物化
- [x] 语法高亮（syntect，仅视口 + 缓存，默认开启）
- [x] 搜索：stream 内 `/` 内容搜索 + 文件栏 `f` 路径过滤
- [x] Hunk 跳转：`]h` / `[h` 下一个/上一个 hunk（二分定位 hunk 索引）
- [x] Watch 模式：`--watch` 实时重载（notify，debounce；保持滚动/选中）
- [x] Pager 模式：`next-hunk pager` 作 git 的 `core.pager`
- [x] `o` 打开到编辑器：跳转到光标行
- [x] 状态栏 diff 统计（per-file + 全局 `+ins/−del`）
- [x] 忽略空白开关（`W`，折叠仅空白变化）
- [x] Agent 桥梁：`--focus` 启动定位、`--note` 注解、`--select` 逐 hunk 审批闸门
- [x] Server 模式：`next-hunk serve` + `push`/`decision`，支持 agent→human 实时流式推送常驻 TUI
- [x] Jujutsu (jj) 一等支持：自动探测、`vcs` / `--vcs`、`jj diff --git` → 同一 IR（`docs/VCS.md`）
- [x] `line_numbers` 配置生效（非 silent no-op）
- [x] `include_untracked` 配置 + `--include-untracked` 参数（默认关闭）
- [x] Working-set 全量审查：`diff --all` / `scope = "working-set"`（staged+unstaged；文件栏 `S`/`M`/`?`）
- [x] `next-hunk filediff <旧文件> <新文件>` — 对比磁盘上两个任意文件
- [x] 文件折叠/展开：`zc`（收起）/ `zo`（展开）
- [x] Stack 布局：`layout = "stack"` 配置（默认 unified）
- [x] Split 布局：`layout = "split"` / `--layout split` 左右分栏
- [x] 折行配置：`wrap = true` 折行显示（默认截断）
- [x] 异步语法高亮（后台 worker + gen-id 拒绝过期结果；miss 先 plain）
- [x] 对常见工具（如 delta）的公开性能说明（设计对比 + bench 见 `docs/PERF.md`）

## 安装

```bash
# 从 GitHub 安装（当前发布渠道）
cargo install --git https://github.com/wuxiaobai24/next-hunk
# 或本地克隆后安装
cargo install --path .
# 或直接运行
cargo run --release -- diff
```

### 预编译静态二进制（musl）

每个带 tag 的 release 都会发布一个**全静态、全特性**的 x86_64 musl 二进制
—— 单个 ~2.6 MB（xz）文件，无任何运行时依赖，可直接在任何 Linux 上运行
（Alpine、distroless、老版本 glibc 等），无需装 Rust 或 C 库。从
[Releases 页](https://github.com/wuxiaobai24/next-hunk/releases) 下载：

```bash
# 以 v0.1.0 为例（URL/版本按实际调整）：
curl -L https://github.com/wuxiaobai24/next-hunk/releases/latest/download/next-hunk-0.1.0-x86_64-musl.tar.xz \
  | tar -xJ
sudo install -m 0755 next-hunk-0.1.0-x86_64-musl/next-hunk /usr/local/bin/
next-hunk --version
```

#### 自己编静态二进制

next-hunk 是纯 Rust（用 gix 而非 libgit2、syntect 的 default-fancy regex、
`zlib-rs`），**不需要 C 交叉工具链**，只要 musl 的 rust-std target：

```bash
rustup target add x86_64-unknown-linux-musl
cargo build --profile dist --all-features --target x86_64-unknown-linux-musl
# → target/x86_64-unknown-linux-musl/dist/next-hunk （静态链接）
ldd target/x86_64-unknown-linux-musl/dist/next-hunk   # 输出 "statically linked"
```

`dist` profile（fat LTO + strip + `panic=abort`）产出 ~7 MB 二进制
（xz 后 ~2.6 MB）。普通 `--release` 构建仍以运行速度为优先。

### 设为 git 的 pager（推荐）

安装后，让日常 `git diff` / `show` / `log` 直接打开 review TUI：

```bash
git config --global core.pager "next-hunk pager"
```

## 用法

```bash
next-hunk                  # 工作区 diff（仅 unstaged）
next-hunk diff --staged    # 仅 staged（`git diff --cached`）
next-hunk diff --all       # 完整工作集：staged + unstaged
next-hunk diff --all --include-untracked  # git status 里能看到的本地改动全量
next-hunk diff --watch     # 文件变化时实时重载
next-hunk diff --include-untracked  # 包含未跟踪文件（worktree / --all）
next-hunk filediff old.rs new.rs    # 对比磁盘上两个文件
next-hunk show HEAD
git diff | next-hunk patch -
next-hunk inspect path/to.patch   # IR 摘要，不开 TUI（脚本用）
next-hunk inspect --all --include-untracked  # 脚本：列出全部本地桶

# 把 next-hunk 设为 git 的 pager，日常 diff/show/log 直接进 TUI：
git config core.pager "next-hunk pager"
git diff        # → 启动 review TUI
git show HEAD   # → 启动 review TUI
```

### 快捷键

| 按键 | 动作 |
|-----|------|
| `j` / `↓` | 向下滚动一行 |
| `k` / `↑` | 向上滚动一行 |
| `J` / `PgDn` | 向下滚动半屏 |
| `K` / `PgUp` | 向上滚动半屏 |
| `Ctrl-D` / `Ctrl-F` | 向下滚动（半屏 / 整屏） |
| `Ctrl-U` / `Ctrl-B` | 向上滚动（半屏 / 整屏） |
| `g` / `Home` | 跳到顶部 |
| `G` / `End` | 跳到底部 |
| `]h` | 下一个 hunk（跨文件回绕） |
| `[h` | 上一个 hunk（跨文件回绕） |
| `Space` | 下一个 hunk（`]h` 的快捷单键） |
| `zc` | 折叠（收起）当前文件 |
| `zo` | 展开当前文件 |
| `Tab` / `l` / `→` | 下一个文件 |
| `Shift+Tab` / `h` / `←` | 上一个文件 |
| `1`–`9` | 跳到第 N 个文件 |
| `b` | 切换文件侧边栏显示 |
| 点击文件栏 | 选中该文件 |
| 点击 diff 区 | 把视口定位到该行 |
| `H` | 切换语法高亮 |
| `#` | 切换行号显示 |
| `w` | 切换词级行内 diff |
| `W` | 切换忽略空白（隐藏仅空白变化） |
| `t` | 循环主题：light → auto → dark |
| `/` | 搜索 diff 内容（`n`/`N` 下一个/上一个） |
| `f` | 按路径子串过滤文件栏 |
| `o` | 在 `$EDITOR` 中打开当前行（跳到那一行） |
| `?` | 切换全屏快捷键帮助 |
| `a` / `r` / `u` | `--select`：接受 / 拒绝 / 未决 当前 hunk（自动跳下一处） |
| `A` / `R` | `--select`：接受 / 拒绝 当前文件剩余 hunk |
| `Ctrl-A` / `Ctrl-R` | `--select`：接受 / 拒绝 从当前位置起全部剩余 hunk |
| `q` / `Esc` / `Ctrl+C` | 退出（`Esc` 先清除激活的搜索） |

## 配置

把偏好写进 `config.toml`,免去每次敲 flag。两层配置,与 CLI flag 合并(优先级从高到低):

```text
CLI flag  >  .next-hunk/config.toml（项目）  >  ~/.config/next-hunk/config.toml（用户）  >  默认
```

字段:

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `scope` | string | `"worktree"` | `"worktree"`（unstaged）、`"staged"`、或 `"working-set"`（staged+unstaged；CLI `--all`） |
| `staged` | bool | `false` | 兼容别名：未设置 `scope` 时 `staged = true` 等价于 `scope = "staged"` |
| `highlight` | bool | `true` | 语法高亮 |
| `watch` | bool | `false` | 文件变化时实时重载 |
| `line_numbers` | bool | — | 显示 old/new 行号 gutter（`#` 运行时切换） |
| `include_untracked` | bool | `false` | 在 worktree / working-set diff 中包含未跟踪文件（`--include-untracked`） |
| `layout` | string | `"unified"` | `"unified"`（默认，交错显示）、`"stack"`（每文件旧/新上下两块）、或 `"split"`（左右分栏；流区 <80 列退化为 stack，<40 列退化为 unified） |
| `wrap` | bool | `false` | 在 diff 区折行显示长行（默认截断） |
| `export_on_quit` | string | `"none"` | 退出 TUI 时导出 agent 可读报告：`"none"` / `"json"` / `"markdown"` / `"both"`（`--export-on-quit`） |
| `vcs` | string | `"auto"` | `"auto"`（有 `.jj` 时优先 jj）/ `"git"` / `"jj"`，见 [`docs/VCS.md`](./docs/VCS.md) |
| `theme` | string | `"light"` | `"dark"` / `"light"` / `"auto"`（`t` 循环切换）。调色板为 [Flexoki](https://flexoki.com)。 |

示例 `~/.config/next-hunk/config.toml`:

```toml
highlight = true
watch = true
```

CLI 覆盖:`--all` / `--staged`（互斥）、`--watch`、`--no-highlight`、`--include-untracked`、`--layout <unified|stack|split>`、`--export-on-quit <none|json|markdown|both>`。

使用 working-set（`--all` 或 `scope = "working-set"`）时，文件栏会标注来源：**`S`** staged、**`M`** modified（unstaged）、**`?`** untracked。同一路径若既有 staged 又有 unstaged，会各出现一次。

## Agent 集成

next-hunk 把 coding agent 的改动桥接给人类审查者。Agent 调 CLI,人拿到一个指向关键位置的交互式 TUI。

**展示改动(无需反馈):**

```bash
next-hunk diff \
  --focus src/auth.rs:42 \
  --note src/auth.rs:42="把 token 校验抽成了独立函数" \
  --note banner="Auth 重构 —— 核心是校验逻辑的拆分"
```

- `--focus <path>[:<line>|:h<n>]` —— 启动时滚动到某文件 / 行 / hunk。
- `--note <target>=<text>` —— agent 注解(可重复):`<path>:<line>`、`<path>:h<n>` 或 `banner=<摘要>`。渲染在 TUI 里。

**获取逐 hunk 审批(`--select`):**

```bash
next-hunk diff --select --focus src/db/migrate.rs:140 \
  --note src/db/migrate.rs:140="删掉旧列 —— 不可逆"
# 阻塞到人退出;stdout 随后输出一行 JSON:
# {"accepted":["src/db/migrate.rs:h1"],"rejected":[...],"undecided":[...]}
```

`--select` 模式下,人按 `a`(接受)/ `r`(拒绝)/ `u`(未决)逐 hunk 决策;退出时把决策以 JSON 输出供 agent 解析。`--select` 需要交互式终端,否则报错。

### Agent skill

仓库内置一个现成 skill(`skill/next-hunk/SKILL.md`),教 coding agent 何时、如何调用 next-hunk —— 把它装进你的 agent skills 目录即可。完整决策指南和示例见 skill 文件。

### Server 模式（常驻 TUI + 实时推送）

默认是**无状态**的（每次 `next-hunk diff` 都是一次性进程）。可选 **server 模式**让 agent 把多次更新流式推送到单个常驻 TUI，并实时读取人的决策，无需每次交互重启进程：

```bash
# 人打开常驻审查 TUI（select 模式自动开启）：
next-hunk serve

# agent 把新的 focus/note 推送到常驻 TUI（立即返回）：
next-hunk push --focus src/auth.rs:88 --note banner="请检查 token 过期"

# agent 读取人累积的逐 hunk 决策（一行 JSON，立即返回）：
next-hunk decision
# {"accepted":["src/auth.rs:h1"],"rejected":[...],"undecided":[...]}
```

`serve` 绑定一个由仓库根目录派生的 Unix socket，所以 `push`/`decision` 在同仓库任意位置运行都能自动找到它 —— 无需 `--socket` 参数。需要 `serve` 特性（默认开启）和 Unix 系统；其它构建下子命令会报告不可用。`decision` 输出与 `--select` 退出时的格式一致，所以 agent 可以用同一套逻辑解析。

## 测试与基准

```bash
# 单元 + 集成 + 无头 TUI 测试
cargo test

# 生成 fixture（small / medium / huge）
./scripts/gen_fixtures.sh

# 基准测试（PERF.md 指标）
cargo bench --bench parse
cargo bench --bench viewport
```

## 架构（简）

```
git/patch ──► 运行时 IR（字节/行 span） ──► 视口查询 ──► TUI
```

绝不为一整份 review 的每一行 diff 建完整 widget 树。IR 是唯一真相；UI 只物化屏上内容（+ 少量 overscan）。

**完整文档：**

| 文档 | 内容 |
|------|------|
| [docs/ARCHITECTURE_zh.md](docs/ARCHITECTURE_zh.md) | 定位、分层、IR、阶段、风险（[EN](docs/ARCHITECTURE.md)） |
| [docs/PERF_zh.md](docs/PERF_zh.md) | Fixture、指标、门禁、反模式（[EN](docs/PERF.md)） |

## 许可证

MIT
