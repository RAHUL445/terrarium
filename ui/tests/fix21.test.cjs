// FIX-21: nested rows (list.tree) — indent, disclosure, collapse, counts.
const { test } = require('node:test');
const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { join } = require('node:path');
const { rendererHarness } = require('./helpers/renderer-dom.cjs');

const css = readFileSync(join(__dirname, '..', 'app.css'), 'utf8');
const bridge = (extra = {}) => ({ today: () => '2026-11-12', query: async () => [], action: async () => ({}), changed() {}, toast() {}, ...extra });
const walk = (node, out = []) => {
  if (!node || typeof node === 'string') return out;
  out.push(node);
  for (const child of node.children || []) walk(child, out);
  return out;
};
const text = node => typeof node === 'string' ? node : (node.textContent || '') + (node.children || []).map(text).join('');
const byClass = (root, name) => walk(root).filter(node => String(node.className || '').split(/\s+/).includes(name));

const rows = [
  { id: 'p', title: 'Parent', depth: 0, g: 'Work' },
  { id: 'a', title: 'Child A', depth: 1, g: 'Work' },
  { id: 'b', title: 'Child B', depth: 1, g: 'Work' },
  { id: 's', title: 'Solo', depth: 0, g: 'Work' },
];
const item = { type: 'item', title: '{{row.title}}' };
const list = (extra = {}) => ({ type: 'list', query: 'tasks', group: 'g', group_count: true, tree: { depth: 'depth' }, item, ...extra });

test('tree rows indent subtasks and give only the parent a disclosure arrow', async () => {
  const ui = rendererHarness();
  const view = ui.renderer.mount(ui.root, { body: list() }, bridge({ query: async () => rows }));
  await view.ready;
  assert.equal(byClass(ui.root, 'pp-list-tree').length, 1);
  assert.equal(byClass(ui.root, 'pp-tree-child').length, 2);
  const toggles = byClass(ui.root, 'pp-tree-toggle');
  assert.equal(toggles.length, 1);
  assert.equal(toggles[0].attrs['aria-expanded'], 'true');
  // FIX-22: the group heading counts every row it shows, subtasks included,
  // matching the sidebar counts.
  assert.deepEqual(byClass(ui.root, 'pp-group-count').map(text), ['4']);
  view.dispose();
});

test('the arrow hides and shows the subtasks and is remembered on the device', async () => {
  const store = new Map();
  const window = { terrariumStorage: { getItem: k => store.get(k) ?? null, setItem: (k, v) => store.set(k, v) } };
  {
    const ui = rendererHarness({ window });
    const spec = { body: list({ tree: { depth: 'depth', remember: 'tasks-tree' } }) };
    const view = ui.renderer.mount(ui.root, spec, bridge({ query: async () => rows }));
    await view.ready;
    const hidden = () => byClass(ui.root, 'pp-tree-child').map(n => !!n.hidden);
    assert.deepEqual(hidden(), [false, false]);
    byClass(ui.root, 'pp-tree-toggle')[0].dispatchEvent({ type: 'click' });
    assert.deepEqual(hidden(), [true, true]);
    assert.equal(byClass(ui.root, 'pp-tree-toggle')[0].attrs['aria-expanded'], 'false');
    assert.match([...store.values()][0], /"p"/);
    view.dispose();
    const again = rendererHarness({ window });
    const view2 = again.renderer.mount(again.root, spec, bridge({ query: async () => rows }));
    await view2.ready;
    assert.deepEqual(byClass(again.root, 'pp-tree-child').map(n => !!n.hidden), [true, true]);
    view2.dispose();
  }
});

test('tree styles exist for indent, arrow and hidden rows', () => {
  for (const needle of ['.pp-list-tree', '.pp-tree-child', '.pp-tree-toggle', '[hidden]']) assert.ok(css.includes(needle), needle);
});
