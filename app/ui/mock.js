// A pretend app, so the window can be opened in a browser while it is being made and tested
// (tests/ui). Loaded only where there is no window.__TAURI__ — never inside the real app.
const q = new URLSearchParams(location.search);
const listeners = {};
const calls = [];
const at = (m) => `2026-10-14 11:${String(m).padStart(2, '0')}:05`;
let state = {
  computer: 'lap684',
  version: '0.1.1',
  update: q.has('update') ? '0.1.2' : null,
  machine: q.has('machine') ? { host: 'lab-server', name: 'lap684', offers: { browser: true, terminal: true, actions: false }, on: true, state: 'connected', message: '',
    logins: { enabled: q.has('logins'), keep: q.has('keychain') ? 'keychain' : 'file', never_ask: false, onepassword: false, onepassword_installed: false, accounts: q.has('logins') ? [{ site: 'login.example.com', username: 'sam@example.com', source: 'vault', has_otp: true }] : [], always: [] },
    browser: { hide_automation: true, exe: '', flags: '' }, browsers: ['/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', '/Applications/Brave Browser.app/Contents/MacOS/Brave Browser'] } : null,
  full_disk: q.has('nofulldisk') ? false : null,
  auto_update: false,
  language: '',
  paused_all: false,
  pairs: q.has('empty') ? [] : [
    {
      id: 'a1', name: 'report-salmonella', url: 'http://lab-server:8090', host: 'lab-server', local: '/Users/sam/argus-bridge/report-salmonella',
      paused: false, big_mb: 500,
      status: {
        state: 'idle', message: '', last_ok: at(32), write: true,
        conflicts: ['tables/summary (conflict from lap684 2026-10-14 1128).xlsx'],
        big: [{ path: 'reads/S1_R1.fastq.gz', size: 4.2e9 }, { path: 'reads/S1_R2.fastq.gz', size: 4.4e9 }],
        notes: [{ kind: 'case', kept: 'figures/fig1.pdf', skipped: 'figures/Fig1.pdf' }],
        recent: [
          { at: at(31), kind: 'down', path: 'report/salmonella_2026-10.pdf', detail: '2483112' },
          { at: at(29), kind: 'up', path: 'notes/meeting.md', detail: '4210' },
          { at: at(28), kind: 'conflict', path: 'tables/summary.xlsx', detail: 'tables/summary (conflict from lap684 2026-10-14 1128).xlsx' },
          { at: at(20), kind: 'deleted-there', path: 'old/draft.docx', detail: '' },
          { at: at(12), kind: 'folder-here', path: 'figures', detail: '' },
        ],
      },
    },
    {
      id: 'b2', name: 'listeria-cgmlst', url: 'http://lab-server:8090', host: 'lab-server', local: '/Users/sam/Documents/listeria-cgmlst',
      paused: false, big_mb: 500,
      status: { state: 'syncing', message: '', last_ok: at(30), write: false, conflicts: [], big: [], notes: [], recent: [
        { at: at(32), kind: 'down', path: 'distances/cgdist_matrix.tsv', detail: '18342001' },
      ] },
    },
  ],
};
const emit = (name, payload) => (listeners[name] || []).forEach((f) => f({ payload }));
const find = (id) => state.pairs.find((p) => p.id === id);
window.__TAURI__ = {
  core: {
    async invoke(cmd, args = {}) {
      calls.push([cmd, args]);
      switch (cmd) {
        case 'get_snapshot': return structuredClone(state);
        case 'take_invite': return q.get('invite');
        case 'inspect':
          if (!/^argus-(sync|bridge):\/\//.test(String(args.link))) throw JSON.stringify({ say: 'That is not a link from Argus — it starts with argus-bridge://', values: {} });
          return { url: 'http://lab-server:8090', host: 'lab-server', name: 'wgs-batch-12', folder: 'wgs-batch-12', write: true, suggested: '/Users/sam/argus-bridge/wgs-batch-12' };
        case 'connect':
          state.pairs.push({ id: `n${state.pairs.length}`, name: 'wgs-batch-12', url: 'http://lab-server:8090', host: 'lab-server', local: args.local, paused: false, big_mb: 500,
            status: { state: 'starting', message: '', last_ok: null, write: true, conflicts: [], big: [], notes: [], recent: [] } });
          setTimeout(() => emit('snapshot', structuredClone(state)));
          return 'n';
        case 'set_paused': find(args.id).paused = args.paused; find(args.id).status.state = args.paused ? 'paused' : 'idle'; break;
        case 'pause_all': state.paused_all = args.paused; break;
        case 'set_big': find(args.id).big_mb = args.mb; break;
        case 'fetch': find(args.id).status.big = find(args.id).status.big.filter((b) => b.path !== args.path); break;
        case 'resolve': find(args.id).status.conflicts = find(args.id).status.conflicts.filter((c) => c !== args.copy); break;
        case 'disconnect': state.pairs = state.pairs.filter((p) => p.id !== args.id); break;
        case 'move_local': find(args.id).local = args.to; break;
        case 'plugin:dialog|open': return '/Users/sam/Desktop/chosen';
        case 'machine_link':
          state.machine = { host: 'lab-server', name: 'lap684', offers: { browser: args.browser, terminal: args.terminal, actions: args.actions }, on: true, state: 'connecting', message: '' };
          break;
        case 'set_auto_update': state.auto_update = args.on; break;
        case 'check_updates': break;
        case 'machine_browser': state.machine = { ...state.machine, browser: { hide_automation: args.hideAutomation, exe: args.exe, flags: args.flags } }; break;
        case 'logins_set': state.machine = { ...state.machine, logins: { ...state.machine.logins, enabled: args.enabled, onepassword: args.onepassword, ...(args.neverAsk === undefined ? {} : { never_ask: args.neverAsk }) } }; break;
        case 'login_add': state.machine.logins.accounts = [...state.machine.logins.accounts, { site: args.site, username: args.username, source: 'vault', has_otp: !!args.otp, code: args.code || '' }]; break;
        case 'logins_keep': state.machine.logins.keep = args.to; setTimeout(() => emit('snapshot', structuredClone(state))); return state.machine.logins.accounts.length;
        case 'login_edit': state.machine.logins.accounts = state.machine.logins.accounts.map((a) => (a.site === args.site && a.username === args.username ? { ...a, site: args.newSite, code: args.code } : a)); break;
        case 'login_remove': state.machine.logins.accounts = state.machine.logins.accounts.filter((a) => !(a.site === args.site && a.username === args.username)); break;
        case 'set_language': state.language = args.lang; break;
        case 'machine_set': state.machine = { ...state.machine, offers: { browser: args.browser, terminal: args.terminal, actions: args.actions }, on: args.on, state: args.on ? 'connected' : 'off' }; break;
        case 'machine_forget': state.machine = null; break;
        case 'open_argus': return null;
        case 'update_now': state.update = null; state.version = '0.1.2'; break;
        default: break;
      }
      setTimeout(() => emit('snapshot', structuredClone(state)));
      return null;
    },
  },
  event: {
    async listen(name, f) { (listeners[name] ||= []).push(f); return () => {}; },
  },
};
window.__mock = { calls, emit, get state() { return state; } };
