const { test } = require('node:test');
const assert = require('node:assert/strict');
const Session = require('../session.js');
test('navigation and in-session drafts remain isolated by space and page', () => {
  const data = new Map();
  const storage = { getItem: key => data.get(key), setItem: (key, value) => data.set(key, value) };
  const session = new Session(storage);
  session.write('personal', { view: 'plugin', plugin: 'notes', page: 'main', item: 'note-1', scroll: 250 });
  session.write('work', { view: 'home', scroll: 0 });
  session.save('personal/page/notes/main', { snapshot: () => ({ state: { open: 'note-1' }, forms: [{ body: 'Private draft' }] }) });
  assert.equal(session.read('personal').item, 'note-1');
  assert.equal(session.read('work').view, 'home');
  assert.equal(session.get('work/page/notes/main'), undefined);
  assert.equal([...data.values()].some(value => value.includes('Private draft')), false);
  assert.equal(new Session(storage).get('personal/page/notes/main'), undefined);
});
test('corrupt, unavailable and invalid navigation storage is safe', () => {
  for (const value of ['{', '{}', 'null', '{"view":"unknown"}']) {
    assert.equal(new Session({ getItem: () => value }).read('s'), null);
  }
  const unavailable = new Session({ getItem() { throw Error(); }, setItem() { throw Error(); } });
  assert.equal(unavailable.read('s'), null);
  assert.doesNotThrow(() => unavailable.write('s', {}));
  const session = new Session({ getItem: () => '{"view":"plugin","scroll":-2,"plugin":{},"page":12}' });
  assert.equal(session.read('s').scroll, 0);
  assert.equal(session.read('s').plugin, null);
});
