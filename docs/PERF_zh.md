# next-hunk 性能

[English](./PERF.md) | **中文**

性能是 **产品特性**，不是后期再优化的附属品。  
性能相关声明必须来自本文 fixture 与门禁上的数字。

相关：[ARCHITECTURE_zh.md](./ARCHITECTURE_zh.md)（[EN](./ARCHITECTURE.md)）。

**范围：** 门禁只覆盖 **延迟与运行时内存（RSS）**。二进制体积、依赖数量、是否 musl 静态 **不是门禁**（最多在 PR 里顺带记录）。

---

## 1. 原则（摘要）

1. 紧凑 **运行时 IR** 是唯一真相；UI 从不为每一行持有完整 widget 列表。  
2. 每帧只物化 **viewport ± overscan**。  
3. 滚动/输入路径 **同步且短**。  
4. 高亮 / word-diff / 搜索是 **可取消的旁路服务**。  
5. 每次 IR 或 viewport 变更都应 **可测**。  
6. Git 使用 gix，不依赖 `git` 子进程；依赖以延迟 / RSS 衡量，不设体积门禁。

---

## 2. Fixture

### 2.1 分级

| ID | 用途 | 目标规模（数量级） |
|----|------|-------------------|
| `small` | 正确性、CI 冒烟 | ~3 文件 / ~200 diff 行 |
| `medium` | 日常手感 | ~50 文件 / ~8k diff 行 |
| `huge` | 压内存与滚动 | ~200 文件 / ~50k–100k diff 行 |

具体生成器放在 `scripts/` 与 `fixtures/`（Phase 0/1 补齐）。  
优先 **确定性** 生成 patch，避免 bench 漂移。

### 2.2 规则

- 解析边界用的 **小** 金样 patch 进 git。  
- **Huge** 体可在 CI/本地生成（`scripts/gen_fixtures.sh`），过大则 gitignore。  
- 禁止只用「未说明的脏 worktree」作为唯一指标。

### 2.3 建议生成参数

```text
files=N
lines_per_file=L
changed_lines_per_file=C
seed=S
```

输出：可喂给 `parse_unified_diff` 的 unified diff；需要测 source 适配器时可 `git apply` 进临时仓库。

---

## 3. 指标

除非另注，计时均在 **release** 构建下。尽量安静机器；结果备注里写 CPU 与 OS。

| 指标 ID | 定义 | 方式 |
|---------|------|------|
| `parse_ms` | 从 patch 字节构建 `Review` 的墙钟时间 | bench / `next-hunk bench parse` |
| `viewport_ms` | 在 scroll S 处物化高度 H 的一个视口 | bench；多次 (S,H) 取平均 |
| `startup_ms` | 进程启动 → 首帧绘制（TUI） | 集成 / 手工 harness |
| `scroll_p50_ms` / `scroll_p99_ms` | 单步滚动 + 绘制 | 模拟按键/滚轮 |
| `file_switch_ms` | 跳转上一/下一文件 + 绘制 | |
| `rss_mb` | 浏览 fixture 约 60s 后的进程 RSS | `/proc` 等 |

可选观测（**非门禁**）：

| 指标 ID | 说明 |
|---------|------|
| `binary_bytes` | strip 后 release 大小；仅记录，不设上限 |
| `competitor_delta_ms` | 同 patch stdin 的 `delta` |
| `competitor_git_diff_ms` | `git diff --no-ext-diff --no-color` |

公平性：同一 fixture、同一机器，记录版本号。

---

## 4. 门禁（阶段完成前必须通过）

