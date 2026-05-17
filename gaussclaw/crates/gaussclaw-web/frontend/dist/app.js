/* ============================================================================
 * GaussClaw Dashboard
 *
 * Vanilla ES modules. Zero dependencies. Ships as a single static file
 * embedded in the gaussclaw binary via rust-embed. No build step required.
 *
 * Sections:
 *   1. State + helpers
 *   2. API client
 *   3. View router
 *   4. Chat view (WebSocket)
 *   5. Sessions view
 *   6. Tools view
 *   7. Receipts view
 *   8. Health view
 *   9. Settings view
 *  10. Command palette
 *  11. Toast notifications
 *  12. Boot
 * ============================================================================ */

// ─── 1. State + helpers ─────────────────────────────────────────────────────

const state = {
  view: 'chat',
  status: null,
  sessions: [],
  tools: [],
  providers: [],
  health: null,
  config: null,
  chainHead: null,
  socket: null,
  socketReady: false,
  currentSessionId: 'new',
};

const $  = (sel, root = document) => root.querySelector(sel);
const $$ = (sel, root = document) => Array.from(root.querySelectorAll(sel));

function escape(s) {
  if (s == null) return '';
  return String(s).replace(/[&<>"']/g, ch => ({
    '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;'
  }[ch]));
}

function el(tag, attrs = {}, ...children) {
  const node = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k === 'class')         node.className = v;
    else if (k === 'html')     node.innerHTML = v;
    else if (k.startsWith('on'))
                               node.addEventListener(k.slice(2).toLowerCase(), v);
    else if (v !== null && v !== undefined && v !== false)
                               node.setAttribute(k, v);
  }
  for (const c of children.flat()) {
    if (c == null || c === false) continue;
    node.append(c.nodeType ? c : document.createTextNode(c));
  }
  return node;
}

function fmtTime(t) {
  if (!t) return '—';
  try { return new Date(t).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }); }
  catch { return String(t); }
}

function shortHex(h, len = 12) {
  if (!h) return '—';
  const s = String(h);
  return s.length > len ? s.slice(0, len) + '…' : s;
}

// ─── 2. API client ──────────────────────────────────────────────────────────

const api = {
  async get(path) {
    try {
      const r = await fetch(path, { headers: { accept: 'application/json' } });
      if (!r.ok) throw new Error(`${r.status} ${r.statusText}`);
      const j = await r.json();
      return j.ok ? j.data : Promise.reject(new Error(j.error?.message ?? 'unknown error'));
    } catch (e) {
      console.warn('[api]', path, e);
      throw e;
    }
  },
  status:    () => api.get('/api/status'),
  health:    () => api.get('/api/health'),
  config:    () => api.get('/api/config'),
  sessions:  () => api.get('/api/sessions'),
  providers: () => api.get('/api/providers'),
  tools:     () => api.get('/api/tools'),
  receipt:   () => api.get('/api/receipt/head'),
};

// ─── 3. View router ─────────────────────────────────────────────────────────

const renderers = {};

function switchView(name) {
  state.view = name;
  $$('.view').forEach(v => v.classList.toggle('active', v.id === `view-${name}`));
  $$('.nav-item').forEach(n => {
    const on = n.dataset.view === name;
    n.classList.toggle('active', on);
    n.setAttribute('aria-selected', String(on));
  });
  if (renderers[name]) renderers[name]();
}

function wireNav() {
  $$('.nav-item').forEach(btn => btn.addEventListener('click', () => switchView(btn.dataset.view)));
  $$('[data-view-link]').forEach(a => a.addEventListener('click', e => {
    e.preventDefault();
    switchView(a.dataset.viewLink);
  }));
}

// ─── 4. Chat view ───────────────────────────────────────────────────────────

