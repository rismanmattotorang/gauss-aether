import React from 'react';
import Layout from '@theme/Layout';
import Link from '@docusaurus/Link';
import useDocusaurusContext from '@docusaurus/useDocusaurusContext';
import clsx from 'clsx';
import styles from './index.module.css';

function Hero() {
  return (
    <header className={styles.hero}>
      <div className={styles.heroBg} aria-hidden="true" />
      <div className={clsx('container', styles.heroInner)}>
        <span className={styles.eyebrow}>
          <span className={styles.eyebrowDot} /> v1.0 · 299 conformance tests green
        </span>
        <h1 className={styles.heroTitle}>
          The self-improving AI agent <br />
          <span className={styles.gradient}>that won't run code you didn't authorise.</span>
        </h1>
        <p className={styles.heroSubtitle}>
          GaussClaw is a Rust-native AI agent that lives on your laptop, your phone, and your $5 VPS at
          the same time. It learns from every conversation, connects to 200+ language models, and
          ships as <strong>one static binary</strong> — no Python, no Node.js, no Electron.
        </p>
        <p className={styles.heroSubtitle}>
          Unlike every other agent in its class, GaussClaw can <em>prove</em> what it did. Every tool
          call passes a capability check. Every turn is signed and chained. Every export carries a
          tamper-proof receipt.
        </p>
        <div className={styles.heroCtas}>
          <Link to="/docs/getting-started/installation" className={clsx('button', styles.ctaPrimary)}>
            Install GaussClaw
          </Link>
          <Link to="#compare" className={clsx('button', styles.ctaSecondary)}>
            Compare to Hermes →
          </Link>
        </div>

        <div className={styles.heroInstall}>
          <span className={styles.heroInstallLabel}>Migrate from Hermes in one command</span>
          <code className={styles.heroInstallCode}>
            $ gaussclaw import hermes ~/.hermes/config.toml &gt; gaussclaw.toml
          </code>
        </div>
      </div>
    </header>
  );
}

function Surfaces() {
  const items = [
    {
      icon: '💻',
      title: 'Terminal',
      body: 'Full-screen TUI with multiline editing, slash-command autocomplete, conversation history, and streaming tool output.',
    },
    {
      icon: '🖥️',
      title: 'Desktop',
      body: 'Signed, notarised installers for macOS, Windows, and Linux. ~20 MB on disk, ~80 MB RAM idle.',
    },
    {
      icon: '🌐',
      title: 'Web dashboard',
      body: 'gaussclaw serve spins up a React frontend and an OpenAI-compatible API relay from the same binary.',
    },
    {
      icon: '📱',
      title: 'Messaging',
      body: 'Telegram, Discord, Slack, WhatsApp, Signal, Matrix, IRC, email, SMS — one gateway. Voice memos transcribed.',
    },
    {
      icon: '🔁',
      title: 'OpenAI SDK',
      body: 'Point any existing OpenAI client at localhost and keep your code. Full Chat Completions + Responses parity.',
    },
    {
      icon: '🦀',
      title: 'Library',
      body: 'Embed the Gauss-Aether runtime directly in your Rust app. Same kernel, same safety guarantees.',
    },
  ];
  return (
    <section className={styles.section}>
      <div className="container">
        <div className={styles.sectionHeader}>
          <span className={styles.kicker}>Lives where you do</span>
          <h2 className={styles.sectionTitle}>One binary. Every surface.</h2>
          <p className={styles.sectionLede}>
            The same <code>gaussclaw</code> hosts the CLI, the TUI, the web dashboard, the OpenAI-compatible
            relay, the desktop shell, and the messaging gateway. There is nothing else to install.
          </p>
        </div>
        <div className={styles.grid3}>
          {items.map((it) => (
            <div key={it.title} className={styles.card}>
              <div className={styles.cardIcon}>{it.icon}</div>
              <h3 className={styles.cardTitle}>{it.title}</h3>
              <p className={styles.cardBody}>{it.body}</p>
            </div>
          ))}
        </div>
      </div>
    </section>
  );
}

