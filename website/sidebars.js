// @ts-check
// The main sidebar mirrors the structure of GAUSSCLAW_ROADMAP.md so
// readers can scan the project the same way contributors implement it.

/** @type {import('@docusaurus/plugin-content-docs').SidebarsConfig} */
const sidebars = {
  main: [
    'intro',
    {
      type: 'category',
      label: 'Getting started',
      collapsed: false,
      items: ['getting-started/installation', 'getting-started/first-run', 'getting-started/migration-from-hermes'],
    },
    {
      type: 'category',
      label: 'Surfaces',
      collapsed: false,
      items: ['cli', 'tui', 'web', 'desktop'],
    },
    {
      type: 'category',
      label: 'Architecture',
      items: ['architecture', 'architecture/kernel-gate', 'architecture/three-plane', 'architecture/audit-chain'],
    },
    {
      type: 'category',
      label: 'Reference',
      items: ['migration', 'roadmap'],
    },
  ],
};

export default sidebars;
