const { test } = require('node:test');
const assert = require('node:assert/strict');
const { rendererHarness } = require('./helpers/renderer-dom.cjs');

function template(layout = 'board') {
  return { state: { q: '' }, body: { type: 'list', layout, label: 'Task board',
    query: 'tasks', params: { q: '{{state.q}}' }, column: 'lane', empty: 'No matching tasks',
    columns: [{ value: 'todo', label: 'To do' }, { value: 'doing', label: 'In progress' }, { value: 'done', label: 'Done' }],
    item: { type: 'text-field', name: 'title', value: '{{row.title}}' },
  } };
}
const fields = root => root.querySelectorAll('[data-pp-name]').map(input => input.value);

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
