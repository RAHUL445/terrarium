const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const path = require('node:path');
const source = fs.readFileSync(path.join(__dirname, '../profile-storage.js'), 'utf8');

test('profile preferences isolate theme, view and layout from release and other profiles', () => {
  const data = new Map([['theme', 'dark'], ['space', 'personal']]);
  const localStorage = {
    getItem: key => data.get(key) ?? null,
    setItem: (key, value) => data.set(key, String(value)),
    removeItem: key => data.delete(key),
  };
  const load = id => {
    const window = { terrariumProfileId: id };
    vm.runInNewContext(source, { window, localStorage });
    return window.terrariumStorage || localStorage;
  };
  const a = load('profile-a'), b = load('profile-b'), release = load();
  for (const key of ['theme', 'space', 'pp-location:personal', 'pp-layout:personal']) {
    assert.equal(a.getItem(key), null);
    a.setItem(key, 'test-value');
    assert.equal(load('profile-a').getItem(key), 'test-value');
    assert.equal(b.getItem(key), null);
    assert.notEqual(release.getItem(key), 'test-value');
    b.setItem(key, 'other-value');
    a.removeItem(key);
    assert.equal(b.getItem(key), 'other-value');
  }
  assert.equal(release.getItem('theme'), 'dark');
  assert.equal(release.getItem('space'), 'personal');
});

const app = fs.readFileSync(path.join(__dirname, '../app.js'), 'utf8');
const backupFunctions = app.slice(app.indexOf('async function loadBackups()'), app.indexOf('async function restore(source)'));
function backups(call) {
  const nodes = new Map();
  const $ = id => {
    if (!nodes.has(id)) nodes.set(id, { textContent: '', hidden: true, replaceChildren(...children) { this.children = children; } });
    return nodes.get(id);
  };
  const context = vm.createContext({ $, call, ago: () => 'today', el: (...args) => args });
  vm.runInContext(backupFunctions, context);
  return { $, load: () => vm.runInContext('loadBackups()', context) };
}

test('backup listing failures stay inside Backups and clear stale contents', async () => {
  const page = backups(async () => { throw Error("Terrarium can't list backups. Check the backup folder's permissions."); });
  page.$('sync-error').textContent = 'unrelated sync error';
  page.$('backup-dir').textContent = 'old directory';
  await page.load();
  assert.match(page.$('backup-error').textContent, /can't list backups/);
  assert.equal(page.$('backup-error').hidden, false);
  assert.equal(page.$('sync-error').textContent, 'unrelated sync error');
  assert.equal(page.$('backup-dir').textContent, '');
  assert.equal(page.$('backup-list').children.length, 0);
});

test('empty backup listing still shows the selected folder and fallback notice', async () => {
  const page = backups(async () => ({ entries: [], directory: '/synthetic/profile/backups', notice: 'Using app data instead.' }));
  await page.load();
  assert.equal(page.$('backup-error').hidden, true);
  assert.match(page.$('backup-dir').textContent, /Using app data instead.*\/synthetic\/profile\/backups/);
  assert.match(page.$('backup-health').textContent, /No backup yet/);
  assert.equal(page.$('backup-list').children.length, 0);
});
