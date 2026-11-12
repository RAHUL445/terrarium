const { test } = require('node:test');
const assert = require('node:assert/strict');
const { rendererHarness } = require('./helpers/renderer-dom.cjs');
const flatten = node => [node, ...(node.children || []).filter(n => typeof n !== 'string').flatMap(flatten)];

test('row conditions compare explicit paths, lazily mount queries and keep literal comparisons', async () => {
  const h = rendererHarness();
  const calls = [];
  const template = { state: { expanded: '', mode: 'row.id' }, body: { type: 'list', query: 'tasks', item: {
    type: 'stack', children: [
      { type: 'button', label: 'Expand {{row.id}}', action: { set: { 'state.expanded': '{{row.id}}' } } },
      { type: 'text', text: 'Literal kept', if: '{{state.mode == row.id}}' },
      { type: 'list', query: 'steps', params: { task_id: '{{row.id}}' }, if: '{{state.expanded == @row.id}}', item: { type: 'text', text: '{{row.title}}' } },
    ],
  } } };
  const view = h.renderer.mount(h.root, template, { today: () => '2026-11-12', query: async (name, args) => {
    calls.push({ name, args }); return name === 'tasks' ? [{ id: 'a' }, { id: 'b' }] : [{ title: 'Step for ' + args.task_id }];
  } });
  await view.ready;
  assert.equal(calls.filter(c => c.name === 'steps').length, 0);
  assert.equal(flatten(h.root).filter(n => n.textContent === 'Literal kept').length, 2);
  await flatten(h.root).find(n => n.tag === 'button' && n.textContent === 'Expand b').fire('click');
  await view.refresh();
  assert.ok(calls.some(c => c.name === 'steps' && c.args.task_id === 'b'));
  assert.equal(calls.some(c => c.name === 'steps' && c.args.task_id === 'a'), false);
  view.dispose();
});

test('library snapshots survive conditional removal and remount in the parent session', async () => {
  const h = rendererHarness(); let draft = { title: 'Unsaved view', config: { fields: 'title' } };
  const snapshots = [];
  const bridge = { today: () => '2026-11-12', mountViewLibrary: (node, spec, options) => {
    snapshots.push(options.snapshot);
    return { snapshot: () => draft, refresh: async () => {}, dispose() {} };
  } };
  const template = { state: { tab: 'views' }, body: { type: 'stack', children: [
    { type: 'button', label: 'Other', action: { set: { 'state.tab': 'other' } } },
    { type: 'button', label: 'Views', action: { set: { 'state.tab': 'views' } } },
    { type: 'view-library', query: 'tasks', if: '{{state.tab == views}}' },
  ] } };
  const view = h.renderer.mount(h.root, template, bridge); await view.ready;
  await flatten(h.root).find(n => n.textContent === 'Other').fire('click');
  await flatten(h.root).find(n => n.textContent === 'Views').fire('click');
  assert.deepEqual(snapshots[1], draft);
  const snapshot = view.snapshot(); view.dispose();
  const next = rendererHarness();
  const restored = next.renderer.mount(next.root, template, bridge, { snapshot }); await restored.ready;
  assert.deepEqual(snapshots.at(-1), draft); restored.dispose();
});

test('a state-driven row toggle restores its explicit keyboard focus key', async () => {
  const base = rendererHarness();
  const document = { body: {}, activeElement: null };
  class Node extends base.root.constructor {
    focus() { document.activeElement = this; }
    querySelectorAll(selector) {
      return selector === '[data-pp-focus]' ? flatten(this).filter(n => n.dataset?.ppFocus) : super.querySelectorAll(selector);
    }
  }
  Object.assign(document, { createElement: tag => new Node(tag), createElementNS: (_, tag) => new Node(tag), createDocumentFragment: () => new Node('fragment') });
  const h = rendererHarness({ document });
  const root = new Node('root');
  const view = h.renderer.mount(root, { state: { open: false }, body: { type: 'stack', children: [
    { type: 'button', label: 'Expand', focus_key: 'steps', if: '{{!state.open}}', action: { set: { 'state.open': true } } },
    { type: 'button', label: 'Collapse', focus_key: 'steps', if: '{{state.open}}', action: { set: { 'state.open': false } } },
  ] } }, { today: () => '2026-11-12' });
  await view.ready;
  const expand = flatten(root).find(n => n.textContent === 'Expand');
  expand.focus(); await expand.fire('click');
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(document.activeElement.textContent, 'Collapse');
  view.dispose();
});