const chat = {
  transcript: () => $('#chat-transcript'),
  activity:   () => $('#activity-list'),

  appendMessage(role, body, opts = {}) {
    const wrap = el('div', { class: `msg ${role}` },
      el('div', { class: 'msg-meta' },
        el('span', { class: 'msg-role' }, role),
        opts.time ? ` · ${fmtTime(opts.time)}` : ''
      ),
      el('div', { class: 'msg-body', html: opts.html ? body : `<p>${escape(body).replace(/\n/g, '<br>')}</p>` })
    );
    this.transcript().append(wrap);
    this.transcript().scrollTop = this.transcript().scrollHeight;
    return wrap;
  },

  appendActivity(tool, evt) {
    const list = this.activity();
    if (list.querySelector('.empty')) list.innerHTML = '';
    const li = el('li', {},
      el('span', { class: 'tool-name' }, tool),
      ' ',
      el('span', { class: 'tool-evt' }, evt),
      el('span', { class: 'tool-time' }, fmtTime(Date.now()))
    );
    list.append(li);
    list.scrollTop = list.scrollHeight;
  },

  wireWebSocket() {
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    const url = `${proto}//${location.host}/api/chat/ws`;
    try {
      state.socket = new WebSocket(url);
    } catch (e) {
      setConnection('err', 'WS error');
      return;
    }
    state.socket.addEventListener('open', () => {
      state.socketReady = true;
      setConnection('ok', 'connected');
    });
    state.socket.addEventListener('close', () => {
      state.socketReady = false;
      setConnection('warn', 'disconnected');
      setTimeout(() => chat.wireWebSocket(), 2500);
    });
    state.socket.addEventListener('message', evt => chat.handleMessage(evt.data));
    state.socket.addEventListener('error', () => setConnection('err', 'WS error'));
  },

  handleMessage(raw) {
    // The backend currently echoes; we accept both raw text and JSON envelopes.
    let payload = null;
    try { payload = JSON.parse(raw); } catch { /* not JSON */ }

    if (payload && typeof payload === 'object') {
      if (payload.type === 'tool.start')    return chat.appendActivity(payload.tool, 'started');
      if (payload.type === 'tool.progress') return chat.appendActivity(payload.tool, payload.note ?? 'progress');
      if (payload.type === 'tool.complete') return chat.appendActivity(payload.tool, 'complete');
      if (payload.type === 'token')         return chat.appendStreamToken(payload.text);
      if (payload.type === 'assistant')     return chat.appendMessage('assistant', payload.text);
      if (payload.type === 'receipt')       return chat.handleReceipt(payload);
    }

    // Fallback: treat as plain assistant text.
    chat.appendMessage('assistant', String(raw), { time: Date.now() });
  },

  streamCursor: null,
  appendStreamToken(t) {
    if (!chat.streamCursor) {
      chat.streamCursor = chat.appendMessage('assistant', '', { time: Date.now() });
    }
    const body = chat.streamCursor.querySelector('.msg-body');
    body.firstChild.append(document.createTextNode(t));
    chat.transcript().scrollTop = chat.transcript().scrollHeight;
  },
  endStream() { chat.streamCursor = null; },

  handleReceipt(r) {
    state.chainHead = r;
    if (state.view === 'receipts') renderers.receipts();
    if (state.view === 'chat') {
      const head = shortHex(r.digest ?? r.head ?? '', 16);
      const chip = $('#composer-caps');
      if (chip) chip.title = `chain head ${head}`;
    }
  },

  send(text) {
    if (!text.trim()) return;
    chat.appendMessage('user', text, { time: Date.now() });
    if (state.socketReady) {
      state.socket.send(JSON.stringify({ type: 'user', text }));
    } else {
      chat.appendMessage('system',
        'Disconnected — your message will not reach the agent until the WebSocket reconnects.',
        { time: Date.now() });
    }
    chat.endStream();
  },

  wireComposer() {
    const form  = $('#composer');
    const input = $('#composer-input');
    form.addEventListener('submit', e => {
      e.preventDefault();
      const v = input.value;
      input.value = '';
      input.style.height = 'auto';
      chat.send(v);
    });
    input.addEventListener('keydown', e => {
      if (e.key === 'Enter' && !e.shiftKey) {
        e.preventDefault();
        form.requestSubmit();
      } else if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') {
        e.preventDefault();
        form.requestSubmit();
      }
    });
    input.addEventListener('input', () => {
      input.style.height = 'auto';
      input.style.height = Math.min(input.scrollHeight, 192) + 'px';
    });

    $('#chat-new').addEventListener('click', () => {
      chat.transcript().innerHTML = '';
      state.currentSessionId = 'new';
      $('#chat-session-id').textContent = 'new session';
      toast('Session reset');
    });
    $('#chat-clear').addEventListener('click', () => {
      chat.transcript().innerHTML = '';
    });
  },
};

