// FIX-20: calm rows — meta line, fixed slots, group counts, empty action, tokens.
const { test } = require('node:test');
const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { join, resolve } = require('node:path');
const { rendererHarness } = require('./helpers/renderer-dom.cjs');

const css = readFileSync(join(__dirname, '..', 'app.css'), 'utf8');
const packages = process.env.TERRA_PLUGIN_SOURCE || resolve(__dirname, '../../../terrarium-plugins/packages');
const bridge = (extra = {}) => ({ today: () => '2026-11-12', query: async () => [], action: async () => ({}), changed() {}, toast() {}, ...extra });
const walk = (node, out = []) => {
  if (!node || typeof node === 'string') return out;
  out.push(node);
  for (const child of node.children || []) walk(child, out);
  return out;
};
const text = node => typeof node === 'string' ? node : (node.textContent || '') + (node.children || []).map(text).join('');
const byClass = (root, name) => walk(root).filter(node => String(node.className || '').split(/\s+/).includes(name));

test('meta chips sit on a line under the title; slots keep empty columns', async () => {
  const ui = rendererHarness();
  const spec = { type: 'item', layout: 'line', calm: true, title: 'Write', fields: 'due,tags',
    meta: [{ text: '0/2', icon: 'list' }, { text: '', icon: 'steps' }],
    slots: [{ name: 'date', width: 80 }, { name: 'time', width: 60 }, { name: 'list', width: 70, field: 'list' }, { name: 'tags', width: 90 }],
    chips: [{ text: 'Today', slot: 'date', field: 'due' }, { text: 'Inbox', slot: 'list', field: 'list' }, { text: '#a', slot: 'tags', field: 'tags' },
      { text: 'hidden', slot: 'tags', if: '{{row.x}}' }] };
  const view = ui.renderer.mount(ui.root, { body: spec }, bridge());
  await view.ready;
  assert.match(byClass(ui.root, 'pp-item')[0].className, /pp-item-calm/);
  assert.deepEqual(byClass(ui.root, 'pp-item-meta').map(text), ['0/2']);
  const body = byClass(ui.root, 'pp-item-line-body')[0];
  assert.deepEqual(body.children.map(c => c.className), ['pp-item-line-text', 'pp-item-meta']);
  const slots = byClass(ui.root, 'pp-slot');
  // The list slot is dropped: `fields` does not name it.
  assert.deepEqual(slots.map(s => s.className.replace('pp-slot pp-slot-', '')), ['date', 'time', 'tags']);
  assert.deepEqual(slots.map(text), ['Today', '', '#a']);
  assert.equal(slots[0].style['--pp-slot-width'], '80px');
  view.dispose();
});

test('a slot or chip with a false `if` is dropped', async () => {
  const ui = rendererHarness();
  const spec = { type: 'item', layout: 'line', title: 'x', slots: [{ name: 'list', width: 70, if: '{{!state.lists}}' }],
    chips: [{ text: 'Inbox', slot: 'list' }] };
  const view = ui.renderer.mount(ui.root, { state: { lists: 'l1' }, body: spec }, bridge());
  await view.ready;
  assert.equal(byClass(ui.root, 'pp-slot').length, 0);
  view.dispose();
});

test('group_count adds a count; empty_action adds one button', async () => {
  const rows = [{ id: 1, title: 'A', g: 'Inbox' }, { id: 2, title: 'B', g: 'Inbox' }, { id: 3, title: 'C', g: 'Work' }];
  const item = { type: 'item', title: '{{row.title}}' };
  const ui = rendererHarness();
  const view = ui.renderer.mount(ui.root, { body: { type: 'list', query: 'tasks', group: 'g', group_count: true, item } }, bridge({ query: async () => rows }));
  await view.ready;
  assert.deepEqual(byClass(ui.root, 'pp-group-count').map(text), ['2', '1']);
  view.dispose();

  const calls = [];
  const empty = rendererHarness();
  const spec = { type: 'list', query: 'tasks', item, empty: 'Nothing here.', empty_action: { label: 'Add a task', set: { 'state.creating': 'yes' } } };
  const emptyView = empty.renderer.mount(empty.root, { state: { creating: '' }, body: spec }, bridge({ action: async (...a) => calls.push(a) }));
  await emptyView.ready;
  const [button] = byClass(empty.root, 'pp-empty-action');
  assert.equal(text(button), 'Add a task');
  await button.fire('click');
  assert.equal(emptyView.state.creating, 'yes');
  emptyView.dispose();
});

