// @ts-check
// Docusaurus 3 configuration. Builds the GaussClaw user-facing site
// at https://gauss.ai/gaussclaw/ (or wherever deployed) with English
// and Simplified Chinese content.

import { themes as prismThemes } from 'prism-react-renderer';

/** @type {import('@docusaurus/types').Config} */
const config = {
  title: 'GaussClaw',
  tagline: 'Hermes-compatible agent on the Gauss-Aether runtime',
  favicon: 'img/favicon.ico',

  url: 'https://gauss.ai',
  baseUrl: '/gaussclaw/',

  organizationName: 'rismanmattotorang',
  projectName: 'gauss-aether',

  onBrokenLinks: 'warn',
  onBrokenMarkdownLinks: 'warn',

  i18n: {
    defaultLocale: 'en',
    locales: ['en', 'zh-Hans'],
    localeConfigs: {
      en: { label: 'English', direction: 'ltr', htmlLang: 'en-US' },
      'zh-Hans': { label: '简体中文', direction: 'ltr', htmlLang: 'zh-CN' },
    },
  },

  markdown: {
    mermaid: false,
  },

  presets: [
    [
      'classic',
      /** @type {import('@docusaurus/preset-classic').Options} */
      ({
        docs: {
          sidebarPath: './sidebars.js',
          routeBasePath: 'docs',
          editUrl: 'https://github.com/rismanmattotorang/gauss-aether/edit/main/website/',
          showLastUpdateTime: true,
          showLastUpdateAuthor: false,
        },
        blog: false,
        theme: {
          customCss: './src/css/custom.css',
        },
      }),
    ],
  ],

  themeConfig:
    /** @type {import('@docusaurus/preset-classic').ThemeConfig} */
    ({
      colorMode: {
        defaultMode: 'dark',
        respectPrefersColorScheme: true,
      },
      image: 'img/social-card.png',
      navbar: {
        title: 'GaussClaw',
        logo: { alt: 'GaussClaw', src: 'img/logo.svg' },
        items: [
          {
            type: 'docSidebar',
            sidebarId: 'main',
            position: 'left',
            label: 'Docs',
          },
          { to: '/docs/cli', label: 'CLI', position: 'left' },
          { to: '/docs/tui', label: 'TUI', position: 'left' },
          { to: '/docs/web', label: 'Web', position: 'left' },
          { to: '/docs/desktop', label: 'Desktop', position: 'left' },
          { to: '/api', label: 'API reference', position: 'left' },
          {
            href: 'https://github.com/rismanmattotorang/gauss-aether',
            label: 'GitHub',
            position: 'right',
          },
          { type: 'localeDropdown', position: 'right' },
        ],
      },
      footer: {
        style: 'dark',
        links: [
          {
            title: 'Docs',
            items: [
              { label: 'Getting started', to: '/docs/intro' },
              { label: 'Architecture', to: '/docs/architecture' },
              { label: 'Migration from Hermes', to: '/docs/migration' },
            ],
          },
          {
            title: 'Companion projects',
            items: [
              { label: 'Gauss-Aether runtime', href: 'https://github.com/rismanmattotorang/gauss-aether' },
              { label: 'Hermes (upstream)', href: 'https://github.com/NousResearch/hermes-agent' },
            ],
          },
          {
            title: 'More',
            items: [
              { label: 'GitHub', href: 'https://github.com/rismanmattotorang/gauss-aether' },
              { label: 'Roadmap', href: 'https://github.com/rismanmattotorang/gauss-aether/blob/main/GAUSSCLAW_ROADMAP.md' },
            ],
          },
        ],
        copyright: `MIT-licensed. Built on Gauss-Aether 1.0.`,
      },
      prism: {
        theme: prismThemes.github,
        darkTheme: prismThemes.dracula,
        additionalLanguages: ['rust', 'toml', 'bash', 'json'],
      },
    }),
};

export default config;
