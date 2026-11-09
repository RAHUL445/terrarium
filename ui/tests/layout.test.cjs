const { test } = require('node:test');
const assert = require('node:assert/strict');
const layout = require('../layout.js');
const storage = () => {
  const data = new Map();
  return { getItem: (key) => data.get(key), setItem: (key, value) => data.set(key, value) };
};
test('malformed and unavailable storage preserve default layout', () => {
  assert.equal(layout.read({ getItem: () => '{broken' }, 'personal').nav, null);
  assert.equal(layout.read({ getItem() { throw Error('denied'); } }, 'personal').widgets, null);
  assert.equal(layout.write({ setItem() { throw Error('full'); } }, 'personal', {}), false);
});
test('pin selections and order round-trip independently per space', () => {
  const device = storage();
  layout.write(device, 'work', { nav: ['notes', 'todo'], widgets: ['todo/today'], actions: [], folded: ['todo/today'] });
  layout.write(device, 'personal', { nav: ['habits'], widgets: ['notes/capture'] });
  assert.deepEqual(layout.read(device, 'work').nav, ['notes', 'todo']);
  assert.deepEqual(layout.read(device, 'work').folded, ['todo/today']);
  assert.deepEqual(layout.read(device, 'personal').nav, ['habits']);
});
test('hiding everything remains distinct from an unconfigured rail', () => {
  assert.deepEqual(layout.selected(null, ['todo', 'notes']), ['todo', 'notes']);
  assert.deepEqual(layout.selected([], ['todo', 'notes']), []);
  assert.deepEqual(layout.selected(null, ['a', 'b'], ['b']), ['b']);
});
test('removed plugins cannot render; their preferences survive reinstall', () => {
  const saved = layout.normalize({ widgets: ['notes/capture', 'todo/today'] });
  assert.deepEqual(layout.selected(saved.widgets, ['todo/today']), ['todo/today']);
  assert.deepEqual(layout.selected(saved.widgets, ['todo/today', 'notes/capture']), ['notes/capture', 'todo/today']);
});
test('pin toggling and reordering cannot duplicate or lose items at boundaries', () => {
  assert.deepEqual(layout.toggle(['notes'], 'notes', true), ['notes']);
  assert.deepEqual(layout.toggle(['notes', 'todo'], 'notes', false), ['todo']);
  assert.deepEqual(layout.move(['notes', 'todo'], 'todo', -1), ['todo', 'notes']);
  assert.deepEqual(layout.move(['notes', 'todo'], 'notes', -1), ['notes', 'todo']);
  assert.deepEqual(layout.move(['notes', 'todo'], 'missing', 1), ['notes', 'todo']);
});
test('untrusted saved values are bounded and normalized to IDs', () => {
  assert.deepEqual(layout.normalize({ nav: ['notes', null, 7, 'notes'], widgets: 'bad' }).nav, ['notes']);
  assert.equal(layout.normalize({ widgets: 'bad' }).widgets, null);
  assert.equal(layout.normalize({ nav: Array.from({ length: 300 }, (_, i) => String(i)) }).nav.length, 200);
});
test('Home layout preserves defaults, explicit empty selections and per-space sizing', () => {
  const device = storage();
  assert.equal(layout.read(device, 'work').home, null);
  layout.write(device, 'work', { home: ['notes/recent', 'todo/today'], homeWide: ['notes/recent'], nav: ['todo'] });
  layout.write(device, 'personal', { home: [] });
  assert.deepEqual(layout.read(device, 'work').home, ['notes/recent', 'todo/today']);
  assert.deepEqual(layout.read(device, 'work').homeWide, ['notes/recent']);
  assert.deepEqual(layout.read(device, 'work').nav, ['todo']);
  assert.deepEqual(layout.read(device, 'personal').home, []);
  assert.deepEqual(layout.normalize({ home: 'bad', homeWide: [null, 'notes/recent', 'notes/recent'] }).homeWide, ['notes/recent']);
});
test('drop placement moves in both directions including the final slot without losing IDs', () => {
  assert.deepEqual(layout.place(['a', 'b', 'c'], 'a', 'c'), ['b', 'c', 'a']);
  assert.deepEqual(layout.place(['a', 'b', 'c'], 'c', 'a'), ['c', 'a', 'b']);
  assert.deepEqual(layout.place(['a', 'b', 'c'], 'b', 'b'), ['a', 'b', 'c']);
  assert.deepEqual(layout.place(['a', 'b'], 'external', 'b'), ['a', 'b']);
});
