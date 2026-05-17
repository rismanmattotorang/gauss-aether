---
id: three-plane
title: Three-plane scheduling
sidebar_position: 2
---

# Three-plane scheduling

The Gauss-Aether kernel runs three independent budget pools:

| plane | request kinds | starvation bound |
|---|---|---|
| **Conversation** | CLI, TUI, REST, WS, OAI-compat, channels | B/ρ |
| **Daemon** | Scheduled turns, gateway long-polls | B/ρ |
| **Approval** | Human-in-the-loop approval round-trips | B/ρ |

Each plane has its own atomic token bucket (one `AtomicU64` packing
`(tokens_fp16.16, epoch_ms)`). Lock-free CAS refill, no mutex, no
shared cross-plane state — Theorem T4 of the paper.

## Why three planes

The upstream Hermes runs every turn — user, background, approval —
on one event loop. A long background turn starves the user; a stuck
approval starves both.

GaussClaw separates them structurally: user-synchronous traffic gets
the **Conversation** budget; scheduled / daemon turns get **Daemon**;
human-in-the-loop approvals get **Approval**. A stuck approval
cannot starve user turns; a long daemon sweep cannot starve user
turns.

## How surfaces map

`gaussclaw-agent::SurfaceRequest` is the surface-side request
descriptor. `PlaneSelector::plane_for` maps it to a plane. The
default mapping covers every surface in the workspace; deployments
override `PlaneSelector` to customise.

## The starvation bound

Each plane refills at a configured `ρ` tokens/sec up to a `B`-token
capacity. A request waits at most `(required − tokens) / ρ` seconds —
the bound is in the spec; the proof is mechanically checked in
`gauss-conformance`.
