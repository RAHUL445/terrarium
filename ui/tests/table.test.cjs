const { test } = require('node:test');
const assert = require('node:assert/strict');
const { rendererHarness } = require('./helpers/renderer-dom.cjs');

const all = (node) => [node, ...node.children.flatMap(all)];

function bridge(overrides = {}) {
  return {
    today: () => '2026-11-07',
    query: async () => [],
    action: async () => ({}),
    changed() {},
    toast() {},
    ...overrides,
  };
}

function text(node) {
  return [node.textContent || '', ...node.children.map(text)].join('');
}

test('table sorts by a column and flips direction', async () => {
  const ui = rendererHarness();
  let queries = 0;
  const template = {
    state: {},
    body: { type: 'table', query: 'tasks', label: 'Tasks', columns: [
      { key: 'title', label: 'Task' },
      { key: 'due', label: 'Due', type: 'date' },
      { key: 'n', label: 'Count', type: 'integer' },
    ], sort: 'due' },
  };
  const rows = [
    { title: 'B', due: '2026-11-09', n: 10 },
    { title: 'A', due: null, n: 2 },
    { title: 'C', due: '2026-11-01', n: 9 },
  ];
  const view = ui.renderer.mount(ui.root, template, bridge({
    query: async () => { queries++; return rows; },
  }));
  await view.ready;

  const titles = () => all(ui.root).filter((node) => node.tag === 'td' && node.attrs['data-label'] === 'Task').map(text);
  const headers = () => all(ui.root).filter((node) => node.tag === 'th');
  const sortButton = (label) => all(ui.root).find((node) => node.className === 'pp-table-sort' && node.textContent === label);

  assert.deepEqual(titles(), ['C', 'B', 'A']);
  assert.equal(headers()[1].attrs['aria-sort'], 'ascending');
  assert.equal(headers()[0].attrs['aria-sort'], 'none');
  assert.equal(headers()[2].attrs['aria-sort'], 'none');

  await sortButton('Due').fire('click');
  assert.deepEqual(titles(), ['B', 'C', 'A']);
  assert.equal(headers()[1].attrs['aria-sort'], 'descending');

  await sortButton('Count').fire('click');
  assert.deepEqual(titles(), ['A', 'C', 'B']);
  assert.equal(headers()[2].attrs['aria-sort'], 'ascending');
  assert.equal(headers()[1].attrs['aria-sort'], 'none');
  assert.equal(queries, 1);
  view.dispose();
});

test('cells carry their labels and booleans read Yes', async () => {
  const ui = rendererHarness();
  const view = ui.renderer.mount(ui.root, {
    state: {},
    body: { type: 'table', query: 'tasks', label: 'Tasks', columns: [
      { key: 'title', label: 'Task' },
      { key: 'done', label: 'Done', type: 'boolean' },
    ] },
  }, bridge({
    query: async () => [{ title: 'A', done: true }, { title: 'B', done: false }],
  }));
  await view.ready;
  const cells = all(ui.root).filter((node) => node.tag === 'td' && node.attrs['data-label']);
  assert.ok(cells.every((node) => ['Task', 'Done'].includes(node.attrs['data-label'])));
  const done = cells.filter((node) => node.attrs['data-label'] === 'Done').map(text);
  assert.deepEqual(done, ['Yes', '']);
  view.dispose();
});

test('the first cell opens the row', async () => {
  const ui = rendererHarness();
  const actions = [];
  const view = ui.renderer.mount(ui.root, {
    state: {},
    body: { type: 'table', query: 'tasks', label: 'Tasks', columns: [
      { key: 'title', label: 'Task' },
      { key: 'due', label: 'Due', type: 'date' },
    ], action: { op: 'open', args: { id: '{{row.id}}' } } },
  }, bridge({
    query: async () => [{ id: 'a', title: 'Alpha', due: '2026-11-07' }],
    action: async (op, args) => { actions.push([op, args]); return {}; },
  }));
  await view.ready;
  const open = all(ui.root).find((node) => node.className === 'pp-table-open');
  await open.fire('click');
  assert.equal(actions.length, 1);
  assert.equal(actions[0][0], 'open');
  assert.equal(actions[0][1].id, 'a');
  view.dispose();
});

