const { test } = require('node:test');
const assert = require('node:assert/strict');
const { rendererHarness } = require('./helpers/renderer-dom.cjs');
const turn = () => new Promise(resolve => setImmediate(resolve));

async function editor(action, query = async (name, params) => [{ id: params.id, title: params.id }]) {
  const harness = rendererHarness();
  const view = harness.renderer.mount(harness.root, { state: { open: 'a' }, body: {
    type: 'sheet', bind: 'state.open', title: 'Note', query: 'note', params: { id: '{{state.open}}' },
    on_close: { op: 'close', args: { id: '{{row.id}}' } },
    children: [{ type: 'form', autosave: 800, submit: { op: 'save', args: { id: '{{row.id}}', title: '{{form.title}}' } },
      children: [{ type: 'text-field', name: 'title', value: '{{row.title}}' }] }],
  } }, { today: () => '2026-11-04', query, action, changed() {}, toast() {} });
  await view.ready; await turn();
  const dialog = harness.root.children[0];
  return { ...harness, view, dialog, close: dialog.children[0].children[1],
    form: () => dialog.querySelector('form'), input: () => dialog.querySelector('[data-pp-name]') };
}

test('closing keeps the modal open until autosave completes and immediate reopen survives the close event', async () => {
  let finish;
  const operations = [];
  const ui = await editor((op, args) => {
    operations.push([op, args.id]);
    return op === 'save' ? new Promise(resolve => { finish = resolve; }) : Promise.resolve({});
  });
  ui.input().value = 'Edited'; ui.form().dispatchEvent({ type: 'input' });
  const closing = ui.close.fire('click'); await turn();
  assert.equal(ui.dialog.open, true, 'underlying page stays inert during save');
  assert.equal(ui.close.disabled, true);
  finish({}); await closing;
  assert.equal(ui.dialog.open, false);
  assert.equal(ui.view.state.open, '');
  ui.view.setState('state.open', 'a'); await turn();
  assert.equal(ui.dialog.open, true);
  assert.equal(ui.view.state.open, 'a');
  assert.deepEqual(operations, [['save', 'a'], ['close', 'a']]);
});

test('an old close cannot clear a newer note selected during a pending save', async () => {
  let finish;
  const ui = await editor(op => op === 'save' ? new Promise(resolve => { finish = resolve; }) : Promise.resolve({}));
  ui.input().value = 'Edited'; ui.form().dispatchEvent({ type: 'input' });
  const closing = ui.close.fire('click'); await turn();
  ui.view.setState('state.open', 'b'); await turn();
  finish({}); await closing;
  assert.equal(ui.view.state.open, 'b');
  assert.equal(ui.dialog.open, true);
  assert.equal(ui.input().value, 'b');
});

test('failed autosave leaves the editor and draft open for retry', async () => {
  let fail = true;
  const ui = await editor(async op => { if (op === 'save' && fail) throw Error('offline'); return {}; });
  ui.input().value = 'Keep this'; ui.form().dispatchEvent({ type: 'input' });
  await ui.close.fire('click');
  assert.equal(ui.dialog.open, true);
  assert.equal(ui.input().value, 'Keep this');
  assert.equal(ui.close.disabled, false);
  fail = false; await ui.close.fire('click');
  assert.equal(ui.dialog.open, false);
});

test('Escape uses the same save-before-close path', async () => {
  const ui = await editor(async () => ({}));
  let prevented = false;
  await ui.dialog.fire('cancel', { preventDefault() { prevented = true; } });
  await turn();
  assert.equal(prevented, true);
  assert.equal(ui.dialog.open, false);
});

test('a stale missing-note response cannot clear the newer selection', async () => {
  let old;
  const ui = await editor(async () => ({}), async (name, params) => {
    if (params.id === 'slow') return new Promise(resolve => { old = resolve; });
    return [{ id: params.id, title: params.id }];
  });
  ui.view.setState('state.open', 'slow');
  ui.view.setState('state.open', 'b'); await turn();
  old([]); await turn();
  assert.equal(ui.view.state.open, 'b');
  assert.equal(ui.dialog.open, true);
});
