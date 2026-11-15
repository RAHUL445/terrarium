// Today view (today-v1): contract checks, merging, fold rules, capture
// routing, keyboard and customize persistence. Layout is checked in the app.
const { test } = require('node:test');
const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { join } = require('node:path');
const today = require('../today.js');

function storage() { const rows = new Map(); return { rows, getItem: k => rows.get(k) ?? null, setItem: (k, v) => rows.set(k, String(v)) }; }

const TODO_OPS = ['create', 'update', 'toggle', 'delete'];
function source(overrides = {}) {
  return {
    id: 'my-day', section: 'my_day', query: 'today_my_day', params: { today: '{{$today}}' }, limit: 2,
    link: { label: '+ Plan from Inbox', action: { navigate: { state: { filter: 'inbox' } } } },
    row: {
      key: '{{row.id}}', kind: 'task', title: '{{row.title}}', note: '{{row.steps}}', time: '{{row.time}}', sort: '{{row.sort}}',
      tone: '{{row.tone}}', done: '{{row.done}}', chips: [{ text: '{{row.tags}}', tone: 'accent' }, { text: '{{row.list}}' }],
      check: { action: { op: 'toggle', args: { id: '{{row.id}}', done: '{{value}}' } } },
      actions: [
        { label: 'Tomorrow', key: 's', action: { op: 'update', args: { id: '{{row.id}}', planned_for: '{{$tomorrow}}' }, success: 'Planned for tomorrow', undo: { op: 'update', args: { id: '{{row.id}}', planned_for: '{{row.planned_for}}' } } } },
        { label: 'Not today', action: { op: 'update', args: { id: '{{row.id}}', planned_for: null } } },
      ],
      open: { navigate: { item: '{{row.id}}' } },
    },
    ...overrides,
  };
}
const todo = (sources = [source()], extra = {}) => ({ id: 'tasks', name: 'Task manager', operations: TODO_OPS, today: sources, quick_actions: [], ...extra });
const NOON = new Date(2026, 10, 13, 12, 0).getTime();
const EVENING = new Date(2026, 10, 13, 19, 10).getTime();

function built(plugin, src, rows, total) {
  const ctx = today.context(NOON);
  return { plugin, source: src, total, rows: rows.map((raw, i) => today.toRow(plugin, src, raw, ctx, i)) };
}

// ---- A1 contract --------------------------------------------------------

