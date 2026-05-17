# gaussclaw-website

Docusaurus content tree for the GaussClaw user-facing site (English +
Simplified Chinese), plus an mdBook-rendered API reference for every
`gaussclaw-*` and `gauss-*` crate.

This is **not** a Rust crate; it is built independently in CI and
deployed as a static site. See `GAUSSCLAW_ROADMAP.md` Phase 1 Task 6
("Website").

## Layout

- `docs/` — canonical English documentation tree
- `i18n/zh-Hans/` — Simplified Chinese translation overlay
- `src/` — Docusaurus theme overrides and React components
- `static/` — assets (logos, diagrams, screenshots)
- `api-reference/` — mdBook config; the build step stitches
  `cargo doc --workspace --no-deps` output into a unified crate-graph
  index served under `/api/`.

## Build (Phase 1 → GA)

```sh
pnpm install
pnpm build         # produces ./build/, deployable to any static host
```

CI lint: every page must reference at least one canonical anchor
(axiom, theorem, or Hermes-module path).
