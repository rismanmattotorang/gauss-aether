---
id: migration
title: Hermes adapter matrix
sidebar_position: 9
---

# Hermes adapter matrix

The canonical inventory of every Hermes surface, channel, and entry
point GaussClaw replaces, plus the crate that takes over. Lives at
[`docs/HERMES_ADAPTER_MATRIX.md`](https://github.com/rismanmattotorang/gauss-aether/blob/main/docs/HERMES_ADAPTER_MATRIX.md)
in the repository.

The matrix is the source of truth for the parity tables in
`gaussclaw-cli::SUBCOMMANDS` and
`gaussclaw-conformance::cli_parity::HERMES_SUBCOMMANDS`. Any addition
or removal in upstream Hermes lands in lock-step.
