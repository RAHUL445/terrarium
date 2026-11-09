const { test } = require('node:test');
const assert = require('node:assert/strict');
const { rendererHarness, listTemplate } = require('./helpers/renderer-dom.cjs');

async function mount(query, template = listTemplate()) {
  const harness = rendererHarness();
  const view = harness.renderer.mount(harness.root, template, {
    today: () => '2026-11-04', query,
  });
  await view.ready;
  return { ...harness, view, list: harness.root.children[0] };
}

test('unchanged refreshes retain row controls and unsaved typing without allocating new rows', async () => {
  let queries = 0;
  const ui = await mount(async () => { queries++; return [{ title: 'Stored title' }]; });
  const input = ui.list.querySelector('[data-pp-name]');
  input.value = 'Unsaved typing';
  const allocations = ui.allocations();
  for (let i = 0; i < 20; i++) await ui.view.refresh();
  assert.equal(queries, 21, 'fresh data must still be checked');
  assert.equal(ui.allocations(), allocations);
  assert.equal(ui.list.querySelector('[data-pp-name]'), input);
  assert.equal(input.value, 'Unsaved typing');
});

test('changing rows repeatedly does not retain watchers for detached forms', async () => {
  let revision = 0;
  const ui = await mount(async () => [{ title: `Revision ${revision++}` }]);
  const watchers = ui.view.watchers.length;
  for (let i = 0; i < 30; i++) await ui.view.refresh();
  assert.equal(ui.view.watchers.length, watchers);
  assert.equal(ui.list.querySelector('[data-pp-name]').value, 'Revision 30');
});

test('state-dependent row content updates even when query data is unchanged', async () => {
  const template = listTemplate();
  template.body.item.children[0].value = '{{state.q}}';
  const ui = await mount(async () => [{ title: 'Same row' }], template);
  ui.view.state.q = 'New state';
  await ui.view.refresh();
  assert.equal(ui.list.querySelector('[data-pp-name]').value, 'New state');
});

test('an older query response or error cannot replace the latest filtered result', async () => {
  const pending = [];
  let initial = true;
  const ui = await mount(() => {
    if (initial) { initial = false; return Promise.resolve([{ title: 'Initial' }]); }
    return new Promise((resolve, reject) => pending.push({ resolve, reject }));
  });
  const old = ui.view.refresh();
  const latest = ui.view.refresh();
  pending[1].resolve([{ title: 'Latest' }]); await latest;
  pending[0].resolve([{ title: 'Old' }]); await old;
  assert.equal(ui.list.querySelector('[data-pp-name]').value, 'Latest');
  const oldFailure = ui.view.refresh();
  const newest = ui.view.refresh();
  pending[3].resolve([{ title: 'Newest' }]); await newest;
  pending[2].reject(Error('Old failure')); await oldFailure;
  assert.equal(ui.list.querySelector('[data-pp-name]').value, 'Newest');
});

test('a failed load can recover to the same rows instead of retaining the error screen', async () => {
  let fail = false;
  const ui = await mount(async () => { if (fail) throw Error('offline'); return [{ title: 'Same row' }]; });
  fail = true; await ui.view.refresh();
  assert.equal(ui.list.querySelector('[data-pp-name]'), null);
  fail = false; await ui.view.refresh();
  assert.equal(ui.list.querySelector('[data-pp-name]').value, 'Same row');
});

test('unchanged parent rows still refresh nested data', async () => {
  const template = listTemplate();
  template.body.item = { type: 'list', query: 'children', item: {
    type: 'text-field', name: 'title', value: '{{row.title}}',
  } };
  let childTitle = 'Before';
  const ui = await mount(async name => name === 'entries' ? [{ id: 1 }] : [{ title: childTitle }], template);
  const parentRow = ui.list.children[0];
  childTitle = 'After'; await ui.view.refresh();
  assert.equal(ui.list.children[0], parentRow);
  assert.equal(ui.list.querySelector('[data-pp-name]').value, 'After');
});

test('large lists yield before completing and preserve every row in order', async () => {
  const harness = rendererHarness();
  const rows = Array.from({ length: 500 }, (_, id) => ({ title: `Row ${id}` }));
  let rowsAtNextTask;
  const view = harness.renderer.mount(harness.root, listTemplate(), {
    today: () => '2026-11-04', query: async () => rows,
  });
  await new Promise(resolve => setTimeout(() => {
    rowsAtNextTask = harness.root.querySelectorAll('[data-pp-name]').length;
    resolve();
  }, 0));
  assert.ok(rowsAtNextTask > 0 && rowsAtNextTask <= 32, 'other tasks run between batches');
  await view.ready;
  assert.deepEqual(harness.root.querySelectorAll('[data-pp-name]').map(node => node.value), rows.map(row => row.title));
  assert.equal(harness.root.children[0].attrs['aria-busy'], 'false');
});

test('a new refresh cancels an incomplete batch sequence without retaining partial results', async () => {
  const harness = rendererHarness();
  let rows = Array.from({ length: 100 }, (_, id) => ({ title: `Old ${id}` }));
  const view = harness.renderer.mount(harness.root, listTemplate(), { today: () => '2026-11-04', query: async () => rows });
  await new Promise(resolve => setTimeout(resolve, 0));
  rows = [{ title: 'Newest' }];
  await view.refresh(); await view.ready;
  assert.deepEqual(harness.root.querySelectorAll('[data-pp-name]').map(node => node.value), ['Newest']);
  await view.refresh();
  assert.deepEqual(harness.root.querySelectorAll('[data-pp-name]').map(node => node.value), ['Newest']);
});

test('group headings remain ordered and unique across render batches', async () => {
  const template = listTemplate(); template.body.group = 'section';
  const rows = Array.from({ length: 100 }, (_, index) => ({ title: `Row ${index}`, section: index < 35 ? 'A' : index < 68 ? 'B' : 'C' }));
  const ui = await mount(async () => rows, template);
  assert.deepEqual(ui.list.children.filter(node => node.className === 'pp-group').map(node => node.textContent), ['A', 'B', 'C']);
  ui.view.dispose();
});
