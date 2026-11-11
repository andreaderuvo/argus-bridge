// argus-bridge's window. Everything it shows comes from the app's "snapshot" (sent after every round
// of every folder); everything it does is one command to the app. No framework, no build step.
import { t, bytes, langChosen } from './words.js';

if (!window.__TAURI__) await import('./mock.js');          // a browser, for development: a pretend app
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (sel) => document.querySelector(sel);
// A handful of line icons, drawn here so they look the same with every system font.
const PATHS = {
  more: 'M5 12h.01M12 12h.01M19 12h.01',
  down: 'M12 4v14M6 12l6 6 6-6',
  up: 'M12 20V6M6 12l6-6 6 6',
  x: 'M6 6l12 12M18 6L6 18',
  warn: 'M12 3l10 18H2zM12 10v5M12 18h.01',
  plus: 'M12 5v14M5 12h14',
  later: 'M12 7v5l3 2M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18z',
  folder: 'M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z',
  screen: 'M3 5h18v12H3zM8 21h8M12 17v4',
  globe: 'M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18zM3 12h18M12 3c2.5 2.5 3.5 5.5 3.5 9s-1 6.5-3.5 9c-2.5-2.5-3.5-5.5-3.5-9s1-6.5 3.5-9z',
  terminal: 'M4 5h16v14H4zM7.5 10l2.5 2-2.5 2M12.5 14h4',
  play: 'M8 5.5v13l10.5-6.5z',
  key: 'M14.5 4a5.5 5.5 0 1 1-4.7 8.4L4 18.2V21h3v-2h2v-2h2l1.4-1.4A5.5 5.5 0 0 1 14.5 4zM16 8.5h.01',
  tag: 'M3 12V4a1 1 0 0 1 1-1h8l9 9-9 9-9-9zM8 8h.01',
  pencil: 'M4 20h4L19 9l-4-4L4 16v4zM13.5 6.5l4 4',
  gear: 'M12 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6zM19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z',
  check: 'M5 12.5l4.5 4.5L19 7.5',
  trash: 'M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13',
  shield: 'M12 3l7 3v6c0 4.5-3 7.6-7 9-4-1.4-7-4.5-7-9V6z',
  external: 'M14 4h6v6M20 4l-9 9M18 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1h5',
  link: 'M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1',
  chev: 'M9 6l6 6-6 6',
  sync: 'M20 11a8 8 0 0 0-14.3-4.6L4 8M4 4v4h4M4 13a8 8 0 0 0 14.3 4.6L20 16M20 20v-4h-4',
  pause: 'M8 5v14M16 5v14',
  update: 'M12 3v12M7 10l5 5 5-5M4 21h16',
};
function icon(name) {
  const ns = 'http://www.w3.org/2000/svg';
  const s = document.createElementNS(ns, 'svg');
  s.setAttribute('viewBox', '0 0 24 24');
  s.setAttribute('class', `ico ${name}`);
  s.setAttribute('aria-hidden', 'true');
  const p = document.createElementNS(ns, 'path');
  p.setAttribute('d', PATHS[name]);
  s.append(p);
  return s;
}
function el(tag, props = {}, kids = []) {
  const n = Object.assign(document.createElement(tag), props);
  for (const k of [].concat(kids)) if (k != null && k !== false) n.append(k);
  return n;
}

let snap = { pairs: [], paused_all: false, computer: '' };
const open = new Set();                     // the activity lists someone opened, kept across repaints

const STATES = {
  idle: ['Up to date', 'ok'],
  syncing: ['Syncing…', 'busy'],
  offline: ['Cannot reach the server', 'bad'],
  refused: ['Key taken back', 'bad'],
  paused: ['Paused', 'warn'],
  starting: ['Starting…', ''],
};
const KINDS = {
  down: ['down', 'Downloaded'], up: ['up', 'Uploaded'], 'deleted-here': ['x', 'Deleted here'], 'deleted-there': ['x', 'Deleted on the server'],
  'folder-here': ['plus', 'New folder here'], 'folder-there': ['plus', 'New folder on the server'], conflict: ['warn', 'Conflict'], later: ['later', 'Later'],
};

/** What the app said — `{"say": template, "values": {…}}` or plain words — in the window's language. */
function explain(x) {
  const s = String(x?.message ?? x);
  try {
    const m = JSON.parse(s);
    if (m && m.say) return t(m.say, m.values || {});
  } catch { /* plain words */ }
  return t(s);
}

// The engine's reasons for leaving a file for later, said in the window's language.
const LATER = [
  ['changed on this computer meanwhile', 'changed on this computer meanwhile'],
  ['could not be replaced', 'could not be replaced — open in another program?'],
  ['differs from the server', 'differs from the server’s copy, which is too big to fetch without asking'],
];
function laterText(why) {
  const hit = LATER.find(([start]) => String(why).startsWith(start));
  return hit ? t(hit[1]) : why;
}

function toast(text) {
  const n = $('#toast');
  n.textContent = text;
  n.hidden = false;
  clearTimeout(toast.timer);
  toast.timer = setTimeout(() => { n.hidden = true; }, 3200);
}

async function run(what, ...args) {
  try { return await invoke(what, ...args); } catch (e) { toast(explain(e)); throw e; }
}

// Opened by the app, and only inside a synced folder.
const reveal = (id, rel = null) => invoke('open_in', { id, rel }).catch((e) => toast(explain(e)));
const pickFolder = (defaultPath) => invoke('plugin:dialog|open', { options: { directory: true, defaultPath } });
const timeOf = (at) => (at || '').slice(11, 16);

// ------------------------------------------------------------------------------------- paint

/* A folded section opened (or closed) by hand stays so across repaints — the state arrives every few
 * seconds and redrew every section closed ("when I open the sections they close again", 2026-11-10). */
const folded = new Map();
function foldable(id, opened, children, cls) {
  const d = el('details', { className: cls, open: folded.has(id) ? folded.get(id) : opened }, children);
  d.dataset.fold = id;
  d.ontoggle = () => folded.set(id, d.open);
  return d;
}

