---
id: cli
title: 命令行参考
sidebar_position: 4
---

# 命令行参考

```bash
gaussclaw [选项] [命令]
```

全局选项适用于每一个子命令：

| 参数 | 说明 |
|---|---|
| `-c, --config <路径>` | 指定另一份 `gaussclaw.toml` |
| `-v, --verbose...` | 可重复以提升日志级别 |
| `-q, --quiet` | 抑制非错误输出 |
| `--help` | 输出子命令的帮助文本 |

## 与 Hermes 平等的子命令

| GaussClaw | Hermes 上游 | 状态 |
|---|---|---|
| `gaussclaw`（无参数） | `hermes` | 启动终端界面（TUI） |
| `gaussclaw model {list,show,set}` | `hermes model` | 提供方平面（阶段 4） |
| `gaussclaw tools {list,show,enable,disable}` | `hermes tools` | 技能清单（阶段 3） |
| `gaussclaw config {list,get,set,path}` | `hermes config` | 阶段 1 ✓ |
| `gaussclaw gateway {start,stop,status}` | `hermes gateway` | 通道基础 ✓ |
| `gaussclaw setup` | `hermes setup` | 阶段 1 |
| `gaussclaw update` | `hermes update` | Tauri 更新器（阶段 5） |
| `gaussclaw doctor` | `hermes doctor` | SDHE（阶段 1） |

## GaussClaw 扩展

| 子命令 | 用途 |
|---|---|
| `gaussclaw chat [-m 文本] [-s ID]` | 不进入完整 TUI 的一次性对话 |
| `gaussclaw import <hermes 配置>` | 迁移 Hermes 部署 |
| `gaussclaw receipt {head,verify}` | 查看凭证链或验证信封 |
| `gaussclaw web [--host 主机] [--port 端口] [--open]` | 启动 Axum 仪表盘后端 |

## 一致性门禁

一致性套件（`gaussclaw-conformance`）冻结了一份 `--help`
语料用于锁定接口，防止意外漂移。每次 PR 都会运行：

1. 每个子命令都能被解析。
2. `SUBCOMMANDS` 表与从 clap 派生的接口一致。
3. 每个 Hermes 子命令都已覆盖（不会过度声明，也不会遗漏）。
4. 每个 `--help` 页面的 `insta` 快照与锁定的基线匹配。

参见 [`crates/gaussclaw-conformance/src/cli_parity.rs`](https://github.com/rismanmattotorang/gauss-aether/blob/main/crates/gaussclaw-conformance/src/cli_parity.rs)。