test('priority rings use theme tokens and the task list declares the calm layout', () => {
  assert.match(css, /\.pp-flag-3 \{ --check-color: var\(--danger\); \}/);
  assert.match(css, /\.pp-flag-2 \{ --check-color: var\(--warn/);
  assert.match(css, /\.pp-flag-1 \{ --check-color: var\(--muted\); \}/);
  assert.match(css, /\.pp-workspace-content \{[^}]*max-width: 880px/);
  const template = JSON.parse(readFileSync(resolve(packages, 'tasks/ui/page-main.json'), 'utf8'));
  const lists = [];
  const find = (node) => {
    if (!node || typeof node !== 'object') return;
    if (node.type === 'list' && node.query === 'workspace_tasks' && node.item) lists.push(node);
    for (const value of Object.values(node)) Array.isArray(value) ? value.forEach(find) : find(value);
  };
  find(template.body);
  const rows = lists[0];
  assert.equal(rows.group_count, true);
  assert.ok(rows.empty_action);
  const item = rows.item.children[0];
  assert.equal(item.calm, true);
  assert.ok(item.meta.length >= 3);
  assert.equal(item.actions.length, 1);
  assert.equal(item.actions[0].type, 'menu');
  const blocked = item.chips.find(c => c.text === '{{row.blocked_label}}');
  assert.equal(blocked.icon, 'lock');
  assert.equal(blocked.icon_only, true);
  assert.equal(item.chips.find(c => c.text === '{{row.list_name}}').if, '{{!state.lists}}');
  assert.equal(template.state.density, 'comfortable');
});

test('conditions combine with && and || (&& binds tighter)', async () => {
  const rows = [{ id: 1, done: 0, n: 'x', d: 'a' }, { id: 2, done: 1, n: '', d: 'b' }];
  const mk = (cond) => ({ type: 'list', query: 'tasks', item: { type: 'stack', children: [{ type: 'text', text: 'Y{{row.id}}', if: cond }] } });
  const shown = async (cond) => {
    const ui = rendererHarness();
    const view = ui.renderer.mount(ui.root, { body: mk(cond) }, bridge({ query: async () => rows }));
    await view.ready;
    const out = walk(ui.root).filter(n => /^Y\d$/.test(n.textContent || '')).map(n => n.textContent);
    view.dispose();
    return out;
  };
  assert.deepEqual(await shown('{{!row.done && row.n}}'), ['Y1']);
  assert.deepEqual(await shown('{{!row.done && row.d != @row.n}}'), ['Y1']);
  assert.deepEqual(await shown('{{row.done || row.n}}'), ['Y1', 'Y2']);
  assert.deepEqual(await shown('{{row.n && row.done || row.d == b}}'), ['Y2']);
  assert.deepEqual(await shown('{{row.n && row.done}}'), []);
});

test('the task row menu hides Plan today on done rows and offers Add subtask on open top-level rows, and keeps focus', () => {
  const template = JSON.parse(readFileSync(resolve(packages, 'tasks/ui/page-main.json'), 'utf8'));
  const menus = [];
  const find = (node) => {
    if (!node || typeof node !== 'object') return;
    if (node.type === 'menu' && String(node.label).startsWith('More for ')) menus.push(node);
    for (const value of Object.values(node)) Array.isArray(value) ? value.forEach(find) : find(value);
  };
  find(template.body);
  const menu = menus[0];
  assert.equal(menu.focus_key, 'menu-{{row.id}}');
  const cond = (label) => menu.items.find(i => i.label === label).if;
  assert.match(cond('Plan today'), /^\{\{!row\.done && /);
  assert.match(cond('Remove from today'), /^\{\{!row\.done && /);
  // FIX-21: subtasks are nested in the list, so the menu adds one instead.
  assert.match(cond('Add subtask'), /^\{\{!row\.done && !row\.parent_id && /);
  assert.match(cond('Hide subtask field'), /^\{\{state\.inline_subtasks == @row\.id\}\}$/);
});
