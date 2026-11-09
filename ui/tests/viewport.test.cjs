const { test } = require('node:test');
const assert = require('node:assert/strict');
const followViewport = require('../viewport.js');

test('keyboard resize and viewport pan update geometry and clean up listeners', () => {
  const values = new Map(), listeners = new Map();
  const viewport = { height: 800, offsetTop: 0,
    addEventListener: (name, fn) => listeners.set(name, fn),
    removeEventListener: (name, fn) => { assert.equal(listeners.get(name), fn); listeners.delete(name); },
  };
  const dispose = followViewport({ visualViewport: viewport, document: { documentElement: { style: {
    setProperty: (name, value) => values.set(name, value), removeProperty: name => values.delete(name),
  } } } });
  assert.equal(values.get('--visible-height'), '800px');
  viewport.height = 380; listeners.get('resize')();
  assert.equal(values.get('--visible-height'), '380px');
  viewport.offsetTop = 42; listeners.get('scroll')();
  assert.equal(values.get('--visible-top'), '42px');
  viewport.height = 800; viewport.offsetTop = 0; listeners.get('resize')();
  assert.equal(values.get('--visible-height'), '800px');
  dispose(); assert.equal(listeners.size, 0); assert.equal(values.size, 0);
});
test('browsers without visualViewport retain CSS fallback', () => {
  assert.doesNotThrow(() => followViewport({})());
});