数值为 x86_64 Linux release、现代笔记本/台式机上的 **初始目标**。修改须在 [门禁变更记录](#6-门禁变更记录) 写简短理由。

### Phase 1（引擎）

| 指标 | Fixture | 门禁 |
|------|---------|------|
| `parse_ms` | huge | **&lt; 80 ms** |
| `viewport_ms`（height=40，1000 次随机起点） | huge | 均值 **&lt; 0.5 ms** |
| 解析 + 1000 次 viewport 后的 RSS | huge | **&lt; 150 MB** |

### Phase 2（TUI MVP）

| 指标 | Fixture | 门禁 |
|------|---------|------|
| `startup_ms` | medium | **&lt; 150 ms** |
| `scroll_p99_ms` | huge | **&lt; 12 ms** |
| `file_switch_ms` | medium | **&lt; 20 ms** |
| `rss_mb`（浏览约 1 分钟） | huge | **&lt; 150 MB** |

### Phase 3+（挑战目标）

| 指标 | Fixture | 门禁 |
|------|---------|------|
| `parse_ms` | huge | **&lt; 50 ms**（挑战） |
| `startup_ms` | medium | **&lt; 100 ms**（挑战） |
| `scroll_p99_ms` | huge | **&lt; 8 ms**（挑战） |
| `rss_mb` | huge | **&lt; 100 MB**（挑战） |

不再设 `binary_bytes` 或 musl 静态产物门禁。

### 实测结果

来自开发机 `cargo bench`（x86_64 Linux，release）。CI 接入 bench 后以 CI 数字为准。

| 指标 | Fixture | 门禁 | 实测 | 状态 |
|------|---------|------|------|------|
| `parse_ms` | huge | < 80 ms | ~1.39 ms | ✅ Phase 1 |
| `viewport_ms`（height=40，单次） | huge | 均值 < 0.5 ms | ~0.0002 ms（197 ns） | ✅ Phase 1 |
| `viewport_ms`（height=40，1000 起点） | huge | 均值 < 0.5 ms | ~0.34 ms（341 µs / 1000） | ✅ Phase 1 |
| `parse_ms` | medium | — | ~0.24 ms | 观测 |
| `parse_ms` | small | — | ~8.2 µs | 观测 |

huge fixture 的 IR arena ~1 MB，远低于 150 MB RSS 门禁；带仪表的 RSS 尚待 `bench` 入口。

### 与 `delta` 的粗对比（设计声明 + bench）

`delta`（https://github.com/dandavison/delta）是常见终端 diff 查看器。本节为 Phase 4 公开对比说明：以 next-hunk 实测为主。直接 wall-clock 对打需本机安装 `delta`（`cargo install git-delta`）；未安装时仅列 next-hunk 数字（AMD Ryzen 7 5700X，32 GB，Linux，release）。

| 维度 | next-hunk | delta | 说明 |
|------|-----------|-------|------|
| **解析延迟（huge，~1.1 MB / 38k 行）** | **~1.4 ms** | 量级应接近 | next-hunk 建紧凑 IR；delta 出高亮输出 |
| **视口物化（40 行）** | **~197 ns** 单次，1000 起点 **~341 µs** | N/A（整 diff 一遍渲染） | next-hunk 仅可见区 |
| **多文件导航** | 文件/hunk 索引，O(log N) | N/A（pager） | 交互 review vs 整页输出 |
| **启动** | 数十 ms（gix + syntect + parse） | 个位数 ms | delta 无 TUI |
| **二进制体积** | ~14 MB | ~2 MB | 非产品目标 |
| **RSS（huge）** | arena ~1 MB；总 RSS 估 < 50 MB | 预期 < 10 MB | next-hunk 常驻 IR |
| **架构** | 仅视口 | 整 diff 输出 | 大 diff 交互路径不同 |

**结论**：stdout 整页 paging 上 delta 更轻；next-hunk 差异化是**可交互的多文件导航与视口物化**。方法：`cargo bench --bench parse` / `viewport`，fixture 见仓库 `fixtures/`。

### 策略

- 未过门禁 → 阶段 **未完成**；不做营销话术。  
- 硬件方差大时，公布同一次运行中的 **相对比**（next-hunk vs delta）。  
- 改门禁必须在下文记一笔。  
- 引入 syntect / gix 等依赖时，门禁仍看延迟与 RSS；体积变大不是否决理由。

---

## 5. 如何跑（规划中的 CLI）

```bash
# 生成 fixture
./scripts/gen_fixtures.sh

# 单元测试
cargo test

# bench（Phase 1+）
cargo bench --bench parse
cargo bench --bench viewport

# 或统一入口
cargo run --release -- bench parse --fixture fixtures/huge.patch
cargo run --release -- bench viewport --fixture fixtures/huge.patch --height 40 --samples 1000
```

结果可记在 `benches/results/`（可 gitignore）或贴进 PR。

### Release 构建（常规）

```bash
cargo build --release
# 体积仅作观测，非 KPI：
ls -lh target/release/next-hunk
```

---

## 6. 门禁变更记录

| 日期 | 变更 | 原因 |
|------|------|------|
| 2026-07-10 | Phase 1–3 初始门禁 | 项目启动 |
| 2026-07-10 | 移除 `binary_bytes` / musl 门禁 | 二进制体积不是产品目标；门禁只保留延迟与 RSS |

---

## 7. 反模式（性能）

| 不要 | 要 |
|------|-----|
| 为整份 review 建带样式的行 `Vec` | 只做 viewport 查询 |
| 每次滚动高亮整文件 | 异步、generation 取消、仅可见区 |
| 同时持有完整 patch `String` 与完整行副本 | arena + span（一份运行时数据） |
| 每次按键都阻塞等 `git` | 加载一次；显式 reload/watch 再刷新 |
| 未过门禁默认开 side-by-side | feature flag + 独立 bench |
| 为减小二进制拒绝 syntect 等 | 用延迟 / RSS 证明收益，再决定默认开关 |

---

## 7.5 与 hunk 0.20 同机实测（2026-08-31）

同机（AMD Ryzen 7 5700X，Linux），hunk 0.20.0 预编译二进制 vs next-hunk
release。TUI 数字取自完全相同的 tmux 200×50 会话，启动后 `sleep 3`，
RSS 从精确二进制进程的 `/proc/<pid>/status` 读取（按 cmdline 核对归属）。

| 测量项 | next-hunk (release) | hunk 0.20.0 | 比值 |
|---|---|---|---|
| 进程基线（`--version`） | **2 ms** | 203 ms | ~100× |
| 无头摘要，1.1 MB / 38k 行（`nh inspect`） | **6 ms** | 无（需要 tty） | — |
| TUI RSS，1.1 MB / 38k 行（200×50） | **25.8 MB** | 115.7 MB | **省 4.5×** |
| TUI RSS，191 KB / 7.8k 行真实 diff（200×50） | **32.5 MB** | 177.8 MB | **省 5.5×** |
| 视口物化（huge diff 上 40 行窗口） | **~350 µs**（`viewport_huge_h40`） | 未公布 | — |

方法说明：RSS 为启动 3 秒后的单次采样（两工具均已首帧渲染完成，无输入
即不再增长）。hunk 的 Node/OpenTUI 运行时自带 ~100 MB 解释器基线，与
diff 大小无关 —— 这也是小 diff 反而差距更大的原因：next-hunk 的 RSS
主要由 diff 本身（紧凑 IR）构成，hunk 主要由运行时构成。滚动流畅度由
视口 bench + 虚拟行滚动模型推出；hunk 未公布等价数字，我们也未对它的
渲染器插桩。

---

## 8. PR 汇报模板

```markdown
### Perf
- Machine:
- Commit:
- Fixture:
- parse_ms:
- viewport_ms (mean):
- scroll_p99_ms: (若涉及 TUI)
- rss_mb:
- Notes: (可选记录 binary 大小，非门禁)
```
