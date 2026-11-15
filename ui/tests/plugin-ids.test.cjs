const test = require('node:test');
const assert = require('node:assert/strict');
const ids = require('../plugin-ids.js');

const plugins = [
  { id: 'tasks', former_ids: ['todo'], name: 'Task manager' },
  { id: 'notes', name: 'Notes' },
];

function memory(entries = {}) {
  const map = new Map(Object.entries(entries));
  return {
    map,
    getItem: key => (map.has(key) ? map.get(key) : null),
    setItem: (key, value) => { map.set(key, String(value)); },
  };
}

test('former ids name the renamed plugin; refs keep the storage id', () => {
  assert.equal(ids.canonical(plugins, 'todo'), 'tasks');
  assert.equal(ids.canonical(plugins, 'tasks'), 'tasks');
  assert.equal(ids.canonical(plugins, 'notes'), 'notes');
  assert.equal(ids.find(plugins, 'todo').id, 'tasks');
  assert.equal(ids.find(plugins, 'nope'), null);
  assert.equal(ids.refId(plugins[0]), 'todo');
  assert.equal(ids.refId(plugins[1]), 'notes');
  assert.deepEqual(ids.renames(plugins), [['todo', 'tasks']]);
});

test('device-local keys move to the new id once and keep the old entries', () => {
  const storage = memory({
    'pp-pins:personal': JSON.stringify({ version: 1, pins: [{ kind: 'page', id: 'p1' }, { kind: 'app', id: 'todo' }] }),
    'pp-location:personal': JSON.stringify({ view: 'plugin', plugin: 'todo', page: 'main', item: null }),
    'pp-location:work': JSON.stringify({ view: 'plugin', plugin: 'notes', page: 'main' }),
    'pp-layout:personal': JSON.stringify({ version: 1, nav: ['todo', 'notes'], widgets: ['todo/today'], actions: ['todo/create/title'], home: null }),
    'pp-today:personal': JSON.stringify({ version: 1, plugins: ['todo'] }),
    'quick-capture-action': 'todo/create/title',
    'share-target': 'notes',
    'terrarium.workspace.personal/page/todo/main.views': 'collapsed',
  });
  const keys = [...storage.map.keys()];
  assert.ok(ids.migrate(storage, keys, plugins) > 0);
  const get = key => JSON.parse(storage.getItem(key));
  assert.deepEqual(get('pp-pins:personal').pins, [{ kind: 'page', id: 'p1' }, { kind: 'app', id: 'tasks' }]);
  assert.equal(get('pp-location:personal').plugin, 'tasks');
  assert.equal(get('pp-location:work').plugin, 'notes');
  const layout = get('pp-layout:personal');
  assert.deepEqual(layout.nav, ['todo', 'tasks', 'notes']);
  assert.deepEqual(layout.widgets, ['todo/today', 'tasks/today']);
  assert.deepEqual(layout.actions, ['todo/create/title', 'tasks/create/title']);
  assert.equal(layout.home, null);
  assert.deepEqual(get('pp-today:personal').plugins, ['todo', 'tasks']);
  assert.equal(storage.getItem('quick-capture-action'), 'tasks/create/title');
  assert.equal(storage.getItem('share-target'), 'notes');
  assert.equal(storage.getItem('terrarium.workspace.personal/page/tasks/main.views'), 'collapsed');
  assert.equal(storage.getItem('terrarium.workspace.personal/page/todo/main.views'), 'collapsed');
  assert.equal(storage.getItem('pp-renamed:todo:tasks'), '1');

  // Once only: a later change is not redone.
  storage.setItem('pp-location:personal', JSON.stringify({ view: 'plugin', plugin: 'todo', page: 'main' }));
  assert.equal(ids.migrate(storage, [...storage.map.keys()], plugins), 0);
  assert.equal(get('pp-location:personal').plugin, 'todo');
});

test('migration tolerates unavailable or broken storage', () => {
  const broken = { getItem() { throw Error('blocked'); }, setItem() { throw Error('blocked'); } };
  assert.equal(ids.migrate(broken, ['pp-pins:personal'], plugins), 0);
  const garbage = memory({ 'pp-pins:personal': '{not json', 'pp-layout:personal': '7' });
  assert.equal(ids.migrate(garbage, [...garbage.map.keys()], plugins), 1); // only the marker
  assert.equal(ids.migrate(memory(), [], [{ id: 'notes' }]), 0);
});