/* Typing in a field of the window: the repaint waits until the field is left, or what was typed and
 * the cursor went with it. */
let paintLater = false;
function typing() {
  const a = document.activeElement;
  return !!a && a.closest('#main') && (a.tagName === 'TEXTAREA' || a.tagName === 'SELECT' || (a.tagName === 'INPUT' && !['checkbox', 'radio', 'button'].includes(a.type)));
}
function paintIfHeld() { if (paintLater && !typing() && !pressed) { paintLater = false; paint(); } }
document.addEventListener('focusout', () => setTimeout(paintIfHeld, 0));

/* A press waits for its click: drawn again between press and release, the button pressed is gone and
 * the click with it — "Open Argus did not open at the first click", "the sidebar's clicks do not always
 * work" (2026-11-11), with the state arriving several times a second while folders sync. The repaint
 * comes after the click (a timeout after the release runs after it). */
let pressed = false;
document.addEventListener('pointerdown', () => { pressed = true; }, true);
const released = () => setTimeout(() => { pressed = false; paintIfHeld(); }, 0);
document.addEventListener('pointerup', released, true);
document.addEventListener('pointercancel', released, true);

/** A switch: a real checkbox (keyboard, screen readers) under a drawn knob. */
function toggle(checked, onchange, { disabled = false, id } = {}) {
  const box = el('input', { type: 'checkbox', checked: !!checked, disabled, onchange: () => onchange(box.checked) });
  if (id) box.id = id;
  return el('label', { className: 'toggle' }, [box, el('span', { className: 'knob' })]);
}

/** One line of a settings panel: an icon, what it is (and a line under it), its control on the right. */
function row({ glyph, title, hint, control, cls = '', on = false }) {
  return el('div', { className: `row ${cls}${on ? ' on' : ''}` }, [
    glyph ? (() => { const g = icon(glyph); g.classList.add('lead'); return g; })() : null,
    el('div', { className: 'label' }, [el('b', { textContent: title }), hint ? el('span', { className: 'hint', textContent: hint }) : null]),
    control ? el('div', { className: 'control' }, [].concat(control)) : null,
  ]);
}

function group(caption, rows, note) {
  return el('section', { className: 'group' }, [
    caption ? el('div', { className: 'groupcap' }, [].concat(caption)) : null,
    el('div', { className: 'panel' }, rows.filter(Boolean)),
    note ? el('p', { className: 'groupnote', textContent: note }) : null,
  ]);
}

function pageHead(title, lede, acts = [], extra = null) {
  return el('header', { className: 'pagehead' }, [
    el('h1', {}, [title, extra]),
    acts.length ? el('div', { className: 'acts' }, acts) : null,
    lede ? el('p', { className: 'lede', textContent: lede }) : null,
  ]);
}

// ------------------------------------------------------------------------------------- pages

/* Four pages, chosen in the sidebar (tabs along the top when the window is narrow); the one last open
 * comes back. */
const PAGES = () => [
  ['folders', 'folder', t('Folders')],
  ['computer', 'screen', t('This computer')],
  ['logins', 'key', t('Logins')],
  ['settings', 'gear', t('Settings')],
];
let page = (() => { try { return localStorage.getItem('argus-bridge.page') || 'folders'; } catch { return 'folders'; } })();
function showPage(to) {
  page = to;
  try { localStorage.setItem('argus-bridge.page', to); } catch { /* remembered for this window only */ }
  // Going elsewhere is leaving the field typed in: the page is drawn now, not when it is left.
  if (typing()) document.activeElement.blur();
  paintLater = false;
  paint(true);
  $('#main').scrollTop = 0;
}

function navBadge(key) {
  if (key === 'folders') {
    const conflicts = snap.pairs.reduce((n, p) => n + (p.status?.conflicts?.length || 0), 0);
    if (conflicts) return el('span', { className: 'count warn', textContent: String(conflicts), title: t('Conflicts') });
    return snap.pairs.length ? el('span', { className: 'count', textContent: String(snap.pairs.length) }) : null;
  }
  if (key === 'computer' && snap.machine) {
    const [, tone] = MACHINE[snap.machine.state] || MACHINE.connecting;
    return el('span', { className: `pip ${tone}` });
  }
  if (key === 'logins') {
    const n = snap.machine?.logins?.accounts?.length || 0;
    return n ? el('span', { className: 'count', textContent: String(n) }) : null;
  }
  if (key === 'settings' && snap.update) return el('span', { className: 'pip ok', title: t('argus-bridge {version} is ready', { version: snap.update }) });
  return null;
}

function paint(now = false) {
  if (!now && (typing() || pressed)) { paintLater = true; return; }
  $('#computer').textContent = snap.computer ? t('on {computer}', { computer: snap.computer }) : '';
  // Argus itself in a window of this app — when there is a server to open: this computer's, or a folder's.
  const argus = $('#argus');
  // Built once per language, not at every state: a button rebuilt under the pointer loses its click.
  if (argus.dataset.lang !== document.documentElement.lang + t('Open Argus')) {
    argus.dataset.lang = document.documentElement.lang + t('Open Argus');
    argus.replaceChildren(icon('external'), t('Open Argus'));
  }
  argus.title = t('Argus in a window of its own, with the notifications of this computer');
  argus.hidden = !snap.machine && !snap.pairs.length;
  $('#version').textContent = snap.version ? `argus-bridge ${snap.version}` : '';
  $('#nav').replaceChildren(...PAGES().map(([key, glyph, word]) => {
    const b = el('button', { type: 'button', className: key === page ? 'on' : '', onclick: () => showPage(key) }, [icon(glyph), word, navBadge(key)]);
    b.dataset.page = key;
    if (key === page) b.setAttribute('aria-current', 'page');
    return b;
  }));
  const main = $('#main');
  const keep = main.scrollTop;
  const body = { folders: foldersPage, computer: computerPage, logins: loginsPage, settings: settingsPage }[page] || foldersPage;
  main.replaceChildren(el('div', { className: 'page' }, [updateBar(), ...body()].filter(Boolean)));
  main.scrollTop = keep;
}

