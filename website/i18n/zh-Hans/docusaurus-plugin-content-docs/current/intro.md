---
id: intro
title: 欢迎
slug: /intro
sidebar_position: 1
---

# 欢迎使用 GaussClaw

**GaussClaw** 是一款可自我提升、与 Hermes 兼容的 AI 代理，以单个静态 Rust
二进制文件的形式发布。它可同时运行在您的笔记本电脑、手机和 \$5 VPS
上——每一个接口，每一项安全保证，全部装进一个可执行文件里。

底层的 [Gauss-Aether](https://github.com/rismanmattotorang/gauss-aether/tree/main/gauss-aether)
内核是一个让安全不变量"被机器检验"而非"靠提示词请求"的运行时。
最终结果：GaussClaw 能做 Hermes 能做的一切，方式也与 Hermes 一致——
但每一次行动都能在事后被**证明**。

## 你可以用它做什么

- 在终端、桌面应用、Web 仪表盘里**对话**，也可以通过你团队已经在用的
  消息平台（Telegram、Discord、Slack、WhatsApp、Signal、Matrix、IRC、
  邮件、短信）。
- 在二十个第一方厂商驱动（Anthropic、OpenAI、Gemini、Mistral、Groq…）以及
  OpenRouter、NotDiamond 元路由之间**自由切换模型**——只需 `gaussclaw model`。
- 直接接入你已有的 **OpenAI SDK 代码**——`gaussclaw serve` 在 localhost 上
  暴露一个 OpenAI 兼容的中继接口。
- 一条命令**从 Hermes 迁移**——`gaussclaw import hermes` 会读取你的 TOML
  配置并产出可用的 `gaussclaw.toml`。
- 在同一内核上**搭建你自己的代理**——只需依赖 `gauss-traits`。

## 与 Hermes 的关键差别

| | Hermes | GaussClaw |
|---|---|---|
| 运行时 | Python + Node.js | **单一静态 Rust 二进制** |
| 工具沙盒 | 父进程凭证 | **WASM + Landlock + seccomp + bwrap** |
| 能力检查 | 无 | **内核准入门控，能力只能单调收缩** |
| 审计日志 | 可变 SQLite | **Ed25519 + Merkle + TSA 锚定** |
| 提供方切换 | 手动回归测试 | **CI 中验证多面体等价性** |
| 桌面安装包 | ~150 MB (Electron) | **~20 MB (Tauri 2)** |
| 冷启动 | 80–150 ms | **≤ 10 ms** |

每一行都对应符合性测试套件中的一条性质测试——299 项测试、约 3 秒、每个 PR
都会重新运行。完整的对应关系见[架构](./architecture)。

## 下一步

- 🚀 [**安装 GaussClaw**](./getting-started/installation)——一条命令。
- 🎬 [**首次运行**](./getting-started/first-run)——启动 TUI、Web 仪表盘、桌面应用。
- 🔁 [**从 Hermes 迁移**](./getting-started/migration-from-hermes)——保留你的配置与工具。
- 🛠️ [**CLI 参考**](./cli)——每一个子命令配以示例。
- 🏗️ [**架构**](./architecture)——安全特性如何被构造出来。
