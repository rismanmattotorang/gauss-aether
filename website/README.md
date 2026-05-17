# GaussClaw website

The user-facing site at <https://gauss.ai/gaussclaw/>.

Built with **Docusaurus 3** in two locales (English + Simplified
Chinese), deployed as a static site, and shipping with a custom landing
page (`src/pages/index.jsx`) that mirrors the product-marketing voice of
the root README.

## Layout

| Path | Contents |
|---|---|
| `docs/` | Canonical English documentation. |
| `i18n/zh-Hans/` | Simplified Chinese translation overlay. |
| `src/pages/index.jsx` | The landing page (hero, surfaces, features, Hermes comparison, code showcase, CTA). |
| `src/pages/index.module.css` | Per-component styles for the landing page. |
| `src/css/custom.css` | Global theme — typography, palette, navbar, footer, tables. |
| `static/img/` | Logo, favicon, social card (all SVG). |
| `docusaurus.config.js` | Site config — navbar, footer, metadata, announcement bar, i18n. |
| `sidebars.js` | Doc sidebar structure. |

## Develop locally

```bash
cd website
npm install
npm start             # http://localhost:3000/gaussclaw/
```

Hot-reload covers MDX, JSX, CSS, and the config file.

## Build for production

```bash
npm run build         # → ./build/
npm run serve         # serve ./build locally for verification
```

The build is fully static and deployable to any object store or CDN
(Cloudflare Pages, Vercel, Netlify, GitHub Pages, S3 + CloudFront).

## Style guide

- **Dark mode by default**, with full light-mode coverage.
- Cyan accent (`#22d3ee`); no other accent colours.
- Body in **Inter**, code in **JetBrains Mono** — both via Google Fonts.
- Sections breathe — generous vertical rhythm (`5–6 rem` section padding).
- Tables use rounded corners + a subtle background row for the GaussClaw column.
- All claims that cite a metric link out to the source axiom, theorem,
  or conformance test.

## Editing tips

- The landing page lives in **one file** (`src/pages/index.jsx`) — easy to
  refactor sections in place.
- The comparison rows are a flat array of `[label, hermesValue, gaussValue]`
  tuples; reorder by editing the array.
- Adding a new doc page: drop it under `docs/`, list it in
  `sidebars.js`, and (optionally) translate it under
  `i18n/zh-Hans/docusaurus-plugin-content-docs/current/`.

## Deployment

The production deploy currently lives behind `https://gauss.ai/gaussclaw/`.
The CI pipeline runs `npm ci && npm run build` and uploads `./build/` to
the configured static host.
