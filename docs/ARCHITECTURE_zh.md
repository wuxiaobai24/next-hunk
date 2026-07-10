# next-hunk 架构

[English](./ARCHITECTURE.md) | **中文**

## 0. 产品定位

**next-hunk** 是面向超大 changeset 的高性能终端 **review 引擎**。

目标是在四个支柱上超越 pager 类工具与沉重的 JS/TS TUI 运行时：

| 支柱 | 承诺 |
|------|------|
| **性能** | 仅视口渲染、紧凑运行时 IR、硬性 bench 门禁 |
| **规模** | 多文件 review 流，不为每一行物化 widget |
| **体验** | 交互式多文件导航、可读布局、可脚本化的 CLI |
| **Agent 时代** | 面向人类与 coding agent 的结构化导出 |

一句话：

> 更快地 review 超大 diff —— 为 agent 时代而建。

### 设计纠偏（重要）

**二进制体积不是产品目标。** 早期文档曾把「小体积 / musl 静态 / 禁 libgit2」写成支柱，这是误区：

| 真正在意的 | 不是产品目标的 |
|------------|----------------|
| 大 diff 下的 **滚动延迟、启动延迟、RSS** | strip 后二进制是否 < 15MB |
| 紧凑 **运行时 IR**（避免整份 widget 树） | 依赖数量最小化 |
| 正确、可扩展的 source / 高亮 / 导出能力 | 为静态链接而拒绝合适库 |

技术选型以 **正确性、可维护性、热路径性能** 为准，不为「看起来很轻」牺牲能力。

---

## 1. 目标与非目标

### 1.1 目标

- **快**：冷启动、滚动、切文件有硬指标  
- **大**：数万级 diff 行不 OOM、不卡死数秒  
- **清**：数据面（IR）与 UI 严格分离；可脚本、可度量  
- **好用**：多文件 rail + 连续 stream，键位清晰  
- **对 agent 有用**：结构化导出是一等公民产品面  

### 1.2 非目标（至少早期 0.x）

| 非目标 | 原因 |
|--------|------|
| 像素级复刻某个现成 TUI | 偏离 review 引擎定位 |
| 完整 git 客户端（stage/commit/rebase 全家桶） | 那是 lazygit / gitui 的战场 |
| 默认对整文件开语法高亮 | 吃掉滚动预算（可异步、可取消、仅视口） |
| 第一天 jj / Sapling 对等 | 适配器后挂 |
| 以 session daemon / MCP 为核心 | 复杂度爆炸 |
| 以二进制体积为成功标准 | **不是需求**；发布体积仅作观测项 |

---

## 2. 系统架构

```text
┌──────────────────────────────────────────────────────────────────┐
│                         CLI (clap)                                │
│   diff | show | patch | bench | export（后续）                      │
└────────────────────────────┬─────────────────────────────────────┘
                             │
                             ▼
┌──────────────────────────────────────────────────────────────────┐
│                    Source 适配器（懒、可替换）                        │
│   gix（唯一 git 后端）  │  patch 文件/stdin  │  两文件            │
└────────────────────────────┬─────────────────────────────────────┘
                             │ unified 文本 / blob 引用
                             ▼
┌──────────────────────────────────────────────────────────────────┐
│                      Diff IR（核心资产）                            │
│  text_arena + File[] + Hunk[] + line spans                       │
│  stream_len + 每文件 stream 区间                                   │
│  禁止：整份 review 的 Vec<WidgetRow>                               │
└───────────────┬─────────────────────────────┬────────────────────┘
                │                             │
                ▼                             ▼
┌───────────────────────────┐   ┌──────────────────────────────────┐
│ Viewport 查询             │   │ 旁路服务（可取消）                  │
│ O(visible ± overscan)     │   │ 高亮 │ 搜索 │ 导出 │ watch      │
│ file_at_row / rows()      │   │ generation id 作废过期任务         │
└─────────────┬─────────────┘   └──────────────────────────────────┘
              │
              ▼
┌──────────────────────────────────────────────────────────────────┐
│ TUI (ratatui) — immediate / 稀疏 retained                         │
│  文件 rail  │  连续 stream  │  状态栏 / help                       │
│  输入路径：同步且短；滚动热路径禁止 await                           │
└──────────────────────────────────────────────────────────────────┘
```

### 2.1 分层职责