test('an empty result shows the empty text across all columns', async () => {
  const ui = rendererHarness();
  const view = ui.renderer.mount(ui.root, {
    state: {},
    body: { type: 'table', query: 'tasks', label: 'Tasks', empty: 'Nothing here', columns: [
      { key: 'title', label: 'Task' },
      { key: 'due', label: 'Due', type: 'date' },
      { key: 'n', label: 'Count', type: 'integer' },
    ] },
  }, bridge());
  await view.ready;
  const empty = all(ui.root).find((node) => node.className === 'pp-empty');
  assert.equal(empty.attrs.colspan, '3');
  assert.equal(empty.textContent, 'Nothing here');
  view.dispose();
});

test('sortRows keeps ties in order and empties last both ways', () => {
  const rows = [{ id: 'a', n: 2 }, { id: 'b', n: null }, { id: 'c', n: 2 }, { id: 'd', n: 10 }, { id: 'e', n: '' }];
  const asc = uiOrder(rows, 'n', 'integer', 'asc');
  const desc = uiOrder(rows, 'n', 'integer', 'desc');
  assert.deepEqual(asc, ['a', 'c', 'd', 'b', 'e']);
  assert.deepEqual(desc, ['d', 'a', 'c', 'b', 'e']);

  function uiOrder(input, key, type, dir) {
    return rendererHarness().renderer.sortRows(input, key, type, dir).map((row) => row.id);
  }
});

test('server sorted table preserves returned order and exposes no local sort controls', async () => {
  const ui = rendererHarness();
  const view = ui.renderer.mount(ui.root, { body: { type: 'table', query: 'tasks', server_sorted: true,
    sort: 'title', dir: 'desc', columns: [{ key: 'title', label: 'Task' }] } }, bridge({
    query: async () => [{ id: 'b', title: 'Second' }, { id: 'a', title: 'First' }],
  }));
  await view.ready;
  assert.deepEqual(all(ui.root).filter(node => node.tag === 'td' && node.attrs['data-label'] === 'Task').map(text), ['Second', 'First']);
  assert.equal(all(ui.root).some(node => node.className === 'pp-table-sortbar'), false);
  assert.equal(all(ui.root).some(node => node.className === 'pp-table-sort'), false);
  assert.equal(all(ui.root).some(node => node.tag === 'select'), false);
  view.dispose();
});

test('table row_action passes row id and followup receives the operation result', async () => {
  const ui = rendererHarness();
  const actions = [], notices = [];
  const rowAction = {
    label: 'Archive', op: 'archive', args: { id: '{{row.id}}' }, success: 'Archived',
    followup: { label: 'Review archive', action: { op: 'review', args: { id: '{{row.id}}', prompt: '{{result.prompt}}' } } },
  };
  const body = { type: 'table', query: 'tasks', columns: [{ key: 'title', label: 'Task' }], row_action: rowAction };
  const view = ui.renderer.mount(ui.root, { body }, bridge({
    query: async () => [{ id: 'task-7', title: 'Tidy desk' }],
    action: async (op, args) => { actions.push({ op, args }); return op === 'archive' ? { prompt: 'Archive task-7?' } : {}; },
    toast: (...notice) => notices.push(notice),
  }));
  await view.ready;
  const button = all(ui.root).find(node => String(node.className).includes('pp-row-action'));
  assert.equal(button.textContent, 'Archive');
  await button.fire('click');
  assert.equal(actions[0].op, 'archive');
  assert.equal(actions[0].args.id, 'task-7');
  assert.equal(notices[0][0], 'Archived');
  assert.equal(notices[0][2].label, 'Review archive');
  await notices[0][2].run();
  assert.equal(actions[1].op, 'review');
  assert.equal(actions[1].args.id, 'task-7');
  assert.equal(actions[1].args.prompt, 'Archive task-7?');
  view.dispose();
});