function foldersPage() {
  const add = el('button', { className: 'primary', type: 'button', id: 'add', onclick: () => connectSheet() }, [icon('plus'), t('Connect a folder')]);
  const pause = el('button', { type: 'button', id: 'pauseall', hidden: !snap.pairs.length,
    onclick: () => run('pause_all', { paused: !snap.paused_all }) }, [icon(snap.paused_all ? 'play' : 'pause'), snap.paused_all ? t('Resume all') : t('Pause all')]);
  return [
    pageHead(t('Folders'), t('Folders of an Argus server kept in sync with folders on this computer — both ways.'), [pause, add]),
    ...(snap.pairs.length ? snap.pairs.map(card) : [emptyState()]),
  ];
}

function emptyState() {
  const input = el('input', { type: 'text', placeholder: 'argus-bridge://pair?…', spellcheck: false });
  const next = el('button', { className: 'primary', type: 'button', textContent: t('Continue'), onclick: () => takeLink(input.value) });
  input.onkeydown = (e) => { if (e.key === 'Enter') next.click(); };
  const step = (n, words) => el('li', {}, [el('span', { className: 'num', textContent: String(n) }), el('span', { textContent: words })]);
  return el('section', { className: 'empty' }, [
    el('div', { className: 'mark' }, icon('sync')),
    el('h2', { textContent: t('No folder connected yet') }),
    el('p', { textContent: t('A folder of an Argus server, kept in sync with a folder on this computer — both ways.') }),
    el('ol', { className: 'steps' }, [
      step(1, t('In Argus, open the ⋮ menu of a folder → Sync to your computer…')),
      step(2, t('Click the link it gives: it opens here.')),
      step(3, t('Choose where the folder goes on this computer.')),
    ]),
    el('p', { textContent: t('Or paste the link:') }),
    el('div', { className: 'pasterow' }, [input, next]),
  ]);
}

function card(p) {
  const s = p.status || {};
  const [label, tone] = STATES[s.state] || STATES.starting;
  const head = el('div', { className: 'cardhead' }, [
    el('span', { className: `dot ${s.state || ''}` }),
    el('span', { className: 'name', textContent: p.name }),
    el('span', { className: 'server', textContent: t('on {host}', { host: p.host }) }),
    el('span', { className: `chip ${tone}`, textContent: t(label) }),
    s.write === false ? el('span', { className: 'chip', textContent: t('read only') }) : null,
    el('span', { className: 'spacer' }),
    el('button', { className: 'ghost small iconbtn', type: 'button', title: t('More'), ariaLabel: t('More'), onclick: (e) => moreMenu(e, p) }, icon('more')),
  ]);
  const where = el('div', { className: 'where' }, [
    icon('folder'),
    el('span', { className: 'path mono', title: p.local }, el('bdi', { textContent: p.local })),
    el('button', { className: 'small', type: 'button', onclick: () => reveal(p.id) }, [icon('external'), t('Open')]),
  ]);
  const line = s.message
    ? el('div', { className: 'line bad', textContent: explain(s.message) })
    : el('div', { className: 'line', textContent: s.last_ok ? t('Last synced {when}', { when: timeOf(s.last_ok) }) : '' });
  const acts = el('div', { className: 'acts' }, [
    el('button', { className: 'small', type: 'button', onclick: () => run('set_paused', { id: p.id, paused: !p.paused }) },
      [icon(p.paused ? 'play' : 'pause'), p.paused ? t('Resume') : t('Pause')]),
    el('button', { className: 'small', type: 'button', disabled: p.paused, onclick: () => run('sync_now') }, [icon('sync'), t('Sync now')]),
  ]);
  return el('article', { className: 'card' }, [head, where, line, acts, conflicts(p), waiting(p), notes(p), activity(p)]);
}

function conflicts(p) {
  const list = p.status?.conflicts || [];
  if (!list.length) return null;
  return el('section', { className: 'sect alert' }, [
    el('h3', {}, [icon('warn'), t('Conflicts'), el('span', { className: 'n', textContent: String(list.length) })]),
    el('p', { className: 'hint', textContent: t('Changed on both sides. The server’s version is in place; yours is the copy:') }),
    ...list.map((copy) => el('div', { className: 'item' }, [
      el('a', { className: 'what mono link', href: '#', textContent: copy, title: t('Open'), onclick: (e) => { e.preventDefault(); reveal(p.id, copy); } }),
      el('button', { className: 'small', type: 'button', textContent: t('Keep mine'), onclick: () => run('resolve', { id: p.id, copy, choice: 'mine' }) }),
      el('button', { className: 'small', type: 'button', textContent: t('Keep the server’s'), onclick: () => run('resolve', { id: p.id, copy, choice: 'theirs' }) }),
      el('button', { className: 'small', type: 'button', textContent: t('Keep both'), onclick: () => run('resolve', { id: p.id, copy, choice: 'both' }) }),
    ])),
  ]);
}

function waiting(p) {
  const list = p.status?.big || [];
  if (!list.length) return null;
  return el('section', { className: 'sect' }, [
    el('h3', {}, [t('Waiting to be asked for'), el('span', { className: 'n', textContent: String(list.length) })]),
    el('p', { className: 'hint', textContent: t('Over {mb} MB: fetched only when you say.', { mb: p.big_mb }) }),
    ...list.map((w) => el('div', { className: 'item' }, [
      el('span', { className: 'what mono', textContent: w.path }),
      el('span', { className: 'size', textContent: bytes(w.size) }),
      el('button', { className: 'small', type: 'button', textContent: t('Fetch'), onclick: () => run('fetch', { id: p.id, path: w.path }) }),
    ])),
  ]);
}

