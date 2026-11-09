const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const root = path.resolve(__dirname, '../..');
const app = path.join(root, 'apps/tauri/src-tauri');

test('every registered shell command has an explicit local main-window ACL grant', () => {
  const source = fs.readFileSync(path.join(app, 'src/lib.rs'), 'utf8');
  const handler = source.match(/tauri::generate_handler!\[([\s\S]*?)\]/)[1];
  const commands = handler.split(',').map(s => s.trim().split('::').at(-1)).filter(Boolean).sort();
  const build = fs.readFileSync(path.join(app, 'build.rs'), 'utf8');
  assert.match(build, /AppManifest::new\(\)\.commands/);
  const manifest = build.match(/\.commands\(&\[([\s\S]*?)\]/)[1];
  assert.deepEqual([...manifest.matchAll(/"([a-z_]+)"/g)].map(m => m[1]).sort(), commands);
  const cap = JSON.parse(fs.readFileSync(path.join(app, 'capabilities/default.json'), 'utf8'));
  assert.deepEqual(cap.windows, ['main']);
  assert.equal(cap.remote, undefined, 'remote pages must not receive shell authority');
  assert.deepEqual(cap.permissions.filter(p => p.startsWith('allow-')).sort(), commands.map(c => `allow-${c.replaceAll('_', '-')}`).sort());
});

test('a synced pending version can be reviewed without selecting another file', async () => {
  const vm = require('node:vm');
  const source = fs.readFileSync(path.join(root, 'ui/app.js'), 'utf8');
  const render = source.slice(source.indexOf('function renderInstalled()'), source.indexOf('async function loadSigners()'));
  const elements = new Map();
  const get = id => {
    if (!elements.has(id)) elements.set(id, { replaceChildren(...children) { this.children = children; } });
    return elements.get(id);
  };
  let request, reviewed;
  const context = { pluginList: { plugins: [], pending: [{ id: 'notes', version: '2.0.0' }], removed: [] },
    $: get, el: (tag, attrs, ...children) => ({ tag, attrs, children }),
    pluginsError() {}, showReview(value) { reviewed = value; },
    async call(command, args) { request = JSON.parse(JSON.stringify({ command, args })); return { token: 'review' }; }
  };
  vm.runInNewContext(`${render}; renderInstalled();`, context);
  assert.equal(get('pending-plugins-card').hidden, false);
  const button = get('pending-plugins-list').children[0].children.find(c => c.tag === 'button');
  await button.attrs.onclick();
  assert.deepEqual(request, { command: 'stage_install', args: { source: { kind: 'synced', id: 'notes' } } });
  assert.equal(reviewed.token, 'review');
});
