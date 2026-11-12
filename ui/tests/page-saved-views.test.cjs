const { test } = require('node:test');
const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { join } = require('node:path');
const vm = require('node:vm');
const grid = require('../grid.js');

function prototype(views = {}) {
  const window = { TerrariumViews: views };
  vm.runInNewContext(readFileSync(join(__dirname, '../pages.js'), 'utf8'), { window });
  return window.TerrariumPages.prototype;
}

function body() {
  return {
    children: [],
    replaceChildren(...children) { this.children = children; },
    append(...children) { this.children.push(...children); },
  };
}

function instance(proto, { saved, views = {}, calls = [] } = {}) {
  const page = Object.create(proto);
  page.space = 'space-a';
  page.page = { title: 'Focus page', focus: 'page-focus' };
  page.plugins = [{ id: 'todo', name: 'To-do', views: { tasks: { title: 'Tasks' } } }];
  page.grid = grid;
  page.placement = { id: 'placement-1', plugin: 'todo', widget: 'view', x: 0, y: 0, w: 4, h: 3 };
  page.o = {
    call: async (name, args) => {
      calls.push([name, args]);
      if (name === 'service' && args.name === 'view_get') return typeof saved === 'function' ? saved(args.id) : saved;
      return null;
    },
    bridge: plugin => ({ plugin }),
    session: { get: () => null },
    mount() {},
  };
  page.el = (tag, attrs = {}, ...children) => ({ tag, ...attrs, children, textContent: children.join('') });
  return page;
}

function mountedViews() {
  const records = [];
  const views = {
    find: (plugins, plugin, query) => plugin === 'todo' && query === 'tasks'
      ? { plugin: plugins[0], view: { title: 'Tasks' } } : null,
    mount: (target, options) => {
      const record = { options, refreshes: 0, disposed: false };
      records.push(record);
      return {
        ready: Promise.resolve(),
        refresh: async () => { record.refreshes++; },
        flush: async () => true,
        snapshot: () => ({ saved: true }),
        dispose: () => { record.disposed = true; },
      };
    },
  };
  return { views, records };
}

const saved = config => ({ space: 'space-a', config });
const taskConfig = q => ({ plugin: 'todo', query: 'tasks', q });

test('refreshing a saved-view reference remounts when its saved config changes', async () => {
  const { views, records } = mountedViews();
  let current = saved(taskConfig('first'));
  const page = instance(prototype(views), { saved: () => current });
  const target = body();
  const handle = page.viewWidget(target, page.placement, { saved_view: 'view-1' });
  await handle.ready;
  current = saved(taskConfig('second'));
  await handle.refresh();
  assert.equal(records.length, 2);
  assert.equal(records[0].options.config.q, 'first');
  assert.equal(records[0].disposed, true);
  assert.equal(records[1].options.config.q, 'second');
});

test('refreshing an unchanged saved config refreshes its child view', async () => {
  const { views, records } = mountedViews();
  const page = instance(prototype(views), { saved: saved(taskConfig('same')) });
  const handle = page.viewWidget(body(), page.placement, { saved_view: 'view-1' });
  await handle.ready;
  await handle.refresh();
  assert.equal(records.length, 1);
  assert.equal(records[0].refreshes, 1);
});

test('a missing saved view reports a visible error', async () => {
  const views = { ...mountedViews().views };
  const page = instance(prototype(views), { saved: async () => { throw new Error('view was removed'); } });
  const target = body();
  const handle = page.viewWidget(target, page.placement, { saved_view: 'gone' });
  await handle.ready;
  assert.match(target.children[0].textContent, /Saved view unavailable: Error: view was removed/);
});

test('disposing before view lookup resolves ignores the late response', async () => {
  const { views, records } = mountedViews();
  let finish;
  const page = instance(prototype(views), { saved: () => new Promise(resolve => { finish = resolve; }) });
  const target = body();
  const handle = page.viewWidget(target, page.placement, { saved_view: 'late' });
  handle.dispose();
  finish(saved(taskConfig('late')));
  await handle.ready;
  assert.equal(target.children.length, 0);
  assert.equal(records.length, 0);
});

test('view placements pass their own focus and size independently', async () => {
  const { views, records } = mountedViews();
  const page = instance(prototype(views));
  const own = { ...page.placement, focus: 'local-focus', w: 3, h: 3 };
  const inherited = { ...page.placement, id: 'placement-2', w: 9, h: 4 };
  await page.viewWidget(body(), own, taskConfig('one')).ready;
  await page.viewWidget(body(), inherited, taskConfig('two')).ready;
  assert.equal(records[0].options.focus, 'local-focus');
  assert.equal(records[0].options.size, 'compact');
  assert.equal(records[1].options.focus, 'page-focus');
  assert.equal(records[1].options.size, 'expanded');
});