function notes(p) {
  const list = p.status?.notes || [];
  if (!list.length) return null;
  return el('section', { className: 'sect' }, [
    el('h3', {}, [t('Notes'), el('span', { className: 'n', textContent: String(list.length) })]),
    ...list.map((n) => el('div', { className: 'item hint', textContent: noteText(n) })),
  ]);
}

function noteText(n) {
  if (typeof n === 'string') return n;
  if (n.kind === 'case') return t('{skipped} differs from {kept} only by upper/lower case: this computer keeps {kept}', n);
  if (n.kind === 'name') {
    const why = n.why.startsWith('a character') ? 'a character Windows does not allow'
      : n.why.startsWith('a name ending') ? 'a name ending with a dot or a space' : 'a name Windows reserves';
    return t('{path} cannot exist on this computer: {why}', { path: n.path, why: t(why) });
  }
  if (n.kind === 'held') return t('{count} files are gone from this folder and were not deleted on the server: too many at once, or the folder is empty. Put them back, or delete them in Argus if you meant it.', n);
  if (n.kind === 'type') return t('{path} is a file on one side and a folder on the other: left alone', n);
  return JSON.stringify(n);
}

function activity(p) {
  const list = p.status?.recent || [];
  const d = el('details', { className: 'sect', open: open.has(p.id) }, [
    el('summary', {}, [icon('chev'), `${t('Recent activity')} (${list.length})`]),
    ...(list.length ? list.slice(0, 30).map((a) => {
      const [glyph, word] = KINDS[a.kind] || ['later', a.kind];
      const detail = a.kind === 'down' || a.kind === 'up' ? bytes(Number(a.detail)) : a.kind === 'later' ? laterText(a.detail) : a.detail;
      return el('div', { className: 'act', title: `${t(word)} — ${a.path}${detail ? ` — ${detail}` : ''}` }, [
        el('span', { className: `k ${a.kind}` }, icon(glyph)),
        el('span', { className: 'p mono', textContent: a.path + (a.kind === 'conflict' ? ` → ${a.detail}` : '') }),
        el('span', { className: 't', textContent: timeOf(a.at) }),
      ]);
    }) : [el('p', { className: 'hint', textContent: t('Nothing has moved yet.') })]),
  ]);
  d.ontoggle = () => (d.open ? open.add(p.id) : open.delete(p.id));
  return d;
}

// ------------------------------------------------------------------------------- a newer version

/* Found on GitHub and signed by us (the app checks the signature): one press installs it and
 * starts again — the folders and this computer's connection carry on as they were. */
function updateBar() {
  if (!snap.update) return null;
  const go = el('button', { className: 'primary', type: 'button', textContent: t('Update and restart') });
  go.onclick = async () => {
    go.disabled = true;
    go.textContent = t('Updating…');
    try { await invoke('update_now'); } catch (e) { toast(explain(e)); go.disabled = false; go.textContent = t('Update and restart'); }
  };
  return el('div', { className: 'updatebar', role: 'status' }, [
    el('span', {}, [el('b', { textContent: t('argus-bridge {version} is ready', { version: snap.update }) }), ' ',
      el('span', { className: 'hint', textContent: t('you have {version}', { version: snap.version }) })]),
    go,
  ]);
}

/* The version, a check for a newer one, whether newer ones install by themselves or are asked first
 * ("someone may not want it automatic", 2026-11-09), and the language. */
function settingsPage() {
  const check = el('button', { type: 'button', id: 'checkupdates' }, [icon('sync'), t('Check for updates')]);
  check.onclick = async () => {
    check.disabled = true;
    check.lastChild.textContent = t('Checking…');
    try { await invoke('check_updates'); } catch (e) { toast(explain(e)); }
    check.disabled = false;
    check.lastChild.textContent = t('Check for updates');
  };
  const language = el('select', { id: 'language', title: t('Language') });
  for (const [v, name] of [['', t('Language of the system')], ['en', 'English'], ['it', 'Italiano'], ['es', 'Español'], ['fr', 'Français']]) {
    language.append(el('option', { value: v, textContent: name, selected: v === langChosen }));
  }
  language.onchange = async () => {
    try { localStorage.setItem('argus-bridge.lang', language.value); } catch { /* the app keeps it too */ }
    await run('set_language', { lang: language.value });
    location.reload();
  };
  return [
    pageHead(t('Settings'), t('This app’s own: how it updates, and the language it speaks.')),
    group(t('Updates'), [
      row({ glyph: 'update', title: `argus-bridge ${snap.version || ''}`, hint: snap.update ? t('argus-bridge {version} is ready', { version: snap.update }) : t('Only versions signed by the project are installed.'), control: check }),
      row({ glyph: 'check', title: t('Install updates by themselves'), hint: t('Off: a new version is offered first, and installed when you say so'),
        control: toggle(snap.auto_update, (on) => run('set_auto_update', { on }), { id: 'autoupdate' }) }),
    ]),
    group(t('Language'), [row({ glyph: 'globe', title: t('Language'), hint: t('Of this window and of the menu'), control: language })]),
  ];
}

// ------------------------------------------------------------------------------- this computer

/* This computer, connected to an Argus server: what it lends, each one a switch, all off until
 * turned on — its browser (a Chrome of its own, for the agents there), a terminal (a shell here, in
 * a window of Argus), actions (a command an agent asks, which you allow in Argus). */
const SWITCHES = () => [
  ['browser', 'globe', t('Browser'), t('A Chrome of its own (none of your sessions), for the agents on the server')],
  ['terminal', 'terminal', t('Terminal'), t('A shell on this computer, in a window of Argus — for you')],
  ['actions', 'play', t('Actions'), t('A command an agent asks to run here — each one allowed by you, in Argus')],
];
const MACHINE = {
  connected: ['Connected', 'ok'], connecting: ['Connecting…', 'busy'], lost: ['Cannot reach the server', 'bad'],
  off: ['Off', ''], 'given-back': ['Disconnected in Argus', 'warn'],
};