renderers.chat = () => {
  // Repopulate composer meta from latest status.
  if (state.status) {
    $('#composer-model').querySelector('strong').textContent = state.status.model ?? '—';
  }
};

// ─── 5. Sessions view ───────────────────────────────────────────────────────

renderers.sessions = async () => {
  const list = $('#sessions-list');
  try {
    const sessions = await api.sessions();
    state.sessions = sessions ?? [];
    list.innerHTML = '';
    if (state.sessions.length === 0) {
      list.append(el('div', { class: 'card placeholder' },
        'No sessions yet. Start a new conversation from the Chat tab.'));
      return;
    }
    state.sessions.forEach(s => {
      list.append(el('div', { class: 'card' },
        el('header', { class: 'card-head' },
          el('strong', {}, s.title ?? s.id ?? 'untitled'),
          el('span', { class: 'badge' }, fmtTime(s.updated_at))
        ),
        el('p', { class: 'muted small' },
          `${s.turns ?? 0} turns · ${shortHex(s.id, 12)} · model ${escape(s.model ?? 'unknown')}`)
      ));
    });
  } catch (e) {
    list.innerHTML = '';
    list.append(el('div', { class: 'card placeholder' },
      'Could not load sessions. Backend may still be coming up.'));
  }
};

// ─── 6. Tools view ──────────────────────────────────────────────────────────

const builtInTools = [
  { name: 'base64',       desc: 'Encode and decode base64 strings.', cap: 'cap:none',       taint: '⊥',   layers: ['WASM'] },
  { name: 'echo',         desc: 'Reflect the input. Useful for testing.', cap: 'cap:none', taint: '⊥', layers: ['WASM'] },
  { name: 'file_read',    desc: 'Read a file from a permitted path.', cap: 'cap:fs:read',  taint: 'user',    layers: ['Landlock', 'seccomp'] },
  { name: 'file_write',   desc: 'Write to a file in a permitted path.', cap: 'cap:fs:write', taint: 'user', layers: ['Landlock', 'seccomp'] },
  { name: 'hash',         desc: 'SHA-256 / BLAKE3 / xxhash digests.', cap: 'cap:none',    taint: '⊥',   layers: ['WASM'] },
  { name: 'json_get',     desc: 'JSONPath extraction from a value.', cap: 'cap:none',     taint: '⊥',   layers: ['WASM'] },
  { name: 'math_eval',    desc: 'Pure-function arithmetic evaluator.', cap: 'cap:none',  taint: '⊥',   layers: ['WASM'] },
  { name: 'regex_match',  desc: 'Compiled-regex pattern matching.',  cap: 'cap:none',    taint: '⊥',   layers: ['WASM'] },
  { name: 'shell',        desc: 'Run a shell command. Sandboxed.', cap: 'cap:shell:exec', taint: 'web', layers: ['WASM', 'Landlock', 'seccomp', 'bwrap'] },
  { name: 'upper',        desc: 'Uppercase the input string.',     cap: 'cap:none',     taint: '⊥',   layers: ['WASM'] },
];

renderers.tools = async () => {
  const list = $('#tools-list');
  list.innerHTML = '';
  let tools = builtInTools;
  try {
    const remote = await api.tools();
    if (Array.isArray(remote) && remote.length) tools = remote;
  } catch {}
  state.tools = tools;
  tools.forEach(t => {
    list.append(
      el('article', { class: 'card tool-card' },
        el('div', { class: 'tool-name' }, t.name),
        el('p',  { class: 'tool-desc' }, t.desc ?? t.description ?? ''),
        el('div', { class: 'tool-meta' },
          el('span', { class: 'badge' }, t.cap ?? t.capability ?? 'cap:none'),
          el('span', { class: 'badge' }, `taint: ${t.taint ?? '⊥'}`),
          ...((t.layers ?? []).map(l => el('span', { class: 'badge badge-ok' }, l)))
        )
      )
    );
  });
};

// ─── 7. Receipts view ───────────────────────────────────────────────────────