| 层 | 职责 | 禁止 |
|----|------|------|
| **CLI** | 参数、子命令、退出码 | 堆业务解析 |
| **Source** | 产出 unified / 原始字节 | 懂 TUI |
| **IR** | 唯一真相：索引 + 紧凑行数据 | 持有 Color / Style / Widget |
| **Viewport** | 为窗口物化 `StreamRow` | 每帧全表扫描（构建索引除外） |
| **TUI** | 绘制、键位、scroll 状态 | 持有第二份全量行缓存 |
| **Services** | 高亮、搜索、导出 | 阻塞 UI 线程 |

### 2.2 Diff IR（草图）

```text
Review {
  text_arena: String,              // 行/header 文本共享存放
  files: [FileDiff],
  stream_len: usize,               // 虚拟总行数
  hunk_starts: [usize],            // 各 hunk header 的绝对行 → ]h/[h 二分定位
}

FileDiff {
  display_path,
  hunks: [Hunk],
  stream_start, stream_len,        // 在全局 stream 中的区间 → 二分
}

Hunk { header_span, old/new range, lines: [DiffLine] }
DiffLine { kind: Context | Add | Delete | Meta, text_span }
```

**「紧凑 IR」指运行时内存模型**，不是发布包大小：一份 arena + span，避免为整份 review 复制带样式的行结构。

**展平 stream 顺序**（必须与 TUI 一致）：

```text
[FileHeader] [HunkHeader] [Line...] [HunkHeader] [Line...] ... 下一文件 ...
```

### 2.3 性能原则

1. **索引 ≠ 内容** —— 先 span，再按需取字节  
2. **视口是唯一热路径** —— 装饰只针对 visible ± overscan  
3. **后台可取消** —— 高亮带 `generation`；滚动丢弃过期任务  
4. **同步路径保持短** —— 按键 → 改 scroll/focus → draw  
5. **默认走 fast** —— 高亮 / word-diff 可选或空闲补全  
6. **测量驱动** —— fixture + bench；回归失败  
7. **Git 用 gix** —— 进程内读库与算 diff，不 spawn `git` CLI；无 subprocess fallback

### 2.4 仓库布局

```text
next-hunk/
  Cargo.toml
  README.md / README_zh.md
  docs/
    ARCHITECTURE.md / ARCHITECTURE_zh.md
    PERF.md / PERF_zh.md
  src/
    main.rs              # CLI 入口
    lib.rs               # 库根：ir / source / tui …
    ir/                  # model, parse, viewport
    source/              # git, patch, files
    tui/                 # app, rail, stream, keys, watch
    config.rs            # 分层 config.toml（用户 + 项目）
    export/              # 后续：json / markdown
    highlight/           # 后续：异步 syntect
  benches/
  fixtures/
  scripts/
```

先单 package（lib + bin）；仅在编译时间或复用边界吃痛时再拆 crate。

---

## 3. 技术选型（0.x 方向）

| 关注点 | 选择 | 备注 |
|--------|------|------|
| 语言 | **Rust** | 无 GC，可控内存与延迟 |
| TUI | **ratatui + crossterm** | 先稳；框架成瓶颈再议 |
| 行级 IR | **自研 unified-diff 解析** | 完全控制布局与性能 |
| 字级 diff | **similar**（后续，仅视口） | |
| Git | **gix（gitoxide）** | 唯一后端；无 CLI fallback |
| 高亮 | **syntect** 或等价（Phase 4，异步） | 默认关或空闲；依赖体积不设门禁 |
| CLI | **clap** | |
| 错误 | **anyhow** / **thiserror** | |
| 发布 | 常规 release；体积仅观测 | 见 PERF |

选型原则：**能力与热路径性能优先**。不把「依赖少 / 二进制小 / 全静态」当成架构约束。

---

## 4. 竞品坐标

```text
          轻量 pager                         完整 git 客户端
                │                                 │
     delta ─────┼────────────────── gitui/lazygit ─┤
                │                                 │
                │      ★ next-hunk                │
                │      review 引擎                │
                │      大 diff + agent 导出       │
                │      可测延迟、可交互            │
```

- **相对 delta**：交互式多文件 review + 结构化导出  
- **相对 gitui/lazygit**：不做完整客户端，专精 review，在超大 diff 上更快  

（**不对「二进制体积」做竞品 KPI。**）

---

## 5. 开发计划

### Phase 0 — 基线与骨架（约 0.5 周）