function switches(offers, onChange) {
  return el('div', { className: 'switches' }, SWITCHES().map(([key, glyph, word, hint]) => {
    const box = el('input', { type: 'checkbox', checked: !!offers[key], onchange: () => onChange({ ...offers, [key]: box.checked }) });
    const row = el('label', { className: 'switch' }, [box, el('span', { className: 'knob' }), icon(glyph),
      el('span', { className: 'sw' }, [el('b', { textContent: word }), el('span', { className: 'hint', textContent: hint })])]);
    row.dataset.offer = key;
    return row;
  }));
}

function computerPage() {
  const m = snap.machine;
  if (!m) {
    return [
      pageHead(t('This computer'), t('Lend Argus this computer’s browser for its agents, a terminal for you, and commands an agent asks to run here — each a switch, off until you turn it on.')),
      el('section', { className: 'empty' }, [
        el('div', { className: 'mark' }, icon('screen')),
        el('h2', { textContent: t('Not connected to Argus') }),
        el('p', { textContent: t('In Argus: Settings → Computers → Connect a computer… — open the link it gives here.') }),
      ]),
    ];
  }
  const [label, tone] = MACHINE[m.state] || MACHINE.connecting;
  const set = (offers, on = m.on) => run('machine_set', { ...offers, on });
  const toggleOn = el('button', { type: 'button', onclick: () => set(m.offers, !m.on) }, [icon(m.on ? 'pause' : 'play'), m.on ? t('Disconnect') : t('Connect')]);
  const forget = el('button', { className: 'danger', type: 'button', onclick: () => sheet(
    t('Forget the connection to {host}?', { host: m.host }),
    [el('p', { textContent: t('This computer stops lending anything. To connect it again, make a new link in Argus.') })],
    [el('button', { type: 'button', textContent: t('Cancel'), onclick: shut }),
      el('button', { className: 'danger solid', type: 'button', textContent: t('Forget'), onclick: async () => { await run('machine_forget'); shut(); } })]) },
  [icon('trash'), t('Forget…')]);
  const lends = SWITCHES().map(([key, glyph, word, hint]) => {
    const r = row({ glyph, title: word, hint, on: !!m.offers[key],
      control: toggle(m.offers[key], (on) => set({ ...m.offers, [key]: on })) });
    r.classList.add('switch');
    r.dataset.offer = key;
    return r;
  });
  return [
    pageHead(t('This computer'), t('Connected to Argus on {host}: what it lends, each one a switch.', { host: m.host }), [toggleOn],
      el('span', { className: `pill ${tone}`, textContent: t(label) })),
    m.message ? el('div', { className: 'callout' }, [icon('warn'), el('div', { className: 'body' }, [el('span', { textContent: explain(m.message) })])]) : null,
    fullDiskNote(m),
    el('section', { className: 'group machine' }, [
      el('div', { className: 'groupcap', textContent: t('What it lends') }),
      el('div', { className: 'panel switches' }, lends),
    ]),
    m.offers.browser ? browserSettings(m) : null,
    group(t('Connection'), [
      row({ glyph: 'link', title: t('Argus on {host}', { host: m.host }), hint: t('The connection is kept: the app connects again when it starts, and when the network comes back.'), control: forget }),
    ]),
  ];
}

/* How the lent browser starts: the person's to decide ("can these be configured, so the user controls
 * the harness?", 2026-11-10). Hidden automation on by default — Google refuses to sign in a browser
 * that tells pages it is driven. Saved here, used from the browser's next start. */
function browserSettings(m) {
  const b = m.browser || { hide_automation: true, exe: '', flags: '' };
  const hide = el('input', { type: 'checkbox', checked: b.hide_automation !== false });
  const found = m.browsers || [];
  const pick = el('select', { id: 'browserexe' });
  pick.append(el('option', { value: '', textContent: t('The first one found') }));
  for (const path of found) pick.append(el('option', { value: path, textContent: path.split('/').filter((x) => x.endsWith('.app'))[0]?.replace(/\.app$/, '') || path }));
  pick.append(el('option', { value: '…', textContent: t('Another…') }));
  const other = el('input', { type: 'text', id: 'browserpath', placeholder: '/Applications/…/Contents/MacOS/…', spellcheck: false });
  pick.value = !b.exe ? '' : found.includes(b.exe) ? b.exe : '…';
  other.value = pick.value === '…' ? b.exe : '';
  const flags = el('input', { type: 'text', id: 'browserflags', value: b.flags || '', placeholder: '--window-size=1400,900', spellcheck: false });
  const save = el('button', { className: 'small primary', type: 'button', textContent: t('Save') });
  save.onclick = async () => {
    await run('machine_browser', { hideAutomation: hide.checked, exe: pick.value === '…' ? other.value : pick.value, flags: flags.value });
    toast(t('Saved: the browser starts this way from its next start'));
  };
  const hideRow = row({ glyph: 'shield', title: t('Don’t tell sites the browser is driven'),
    hint: t('recommended: Google refuses to sign in a browser that says so'),
    control: el('label', { className: 'toggle' }, [hide, el('span', { className: 'knob' })]) });
  const exeRow = row({ glyph: 'globe', title: t('Browser'), hint: t('Chrome, Chromium, Edge or Brave — or a path'), control: pick });
  const otherRow = el('div', { className: 'row stack', hidden: pick.value !== '…' }, [other]);
  pick.onchange = () => { otherRow.hidden = pick.value !== '…'; if (!otherRow.hidden) other.focus(); };
  const flagsRow = el('div', { className: 'row stack' }, [
    el('div', { className: 'label' }, [el('b', { textContent: t('More switches for it') }), el('span', { className: 'hint', textContent: t('Used from the browser’s next start.') })]),
    el('div', { className: 'control' }, [flags, save]),
  ]);
  const panel = el('section', { className: 'group browserset' }, [
    el('div', { className: 'groupcap', textContent: t('Browser settings') }),
    el('div', { className: 'panel' }, [hideRow, exeRow, otherRow, flagsRow]),
  ]);
  return panel;
}

