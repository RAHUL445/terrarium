const test = require('node:test');
const assert = require('node:assert/strict');

const { actionsFrom, argsFor } = require('../quick.js');

test('quick actions from every plugin are listed with their app name', () => {
  const out = actionsFrom({
    plugins: [
      {
        id: 'todo',
        name: 'To-do',
        quick_actions: [{ label: 'New task', op: 'create', arg: 'title' }],
      },
      { id: 'x', name: 'X', quick_actions: [] },
    ],
  });
  assert.equal(out.length, 1);
  assert.equal(out[0].label, 'New task · To-do');
  assert.equal(out[0].key, 'todo/create/title');
});

test('typed text goes to the declared argument', () => {
  assert.deepEqual(argsFor({ arg: 'body' }, 'Hello'), { body: 'Hello' });
});
