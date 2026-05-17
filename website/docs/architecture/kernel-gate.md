---
id: kernel-gate
title: Kernel gate
sidebar_position: 1
---

# Kernel gate

Every surface holds a `KernelHandle` (defined in `gaussclaw-agent`)
and consults it before processing any request. The handle wraps an
`Arc<dyn Kernel>` and forwards admit through to the underlying
`gauss-kernel::PrivilegedKernel`.

## The admit gate

```rust
pub fn admit(&self, required: CapToken, taint: TaintLabel) -> GaussResult<()>
```

Joint capability/taint check (Axiom A2 + A6 + Theorem T9 of the
paper). Refuses when either:

- `required ⊑ current_grant` fails — the agent does not have the
  capability to take this action.
- `required ⊑ declass(taint)` fails — the information-flow lattice
  does not allow this action under the request's taint floor.

The structural property: every surface call goes through the gate
before any side-effect. The audit-trace records the inbound BEFORE
the gate fires, so even refused requests are auditable (Axiom A1,
WAL-before-effect).

## PlaneSelector

The handle also carries a `PlaneSelector` that maps a
`SurfaceRequest` to one of the three scheduler planes:

| request | plane |
|---|---|
| `UserSync`, `SdkChat`, `Channel` | `Conversation` |
| `Scheduled` | `Daemon` |
| `Approval` | `Approval` |

The mapping is data — deployments swap `PlaneSelector` on the
handle to customise.

## Per-surface cap requirements

| Surface | Cap required |
|---|---|
| CLI / TUI input | `NETWORK_GET` |
| `/v1/chat/completions` | `NETWORK_GET` |
| `/v1/turn` | `NETWORK_GET` |
| Webhook ingress | `NETWORK_GET` |
| Inbound channel message | `NETWORK_GET` |
| Tool dispatch (Phase 3) | declared in Skill Manifest |
| Outbound webhook POST | `NETWORK_POST` |
| Filesystem read (Phase 3) | `FILESYSTEM_READ` |

`NETWORK_GET` is the lowest-privilege cap that still triggers a
meaningful admit check. Under the default declass it admits every
non-adversarial taint — exactly the right contract for passive
ingress.
