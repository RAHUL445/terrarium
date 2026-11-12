const { test } = require('node:test');
const assert = require('node:assert/strict');
const { rendererHarness } = require('./helpers/renderer-dom.cjs');

function template(layout = 'board', extra = {}) {
  return { state: { q: '' }, body: { type: 'list', layout, label: 'Task board',
    query: 'tasks', params: { q: '{{state.q}}' }, column: 'lane', empty: 'No matching tasks',
    columns: [{ value: 'todo', label: 'To do' }, { value: 'doing', label: 'In progress' }, { value: 'done', label: 'Done' }],
    item: { type: 'text-field', name: 'title', value: '{{row.title}}' },
    ...extra,
  } };
}
const fields = root => root.querySelectorAll('[data-pp-name]').map(input => input.value);
const textContent = (node) => (node ? `${node.textContent || ''}${node.children.map(textContent).join('')}` : '');
const statusText = (board) => textContent(board.querySelector('.pp-board-status'));
const boardColumns = [
  { value: 'todo', label: 'To do' },
  { value: 'doing', label: 'In progress' },
  { value: 'done', label: 'Done' },
  { value: 'blocked', label: 'Blocked' },
];

test('board distributes a single filtered result into counted lanes and retains empty lanes', async () => {
  const ui = rendererHarness(); let calls = 0;
  const view = ui.renderer.mount(ui.root, template(), { today: () => '2026-11-07', query: async (name, params) => {
    calls++; assert.equal(name, 'tasks');
    return params.q ? [] : [{title:'First',lane:'todo'}, {title:'Second',lane:'done'}];
  }});
  await view.ready;
  const board = ui.root.children[0];
  assert.equal(calls, 1);
  assert.equal(board.attrs['aria-label'], 'Task board');
  assert.deepEqual(board.children.map(lane => lane.children[0].children[1].textContent), ['1','0','1']);
  assert.deepEqual(fields(board), ['First','Second']);
  view.state.q = 'No match'; await view.refresh();
  assert.equal(board.children.length, 4, 'empty message plus three visible lanes');
  assert.deepEqual(fields(board), []);
  view.dispose();
});

test('matrix refresh moves a changed item and an older response cannot restore its old quadrant', async () => {
  const ui = rendererHarness(); let rows = [{title:'Task', lane:'todo'}]; let pending;
  const view = ui.renderer.mount(ui.root, template('matrix'), { today: () => '2026-11-07', query: async () => rows });
  await view.ready;
  view.bridge.query = () => new Promise(resolve => { pending = resolve; });
  const old = view.refresh();
  view.bridge.query = async () => [{title:'Task',lane:'doing'}];
  await view.refresh(); pending(rows); await old;
  const board = ui.root.children[0];
  assert.deepEqual(fields(board.children[0]), []);
  assert.deepEqual(fields(board.children[1]), ['Task']);
  const watchers = view.watchers.length;
  for (let i = 0; i < 10; i++) await view.refresh();
  assert.equal(view.watchers.length, watchers);
  view.dispose();
});

test('a board without move renders no handles', async () => {
  const ui = rendererHarness();
  const view = ui.renderer.mount(ui.root, template('board', {
    item: { type: 'text', text: '{{row.title}}' },
  }), { today: () => '2026-11-07', query: async () => [{ id: 'a', title: 'First', lane: 'todo' }] });
  await view.ready;
  assert.equal(ui.root.querySelectorAll('.pp-board-handle').length, 0);
  view.dispose();
});

