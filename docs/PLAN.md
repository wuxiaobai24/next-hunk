# next-hunk 路线图

> 定位：**实用的 diff 查看器 + agent↔human 审查桥**。  
> 对标：[modem-dev/hunk](https://github.com/modem-dev/hunk)（体验与 agent 主路径，不是 monorepo 克隆）。  
> 差异化保留：紧凑 IR + 视口物化性能、`--select` 审批闸门、纯 Rust 静态分发。

当前版本：**0.4.0**。本文件是产品路线源；实现细节见 `ARCHITECTURE.md` / `PERF.md`。

---

## 0.8 对齐定义

**目标：** 日常审查 + agent 桥达到 hunk **主路径约 80% 可用性**。

| 算对齐 | 不算 0.8 范围 |
|--------|----------------|
| 大 diff 丝滑 review（已有基础，继续守门） | 复刻 OpenTUI / React 栈 |
| 两文件 diff、untracked worktree（可关） | jj / Sapling 一等支持 |
| 文件折叠 | 完整 HTTP/WS session-broker monorepo |
| 至少一种 split **或** stack 布局（auto 可选） | STML 富文本 note、可嵌入组件库 |
| 配置真接线（`line_numbers` 等，消灭 silent no-op） | Homebrew / 社区运营对标 |
| 轻量 live session CLI：list / review / navigate / comment / reload | MCP 全套运维面 |
| skill 与 hunk-review **工作流同构** | 像素级 UI 抄袭 |
| 保留并强化 `--select` + `decision` | Windows 完整 serve（可推 0.9） |

**明确不做（至少到 0.8）：** 完整 git 客户端、以二进制体积为 KPI、为抄布局破坏 IR 不变量。

---

## 里程碑

### 已交付（0.3 摘要）

- IR + viewport 二分、虚拟化多文件 TUI、gix worktree/staged/show、patch/pager
- 高亮（同步 viewport）、word-diff、搜索/过滤、hunk 导航、watch、主题 chrome、行号 gutter（运行时 `#`）
- Agent：`--focus` / `--note` / `--select`；`serve` + `push` / `decision`；skill
- Bench：parse + viewport；install.sh / musl dist

已知欠账（0.4 前要处理）：

- `config.line_numbers` 可解析但 **未进 `ResolvedConfig`**（silent no-op）
- 文档 phase 勾选过期（ARCHITECTURE 与现状漂移）
- 无两文件 diff / untracked / 折叠 / 真 split|stack
- Agent live 面远薄于 hunk（无 review 结构 inspect、comment CRUD、reload）

### 0.4 — 诚实补齐（先做）

- [x] `line_numbers` 配置接线，或从 config/README 删除
- [x] worktree diff 支持 untracked（可配置/可关）
- [x] `next-hunk diff a b` 两文件直比
- [x] 文件折叠（`zc`/`zo` 或等价）
- [x] README Status / 本文件与代码一致（ARCHITECTURE 勾选可另 PR）

### 0.5 — 布局与观感

- [x] 落地 **一种** 布局：优先 **split** 或 **stack**（先一种 + 窄终端可退化）
- [x] 布局变更不破坏 viewport-only 物化；过 PERF 门禁再考虑默认
- [x] light/dark 与 syntect 语法主题一致（至少 light 不再固定 dark 语法）
- [x] wrap / 长行策略可预期（截断或 wrap，配置生效）

### 0.6 — Agent session v2（轻量 live control）

在现有 `serve` + socket 上加厚 **CLI 语义**（不必上 HTTP broker monorepo）：

- [x] `list` / `get`：发现活会话
- [x] `review --json`：文件/hunk 结构（默认不含全文 patch）
- [x] `navigate`：file / hunk / line
- [x] `comment add|apply|list|rm`（可先无 markup）
- [x] `reload`：换 diff/show 内容且尽量保 focus/notes/decisions
- [x] 重写 `skill/next-hunk`：list → review → navigate → comment 工作流
- [x] 保留 `--select` / `decision` 为审批差异点

### 0.7 — 打磨

- [x] watch + session 语义不丢
- [x] 搜索/导航/状态提示与主路径一致
- [x] 公开一份 vs delta（或 vs hunk）的延迟/RSS 说明（可粗）
- [x] 异步高亮（gen-id 取消）按需

### 0.8 — 对齐验收（DoD）

**人用：**

- [x] untracked 可审可关
- [x] 两文件 diff
- [x] 文件折叠
- [x] 至少一种 split 或 stack，窄终端不炸
- [x] 配置字段无 silent no-op；主题与语法高亮不打架

**Agent：**

- [x] 人先开 TUI/serve
- [x] agent：`list` → `review --json` → `navigate` → `comment` → 可选 `reload` / `decision`
- [x] skill 与 hunk-review 步骤同构（命令名可不同）
- [x] `--select` 仍可用

**工程：**

- [x] huge fixture 打开/滚动仍过内部 gate
- [x] CHANGELOG 用户可见项齐全（0.4.0 的失实条目已于 Unreleased 修正）

---

## 2026-08 重评：对 hunk 0.20 实测（接手基线）

同一份 7783 行真实 diff，200×50 终端，next-hunk 0.4.0 release vs hunk 0.20.0
并排实测。结论：**引擎达标，阅读体验不成比例地落后**。上表 0.8 DoD
对照的是自设清单而非 hunk 实物，验收口径过松。

实测事实：

- 引擎：`inspect` 3 ms 冷启动（hunk 为 Node 运行时，`--version` 即 ~200 ms）；
  294 测试、clippy/fmt 三 OS CI 全绿 —— 这部分是资产，保留。
- 体验：hunk 默认 **side-by-side split + 上下文折叠**（`··· N unchanged
  lines ···`）+ 行内 agent 注释；next-hunk 默认 unified 全量平铺，无 split
  布局（只有 unified/stack），无上下文折叠，`--note` 只渲染为独立注释行。
- 信任：0.4.0 CHANGELOG 大段失实（已在 Unreleased 修正）；`mcp` feature
  为空壳（另行删除）。

### 0.9 — 成熟度（review-first 体验补课）

按体验收益排序，验收口径改为「与 hunk 0.20 并排同屏对比」：

- [ ] **上下文折叠**：连续 ≥N 行 context 折叠为 `··· N unchanged lines ···`
  标记行（默认开，`context_collapse` 可配，运行时可切）；搜索/hunk 跳转/
  focus 必须经折叠映射正确落点
- [ ] **split 布局**：side-by-side（宽终端），基于 hunk 内 old/new 行配对；
  保持 viewport-only 物化（PERF 门禁不回退）
- [ ] **`layout = "auto"`**：按流面板宽度选 split/stack/unified
- [ ] **行内 agent 注释**：`--note` / serve comment 渲染到对应代码行右侧，
  而非独立注释行
- [ ] 布局/折叠状态进 watch/reload 保序路径（与 decisions/folds 同批）
- [ ] CI 增加 bench 门禁（或从 ARCHITECTURE 撤回「regressions fail CI」承诺）

**继续不做：** 复刻 OpenTUI/React 栈、完整 broker/MCP 运维面、像素级 UI 抄袭、
以二进制体积为 KPI。

---

## 工作量粗估（单人全职，范围克制）

| 段 | 约 |
|----|----|
| 0.4 | 1–2 周 |
| 0.5 | 3–6 周（布局风险最高） |
| 0.6 | 3–5 周 |
| 0.7–0.8 | 2–4 周 |
| **合计** | **约 2.5–4 个月** |

若强上 jj/sl + 完整 broker + 双布局 auto 全开 → 明显超出 0.8。

---

## 风险

1. **为抄 split 破坏 IR** — 布局单独一层，继续只物化视口。  
2. **为抄 broker 上大 daemon** — 0.6 只加厚 CLI 语义。  
3. **jj/sl 挤进 0.8 关键路径** — 推后。  
4. **文档再漂移** — 以本文件为准，发版时同步勾选。

---

## 暂缓（0.8 后）

- jj / Sapling 适配  
- 完整 session-broker / MCP  
- STML / 富文本 note  
- OpenTUI 式可嵌入组件  
- parse fuzz、增量 IR 编辑（非全量 reload）  
- CLI「有 socket 就转发」无感切换（可评估，非必须）