function Features() {
  const items = [
    {
      title: 'It remembers, and it learns.',
      body: (
        <>
          GaussClaw curates its own memory. After complex tasks it writes itself a skill it can pull back
          next time. It nudges itself to persist important context and searches its past with full-text
          and vector recall together.
        </>
      ),
      stat: '≤ 1.5 %',
      statLabel: 'hybrid recall miss rate (Hermes baseline: 8 %)',
    },
    {
      title: 'Tools that can’t go rogue.',
      body: (
        <>
          Every tool declares a capability before it runs. The kernel checks it against the active grant
          — and the grant can only <em>shrink</em>, never grow. The type system refuses to compile code
          that tries to widen one.
        </>
      ),
      stat: '≤ 1 × 10⁻⁷',
      statLabel: 'sandbox compromise probability (4-layer composite)',
    },
    {
      title: 'Resistant to prompt injection by design.',
      body: (
        <>
          When a tool returns text — from a web page, a PDF, an email — GaussClaw runs the output through
          a four-stage schema gate before any of it touches the next prompt. Untrusted instructions
          can’t smuggle themselves back in.
        </>
      ),
      stat: '0 / 20',
      statLabel: 'empirical injections on standard corpus (≤ 2.19 % theoretical)',
    },
    {
      title: 'An audit log that holds up in court.',
      body: (
        <>
          Every turn — input, model output, tool calls, approvals — is hashed into a Merkle chain, signed
          with Ed25519, and anchored to an RFC 3161 timestamp authority every thousand entries.
        </>
      ),
      stat: 'negl(λ)',
      statLabel: 'receipt forgery probability (EUF-CMA Ed25519)',
    },
    {
      title: 'Any model. No lock-in.',
      body: (
        <>
          Twenty first-party vendor drivers. Plus OpenRouter as an aggregator and NotDiamond as a learned
          router. Switch with <code>gaussclaw model</code> — the polyhedral verifier proves the swap is
          behaviourally equivalent before it commits.
        </>
      ),
      stat: '20+',
      statLabel: 'providers (Anthropic, OpenAI, Gemini, Mistral, Groq, …)',
    },
    {
      title: 'One binary. No interpreter.',
      body: (
        <>
          No Python at runtime. No Node.js at runtime. No Chromium bundled with the desktop app. About
          a tenth the size and a third the memory of a Hermes Electron build.
        </>
      ),
      stat: '≤ 10 ms',
      statLabel: 'cold start to first turn (Hermes: 80–150 ms)',
    },
  ];
  return (
    <section className={clsx(styles.section, styles.sectionDark)}>
      <div className="container">
        <div className={styles.sectionHeader}>
          <span className={styles.kicker}>What makes it different</span>
          <h2 className={styles.sectionTitle}>Six properties Hermes has no equivalent for.</h2>
          <p className={styles.sectionLede}>
            Every claim below is backed by a property test in the conformance suite — 299 tests, three
            seconds, re-run on every PR.
          </p>
        </div>
        <div className={styles.featuresGrid}>
          {items.map((it) => (
            <article key={it.title} className={styles.featureCard}>
              <h3 className={styles.featureTitle}>{it.title}</h3>
              <p className={styles.featureBody}>{it.body}</p>
              <div className={styles.featureStat}>
                <span className={styles.featureStatValue}>{it.stat}</span>
                <span className={styles.featureStatLabel}>{it.statLabel}</span>
              </div>
            </article>
          ))}
        </div>
      </div>
    </section>
  );
}

