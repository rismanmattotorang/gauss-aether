---
id: tui
title: TUI 参考
sidebar_position: 5
---

# TUI 参考

交互式终端外壳。基于 [Ratatui](https://ratatui.rs)
+ [crossterm](https://github.com/crossterm-rs/crossterm) + tui-textarea
构建 —— **无需 Node 运行时**，体积比上游 Hermes
React + Ink 的 TUI 小约 10 倍。

## 布局

```
 ┌────────────────────────── GaussClaw v0.0.1 ─────────────────────────────┐
 │ session=…  model=…  turn=…  chain=…  taint=⊥  caps=…                    │ ← 状态栏
 ├──────────────────────────────────────────────────────────────────────────┤
 │ 历史面板（可滚动）                                                        │
 ├──────────────────────────────────────────────────────────────────────────┤
 │ > 输入区（多行；Shift+Enter 换行）                                       │
 └── Enter 提交 · Ctrl+C 退出 · Ctrl+L 清空 · /help 帮助 ──────────────────┘
```

## 按键绑定

| 按键 | 动作 |
|---|---|
| `Enter` | 提交 |
| `Shift+Enter` | 换行 |
| `Ctrl+C` / `Ctrl+D` | 退出 |
| `Ctrl+L` | 清空历史 |
| `PageUp` / `PageDown` | 滚动历史 |
| `Tab` | 应用补全（规划中） |

## 斜杠命令

阶段 1 已实现 `/help`、`/quit`、`/exit`、`/clear`、`/new`。下列命令
目前已被识别，并以桩响应方式告知将由哪个阶段补齐：

`/receipt`、`/taint`、`/caps`、`/sandbox`、`/model`、`/tools`、
`/config`、`/logs`、`/statusbar`、`/queue`、`/undo`、`/retry`、
`/copy`、`/paste`、`/details`、`/compact`、`/resume`。

## GaussClaw 独有的状态栏字段

上游 Hermes 的 Ink TUI 无法展示的三个字段：

- `chain=<hex>` —— 实时凭证链头的前 8 位十六进制字符。
  每一轮（WAL 先于副作用）都会推进。
- `taint=<标签>` —— 当前会话的污点下界。`⊥` /
  `user` / `web` / `adversarial`。
- `caps=<n>` —— 已授予的能力数量。
