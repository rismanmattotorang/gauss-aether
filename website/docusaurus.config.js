// @ts-check
// Docusaurus 3 configuration for the GaussClaw site.

import { themes as prismThemes } from 'prism-react-renderer';

/** @type {import('@docusaurus/types').Config} */
const config = {
  title: 'GaussClaw',
  tagline: 'The verifiable AI agent. Built in Rust. Proven safe by construction.',
  favicon: 'img/favicon.svg',

  url: 'https://gauss.ai',
  baseUrl: '/gaussclaw/',

  organizationName: 'rismanmattotorang',
  projectName: 'gauss-aether',

  onBrokenLinks: 'warn',
  onBrokenMarkdownLinks: 'warn',
  onBrokenAnchors: 'warn',

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

  headTags: [
    {
      tagName: 'meta',
      attributes: { name: 'theme-color', content: '#0a0f1a' },
    },
    {
      tagName: 'link',
      attributes: { rel: 'preconnect', href: 'https://fonts.googleapis.com' },
    },
    {
      tagName: 'link',
      attributes: { rel: 'preconnect', href: 'https://fonts.gstatic.com', crossorigin: 'anonymous' },
    },
  ],

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
        sitemap: {
          changefreq: 'weekly',
          priority: 0.5,
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
      image: 'img/social-card.svg',
      metadata: [
        {
          name: 'keywords',
          content: 'AI agent, LLM, Rust, Hermes, verifiable, sandbox, audit, Tauri, OpenAI, agent framework',
        },
        {
          name: 'description',
          content:
            'GaussClaw is a Hermes-compatible AI agent that ships as one static Rust binary and proves every action with a signed receipt chain.',
        },
        { property: 'og:type', content: 'website' },
        { property: 'twitter:card', content: 'summary_large_image' },
      ],
      announcementBar: {
        id: 'release-1-0',
        content:
          '✨ <strong>GaussClaw 1.0</strong> is shipping — single static binary, 4-layer sandbox, signed receipt chain. <a href="/docs/getting-started/installation">Get started →</a>',
        backgroundColor: '#06b6d4',
        textColor: '#06121b',
        isCloseable: true,
      },
      navbar: {
        title: 'GaussClaw',
        logo: {
          alt: 'GaussClaw logo',
          src: 'img/logo.svg',
          width: 30,
          height: 30,
        },
        hideOnScroll: false,
        items: [
          {
            type: 'docSidebar',
            sidebarId: 'main',
            position: 'left',
            label: 'Documentation',
          },
          { to: '/#compare', label: 'Why GaussClaw', position: 'left' },
          {
            label: 'Surfaces',
            position: 'left',
            items: [
              { to: '/docs/cli', label: 'CLI' },
              { to: '/docs/tui', label: 'TUI' },
              { to: '/docs/web', label: 'Web' },
              { to: '/docs/desktop', label: 'Desktop' },
            ],
          },
          { to: '/docs/architecture', label: 'Architecture', position: 'left' },
          { to: '/docs/getting-started/migration-from-hermes', label: 'Migrate', position: 'left' },
          {
            href: 'https://github.com/rismanmattotorang/gauss-aether',
            'aria-label': 'GitHub repository',
            label: 'GitHub',
            position: 'right',
          },
          { type: 'localeDropdown', position: 'right' },
        ],
      },
      footer: {
        style: 'dark',
        logo: {
          alt: 'GaussClaw',
          src: 'img/logo.svg',
          width: 32,
          height: 32,
        },
        links: [
          {
            title: 'Get started',
            items: [
              { label: 'Install', to: '/docs/getting-started/installation' },
              { label: 'First run', to: '/docs/getting-started/first-run' },
              { label: 'Migrate from Hermes', to: '/docs/getting-started/migration-from-hermes' },
              { label: 'CLI reference', to: '/docs/cli' },
            ],
          },
          {
            title: 'Architecture',
            items: [
              { label: 'Overview', to: '/docs/architecture' },
              { label: 'Kernel admit gate', to: '/docs/architecture/kernel-gate' },
              { label: 'Three-plane scheduler', to: '/docs/architecture/three-plane' },
              { label: 'Receipt chain', to: '/docs/architecture/audit-chain' },
            ],
          },
          {
            title: 'Project',
            items: [
              { label: 'GitHub', href: 'https://github.com/rismanmattotorang/gauss-aether' },
              { label: 'Gauss-Aether runtime', href: 'https://github.com/rismanmattotorang/gauss-aether/tree/main/gauss-aether' },
              { label: 'Releases', href: 'https://github.com/rismanmattotorang/gauss-aether/releases' },
              { label: 'Issues', href: 'https://github.com/rismanmattotorang/gauss-aether/issues' },
            ],
          },
          {
            title: 'Compare',
            items: [
              { label: 'Hermes upstream', href: 'https://github.com/NousResearch/hermes-agent' },
              { label: 'GaussClaw vs. Hermes', to: '/#compare' },
              { label: 'Architecture paper', href: 'https://github.com/rismanmattotorang/gauss-aether/blob/main/gaussclaw/SPEC.pdf' },
            ],
          },
        ],
        copyright: `MIT-licensed · Built on Gauss-Aether · © ${new Date().getFullYear()}`,
      },
      prism: {
        theme: prismThemes.oneLight,
        darkTheme: prismThemes.oneDark,
        additionalLanguages: ['rust', 'toml', 'bash', 'json', 'yaml'],
      },
      tableOfContents: {
        minHeadingLevel: 2,
        maxHeadingLevel: 4,
      },
      docs: {
        sidebar: {
          hideable: true,
          autoCollapseCategories: true,
        },
      },
    }),
};

export default config;