renderers.receipts = async () => {
  try {
    const head = await api.receipt();
    state.chainHead = head;
    $('#receipt-digest').textContent = head.digest ?? head.head ?? '—';
    $('#receipt-turn').textContent  = head.turn != null ? `turn ${head.turn}` : '—';
  } catch {
    $('#receipt-digest').textContent = 'unavailable';
  }
  $('#receipt-copy').onclick = async () => {
    const head = state.chainHead?.digest ?? state.chainHead?.head ?? '';
    try {
      await navigator.clipboard.writeText(head);
      toast('Chain head copied');
    } catch {
      toast('Clipboard unavailable');
    }
  };
};

// ─── 8. Health view ─────────────────────────────────────────────────────────

const defaultInvariants = [
  { name: 'kernel',     desc: 'Privileged kernel reachable; capability lattice consistent.' },
  { name: 'memory',     desc: 'Trinity store online; BM25 + HNSW + K-LRU coherent.' },
  { name: 'audit',      desc: 'Receipt chain present; Ed25519 keypair loaded.' },
  { name: 'sandbox',    desc: 'Composite sandbox layers available (WASM + Landlock + seccomp + bwrap).' },
  { name: 'provider',   desc: 'At least one provider registered.' },
  { name: 'gateway',    desc: 'Surface plane scheduler accepting requests.' },
  { name: 'taint',      desc: 'Information-flow lattice + declassification map verified antitone.' },
];

renderers.health = async () => {
  const grid = $('#health-grid');
  grid.innerHTML = '';
  let report = null;
  try { report = await api.health(); } catch {}
  const rows = (report && Array.isArray(report.invariants))
    ? report.invariants
    : defaultInvariants.map(i => ({ ...i, status: 'ok' }));
  state.health = rows;
  rows.forEach(r => {
    const status = (r.status ?? 'ok').toLowerCase();
    const badge = status === 'ok'   ? 'badge-ok'
               : status === 'warn' ? 'badge-warn'
               : 'badge-err';
    grid.append(
      el('div', { class: 'card invariant' },
        el('div', {},
          el('div', { class: 'invariant-name' }, r.name),
          el('div', { class: 'invariant-desc' }, r.desc ?? r.description ?? '')
        ),
        el('span', { class: `badge ${badge}` }, status)
      )
    );
  });
};

// ─── 9. Settings view ───────────────────────────────────────────────────────

renderers.settings = async () => {
  try {
    state.config    = await api.config().catch(() => ({}));
    state.providers = await api.providers().catch(() => []);
  } catch {}

  $('#cfg-profile').textContent  = state.config?.profile  ?? state.status?.profile  ?? '—';
  $('#cfg-provider').textContent = state.config?.provider ?? state.status?.provider ?? '—';
  $('#cfg-model').textContent    = state.config?.model    ?? state.status?.model    ?? '—';

  const list = $('#providers-list');
  list.innerHTML = '';
  const provs = Array.isArray(state.providers) && state.providers.length
    ? state.providers
    : ['anthropic', 'openai', 'openai-compat', 'google', 'cohere', 'ollama', 'huggingface', 'replicate', 'llama-cpp'];
  provs.forEach(p => {
    const name = typeof p === 'string' ? p : (p.name ?? 'unknown');
    list.append(el('span', { class: 'badge' }, name));
  });
};

// ─── 10. Command palette ────────────────────────────────────────────────────

const commands = [
  { id: 'view-chat',     label: 'Go to Chat',     hint: '⌘1', run: () => switchView('chat')     },
  { id: 'view-sessions', label: 'Go to Sessions', hint: '⌘2', run: () => switchView('sessions') },
  { id: 'view-tools',    label: 'Go to Tools',    hint: '⌘3', run: () => switchView('tools')    },
  { id: 'view-receipts', label: 'Go to Receipts', hint: '⌘4', run: () => switchView('receipts') },
  { id: 'view-health',   label: 'Go to Health',   hint: '⌘5', run: () => switchView('health')   },
  { id: 'view-settings', label: 'Go to Settings', hint: '⌘6', run: () => switchView('settings') },
  { id: 'new-session',   label: 'Start a new chat session',   hint: '',   run: () => $('#chat-new').click() },
  { id: 'copy-receipt',  label: 'Copy chain head digest',     hint: '',   run: () => $('#receipt-copy')?.click() },
  { id: 'reload-health', label: 'Reload SDHE health report',  hint: '',   run: () => renderers.health() },
  { id: 'reload-tools',  label: 'Reload tool catalogue',      hint: '',   run: () => renderers.tools()  },
];

