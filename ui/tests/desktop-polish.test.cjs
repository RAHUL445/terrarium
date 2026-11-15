const { test } = require('node:test');
const assert = require('node:assert/strict');
const { join, resolve } = require('node:path');
const { execFileSync } = require('node:child_process');
const { rendererHarness } = require('./helpers/renderer-dom.cjs');
const packages = process.env.TERRA_PLUGIN_SOURCE || resolve(__dirname, '../../../terrarium-plugins/packages');
const read = (plugin, file) => require(join(packages, plugin, file));
const turn = () => new Promise(done => setImmediate(done));
const walk = node => !node || typeof node !== 'object' ? [] : [node, ...Object.values(node).flatMap(value => Array.isArray(value) ? value.flatMap(walk) : walk(value))];
const text = node => typeof node === 'string' ? node : (node.textContent || '') + (node.children || []).map(text).join('');
const bridge = (extra = {}) => ({ today: () => '2026-11-12', query: async () => [], action: async () => ({}), changed() {}, toast() {}, ...extra });

test('note query counts the same checklist nodes as Markdown, including fences and unusual whitespace', () => {
  const sql = read('notes', 'manifest.json').queries.list.sql;
  const expression = sql.slice(sql.indexOf('(WITH RECURSIVE split'), sql.indexOf(' AS checklist'));
  const bodies = [
    'A paragraph with - [ ] inline\n- [ ] First\n- [x] Second',
    '```markdown\n- [x] code\n```\n  * [X] Done\n* [ ] Open',
    '- [ ] \n- [x]\n- [z] Not a checkbox\n> - [ ] Quote',
    '*\t[x]\tTabbed\r\n\u00a0-  [ ]\u2003Spaced\n- [X] Unicode',
    '- [ ] A "quote" and \\n a literal slash\n```\n* [ ] hidden',
    '```\n```\n- [x] yes\n```\n* [ ] no\n```\n- [ ] yes',
    '- [ ] One\n'.repeat(1200),
    '',
  ];
  const actual = JSON.parse(execFileSync('python3', ['-c', `
import json,sqlite3,sys
expr,bodies=json.load(sys.stdin)
db=sqlite3.connect(':memory:')
print(json.dumps([db.execute('SELECT '+expr+' FROM (SELECT ? AS body)',(body,)).fetchone()[0] for body in bodies]))
`], { input: JSON.stringify([expression, bodies]), encoding: 'utf8' }));
  const { renderer } = rendererHarness();
  bodies.forEach((body, index) => {
    const tasks = renderer.markdown(body).querySelectorAll('.pp-md-task');
    const expected = tasks.length ? `${tasks.filter(task => task.children[0].checked).length}/${tasks.length}` : null;
    assert.equal(actual[index], expected, body.slice(0, 80));
  });
});

test('habit sheet refreshes stats when a check-in changes its row', async () => {
  const sheet = structuredClone(walk(read('habits', 'ui/page-main.json')).find(n => n.type === 'sheet' && n.query === 'habit'));
  // Keep the actual stats and refresh contract without unrelated habit controls.
  sheet.children = [sheet.children[0]];
  const ui = rendererHarness(); let total = 0;
  const view = ui.renderer.mount(ui.root, { state: { open: 'h' }, body: sheet }, bridge({ query: async () => [{ id: 'h', title: 'Walk', streak: total, best: total, rate30: '0%', total }] }));
  await view.ready; await turn();
  assert.equal(ui.root.querySelectorAll('.pp-stat-value')[0].textContent, '0');
  total = 1; await view.refresh();
  assert.equal(ui.root.querySelectorAll('.pp-stat-value')[0].textContent, '1');
  assert.equal(ui.root.querySelectorAll('.pp-stat-value')[3].textContent, '1');
  view.dispose();
});