test('arrow keys move a card to the next lane and call the move op', async () => {
  const ui = rendererHarness(); const calls = [];
  const view = ui.renderer.mount(ui.root, template('board', {
    move: { op: 'move', args: { id: '{{row.id}}', lane: '{{value}}' } },
    item: { type: 'text', text: '{{row.title}}' },
  }), {
    today: () => '2026-11-07',
    query: async () => [{ id: 'a', title: 'First', lane: 'todo' }],
    action: async (op, args) => { calls.push([op, args]); return {}; },
    changed() {},
    toast() {},
  });
  await view.ready;
  const handle = ui.root.querySelector('.pp-board-handle');
  await handle.fire('keydown', { key: 'ArrowRight' });
  assert.equal(calls.length, 1);
  assert.equal(calls[0][0], 'move');
  assert.equal(JSON.stringify(calls[0][1]), JSON.stringify({ id: 'a', lane: 'doing' }));
  const board = ui.root.children[0];
  assert.equal(statusText(board), 'Moved First to In progress');
  view.dispose();
});

test('the first and last lanes stop arrow moves', async () => {
  const ui = rendererHarness(); const calls = [];
  const view = ui.renderer.mount(ui.root, template('board', {
    move: { op: 'move', args: { id: '{{row.id}}', lane: '{{value}}' } },
    item: { type: 'text', text: '{{row.title}}' },
  }), {
    today: () => '2026-11-07',
    query: async () => [{ id: 'a', title: 'First', lane: 'todo' }],
    action: async (op, args) => { calls.push([op, args]); return {}; },
    changed() {},
    toast() {},
  });
  await view.ready;
  const handle = ui.root.querySelector('.pp-board-handle');
  await handle.fire('keydown', { key: 'ArrowLeft' });
  assert.deepEqual(calls, []);
  view.dispose();
});

test('matrix arrows move by row and column', async () => {
  const ui = rendererHarness(); const calls = [];
  const view = ui.renderer.mount(ui.root, template('matrix', {
    columns: boardColumns,
    move: { op: 'move', args: { id: '{{row.id}}', lane: '{{value}}' } },
    item: { type: 'text', text: '{{row.title}}' },
  }), {
    today: () => '2026-11-07',
    query: async () => [{ id: 'a', title: 'First', lane: 'todo' }],
    action: async (op, args) => { calls.push([op, args]); return {}; },
    changed() {},
    toast() {},
  });
  await view.ready;
  const handle = ui.root.querySelector('.pp-board-handle');
  await handle.fire('keydown', { key: 'ArrowDown' });
  assert.equal(calls.length, 1);
  assert.equal(calls[0][0], 'move');
  assert.equal(JSON.stringify(calls[0][1]), JSON.stringify({ id: 'a', lane: 'done' }));
  view.dispose();
});

test('a failed move reloads the board', async () => {
  const ui = rendererHarness();
  let queries = 0; const toasts = [];
  const rows = [{ id: 'a', title: 'First', lane: 'todo' }];
  const view = ui.renderer.mount(ui.root, template('board', {
    move: { op: 'move', args: { id: '{{row.id}}', lane: '{{value}}' } },
    item: { type: 'text', text: '{{row.title}}' },
  }), {
    today: () => '2026-11-07',
    query: async () => { queries++; return rows; },
    action: async () => { throw new Error('Move failed'); },
    changed() {},
    toast(message) { toasts.push(String(message)); },
  });
  await view.ready;
  const before = queries;
  const handle = ui.root.querySelector('.pp-board-handle');
  await handle.fire('keydown', { key: 'ArrowRight' });
  assert.ok(toasts.some((message) => message.includes('Move failed')));
  assert.ok(queries > before);
  const board = ui.root.children[0];
  assert.equal(statusText(board), "Couldn't move First");
  view.dispose();
});

test('boards accept any number of lanes', async () => {
  const ui = rendererHarness();
  const view = ui.renderer.mount(ui.root, template('board', {
    columns: [
      { value: 'todo', label: 'To do' },
      { value: 'doing', label: 'In progress' },
      { value: 'done', label: 'Done' },
      { value: 'later', label: 'Later' },
      { value: 'blocked', label: 'Blocked' },
    ],
  }), { today: () => '2026-11-07', query: async () => [] });
  await view.ready;
  const board = ui.root.children[0];
  assert.equal(board.style['--pp-lanes'], '5');
  view.dispose();
});
