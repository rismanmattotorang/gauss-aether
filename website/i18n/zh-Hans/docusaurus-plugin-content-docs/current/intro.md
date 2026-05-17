---
id: intro
title: GaussClaw
slug: /intro
sidebar_position: 1
---

# GaussClaw

**Hermes 代理的 Rust 移植，运行于 Gauss-Aether 公理化内核之上。**

GaussClaw 保留 Hermes 的每一项人体工程学原语（`@tool` 装饰器、TOML 配置模式、CLI / TUI / Web / 通道接口、SFT/DPO 轨迹导出），并将其与 Gauss-Aether 子系统绑定，每个子系统都由一个编号定理支撑。

## 为什么存在

Hermes 是一个优雅的代理，但底层基础脆弱。工具调用在宿主解释器中运行，拥有代理的完整凭证集；会话存储可变且未签名；网络抓取的文本会被原样作为下一个提示词；后台和用户回合共享一个事件循环；密钥从 `os.environ` 读取并被永久信任。

GaussClaw 将相同的代理放入一个之前缺失的内核之上：

- **工具调度受准入门控和沙盒保护。**
- **会话存储是防篡改链。**
- **网络抓取的文本永远不会越过工作上下文边界。**
- **后台、用户和审批回合各有独立的预算池。**
- **密钥通过可证明的存储解析，而不是原始环境变量。**

## 从哪里开始

完整的[路线图](https://github.com/rismanmattotorang/gauss-aether/blob/main/GAUSSCLAW_ROADMAP.md)记录了五个阶段、二十四周的开发计划。