/* Signing agents in with the person's saved logins (logins.rs): the vault in the system's keychain,
 * filled by hand or from a password manager's export, and 1Password read live. An agent never sees a
 * password: argus-bridge types it into the page, on its own site only, each time asked here first
 * unless the site is "Always". Off until switched on. */
function loginsPage() {
  const m = snap.machine;
  const lede = t('Sign agents in with the logins you already keep. The agent never sees a password: argus-bridge types it into the page, on its own site only, after you allow it here.');
  const head = pageHead(t('Logins'), lede);
  if (!m || !m.offers.browser) {
    return [head, el('section', { className: 'empty' }, [
      el('div', { className: 'mark' }, icon('key')),
      el('h2', { textContent: t('The browser is not lent') }),
      el('p', { textContent: t('Logins are for the browser this computer lends to the agents: switch Browser on in This computer.') }),
      el('button', { type: 'button', onclick: () => showPage('computer') }, [icon('screen'), t('This computer')]),
    ])];
  }
  const l = m.logins || { enabled: false, accounts: [], always: [] };
  const save = (enabled, onepassword) => run('logins_set', { enabled, onepassword });
  const importBtn = el('button', { type: 'button' }, [icon('down'), t('Import an export (CSV)…')]);
  importBtn.onclick = async () => {
    try {
      const n = await invoke('logins_import');
      if (n !== null && n !== undefined) toast(t('{n} logins kept in the keychain', { n }));
    } catch (e) { toast(explain(e)); }
  };
  const add = el('button', { className: 'primary', type: 'button', onclick: () => addLoginSheet() }, [icon('plus'), t('Add a login…')]);
  const rows = (l.accounts || []).map((a) => {
    const r = el('div', { className: 'row item login' }, [
      el('span', { className: 'favicon', textContent: (a.site.replace(/^www\./, '')[0] || '?') }),
      el('div', { className: 'label' }, [el('span', { className: 'site', textContent: a.site }), el('span', { className: 'acct', textContent: a.username })]),
      el('div', { className: 'badges' }, [
        // The code: what you tell an agent to use. Pressed to give one, change it or clear it.
        a.source === '1password' ? null : el('button', { type: 'button', className: `chip code${a.code ? ' on' : ''}`,
          title: a.code ? t('Tell an agent: sign in with {code}', { code: a.code }) : t('Give it a code to tell an agent which login to use'),
          onclick: () => editLoginSheet(a) }, [icon('tag'), a.code || t('add a code')]),
        a.has_otp ? el('span', { className: 'chip ok', textContent: t('one-time codes') }) : null,
        el('span', { className: 'chip', textContent: a.source === '1password' ? '1Password' : t('keychain') }),
      ]),
      a.source === '1password' ? null : el('button', { className: 'ghost iconbtn edit', type: 'button', title: t('Change its site or code'), ariaLabel: t('Change its site or code'),
        onclick: () => editLoginSheet(a) }, icon('pencil')),
      el('button', { className: 'ghost iconbtn remove', type: 'button', title: t('Remove this login'), ariaLabel: t('Remove this login'),
        onclick: () => run('login_remove', { site: a.site, username: a.username }) }, icon('trash')),
    ]);
    return r;
  });
  const always = (l.always || []).map((site) => el('span', { className: 'chip on' }, [site,
    el('button', { type: 'button', title: t('Ask again for this site'), ariaLabel: t('Ask again for this site'),
      onclick: () => run('login_forget_always', { site }) }, icon('x'))]));
  const kept = (l.accounts || []).filter((a) => a.source !== '1password').length;
  return [
    pageHead(t('Logins'), lede, [importBtn, add]),
    el('section', { className: 'group logins' }, [
      el('div', { className: 'panel' }, [
        row({ glyph: 'key', title: t('Sign agents in with my saved logins'), hint: t('Off until you switch it on. Each sign-in is asked here first, unless the site is “Always”.'),
          on: !!l.enabled, control: el('label', { className: 'toggle check' }, [el('input', { type: 'checkbox', checked: !!l.enabled, onchange: (e) => save(e.target.checked, !!l.onepassword) }), el('span', { className: 'knob' })]) }),
      ]),
    ]),
    group(t('Where they come from'), [
      row({ glyph: 'shield', title: t('The keychain of this computer'),
        hint: kept === 1 ? t('1 login, added here or imported — encrypted by the system') : t('{n} logins, added here or imported — encrypted by the system', { n: kept }) }),
      row({ glyph: 'key', title: '1Password', hint: l.onepassword_installed ? t('its logins too, read when needed — unlocked by its app')
        : t('install 1Password’s command line and turn on “Integrate with 1Password CLI” in its app'),
        control: toggle(l.onepassword, (on) => save(!!l.enabled, on), { disabled: !l.onepassword_installed }) }),
    ], t('Chrome: Settings → Passwords → Export. Apple Passwords: File → Export All Passwords. Firefox, Bitwarden and 1Password export the same kind of file.')),
    rows.length ? el('section', { className: 'group' }, [
      el('div', { className: 'groupcap' }, [t('Kept here'), el('span', { className: 'n', textContent: String(rows.length) })]),
      el('div', { className: 'panel loginlist' }, rows),
    ]) : el('p', { className: 'groupnote', textContent: t('No login kept here yet.') }),
    always.length ? el('section', { className: 'group' }, [
      el('div', { className: 'groupcap', textContent: t('Signed in without asking:') }),
      el('div', { className: 'panel' }, [el('div', { className: 'chips' }, always)]),
    ]) : null,
  ];
}

