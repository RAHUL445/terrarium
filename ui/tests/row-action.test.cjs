const test = require('node:test');
const assert = require('node:assert/strict');
const { rendererHarness } = require('./helpers/renderer-dom.cjs');

const all = node => [node, ...(node.children || []).filter(child => typeof child !== 'string').flatMap(all)];

test('list item row_action runs through the renderer and yields to an existing trailing slot', async () => {
  const calls = [];
  const bridge = { today: () => '2026-11-13', query: async () => [{ id: 'n1', title: 'Note' }],
    action: async (op, args) => { calls.push([op, args]); return {}; }, toast() {} };
  const h = rendererHarness();
  const view = h.renderer.mount(h.root, { body: { type: 'list', query: 'notes', item: {
    type: 'item', title: '{{row.title}}', row_action: { label: 'Pin', op: 'pin', args: { id: '{{row.id}}' } },
  } } }, bridge);
  await view.ready;
  const action = all(h.root).find(node => String(node.className).includes('pp-row-action'));
  assert.equal(action.textContent, 'Pin');
  await action.fire('click');
  assert.equal(calls.length, 1);
  assert.equal(calls[0][0], 'pin');
  assert.equal(calls[0][1].id, 'n1');
  view.dispose();

  const withTrailing = rendererHarness();
  const second = withTrailing.renderer.mount(withTrailing.root, { body: { type: 'item', title: 'Note',
    row_action: { label: 'Pin', op: 'pin' }, trailing: [{ type: 'text', text: 'Existing action' }] } }, bridge);
  await second.ready;
  assert.equal(all(withTrailing.root).some(node => String(node.className).includes('pp-row-action')), false);
  second.dispose();
});