test('input wording and new reminder defaults follow pointer type', async () => {
  const habit = walk(read('habits', 'ui/page-main.json')).filter(n => n.name === 'notify_on').find(n => n.value !== '{{row.notify_on}}');
  for (const touch of [false, true]) {
    const ui = rendererHarness({ matchMedia: query => ({ matches: query === '(pointer: coarse)' && touch }) });
    const view = ui.renderer.mount(ui.root, { body: { type: 'stack', children: [
      { type: 'text', text: '{{$gesture}} to record' }, habit,
    ] } }, bridge());
    await view.ready;
    assert.equal(view.scope().$remind_on, touch ? 'phone' : 'desktop');
    assert.match(text(ui.root), touch ? /Tap to record/ : /Click to record/);
    view.dispose();
  }
});

test('task toggles show completion or reopen feedback and only offer a note when requested', async () => {
  const actions = walk(read('tasks', 'ui/page-main.json')).filter(n => n.op === 'toggle' && n.followup);
  assert.equal(actions.length, 4); // List, board, Eisenhower and task sheet.
  for (const action of actions) {
    const ui = rendererHarness(); const toasts = []; let askNote = true;
    const view = ui.renderer.mount(ui.root, { state: { note_for: '' } }, bridge({
      action: async (op, args) => ({ message: args.done ? 'Task done' : 'Task reopened', ask_note: args.done && askNote ? 't' : null }),
      toast: (...args) => toasts.push(args),
    }));
    await view.ready;
    await view.act(action, { row: { id: 't', done: false }, value: true });
    assert.equal(view.state.note_for, '');
    assert.equal(toasts.at(-1)[0], 'Task done');
    assert.equal(toasts.at(-1)[2].label, 'Add note');
    await toasts.at(-1)[2].run();
    assert.equal(view.state.note_for, 't');
    await view.act(action, { row: { id: 't', done: true }, value: false });
    assert.equal(toasts.at(-1)[0], 'Task reopened');
    assert.equal(toasts.at(-1)[2], null);
    askNote = false;
    await view.act(action, { row: { id: 't', done: false }, value: true });
    assert.equal(toasts.at(-1)[0], 'Task done');
    assert.equal(toasts.at(-1)[2], null);
    view.dispose();
  }
});

test('voice send offers Open in its destination and preserves the created ID', async () => {
  const action = walk(read('voice', 'ui/page-main.json')).find(n => n.op === 'send' && n.args.to === 'todo' && n.followup);
  const ui = rendererHarness(); const opened = []; let offer;
  const view = ui.renderer.mount(ui.root, {}, bridge({ action: async () => ({ id: 'new-task', message: 'Sent to To-do' }), toast: (message, kind, action) => { offer = action; }, openPluginItem: (...args) => opened.push(args) }));
  await view.ready; await view.act(action, { row: { id: 'capture' } });
  assert.equal(offer.label, 'Open'); await offer.run();
  assert.deepEqual(opened, [['tasks', 'new-task']]);
  view.dispose();
});

test('empty lists hide Select and task priority has a visible label', async () => {
  const ui = rendererHarness(); let rows = [];
  const view = ui.renderer.mount(ui.root, { body: { type: 'list', query: 'tasks', empty: 'No tasks', select: { actions: [] }, item: { type: 'item', title: '{{row.title}}', flag: '{{row.priority}}' } } }, bridge({ query: async () => rows }));
  await view.ready;
  assert.equal(ui.root.querySelectorAll('.pp-select-bar').some(n => text(n).includes('Select')), false);
  rows = [{ id: 't', title: 'Task', priority: 3 }]; await view.refresh();
  assert.match(text(ui.root), /High priority/);
  view.dispose();
});


test('a desktop with touch input still defaults reminders to desktop', () => {
  const ui = rendererHarness({ matchMedia: () => ({ matches: true }), navigator: { userAgent: 'Macintosh' } });
  const view = ui.renderer.mount(ui.root, {}, bridge());
  assert.equal(view.scope().$gesture, 'Tap');
  assert.equal(view.scope().$remind_on, 'desktop');
  view.dispose();
});