function addLoginSheet() {
  const site = el('input', { type: 'text', id: 'loginsite', placeholder: 'login.example.com', spellcheck: false });
  const user = el('input', { type: 'text', id: 'loginuser', autocomplete: 'off', spellcheck: false });
  const pass = el('input', { type: 'password', id: 'loginpass', autocomplete: 'new-password' });
  const otp = el('input', { type: 'text', id: 'loginotp', placeholder: t('optional — the key or the otpauth:// link'), spellcheck: false });
  const code = el('input', { type: 'text', id: 'logincode', placeholder: t('optional — for example WORK or LAB-2'), spellcheck: false, autocomplete: 'off' });
  const err = el('p', { className: 'err', hidden: true });
  const go = el('button', { className: 'primary', type: 'button', textContent: t('Keep it') });
  go.onclick = async () => {
    try {
      await invoke('login_add', { site: site.value, username: user.value, password: pass.value, otp: otp.value, code: code.value });
      pass.value = '';
      shut();
      toast(t('Kept in the keychain'));
    } catch (e) { err.textContent = explain(e); err.hidden = false; }
  };
  sheet(t('A login for agents'), [
    el('label', {}, [t('Site'), site, el('span', { className: 'hint', textContent: siteHint() })]),
    el('label', {}, [t('Account'), user]), el('label', {}, [t('Password'), pass]),
    el('label', {}, [t('One-time codes'), otp]),
    el('label', {}, [t('Code'), code, el('span', { className: 'hint', textContent: codeHint() })]), err,
  ], [el('button', { type: 'button', textContent: t('Cancel'), onclick: shut }), go]);
}

const codeHint = () => t('A name of yours for this login. Tell an agent “sign in with WORK” and it uses this one — useful when a site has several accounts. The agent never sees the account or the password.');

const siteHint = () => t('The site covers its subdomains: example.com is also used on login.example.com. For an account used across a whole domain, give the domain.');

/** A kept login changed: its site (the password stays where it is, not typed again) and its code. */
function editLoginSheet(a) {
  const site = el('input', { type: 'text', id: 'loginsite', value: a.site, spellcheck: false, autocomplete: 'off' });
  const code = el('input', { type: 'text', id: 'logincode', value: a.code || '', placeholder: t('optional — for example WORK or LAB-2'), spellcheck: false, autocomplete: 'off' });
  const err = el('p', { className: 'err', hidden: true });
  const go = el('button', { className: 'primary', type: 'button', textContent: t('Save') });
  go.onclick = async () => {
    try {
      await invoke('login_edit', { site: a.site, username: a.username, newSite: site.value.trim(), code: code.value.trim() });
      shut();
      toast(code.value.trim() ? t('Tell an agent: sign in with {code}', { code: code.value.trim() }) : t('Saved'));
    } catch (e) { err.textContent = explain(e); err.hidden = false; }
  };
  for (const box of [site, code]) box.onkeydown = (e) => { if (e.key === 'Enter') go.click(); };
  sheet(t('The login of {account}', { account: a.username }), [
    el('label', {}, [t('Site'), site, el('span', { className: 'hint', textContent: siteHint() })]),
    el('label', {}, [t('Code'), code, el('span', { className: 'hint', textContent: codeHint() })]), err,
  ], [el('button', { type: 'button', textContent: t('Cancel'), onclick: shut }), go]);
  setTimeout(() => (a.code ? site : code).focus(), 0);
}

/* On a Mac with Terminal or Actions lent and no Full Disk Access: macOS would stop an agent's command at
 * each protected folder to ask "Allow?", and the agent waits for whoever is at the computer. One switch,
 * once, and it never asks again; gone by itself once given (the app looks at every refresh). */
function fullDiskNote(m) {
  if (snap.full_disk !== false || !(m.offers.terminal || m.offers.actions)) return null;
  return el('section', { className: 'callout fulldisk' }, [icon('warn'), el('div', { className: 'body' }, [
    el('b', { textContent: t('Let agents work without asking') }),
    el('span', { className: 'hint', textContent: t('macOS stops a command that reads Documents, Desktop or another app’s data to ask “Allow?” — and the agent waits until someone answers. Give argus-bridge Full Disk Access once and it never asks again.') }),
    el('div', { className: 'acts' }, [
      el('button', { className: 'primary small', type: 'button', onclick: () => run('open_full_disk_settings') }, [icon('external'), t('Open the setting')]),
      el('span', { className: 'hint', textContent: t('Then switch argus-bridge on in the list (add it with + if it is not there).') }),
    ]),
  ])]);
}

/** A link from Argus to connect this computer: what it will lend, chosen before anything goes. */
function machineSheet(link) {
  let offers = { browser: false, terminal: false, actions: false };
  let host = '';
  try { host = new URL(decodeURIComponent((link.match(/url=([^&]+)/) || [])[1] || '')).hostname; } catch { /* said below */ }
  const go = el('button', { className: 'primary', type: 'button', textContent: t('Connect') });
  const box = sheet(t('Connect this computer to {host}?', { host: host || 'Argus' }), [
    el('p', { textContent: t('Nothing listens on this computer and nothing goes over SSH: it connects to Argus, the way the folders do. Choose what it lends — everything is off until you switch it on, and you can change it later.') }),
    switches(offers, (o) => { offers = o; }),
  ], [el('button', { type: 'button', textContent: t('Cancel'), onclick: shut }), go]);
  // The switches in a sheet keep their own state: read it back when Connect is pressed.
  go.onclick = async () => {
    for (const row of box.querySelectorAll('.switch')) offers[row.dataset.offer] = row.querySelector('input').checked;
    go.disabled = true;
    try {
      await invoke('machine_link', { link, ...offers });
      shut();
      showPage('computer');
      toast(t('Connecting to {host}…', { host }));
    } catch (e) {
      toast(explain(e));
      go.disabled = false;
    }
  };
}

const takeLink = (link) => (/^argus-(sync|bridge):\/\/computer/.test(String(link)) || /[?&]key=ac_/.test(String(link)) ? machineSheet(link) : connectSheet(link));

// ---------------------------------------------------------------------------------- menus, sheets

function closeMenu() { document.querySelector('.menu')?.remove(); }

