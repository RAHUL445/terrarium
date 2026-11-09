const { test } = require('node:test');
const assert = require('node:assert/strict');
const { rendererHarness } = require('./helpers/renderer-dom.cjs');

const template = {
  state: {},
  page: { config: { filter: { type: 'enum', values: ['today', 'open'], default: 'today' }, label: { type: 'text' } } },
  body: { type: 'list', query: 'tasks', params: { filter: '{{$config.filter}}', label: '{{$config.label}}', size: '{{$size}}' }, item: { type: 'text', text: '{{row.title}}' } },
};

async function paramsFor(options) {
  const calls = [];
  const bridge = { today: () => '2026-11-07', query: async (name, params) => { calls.push(params); return []; }, action: async () => ({}), changed() {}, toast() {} };
  const ui = rendererHarness();
  const view = ui.renderer.mount(ui.root, template, bridge, options);
  await view.ready;
  view.dispose();
  return JSON.parse(JSON.stringify(calls[0]));
}

test('widget settings start from declared defaults everywhere the widget mounts', async () => {
  assert.deepEqual(await paramsFor(undefined), { filter: 'today', size: 'regular' });
});

test('a placement overrides settings and passes its size class', async () => {
  assert.deepEqual(await paramsFor({ config: { filter: 'open', label: 'Work' }, size: 'compact' }), { filter: 'open', label: 'Work', size: 'compact' });
});

test('widgets read the page focus, blank outside pages', async () => {
  const calls = [];
  const bridge = { today: () => '2026-11-07', query: async (name, params) => { calls.push(params); return []; }, action: async () => ({}), changed() {}, toast() {} };
  const focusTemplate = { state: {}, body: { type: 'list', query: 'tasks', params: { q: '{{$page.focus}}', title: '{{$page.title}}' }, item: { type: 'text', text: '{{row.title}}' } } };
  for (const options of [undefined, { page: { focus: 'Launch', title: 'Project X' } }]) {
    const ui = rendererHarness();
    const view = ui.renderer.mount(ui.root, focusTemplate, bridge, options);
    await view.ready;
    view.dispose();
  }
  assert.deepEqual(JSON.parse(JSON.stringify(calls)), [{ q: '', title: '' }, { q: 'Launch', title: 'Project X' }]);
});

test('a focused page shows the list text for nothing matching', async () => {
  const bridge = { today: () => '2026-11-07', query: async () => [], action: async () => ({}), changed() {}, toast() {} };
  const emptyTemplate = { state: {}, body: { type: 'list', query: 'recent', empty: 'No notes yet.', empty_focus: 'No notes match “{{$page.focus}}”.', item: { type: 'text', text: '{{row.title}}' } } };
  const texts = [];
  for (const options of [undefined, { page: { focus: 'Launch' } }]) {
    const ui = rendererHarness();
    const view = ui.renderer.mount(ui.root, emptyTemplate, bridge, options);
    await view.ready;
    await new Promise((resolve) => setTimeout(resolve, 0));
    const text = (node) => [typeof node.textContent === 'string' ? node.textContent : '', ...(node.children || []).map(text)].join('');
    texts.push(text(ui.root).trim());
    view.dispose();
  }
  assert.deepEqual(texts, ['No notes yet.', 'No notes match “Launch”.']);
});
