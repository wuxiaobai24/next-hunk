# next-hunk

**更快地 review 超大 diff，为 agent 时代而建。**

[English](./README.md) | **中文**

面向超大 changeset 的高性能终端 **review 引擎**。

| 支柱 | 承诺 |
|------|------|
| **性能** | 仅视口渲染、紧凑运行时 IR、硬性 bench 门禁 |
| **规模** | 多文件 review 流，不为每一行建 widget |
| **体验** | 交互式多文件导航、可读布局、可脚本化 CLI |
| **Agent 时代** | 面向人类与 coding agent 的结构化导出（路线图） |

**二进制体积不是产品目标。** 我们优化的是大 diff 下的延迟与运行时内存，不是「谁更小谁赢」。

## 命名

**next-hunk** —— 按文件与 hunk 导航 review 流。CLI：`next-hunk`。以后可加短别名（如 `nh`）。

## 状态

早期原型（`v0.1.0-dev`）：

- [x] 项目骨架 + 紧凑 unified-diff IR（运行时模型）
- [x] 视口查询骨架
- [ ] 虚拟化多文件 TUI
- [x] gix 驱动的 worktree / staged / show（+ patch stdin）
- [ ] 异步语法高亮
- [ ] Agent 导出（JSON / Markdown）
- [ ] 对常见工具（如 delta）的公开性能对比（延迟 / RSS）

## 安装（开发）

```bash
cargo install --path .
# 或
cargo run --release -- diff
```

## 用法（规划）

```bash
next-hunk                  # 工作区 diff
next-hunk diff --staged
next-hunk show HEAD
git diff | next-hunk patch -
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