function moreMenu(e, p) {
  closeMenu();
  const r = e.currentTarget.getBoundingClientRect();
  const m = el('div', { className: 'menu', role: 'menu' }, [
    el('button', { type: 'button', textContent: t('Move on this computer…'), onclick: () => { closeMenu(); moveIt(p); } }),
    el('button', { type: 'button', textContent: t('Large files…'), onclick: () => { closeMenu(); bigSheet(p); } }),
    el('button', { type: 'button', className: 'danger', textContent: t('Disconnect…'), onclick: () => { closeMenu(); disconnectSheet(p); } }),
  ]);
  document.body.append(m);
  m.style.top = `${Math.min(r.bottom + 4, innerHeight - m.offsetHeight - 8)}px`;
  m.style.left = `${Math.max(8, r.right - m.offsetWidth)}px`;
  setTimeout(() => addEventListener('pointerdown', function away(ev) {
    if (!m.contains(ev.target)) { closeMenu(); removeEventListener('pointerdown', away, true); }
  }, true));
}

function sheet(title, body, buttons) {
  const veil = $('#sheet');
  const box = el('div', { className: 'box', role: 'dialog', ariaModal: 'true' }, [el('h2', { textContent: title }), ...body, el('div', { className: 'foot' }, buttons)]);
  veil.replaceChildren(box);
  veil.hidden = false;
  veil.onpointerdown = (e) => { if (e.target === veil) shut(); };
  document.onkeydown = (e) => { if (e.key === 'Escape') shut(); };
  box.querySelector('input')?.focus();
  return box;
}
function shut() { $('#sheet').hidden = true; $('#sheet').replaceChildren(); document.onkeydown = null; }

async function connectSheet(link = '') {
  const input = el('input', { type: 'text', value: link, placeholder: 'argus-bridge://pair?…', spellcheck: false });
  const found = el('div', { hidden: true });
  const dest = el('input', { type: 'text', spellcheck: false });
  const destRow = el('label', { hidden: true }, [t('On this computer'), el('div', { className: 'row' }, [dest,
    el('button', { type: 'button', textContent: t('Choose…'), onclick: async () => {
      const picked = await pickFolder(dest.value || undefined).catch(() => null);
      if (picked) dest.value = picked;
    } })])]);
  const note = el('p', { className: 'hint', hidden: true, textContent: t('If the folder already has files, they are merged with the server’s: nothing is overwritten, differing files are kept both.') });
  const err = el('p', { className: 'err', hidden: true });
  const go = el('button', { className: 'primary', type: 'button', textContent: t('Connect'), disabled: true });
  let looked = '';
  const look = async () => {
    const value = input.value.trim();
    if (!value || value === looked) return;
    looked = value;
    err.hidden = true;
    found.hidden = false;
    found.className = 'hint';
    found.textContent = t('Looking…');
    try {
      const v = await invoke('inspect', { link: value });
      found.className = 'found';
      found.replaceChildren(el('img', { src: 'logo.png', width: 24, height: 24, alt: '' }), el('div', {}, [
        el('b', { textContent: t('{folder} on {host}', { folder: v.folder, host: v.host }) }),
        el('div', { className: 'hint', textContent: v.write ? t('both ways') : t('read only — this computer receives, the server does not change') }),
      ]));
      if (!dest.value) dest.value = v.suggested;
      destRow.hidden = false;
      note.hidden = false;
      go.disabled = false;
    } catch (e) {
      found.hidden = true;
      err.textContent = explain(e);
      err.hidden = false;
      go.disabled = true;
      looked = '';
    }
  };
  input.oninput = () => { clearTimeout(input.timer); input.timer = setTimeout(look, 300); };
  go.onclick = async () => {
    go.disabled = true;
    try {
      await invoke('connect', { link: input.value.trim(), local: dest.value.trim() });
      shut();
      toast(t('Connected: {name}', { name: dest.value.split(/[\\/]/).pop() }));
    } catch (e) {
      err.textContent = explain(e);
      err.hidden = false;
      go.disabled = false;
    }
  };
  sheet(t('Connect a folder from Argus'), [el('label', {}, [t('Link from Argus'), input]), found, destRow, note, err],
    [el('button', { type: 'button', textContent: t('Cancel'), onclick: shut }), go]);
  if (link) look();
}

function disconnectSheet(p) {
  sheet(t('Disconnect {name}?', { name: p.name }), [el('p', { textContent: t('Syncing stops. The files stay where they are, on this computer and on the server.') })], [
    el('button', { type: 'button', textContent: t('Cancel'), onclick: shut }),
    el('button', { className: 'danger', type: 'button', textContent: t('Disconnect'), onclick: async () => { await run('disconnect', { id: p.id }); shut(); } }),
  ]);
}

function bigSheet(p) {
  const mb = el('input', { type: 'number', min: 0, step: 50, value: p.big_mb });
  sheet(t('Large files'), [el('label', {}, [t('Files bigger than this wait for you to fetch them (0 = no limit).'),
    el('div', { className: 'row' }, [mb, el('span', { className: 'hint', textContent: 'MB' })])])], [
    el('button', { type: 'button', textContent: t('Cancel'), onclick: shut }),
    el('button', { className: 'primary', type: 'button', textContent: t('Save'), onclick: async () => {
      await run('set_big', { id: p.id, mb: Math.max(0, Math.round(Number(mb.value) || 0)) });
      shut();
    } }),
  ]);
}

async function moveIt(p) {
  const to = await pickFolder().catch(() => null);
  if (!to) return;
  await run('move_local', { id: p.id, to });
  toast(t('Moved.'));
}

// --------------------------------------------------------------------------------------- start

$('#argus').onclick = () => run('open_argus');
document.documentElement.lang = (navigator.language || 'en').slice(0, 2);
// The same state again changes nothing on screen: not drawn.
let lastSnap = '';
listen('snapshot', (e) => {
  const said = JSON.stringify(e.payload);
  if (said === lastSnap) return;
  lastSnap = said;
  snap = e.payload;
  paint();
});
listen('invite', (e) => takeLink(e.payload));
snap = await invoke('get_snapshot');
paint();
const pending = await invoke('take_invite');
if (pending) takeLink(pending);