function Compare() {
  const rows = [
    ['Runtime',                        'Python + Node.js',          'Single static Rust binary'],
    ['Desktop installer',              '~150 MB (Electron)',        '~20 MB (Tauri 2)'],
    ['Desktop RAM idle',               '~250 MB',                   '~80 MB'],
    ['Cold start',                     '80–150 ms',                 '≤ 10 ms'],
    ['Tool sandbox',                   'parent credentials',        'WASM + Landlock + seccomp + bwrap'],
    ['Capability check',               'none',                      'Kernel admit gate, monotone shrink'],
    ['Prompt-injection containment',   'none',                      '≤ 2.19 % (0/20 empirical)'],
    ['Audit log',                      'mutable SQLite',            'Ed25519 + Merkle + TSA anchor'],
    ['Provider swap',                  'manual retest',             'Polyhedral equivalence verified in CI'],
    ['Trajectory exports',             'raw JSONL',                 'Cryptographic envelope, verifiable'],
    ['Hybrid recall miss rate',        '~8 %',                      '≤ 1.5 %'],
    ['Code signing on desktop',        'unsigned',                  'Signed + notarised on 3 OSes'],
    ['Migration from Hermes',          'n/a',                       'One command'],
  ];
  return (
    <section id="compare" className={styles.section}>
      <div className="container">
        <div className={styles.sectionHeader}>
          <span className={styles.kicker}>Head to head</span>
          <h2 className={styles.sectionTitle}>GaussClaw vs. Hermes</h2>
          <p className={styles.sectionLede}>
            Hermes is a delightful agent and a fragile substrate. GaussClaw preserves every Hermes
            ergonomic primitive — the <code>@tool</code> decorator, the TOML config, the surface
            inventory, the SFT/DPO trajectory schema — and closes its architectural gaps.
          </p>
        </div>
        <div className={styles.compareWrap}>
          <table className={styles.compare}>
            <thead>
              <tr>
                <th></th>
                <th className={styles.compareHeadHermes}>Hermes</th>
                <th className={styles.compareHeadGc}>GaussClaw</th>
              </tr>
            </thead>
            <tbody>
              {rows.map(([label, h, g]) => (
                <tr key={label}>
                  <th scope="row">{label}</th>
                  <td className={styles.compareCellHermes}>{h}</td>
                  <td className={styles.compareCellGc}>{g}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </div>
    </section>
  );
}

function CodeShowcase() {
  return (
    <section className={clsx(styles.section, styles.sectionDark)}>
      <div className="container">
        <div className={styles.sectionHeader}>
          <span className={styles.kicker}>Get started in seconds</span>
          <h2 className={styles.sectionTitle}>Install, run, talk.</h2>
        </div>
        <div className={styles.codeGrid}>
          <div className={styles.codeCard}>
            <div className={styles.codeHeader}>1. Install</div>
            <pre className={styles.codeBlock}>{`git clone https://github.com/rismanmattotorang/gauss-aether
cd gauss-aether
cargo install --path gaussclaw/crates/gaussclaw-bin

gaussclaw doctor   # green = ready`}</pre>
          </div>
          <div className={styles.codeCard}>
            <div className={styles.codeHeader}>2. Run</div>
            <pre className={styles.codeBlock}>{`gaussclaw                         # TUI
gaussclaw model                   # pick a model
gaussclaw gateway                 # connect Telegram, Slack…
gaussclaw serve --port 8080       # web + API relay
gaussclaw receipt verify env.json # prove a trajectory`}</pre>
          </div>
          <div className={styles.codeCard}>
            <div className={styles.codeHeader}>3. Embed</div>
            <pre className={styles.codeBlock}>{`use gauss_kernel::PrivilegedKernel;
use gauss_turn::TurnEngine;

let engine = TurnEngine::new(kernel, memory, provider)
    .with_sag(approval_gate);

let summary = engine.run_turn(input).await?;
println!("{}", hex::encode(summary.chain_head.digest));`}</pre>
          </div>
        </div>
      </div>
    </section>
  );
}

function CTA() {
  return (
    <section className={styles.cta}>
      <div className="container">
        <div className={styles.ctaInner}>
          <h2 className={styles.ctaTitle}>Ready to leave Python + Electron behind?</h2>
          <p className={styles.ctaBody}>
            One binary. Every surface. Every safety guarantee proved by the type system, by the
            conformance suite, and — for the patient — by Lean 4.
          </p>
          <div className={styles.heroCtas}>
            <Link to="/docs/getting-started/installation" className={clsx('button', styles.ctaPrimary)}>
              Install GaussClaw
            </Link>
            <Link
              to="https://github.com/rismanmattotorang/gauss-aether"
              className={clsx('button', styles.ctaSecondary)}
            >
              Star on GitHub ★
            </Link>
          </div>
        </div>
      </div>
    </section>
  );
}

export default function Home() {
  const { siteConfig } = useDocusaurusContext();
  return (
    <Layout
      title={`${siteConfig.title} — The verifiable AI agent`}
      description="A self-improving, Hermes-compatible AI agent that ships as one static Rust binary and proves every action with a signed receipt chain."
    >
      <Hero />
      <main>
        <Surfaces />
        <Features />
        <Compare />
        <CodeShowcase />
        <CTA />
      </main>
    </Layout>
  );
}