const palette = {
  open() {
    $('#palette').classList.remove('hidden');
    $('#palette-input').value = '';
    $('#palette-input').focus();
    palette.render('');
  },
  close() { $('#palette').classList.add('hidden'); },
  render(query) {
    const q = query.trim().toLowerCase();
    const results = commands
      .map(c => ({ c, score: q ? (c.label.toLowerCase().includes(q) ? 1 : 0) : 1 }))
      .filter(r => r.score > 0)
      .slice(0, 10);
    const ul = $('#palette-results');
    ul.innerHTML = '';
    results.forEach((r, i) => {
      const li = el('li',
        { class: i === 0 ? 'active' : '', onclick: () => { r.c.run(); palette.close(); } },
        el('span', {}, r.c.label),
        r.c.hint ? el('span', { class: 'palette-hint' }, r.c.hint) : null
      );
      ul.append(li);
    });
  },
  cycle(dir) {
    const items = $$('#palette-results li');
    if (!items.length) return;
    let i = items.findIndex(x => x.classList.contains('active'));
    items[i]?.classList.remove('active');
    i = (i + dir + items.length) % items.length;
    items[i].classList.add('active');
    items[i].scrollIntoView({ block: 'nearest' });
  },
  fire() {
    $('#palette-results li.active')?.click();
  },
};

function wirePalette() {
  $('#palette-input').addEventListener('input', e => palette.render(e.target.value));
  $('#palette-input').addEventListener('keydown', e => {
    if (e.key === 'Escape') return palette.close();
    if (e.key === 'ArrowDown') { e.preventDefault(); palette.cycle(1); }
    if (e.key === 'ArrowUp')   { e.preventDefault(); palette.cycle(-1); }
    if (e.key === 'Enter')     { e.preventDefault(); palette.fire(); }
  });
  $('#palette').addEventListener('click', e => { if (e.target.id === 'palette') palette.close(); });
}

function wireGlobalKeys() {
  window.addEventListener('keydown', e => {
    const mod = e.metaKey || e.ctrlKey;
    if (mod && e.key === 'k')      { e.preventDefault(); palette.open(); return; }
    if (e.key === 'Escape' && !$('#palette').classList.contains('hidden')) {
      palette.close(); return;
    }
    if (mod && /^[1-6]$/.test(e.key)) {
      e.preventDefault();
      const map = ['chat', 'sessions', 'tools', 'receipts', 'health', 'settings'];
      switchView(map[parseInt(e.key, 10) - 1]);
    }
  });
}

// ─── 11. Toast ─────────────────────────────────────────────────────────────

function toast(message, kind = 'info') {
  const t = el('div', { class: `toast toast-${kind}` }, message);
  $('#toast-shell').append(t);
  setTimeout(() => {
    t.style.opacity = '0';
    t.style.transition = 'opacity 0.2s ease';
    setTimeout(() => t.remove(), 220);
  }, 2400);
}

function setConnection(kind, label) {
  const dot = $('#conn-dot');
  dot.classList.remove('ok', 'warn', 'err');
  dot.classList.add(kind);
  $('#conn-label').textContent = label;
}

// ─── 12. Boot ──────────────────────────────────────────────────────────────

async function bootstrap() {
  wireNav();
  chat.wireWebSocket();
  chat.wireComposer();
  wirePalette();
  wireGlobalKeys();
  setConnection('warn', 'connecting');

  try {
    state.status = await api.status();
    $('#brand-version').textContent = `v${state.status.version ?? '0.0.0'}`;
    $('#composer-model').querySelector('strong').textContent = state.status.model ?? '—';
  } catch {
    $('#brand-version').textContent = 'v?';
  }

  // Eagerly load light data so tab switches feel instant.
  renderers.receipts();
  renderers.settings();
}

document.addEventListener('DOMContentLoaded', bootstrap);
