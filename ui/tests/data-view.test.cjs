const test = require('node:test');
const assert = require('node:assert/strict');
const views = require('../views.js');
const { rendererHarness } = require('./helpers/renderer-dom.cjs');

const flatten = node => [node, ...(node.children || []).filter(child => typeof child !== 'string').flatMap(flatten)];

function dataViewBridge(plugin, calls, mounted) {
  const plugins = [plugin];
  const queryBridge = {
    today: () => '2026-11-13',
    query: async (name, params) => {
      calls.push({ plugin: plugin.id, name, params });
      if (name !== 'tasks') throw new Error('query is outside the owning plugin');
      return [{ id: 't1', title: 'One' }, { id: 't2', title: 'Two' }];
    },
  };
  return {
    today: () => '2026-11-13',
    mountDataView: (root, spec, state) => {
      const entry = views.find(plugins, plugin.id, spec.query);
      if (!entry) return { snapshot: () => state.snapshot, refresh() {}, flush: async () => true, dispose() {} };
      mounted.push({ spec, snapshot: state.snapshot });
      return views.mount(root, { entry, config: spec.config, bridge: queryBridge,
        mount: (target, template, bridge, options) => mounted.renderer.mount(target, template, bridge, options),
        snapshot: state.snapshot });
    },
  };
}

test('data-view resolves bindings and carries mount, refresh, flush, snapshot, and disposal lifecycle', async t => {
  const previousDocument = global.document;
  global.document = { createElement: tag => ({ tag, className: '', textContent: '' }) };
  t.after(() => { global.document = previousDocument; });
  const plugin = { id: 'todo', views: { tasks: {
    title: 'Tasks', title_column: 'title', params: {},
    columns: [{ key: 'title', label: 'Task', type: 'string' }],
  } } };
  const calls = [], mounted = [];
  const first = rendererHarness();
  mounted.renderer = first.renderer;
  const bridge = dataViewBridge(plugin, calls, mounted);
  const template = { state: { layout: 'table', query: 'tasks' }, body: { type: 'data-view',
    query: '{{state.query}}', config: { layout: '{{state.layout}}', fields: 'title' } } };

  const view = first.renderer.mount(first.root, template, bridge);
  await view.ready;
  assert.equal(JSON.stringify(mounted[0].spec), JSON.stringify({ type: 'data-view', query: 'tasks', config: { layout: 'table', fields: 'title' } }));
  assert.equal(calls.length, 1, JSON.stringify(flatten(first.root).map(node => ({ className: node.className, text: node.textContent }))));
  assert.equal(calls[0].plugin, 'todo');
  assert.equal(calls[0].name, 'tasks');
  assert.ok(flatten(first.root).some(node => String(node.className).split(/\s+/).includes('pp-table')));

  await view.refresh();
  assert.equal(calls.length, 2);
  assert.equal(await view.flush({ unmount: true }), true);
  const snapshot = view.snapshot();
  assert.ok(snapshot.dataViews['data-view-1']);
  view.dispose();

  const second = rendererHarness();
  mounted.renderer = second.renderer;
  const restored = second.renderer.mount(second.root, template, bridge, { snapshot });
  await restored.ready;
  assert.deepEqual(mounted.at(-1).snapshot, snapshot.dataViews['data-view-1']);
  assert.ok(flatten(second.root).some(node => String(node.className).split(/\s+/).includes('pp-table')));
  restored.dispose();
});

test('data-view refresh remounts changed bindings, keeps same-node snapshots, and separates identical queries', async () => {
  const h = rendererHarness();
  const mounts = [], refreshes = [];
  const bridge = {
    today: () => '2026-11-13',
    mountDataView: (_root, spec, state) => {
      mounts.push({ spec, snapshot: state.snapshot });
      let version = mounts.length;
      return { ready: Promise.resolve(), refresh: async () => refreshes.push(JSON.stringify(spec)),
        snapshot: () => ({ version }), flush: async () => true, dispose() {} };
    },
  };
  const view = h.renderer.mount(h.root, { state: { query: 'tasks', layout: 'table' }, body: { type: 'stack', children: [
    { type: 'data-view', query: '{{state.query}}', config: { layout: '{{state.layout}}' } },
    { type: 'data-view', query: '{{state.query}}', config: { layout: '{{state.layout}}' } },
  ] } }, bridge);
  await view.ready;
  assert.equal(mounts.length, 2);
  await view.refresh();
  assert.equal(refreshes.length, 2);
  view.setState('state.layout', 'list');
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(mounts.length, 4);
  assert.equal(mounts[2].snapshot.version, 1);
  assert.equal(mounts[3].snapshot.version, 2);
  view.setState('state.query', 'other-tasks');
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(mounts.length, 6);
  assert.equal(mounts[4].spec.query, 'other-tasks');
  assert.equal(mounts[5].spec.query, 'other-tasks');
  assert.equal(mounts[4].snapshot.version, 3);
  assert.equal(mounts[5].snapshot.version, 4);
  assert.notEqual(JSON.stringify(view.snapshot().dataViews['data-view-1']), JSON.stringify(view.snapshot().dataViews['data-view-2']));
  view.dispose();
});

test('data-view refuses a query not declared by its owning plugin', async () => {
  const plugin = { id: 'todo', views: { tasks: { title: 'Tasks', columns: [{ key: 'title', type: 'string' }] } } };
  const calls = [], mounted = [];
  const h = rendererHarness(); mounted.renderer = h.renderer;
  const bridge = dataViewBridge(plugin, calls, mounted);
  const view = h.renderer.mount(h.root, { body: { type: 'data-view', query: 'notes.private', config: {} } }, bridge);
  await view.ready;
  assert.equal(mounted.length, 0);
  assert.equal(calls.length, 0);
  view.dispose();
});
