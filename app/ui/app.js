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

function paint() {
  $('#computer').textContent = snap.computer ? t('on {computer}', { computer: snap.computer }) : '';
  $('#add').textContent = `+ ${t('Connect a folder')}`;
  // Argus itself in a window of this app — when there is a server to open: this computer's, or a folder's.
  $('#argus').textContent = t('Open Argus');
  $('#argus').title = t('Argus in a window of its own, with the notifications of this computer');
  $('#argus').hidden = !snap.machine && !snap.pairs.length;
  $('#pauseall').textContent = snap.paused_all ? t('Resume all') : t('Pause all');
  $('#pauseall').hidden = !snap.pairs.length;
  const main = $('#main');
  const keep = main.scrollTop;
  main.replaceChildren(...[updateBar(), machineCard(), ...(snap.pairs.length ? snap.pairs.map(card) : [emptyState()]), aboutRow()].filter(Boolean));
  main.scrollTop = keep;
}

function emptyState() {
  const input = el('input', { type: 'text', placeholder: 'argus-bridge://pair?…', spellcheck: false });
  const go = el('button', { className: 'primary', type: 'button', textContent: t('Continue'), onclick: () => takeLink(input.value) });
  input.onkeydown = (e) => { if (e.key === 'Enter') go.click(); };
  return el('section', { className: 'empty' }, [
    el('h1', { textContent: t('No folder connected yet') }),
    el('p', { textContent: t('A folder of an Argus server, kept in sync with a folder on this computer — both ways.') }),
    el('ol', {}, [
      el('li', { textContent: t('In Argus, open the ⋮ menu of a folder → Sync to your computer…') }),
      el('li', { textContent: t('Click the link it gives: it opens here.') }),
      el('li', { textContent: t('Choose where the folder goes on this computer.') }),
    ]),
    el('p', { textContent: t('Or paste the link:') }),
    el('div', { className: 'pasterow' }, [input, go]),
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
    el('span', { className: 'path mono', title: p.local }, el('bdi', { textContent: p.local })),
    el('button', { className: 'small', type: 'button', onclick: () => reveal(p.id) }, [icon('folder'), t('Open')]),
  ]);
  const line = s.message
    ? el('div', { className: 'line bad', textContent: explain(s.message) })
    : el('div', { className: 'line', textContent: s.last_ok ? t('Last synced {when}', { when: timeOf(s.last_ok) }) : '' });
  const acts = el('div', { className: 'acts' }, [
    el('button', { className: 'small', type: 'button', textContent: p.paused ? t('Resume') : t('Pause'),
      onclick: () => run('set_paused', { id: p.id, paused: !p.paused }) }),
    el('button', { className: 'small', type: 'button', textContent: t('Sync now'), disabled: p.paused, onclick: () => run('sync_now') }),
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
    el('summary', { textContent: `${t('Recent activity')} (${list.length})` }),
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

/* The version, a check for a newer one, and whether newer ones install by themselves or are asked
 * first ("someone may not want it automatic", 2026-11-09). Asked first unless switched. */
function aboutRow() {
  const check = el('button', { className: 'small', type: 'button', textContent: t('Check for updates') });
  check.onclick = async () => {
    check.disabled = true;
    check.textContent = t('Checking…');
    try { await invoke('check_updates'); } catch (e) { toast(explain(e)); }
    check.disabled = false;
    check.textContent = t('Check for updates');
  };
  const auto = el('input', { type: 'checkbox', checked: !!snap.auto_update, onchange: () => run('set_auto_update', { on: auto.checked }) });
  const language = el('select', { id: 'language', title: t('Language') });
  for (const [v, name] of [['', t('Language of the system')], ['en', 'English'], ['it', 'Italiano'], ['es', 'Español'], ['fr', 'Français']]) {
    language.append(el('option', { value: v, textContent: name, selected: v === langChosen }));
  }
  language.onchange = async () => {
    try { localStorage.setItem('argus-bridge.lang', language.value); } catch { /* the app keeps it too */ }
    await run('set_language', { lang: language.value });
    location.reload();
  };
  return el('footer', { className: 'aboutrow' }, [
    el('span', { className: 'hint', textContent: `argus-bridge ${snap.version}` }),
    check,
    el('label', { className: 'autoupd', title: t('Off: a new version is offered first, and installed when you say so') }, [auto, t('Install updates by themselves')]),
    el('label', { className: 'langpick' }, [icon('globe'), language]),
  ]);
}

// ------------------------------------------------------------------------------- this computer

/* This computer, connected to an Argus server: what it lends, each one a switch, all off until
 * turned on — its browser (a Chrome of its own, for the agents there), a terminal (a shell here, in
 * a window of Argus), actions (a command an agent asks, which you allow in Argus). */
const SWITCHES = () => [
  ['browser', icon('globe'), t('Browser'), t('A Chrome of its own (none of your sessions), for the agents on the server')],
  ['terminal', icon('terminal'), t('Terminal'), t('A shell on this computer, in a window of Argus — for you')],
  ['actions', icon('play'), t('Actions'), t('A command an agent asks to run here — each one allowed by you, in Argus')],
];
const MACHINE = {
  connected: ['Connected', 'ok'], connecting: ['Connecting…', 'busy'], lost: ['Cannot reach the server', 'bad'],
  off: ['Off', ''], 'given-back': ['Disconnected in Argus', 'warn'],
};

function switches(offers, onChange) {
  return el('div', { className: 'switches' }, SWITCHES().map(([key, glyph, word, hint]) => {
    const box = el('input', { type: 'checkbox', checked: !!offers[key], onchange: () => onChange({ ...offers, [key]: box.checked }) });
    const row = el('label', { className: 'switch' }, [box, el('span', { className: 'knob' }), glyph,
      el('span', { className: 'sw' }, [el('b', { textContent: word }), el('span', { className: 'hint', textContent: hint })])]);
    row.dataset.offer = key;
    return row;
  }));
}

function machineCard() {
  const m = snap.machine;
  if (!m) return null;
  const [label, tone] = MACHINE[m.state] || MACHINE.connecting;
  const set = (offers, on = m.on) => run('machine_set', { ...offers, on });
  const toggle = el('button', { className: 'small', type: 'button', textContent: m.on ? t('Disconnect') : t('Connect'),
    onclick: () => set(m.offers, !m.on) });
  const forget = el('button', { className: 'small danger', type: 'button', textContent: t('Forget…'), onclick: () => sheet(
    t('Forget the connection to {host}?', { host: m.host }),
    [el('p', { textContent: t('This computer stops lending anything. To connect it again, make a new link in Argus.') })],
    [el('button', { type: 'button', textContent: t('Cancel'), onclick: shut }),
      el('button', { className: 'danger', type: 'button', textContent: t('Forget'), onclick: async () => { await run('machine_forget'); shut(); } })]) });
  const card = el('article', { className: 'card machine' }, [
    el('div', { className: 'cardhead' }, [
      el('span', { className: `dot ${m.state === 'connected' ? 'idle' : m.state === 'lost' ? 'offline' : m.state === 'connecting' ? 'syncing' : 'paused'}` }),
      icon('screen'),
      el('span', { className: 'name', textContent: t('This computer') }),
      el('span', { className: 'server', textContent: t('on {host}', { host: m.host }) }),
      el('span', { className: `chip ${tone}`, textContent: t(label) }),
    ]),
    m.message ? el('div', { className: 'line bad', textContent: explain(m.message) }) : null,
    switches(m.offers, (offers) => set(offers)),
    m.offers.browser ? browserSettings(m) : null,
    m.offers.browser ? loginsSection(m) : null,
    fullDiskNote(m),
    el('div', { className: 'acts' }, [toggle, forget]),
  ]);
  return card;
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
  other.hidden = pick.value !== '…';
  pick.onchange = () => { other.hidden = pick.value !== '…'; if (!other.hidden) other.focus(); };
  const flags = el('input', { type: 'text', id: 'browserflags', value: b.flags || '', placeholder: '--lang=it --window-size=1400,900', spellcheck: false });
  const save = el('button', { className: 'small primary', type: 'button', textContent: t('Save') });
  save.onclick = async () => {
    await run('machine_browser', { hideAutomation: hide.checked, exe: pick.value === '…' ? other.value : pick.value, flags: flags.value });
    toast(t('Saved: the browser starts this way from its next start'));
  };
  return el('details', { className: 'sect browserset' }, [
    el('summary', { textContent: t('Browser settings') }),
    el('label', { className: 'check' }, [hide, el('span', {}, [
      el('b', { textContent: t('Don’t tell sites the browser is driven') }), ' ',
      el('span', { className: 'hint', textContent: t('recommended: Google refuses to sign in a browser that says so') })])]),
    el('label', {}, [t('Browser'), el('div', { className: 'row' }, [pick]), other]),
    el('label', {}, [t('More switches for it'), flags]),
    el('div', { className: 'acts' }, [save, el('span', { className: 'hint', textContent: t('Used from the browser’s next start.') })]),
  ]);
}

/* Signing agents in with the person's saved logins (logins.rs): the vault in the system's keychain,
 * filled by hand or from a password manager's export, and 1Password read live. An agent never sees a
 * password: argus-bridge types it into the page, on its own site only, each time asked here first
 * unless the site is "Always". Off until switched on. */
function loginsSection(m) {
  const l = m.logins || { enabled: false, accounts: [], always: [] };
  const on = el('input', { type: 'checkbox', checked: !!l.enabled });
  const op = el('input', { type: 'checkbox', checked: !!l.onepassword, disabled: !l.onepassword_installed });
  const save = () => run('logins_set', { enabled: on.checked, onepassword: op.checked });
  on.onchange = save;
  op.onchange = save;
  const rows = (l.accounts || []).map((a) => el('div', { className: 'item login' }, [
    el('span', { className: 'what' }, [el('b', { textContent: a.site }), ' ', el('span', { className: 'hint', textContent: a.username })]),
    a.has_otp ? el('span', { className: 'chip', textContent: t('one-time codes') }) : null,
    el('button', { className: 'small ghost', type: 'button', title: t('Remove this login'), textContent: '✕',
      onclick: () => run('login_remove', { site: a.site, username: a.username }) }),
  ]));
  const always = (l.always || []).map((site) => el('span', { className: 'chip on' }, [site, ' ',
    el('button', { className: 'ghost iconx', type: 'button', title: t('Ask again for this site'), textContent: '✕',
      onclick: () => run('login_forget_always', { site }) })]));
  const importBtn = el('button', { className: 'small', type: 'button', textContent: t('Import an export (CSV)…') });
  importBtn.onclick = async () => {
    try {
      const n = await invoke('logins_import');
      if (n !== null && n !== undefined) toast(t('{n} logins kept in the keychain', { n }));
    } catch (e) { toast(explain(e)); }
  };
  return el('details', { className: 'sect logins', open: !!l.enabled && !(l.accounts || []).length }, [
    el('summary', { textContent: t('Logins for agents') }),
    el('label', { className: 'check' }, [on, el('span', {}, [el('b', { textContent: t('Sign agents in with my saved logins') }), ' ',
      el('span', { className: 'hint', textContent: t('the agent never sees a password: it is typed into the page, on its own site only, after you allow it here') })])]),
    el('label', { className: 'check' }, [op, el('span', {}, [el('b', { textContent: '1Password' }), ' ',
      el('span', { className: 'hint', textContent: l.onepassword_installed ? t('its logins too, read when needed — unlocked by its app')
        : t('install 1Password’s command line and turn on “Integrate with 1Password CLI” in its app') })])]),
    el('div', { className: 'acts' }, [importBtn,
      el('button', { className: 'small', type: 'button', textContent: t('Add a login…'), onclick: () => addLoginSheet() })]),
    el('p', { className: 'hint', textContent: t('Chrome: Settings → Passwords → Export. Apple Passwords: File → Export All Passwords. Firefox, Bitwarden and 1Password export the same kind of file.') }),
    rows.length ? el('div', { className: 'loginlist' }, rows) : el('p', { className: 'hint', textContent: t('No login kept here yet.') }),
    always.length ? el('div', {}, [el('div', { className: 'hint', textContent: t('Signed in without asking:') }), el('div', { className: 'chips' }, always)]) : null,
  ]);
}

function addLoginSheet() {
  const site = el('input', { type: 'text', id: 'loginsite', placeholder: 'wgsportal-test.efsa.europa.eu', spellcheck: false });
  const user = el('input', { type: 'text', id: 'loginuser', autocomplete: 'off', spellcheck: false });
  const pass = el('input', { type: 'password', id: 'loginpass', autocomplete: 'new-password' });
  const otp = el('input', { type: 'text', id: 'loginotp', placeholder: t('optional — the key or the otpauth:// link'), spellcheck: false });
  const err = el('p', { className: 'err', hidden: true });
  const go = el('button', { className: 'primary', type: 'button', textContent: t('Keep it') });
  go.onclick = async () => {
    try {
      await invoke('login_add', { site: site.value, username: user.value, password: pass.value, otp: otp.value });
      pass.value = '';
      shut();
      toast(t('Kept in the keychain'));
    } catch (e) { err.textContent = explain(e); err.hidden = false; }
  };
  sheet(t('A login for agents'), [
    el('label', {}, [t('Site'), site]), el('label', {}, [t('Account'), user]), el('label', {}, [t('Password'), pass]),
    el('label', {}, [t('One-time codes'), otp]), err,
  ], [el('button', { type: 'button', textContent: t('Cancel'), onclick: shut }), go]);
}

/* On a Mac with Terminal or Actions lent and no Full Disk Access: macOS would stop an agent's command at
 * each protected folder to ask "Allow?", and the agent waits for whoever is at the computer. One switch,
 * once, and it never asks again; gone by itself once given (the app looks at every refresh). */
function fullDiskNote(m) {
  if (snap.full_disk !== false || !(m.offers.terminal || m.offers.actions)) return null;
  return el('section', { className: 'sect fulldisk' }, [
    el('h3', {}, [icon('warn'), t('Let agents work without asking')]),
    el('p', { className: 'hint', textContent: t('macOS stops a command that reads Documents, Desktop or another app’s data to ask “Allow?” — and the agent waits until someone answers. Give argus-bridge Full Disk Access once and it never asks again.') }),
    el('div', { className: 'acts' }, [
      el('button', { className: 'primary small', type: 'button', textContent: t('Open the setting'), onclick: () => run('open_full_disk_settings') }),
      el('span', { className: 'hint', textContent: t('Then switch argus-bridge on in the list (add it with + if it is not there).') }),
    ]),
  ]);
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

$('#add').onclick = () => connectSheet();
$('#argus').onclick = () => run('open_argus');
$('#pauseall').onclick = () => run('pause_all', { paused: !snap.paused_all });
listen('snapshot', (e) => { snap = e.payload; paint(); });
listen('invite', (e) => takeLink(e.payload));
snap = await invoke('get_snapshot');
paint();
const pending = await invoke('take_invite');
if (pending) takeLink(pending);
