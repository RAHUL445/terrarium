const { test } = require('node:test');
const assert = require('node:assert/strict');
const { summarize } = require('../status.js');
test('status reports active work only from the current space', () => {
  const runs = ['running', 'waiting', 'retry', 'done'].map((status, i) => ({ id: String(i), space: 'personal', status }));
  runs.push({ id: 'other', space: 'work', status: 'running' });
  assert.deepEqual(summarize(runs, 'personal'), { active: 3, failed: 0 });
});
test('a later successful run resolves a historical schedule failure regardless of input order', () => {
  const newer = { schedule: 'review', space: 'personal', slot_ms: 20, status: 'done' };
  const older = { ...newer, slot_ms: 10, status: 'failed' };
  assert.equal(summarize([newer, older], 'personal').failed, 0);
  assert.equal(summarize([older, newer], 'personal').failed, 0);
  assert.equal(summarize([{ ...newer, status: 'failed' }, { ...older, status: 'done' }], 'personal').failed, 1);
});
test('missing history and another space failure do not invent an issue', () => {
  assert.deepEqual(summarize(null, 'personal'), { active: 0, failed: 0 });
  assert.deepEqual(summarize([{ space: 'work', status: 'failed', id: 'w' }], 'personal'), { active: 0, failed: 0 });
});