- [x] 定名：`next-hunk`
- [x] 仓库骨架 + 本文档
- [x] `docs/PERF.md` + fixture 策略
- [x] IR parse + 单元测试（库入口已接上）
- [ ] 可选：对 delta（或同类工具）采一次对比数字

**退出：** 能解析 patch；文档齐；bench 入口有定义。

### Phase 1 — 引擎可证伪（约 1 周）

- [x] 稳健 unified parse（rename、binary 占位、no newline）
- [x] `ViewportQuery`：按文件 span 二分（已有初版）
- [x] Source：gix worktree / staged / show / range（patch 文件 / stdin 已可走 CLI）
- [x] Bench：parse + viewport 物化

**退出：** huge fixture 解析不 OOM；随机 viewport 查询达到 [PERF_zh.md](./PERF_zh.md) 门禁。

### Phase 2 — TUI MVP（约 1–1.5 周）

- [x] ratatui：左文件 rail + 右连续 stream
- [x] 虚拟滚动（`scroll_y` + viewport）
- [x] 键位：j/k、下一/上一文件、g/G、q、Tab
- [x] 状态栏：文件数 / 位置 / 模式
- [x] 空 diff、非 git 仓库友好提示
- [x] `next-hunk` / `next-hunk diff` 默认可交互

**退出：** medium 可日常用；huge 能打开能滚（高亮可选）。

### Phase 3 — 产品完整度（约 1–2 周）

- [ ] `show` / `patch -` 打通
- [ ] staged + git 额外参数透传
- [ ] 可选两文件 diff
- [x] 轻量搜索（路径过滤和/或流内 `/`）
- [ ] 最小配置（颜色、rail 宽度）

**退出：** 可替代「delta + 手翻文件」主路径。

### Phase 4 — 差异化（约 2–3 周）

- [ ] Agent 导出：文件 / 选区 / 全 review → Markdown + JSON
- [ ] 简单本地 note（行/文件）
- [x] 语法高亮（syntect，仅视口 + 缓存，默认开启）
- [ ] 异步 syntect（可取消，默认关或空闲）— 当前为同步视口实现
- [ ] **仅视口内** word-level diff
- [ ] 公开对比说明（与 delta 或同类工具公平可比时：延迟 / RSS，非体积）

**退出：** 叙事从「快」升级到「agent 时代 review 引擎」。

### Phase 5 — 硬化（持续）

- [ ] Side-by-side（独立性能设计；未过门禁不做默认）
- [ ] Watch / 增量 IR 刷新
- [ ] jj 适配器
- [ ] 主题、help overlay
- [ ] Fuzz parse；更多真实仓库回归

---

## 6. 流程规则

```text
建议周节奏：
  前半周：引擎 / 正确性 / bench
  后半周：TUI / 体验
  周末前：跑 fixture 门禁；更新 PERF 数字

PR 规则：
  - 动 IR / viewport → 必须有 test 或 bench
  - 滚动热路径：禁止 await、禁止整文件高亮
  - 新功能默认关闭或离开热路径，直到有数据
  - 不为「减小二进制」拒绝合理依赖
```

---

## 7. 风险与缓解

| 风险 | 缓解 |
|------|------|
| 功能追着所有 git TUI 跑 | 锁死非目标；导出优先于视觉对齐 |
| 先 UI 后虚拟化 | Phase 1 门禁挡住 Phase 2 打磨 |
| 极端超长行 | 先截断，横向滚动后做；先保住纵向滚动 |
| 老终端兼容 | 真机 TERM 矩阵；不靠「必须 musl」定义成功 |
| Parse 边界 | 金样 patch + 后期 fuzz |
| 误把体积当目标 | 本文「设计纠偏」+ PERF 不设 binary 门禁 |

---

## 8. 成功长什么样

| 阶段 | 对外一句话 |
|------|------------|
| Phase 2 | 「`next-hunk` 能顺滑滚超大 diff」 |
| Phase 4 | 「可测延迟、一键给 agent 的结构化导出」 |
| 长期 | 品类是 **review 引擎**，不是又一个 git TUI，也不是「最小体积 diff 工具」 |

---

## 9. 相关文档

- [PERF_zh.md](./PERF_zh.md) — fixture、指标、CI 门禁（[EN](./PERF.md)）
- [../README_zh.md](../README_zh.md) — 用户向概览（[EN](../README.md)）