test('empty date fields show No date and switch to a date picker on focus', async () => {
  const ui = rendererHarness();
  const view = ui.renderer.mount(ui.root, { body: { type: 'date', name: 'due', label: 'Due', value: '' } }, bridge());
  await view.ready;
  const input = ui.root.querySelector('input');
  assert.equal(input.type, 'text'); assert.equal(input.placeholder, 'No date');
  await input.fire('focus'); assert.equal(input.type, 'date');
  await input.fire('blur'); assert.equal(input.type, 'text');
  input.value = '2026-11-12'; await input.fire('blur'); assert.equal(input.type, 'date');
  view.dispose();
});

function pagePrototype() {
  const vm = require('node:vm');
  const { readFileSync } = require('node:fs');
  const window = { TerrariumLayout: require('../layout.js') };
  vm.runInNewContext(readFileSync(join(__dirname, '../pages.js'), 'utf8'), { window });
  return window.TerrariumPages.prototype;
}

test('new Home seeds exclude widgets needing setup, while keeping working clock and plugin widgets', () => {
  const pages = Object.create(pagePrototype());
  pages.grid = require('../grid.js'); pages.o = {};
  const entry = (id, builtIn = false) => ({ plugin: { id: builtIn ? '_host' : 'todo' }, widget: { id }, builtIn, template: { page: {} } });
  const entries = [entry('text', true), entry('link', true), entry('view', true), entry('clock', true), entry('today')];
  pages.catalog = new Map(entries.map(e => [`${e.plugin.id}/${e.widget.id}`, e]));
  assert.deepEqual(Array.from(pages.seed(), p => p.widget), ['clock', 'today']);
});

test('new pages enter rename mode and Undo removes the created page', async () => {
  const pages = Object.create(pagePrototype());
  const calls = []; let offer, focused = false, selected = false;
  pages.space = 'personal'; pages.draft = null;
  pages.$ = () => ({ focus() { focused = true; }, select() { selected = true; } });
  pages.edit = () => { pages.draft = []; };
  pages.service = async (name, args) => {
    calls.push([name, args]);
    if (name === 'page_create') return { id: 'new' };
    return [{ id: 'home', system: true }];
  };
  pages.o = { show: async id => { pages.page = { id }; }, toast: (message, kind, action) => { offer = action; } };
  await pages.create();
  assert.equal(focused, true); assert.equal(selected, true);
  assert.equal(offer.label, 'Undo');
  await offer.run();
  assert.ok(calls.some(([name, args]) => name === 'page_remove' && args.page === 'new'));
  assert.equal(pages.draft, null); assert.equal(pages.page.id, 'home');
});

test('Home view packs cards by their own height and edit mode keeps the row grid', () => {
  const pages = Object.create(pagePrototype());
  const classes = new Set();
  const root = { classList: { toggle(name, on) { on ? classes.add(name) : classes.delete(name); }, contains: name => classes.has(name) } };
  const card = (h) => { const style = new Map(); return { offsetHeight: h, style: { getPropertyValue: k => style.get(k) || '', setProperty: (k, v) => style.set(k, v) }, span: () => style.get('--pack-span') }; };
  const tall = card(300), short = card(84);
  pages.$ = () => root; pages.cards = new Map([['a', tall], ['b', short]]);
  pages.narrow = { matches: false }; pages.draft = null; pages.packer = { disconnect() {}, observe() {} };
  pages.pack();
  assert.ok(classes.has('packed'));
  assert.equal(tall.span(), String(Math.ceil((300 + 16) / 4)));
  assert.equal(short.span(), String(Math.ceil((84 + 16) / 4)));
  pages.draft = []; pages.pack();
  assert.ok(!classes.has('packed'));
  pages.draft = null; pages.narrow.matches = true; pages.pack();
  assert.ok(!classes.has('packed'));
});