test('contract: a bad entry is rejected with a reason and the rest of the plugin loads', () => {
  const cases = [
    [{ section: 'later' }, 'unknown section `later`'],
    [{ query: '' }, 'query is required'],
    [{ limit: 0 }, 'limit must be'],
    [{ row: { ...source().row, key: '' } }, 'row.key is required'],
    [{ row: { ...source().row, check: { action: { op: 'explode', args: {} } } } }, '`explode` is not a declared operation'],
    [{ row: { ...source().row, open: { op: 'toggle', navigate: { item: 'x' } } } }, 'cannot combine navigate'],
    [{ row: { ...source().row, open: { navigate: { page: 'x' } } } }, 'navigate takes item and state'],
    [{ row: { ...source().row, actions: Array(5).fill(source().row.actions[1]) } }, 'at most 4 actions'],
  ];
  for (const [change, message] of cases) {
    const bad = source({ id: 'bad', ...change });
    assert.match(today.validateSource(todo(), bad), new RegExp(message.replace(/[.*+?^${}()|[\]\\`]/g, '\\$&')));
    const { sources, errors } = today.sourcesOf([todo([bad, source()])]);
    assert.deepEqual(sources.map(s => s.source.id), ['my-day'], message);
    assert.equal(errors.length, 1);
    assert.match(errors[0], /tasks: today `bad`/);
  }
  assert.equal(today.validateSource(todo(), source()), null);
});

test('contract: backend errors are surfaced, repeated ids keep the first, hidden plugins contribute nothing', () => {
  const { sources, errors } = today.sourcesOf([
    { id: 'feeds', name: 'Feeds', operations: [], today: [], today_errors: ['feeds: today `x`: unknown section'] },
    todo([source(), source()]),
  ]);
  assert.equal(sources.length, 1);
  assert.deepEqual(errors, ['tasks: today `my-day`: id repeats', 'feeds: today `x`: unknown section']);
  const prefs = today.normalizePrefs({ plugins: ['tasks'] });
  assert.equal(today.sourcesOf([todo()], prefs).sources.length, 0);
});

test('contract: an older backend without operation names falls back to declared quick actions', () => {
  const plugin = { id: 'tasks', quick_actions: [{ op: 'create', arg: 'title' }] };
  assert.equal(today.declares(plugin, 'create'), true);
  assert.equal(today.declares(plugin, 'toggle'), false);
});

test('contract: every shipped plugin source passes the host checks', () => {
  const packages = process.env.TERRA_PLUGIN_SOURCE || join(__dirname, '../../../terrarium-plugins/packages');
  const seen = {};
  for (const id of ['tasks', 'habits', 'voice', 'bookmarks', 'feeds']) {
    const manifest = JSON.parse(readFileSync(join(packages, id, 'manifest.json'), 'utf8'));
    assert.ok(manifest.requires.includes('today-v1'), id);
    const plugin = { id, name: manifest.name, operations: Object.keys(manifest.operations), today: manifest.today };
    const { sources, errors } = today.sourcesOf([plugin]);
    assert.deepEqual(errors, [], id);
    for (const { source: s } of sources) {
      assert.ok(manifest.queries[s.query], `${id}: ${s.query}`);
      if (s.count_query) assert.ok(manifest.queries[s.count_query], `${id}: ${s.count_query}`);
      (seen[s.section] ||= []).push(`${id}/${s.id}`);
    }
  }
  assert.deepEqual(Object.keys(seen).sort(), ['catch_up', 'done', 'due', 'earlier', 'habits', 'my_day', 'sort', 'wrap_up']);
  assert.deepEqual(seen.sort, ['tasks/inbox', 'voice/review', 'bookmarks/untagged']);
});

// ---- Rows, merging, dedupe ----------------------------------------------

test('rows resolve templates with the clock and keep raw values for actions', () => {
  const plugin = todo();
  const row = today.toRow(plugin, source(), { id: 't1', title: 'Call Sam', steps: '1/2 steps', time: '19:30', sort: 5, done: 0, tags: '#work', list: 'Home', planned_for: '2026-11-13' }, today.context(NOON), 0);
  assert.equal(row.title, 'Call Sam');
  assert.equal(row.note, '1/2 steps');
  assert.equal(row.sort, 5);
  assert.equal(row.done, false);
  assert.deepEqual(row.chips, [{ text: '#work', tone: 'accent' }, { text: 'Home', tone: '' }]);
  assert.equal(row.scope.$tomorrow, '2026-11-14');
  assert.equal(today.toRow(plugin, source(), { id: 't2', title: 'x', tags: null, list: '' }, today.context(NOON), 2).chips.length, 0);
  assert.equal(today.toRow(plugin, source(), { id: 't3', title: 'x' }, today.context(NOON), 2).extra, true, 'rows past the limit fold');
  const ctx = today.context(new Date(2026, 11, 31, 23, 0).getTime());
  assert.equal(ctx.$today, '2026-12-31');
  assert.equal(ctx.$tomorrow, '2027-01-01');
});

test('merge keeps the earliest section for a repeated (plugin, key) and adjusts totals', () => {
  const plugin = todo();
  const due = source({ id: 'due', section: 'due' });
  const inbox = source({ id: 'inbox', section: 'sort' });
  const sections = today.merge([
    built(plugin, inbox, [{ id: 'a', title: 'A' }, { id: 'c', title: 'C' }], 5),
    built(plugin, due, [{ id: 'a', title: 'A' }, { id: 'b', title: 'B' }], 2),
    built(plugin, source(), [{ id: 'b', title: 'B' }], 1),
  ]);
  assert.deepEqual(sections.my_day.rows.map(r => r.key), ['b']);
  assert.deepEqual(sections.due.rows.map(r => r.key), ['a']);
  assert.equal(sections.due.total, 1);
  assert.deepEqual(sections.sort.rows.map(r => r.key), ['c']);
  assert.equal(sections.sort.total, 4, 'count query total minus the duplicate');
  const other = { ...todo(), id: 'voice', name: 'Voice' };
  const again = today.merge([built(plugin, source(), [{ id: 'a', title: 'A' }], 1), built(other, source(), [{ id: 'a', title: 'A' }], 1)]);
  assert.equal(again.my_day.rows.length, 2, 'the same key in two plugins is two rows');
});

test('merge interleaves timed rows across sources and keeps one source in its own order', () => {
  const a = todo(), b = { ...todo(), id: 'cal', name: 'Calendar' };
  const due = source({ section: 'due' });
  const single = today.merge([built(a, due, [{ id: '1', title: 'late', sort: 30 }, { id: '2', title: 'early', sort: 10 }], 2)]);
  assert.deepEqual(single.due.rows.map(r => r.key), ['1', '2']);
  const mixed = today.merge([
    built(a, due, [{ id: 'od', title: 'overdue', sort: null }, { id: 'r1', title: 'r1', sort: 30 }], 2),
    built(b, due, [{ id: 'r2', title: 'r2', sort: 20 }], 1),
  ]);
  assert.deepEqual(mixed.due.rows.map(r => r.key), ['od', 'r2', 'r1']);
});

test('plugin order puts Task manager first so its rows lead shared sections', () => {
  const voice = { id: 'voice', name: 'Voice', operations: ['archive'], today: [source({ id: 'review', section: 'sort', row: { key: '{{row.id}}', title: '{{row.title}}' } })] };
  const { sources } = today.sourcesOf([voice, todo([source({ id: 'inbox', section: 'sort' })])]);
  assert.deepEqual(sources.map(s => s.plugin.id), ['tasks', 'voice']);
});

// ---- Section hiding and folds (clock injected) ---------------------------

test('empty sections hide except My Day; hidden sections and order follow the preferences', () => {
  const plugin = todo();
  const sections = today.merge([
    built(plugin, source(), [], 0),
    built(plugin, source({ id: 'due', section: 'due' }), [], 0),
    built(plugin, source({ id: 'inbox', section: 'sort' }), [{ id: 'x', title: 'X' }], 1),
  ]);
  const prefs = today.normalizePrefs(null);
  assert.deepEqual(today.visibleSections(sections, prefs), ['my_day', 'sort']);
  assert.deepEqual(today.visibleSections(sections, today.normalizePrefs({ hidden: ['my_day'] })), ['sort']);
  assert.deepEqual(today.visibleSections(sections, today.normalizePrefs({ order: ['sort', 'my_day'] })), ['sort', 'my_day']);
  assert.deepEqual(today.visibleSections(today.merge([]), prefs), [], 'My Day needs a source to show');
});

test('wrap up folds before 17:00 and opens after; done today always starts folded', () => {
  const at = h => new Date(2026, 10, 13, h, 0).getTime();
  assert.equal(today.folded('wrap_up', at(16)), true);
  assert.equal(today.folded('wrap_up', at(17)), false);
  assert.equal(today.folded('done', at(23)), true);
  assert.equal(today.folded('my_day', at(9)), false);
  assert.equal(today.folded('done', at(9), { done: false }), false, 'a user unfold sticks');
  assert.equal(today.folded('wrap_up', at(20), { wrap_up: true }), true);
});

test('the now line sits between past and future timed rows', () => {
  const rows = [{ sort: null }, { sort: 100 }, { sort: 300 }];
  assert.equal(today.nowIndex(rows, 200), 2);
  assert.equal(today.nowIndex(rows, 50), 1);
  assert.equal(today.nowIndex(rows, 400), 3);
  assert.equal(today.nowIndex([{ sort: null }], 1), -1);
});

test('summary line counts planned, due, habits, to sort and overdue', () => {
  const plugin = todo();
  const sections = today.merge([
    built(plugin, source(), [{ id: 'a', title: 'A' }, { id: 'b', title: 'B' }, { id: 'c', title: 'C' }], 3),
    built(plugin, source({ id: 'due', section: 'due' }), [{ id: 'd', title: 'D', tone: 'overdue' }, { id: 'e', title: 'E' }], 2),
    built(plugin, source({ id: 'h', section: 'habits' }), [{ id: 'h1', title: 'Gym', done: 1 }, { id: 'h2', title: 'Read' }], 2),
    built(plugin, source({ id: 'inbox', section: 'sort' }), [{ id: 'f', title: 'F' }], 4),
  ]);
  const prefs = today.normalizePrefs(null);
  assert.equal(today.summary(sections, prefs), '3 planned · 2 due · 2 habits · 4 to sort · 1 overdue');
  assert.equal(today.countText(sections.habits), '1 of 2');
  assert.equal(today.summary(sections, today.normalizePrefs({ hidden: ['due', 'sort'] })), '3 planned · 2 habits');
  assert.equal(today.summary(today.merge([]), prefs), 'Nothing planned yet');
});

// ---- Capture routing ----------------------------------------------------

const PLUGINS = [
  { id: 'tasks', operations: ['create', 'update'] },
  { id: 'notes', operations: ['create'] },
  { id: 'bookmarks', operations: ['create'] },
];

test('capture: links go to Bookmarks with the words before them as the title', () => {
  for (const text of ['https://doc.rust-lang.org/nomicon', 'Rust nomicon https://doc.rust-lang.org/nomicon #rust', 'doc.rust-lang.org/nomicon', 'www.example.com', 'example.com']) {
    assert.deepEqual(today.routeCapture(text, PLUGINS), { kind: 'link', plugin: 'bookmarks', op: 'create', args: { text } }, text);
  }
  for (const text of ['call Sam 6pm #work', 'read notes.md tonight', 'e.g. buy milk']) {
    assert.equal(today.routeCapture(text, PLUGINS).kind, 'task', text);
  }
});

test('capture: note: goes to Notes and plain text to To-do in the chosen destination', () => {
  assert.deepEqual(today.routeCapture('note: Garden ideas\nraised beds', PLUGINS), { kind: 'note', plugin: 'notes', op: 'create', args: { body: 'Garden ideas\nraised beds' } });
  assert.match(today.routeCapture('note:   ', PLUGINS).error, /after “note:”/);
  assert.deepEqual(today.routeCapture(' call Sam 6pm ', PLUGINS), { kind: 'task', plugin: 'tasks', op: 'create', args: { title: 'call Sam 6pm' }, plan: false });
  assert.deepEqual(today.routeCapture('call Sam', PLUGINS, { id: 'list-1' }).args, { title: 'call Sam', list_id: 'list-1' });
  const myDay = today.routeCapture('call Sam', PLUGINS, { id: 'my_day' });
  assert.equal(myDay.plan, true);
  assert.deepEqual(myDay.args, { title: 'call Sam' });
  assert.equal(today.routeCapture('   ', PLUGINS), null);
});

test('capture: a missing plugin or operation falls through, and nothing installed is an error', () => {
  const noBookmarks = PLUGINS.filter(p => p.id !== 'bookmarks');
  assert.equal(today.routeCapture('https://example.com', noBookmarks).kind, 'task');
  const noNotes = PLUGINS.filter(p => p.id !== 'notes');
  assert.deepEqual(today.routeCapture('note: x', noNotes).args, { title: 'note: x' });
  assert.equal(today.routeCapture('x', [{ id: 'tasks', operations: ['create'] }], { id: 'my_day' }).plan, false, 'no update op, no plan');
  assert.match(today.routeCapture('x', [{ id: 'bookmarks', operations: ['create'] }]).error, /No app here/);
  assert.match(today.routeCapture('x', [{ id: 'tasks', operations: ['toggle'] }]).error, /No app here/);
});

// ---- Keyboard -----------------------------------------------------------

test('keyboard: J/K and arrows move, X toggles, T/S run actions, Enter opens, C captures', () => {
  const k = (key, extra = {}) => today.keyCommand({ key, ...extra });
  assert.deepEqual(['j', 'J', 'ArrowDown', 'k', 'ArrowUp', 'x', 't', 's', 'Enter', 'c', 'Escape'].map(key => k(key)),
    ['next', 'next', 'next', 'prev', 'prev', 'toggle', 'action:t', 'action:s', 'open', 'capture', 'escape']);
  assert.equal(k('j', { metaKey: true }), null, 'shortcuts with modifiers belong to the shell');
  assert.equal(k('1', { metaKey: true }), null);
  assert.equal(today.keyCommand({ key: 'j' }, { typing: true }), null, 'typing in the capture bar is text');
  assert.equal(today.keyCommand({ key: 'Escape' }, { typing: true }), 'escape');
  assert.equal(k('q'), null);
});

// A small DOM stand-in: enough for mount() to build, focus and click.
function fakeDom() {
  let active = null;
  class Node {
    constructor(tag) { this.tag = tag; this.children = []; this.attrs = {}; this.dataset = {}; this.listeners = {}; this.parent = null; this.value = ''; this.tabIndex = -1; }
    set className(v) { this.attrs.class = v; } get className() { return this.attrs.class || ''; }
    set textContent(v) { this.children = [String(v)]; } get textContent() { return this.children.map(c => typeof c === 'string' ? c : c.textContent).join(''); }
    get hidden() { return 'hidden' in this.attrs; } set hidden(v) { if (v) this.attrs.hidden = ''; else delete this.attrs.hidden; }
    get disabled() { return 'disabled' in this.attrs; } set disabled(v) { if (v) this.attrs.disabled = ''; else delete this.attrs.disabled; }
    setAttribute(k, v) { this.attrs[k] = String(v); } getAttribute(k) { return this.attrs[k] ?? null; } removeAttribute(k) { delete this.attrs[k]; }
    append(...nodes) { for (const n of nodes) { if (n == null) continue; if (typeof n === 'object') n.parent = this; this.children.push(n); } }
    replaceChildren(...nodes) { this.children = []; this.append(...nodes); }
    addEventListener(t, f) { (this.listeners[t] ||= []).push(f); }
    async fire(t, e = {}) { const ev = { target: this, preventDefault() { ev.defaultPrevented = true; }, stopPropagation() {}, ...e }; for (const f of this.listeners[t] || []) await f(ev); return ev; }
    focus() { active = this; for (let n = this; n; n = n.parent) for (const f of n.listeners.focusin || []) f({ target: this }); }
    blur() { if (active === this) active = null; }
    contains(n) { for (let x = n; x; x = x.parent) if (x === this) return true; return false; }
    closest(sel) { for (let n = this; n; n = n.parent) if (match(n, sel)) return n; return null; }
    all() { return this.children.filter(c => typeof c === 'object').flatMap(c => [c, ...c.all()]); }
    querySelector(sel) { return this.all().find(n => match(n, sel)) || null; }
    querySelectorAll(sel) { return this.all().filter(n => match(n, sel)); }
  }
  function match(n, sel) {
    return sel.split(',').some(s => {
      s = s.trim();
      if (s === '[data-today-item]') return 'todayItem' in n.dataset;
      if (s.startsWith('.')) return n.className.split(/\s+/).includes(s.slice(1));
      return n.tag === s;
    });
  }
  const el = (tag, attrs = {}, ...children) => {
    const n = new Node(tag);
    for (const [k, v] of Object.entries(attrs)) {
      if (k.startsWith('on')) n.addEventListener(k.slice(2), v);
      else if (k === 'hidden' || k === 'disabled' || k === 'checked') { if (v) n.attrs[k] = ''; }
      else if (v !== false && v != null) n.setAttribute(k, v);
    }
    for (const c of children) if (c != null && c !== false) n.append(typeof c === 'object' ? c : String(c));
    return n;
  };
  return { el, Node, get active() { return active; } };
}

function harness(rowsByQuery, extra = {}) {
  const dom = fakeDom();
  const calls = [], toasts = [], opened = [];
  const plugins = [todo([source(), source({ id: 'due', section: 'due', query: 'today_due', limit: 5 })])];
  const deps = {
    el: dom.el, icon: name => dom.el('svg', { 'data-icon': name }), toast: (m, kind, action) => toasts.push({ m, kind, action }),
    plugins: () => plugins, space: () => 'personal', storage: storage(), visible: () => true, now: () => NOON,
    query: async (plugin, name) => rowsByQuery[name] || [], action: async (plugin, op, args) => { calls.push({ plugin, op, args }); return { id: 'new-1' }; },
    open: async (plugin, target) => opened.push({ plugin, target }), openExternal: () => {}, changed: () => calls.push({ changed: true }),
    ...extra,
  };
  const root = dom.el('section');
  const view = today.mount(root, deps);
  return { dom, root, view, calls, toasts, opened, deps };
}

test('keyboard on the mounted view moves focus, toggles, runs keyed actions and opens', async () => {
  const h = harness({ today_my_day: [{ id: 't1', title: 'Call Sam', done: 0, planned_for: '2026-11-13' }, { id: 't2', title: 'Leg day', done: 0 }] });
  await h.view.refresh();
  const rows = h.root.querySelectorAll('.today-row');
  assert.equal(rows.length, 2);
  assert.equal(rows[0].tabIndex, 0, 'one row is in the tab order');
  h.view.handle('next');
  assert.equal(h.dom.active.id, rows[0].id);
  h.view.handle('next');
  assert.equal(h.view.state.focus, 'tasks\u0000t2');
  h.view.handle('prev');
  assert.equal(h.view.state.focus, 'tasks\u0000t1');
  h.view.handle('toggle');
  await new Promise(r => setTimeout(r, 0));
  assert.deepEqual(h.calls[0], { plugin: 'tasks', op: 'toggle', args: { id: 't1', done: true } });
  h.view.handle('action:s');
  await new Promise(r => setTimeout(r, 0));
  assert.deepEqual(h.calls.find(c => c.args?.planned_for === '2026-11-14')?.args, { id: 't1', planned_for: '2026-11-14' });
  const undo = h.toasts.find(t => t.m === 'Planned for tomorrow').action;
  assert.equal(undo.label, 'Undo');
  await undo.run();
  assert.deepEqual(h.calls.filter(c => c.op === 'update').at(-1).args, { id: 't1', planned_for: '2026-11-13' });
  assert.equal(h.view.handle('action:t'), false, 'no T action on this row');
  h.view.handle('open');
  assert.deepEqual(h.opened, [{ plugin: 'tasks', target: { item: 't1' } }]);
  assert.equal(h.view.handle('capture'), true);
  assert.equal(h.dom.active.className, 'today-capture-input');
  h.view.dispose();
});

test('mounted view: My Day empty state, summary, folded more rows and capture to My Day', async () => {
  const many = Array.from({ length: 7 }, (_, i) => ({ id: `d${i}`, title: `Due ${i}` }));
  const h = harness({ today_my_day: [], today_due: many });
  await h.view.refresh();
  assert.match(h.root.querySelector('.today-empty').textContent, /Nothing planned/);
  assert.equal(h.root.querySelector('.today-summary').getAttribute('aria-live'), 'polite');
  assert.equal(h.root.querySelector('.today-summary').textContent, '7 due');
  assert.equal(h.root.querySelectorAll('.today-row').length, 5);
  const more = h.root.querySelectorAll('.today-fold').find(n => /more/.test(n.textContent));
  assert.equal(more.textContent, '▸ 2 more');
  await more.fire('click');
  assert.equal(h.root.querySelectorAll('.today-row').length, 7);
  h.view.state.dest = { id: 'my_day', label: 'My Day' };
  const input = h.root.querySelector('.today-capture-input');
  input.value = 'call Sam 6pm';
  await h.root.querySelector('form').fire('submit');
  assert.deepEqual(h.calls.slice(0, 2).map(c => [c.op, c.args]), [['create', { title: 'call Sam 6pm' }], ['update', { id: 'new-1', planned_for: '2026-11-13' }]]);
  assert.equal(input.value, '');
  assert.equal(h.toasts.at(-1).action.label, 'Open');
  h.view.dispose();
});

test('mounted view: the due now line and overdue link appear at the injected time', async () => {
  const at = ms => new Date(2026, 10, 13, ms).getTime();
  const plugins = [todo([source({ id: 'due', section: 'due', query: 'today_due', link: { label: 'Move all to today', each: 'overdue', action: { op: 'update', args: { id: '{{row.id}}', due: '{{$today}}' } } } })])];
  const h = harness({ today_due: [{ id: 'o', title: 'Passport', tone: 'overdue' }, { id: 'r', title: 'Bill', sort: at(21) }] }, { plugins: () => plugins, now: () => EVENING });
  await h.view.refresh();
  const list = h.root.querySelector('.today-rows');
  assert.deepEqual(list.children.map(n => n.className.split(' ')[0]), ['today-row', 'today-now', 'today-row']);
  assert.equal(list.children[1].textContent, 'now 19:10');
  const link = h.root.querySelector('.today-link');
  assert.equal(link.textContent, 'Move all to today');
  await link.fire('click');
  assert.deepEqual(h.calls.filter(c => c.op).map(c => c.args), [{ id: 'o', due: '2026-11-13' }]);
  h.view.dispose();
});

// ---- Customize persistence ------------------------------------------------

test('customize: preferences persist per space on this device and survive bad storage', () => {
  const s = storage();
  let prefs = today.readPrefs(s, 'personal');
  assert.deepEqual(prefs.order, today.ORDERABLE);
  assert.equal(prefs.density, 'compact');
  prefs = today.moveSection(prefs, 'habits', -1);
  prefs = today.toggleSection(prefs, 'catch_up');
  prefs = today.togglePlugin(prefs, 'feeds');
  assert.equal(today.writePrefs(s, 'personal', { ...prefs, density: 'comfortable' }), true);
  const back = today.readPrefs(s, 'personal');
  assert.deepEqual(back.order.slice(0, 3), ['my_day', 'habits', 'due']);
  assert.deepEqual(back.hidden, ['catch_up']);
  assert.deepEqual(back.plugins, ['feeds']);
  assert.equal(back.density, 'comfortable');
  assert.deepEqual(today.readPrefs(s, 'work').hidden, [], 'another space keeps its own');
  assert.equal(today.toggleSection(back, 'catch_up').hidden.length, 0);
  assert.deepEqual(today.moveSection(back, 'my_day', -1).order, back.order, 'moving past the top does nothing');
  s.setItem('pp-today:broken', '{nope');
  assert.deepEqual(today.readPrefs(s, 'broken'), today.normalizePrefs(null));
  const odd = today.normalizePrefs({ order: ['done', 'earlier', 'bogus', 'done'], hidden: ['bogus', 'due'], plugins: ['ok', 'bad/id', 7] });
  assert.equal(odd.order[0], 'done');
  assert.equal(odd.order.length, today.ORDERABLE.length);
  assert.deepEqual(odd.hidden, ['due']);
  assert.deepEqual(odd.plugins, ['ok']);
  assert.equal(today.writePrefs({ setItem() { throw Error('full'); } }, 'a', prefs), false);
  assert.deepEqual(today.readPrefs({ getItem() { throw Error('denied'); } }, 'a'), today.normalizePrefs(null));
});

test('customize: hiding a section or app from the mounted panel is saved and applied', async () => {
  const h = harness({ today_my_day: [{ id: 't1', title: 'Call Sam' }] });
  await h.view.refresh();
  const button = h.root.querySelector('.today-head-button');
  await button.fire('click');
  const panel = h.root.querySelector('.today-customize');
  assert.equal(panel.hidden, false);
  const box = panel.querySelectorAll('input')[0];
  await box.fire('change');
  await new Promise(r => setTimeout(r, 0));
  assert.deepEqual(JSON.parse(h.deps.storage.getItem('pp-today:personal')).hidden, ['my_day']);
  assert.equal(h.root.querySelectorAll('.today-row').length, 0);
  assert.equal(h.view.handle('escape'), true, 'Esc closes the panel');
  assert.equal(panel.hidden, true);
  h.view.dispose();
});

test('phone layout: the capture field shrinks and row meta wraps under the title', () => {
  const css = require('node:fs').readFileSync(require('node:path').join(__dirname, '../app.css'), 'utf8');
  // Beats the global input[type="text"] min-width so the chip and mic stay in the bar.
  assert.match(css, /input\.today-capture-input \{[^}]*min-width: 0;/);
  assert.match(css, /\.today-capture > :not\(input\) \{ flex: none; \}/);
  const phone = css.slice(css.indexOf('.today-col { padding: 8px 0; }'));
  const block = phone.slice(0, phone.indexOf('\n}'));
  assert.match(block, /\.today-kbd \{ display: none; \}/);
  assert.match(block, /\.today-row \{ grid-template-columns: 24px minmax\(0, 1fr\);[^}]*min-height: 44px;/);
  assert.match(block, /\.today-row > \.today-meta, \.today-row > \.today-actions \{ grid-column: 2; \}/);
});
