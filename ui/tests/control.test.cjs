const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

const source = fs.readFileSync(path.join(__dirname, '../app.js'), 'utf8');
const start = source.indexOf('// ---- Settings → Developer → Command-line control ----');
const end = source.indexOf('function showSettings(section) {');
const code = source.slice(start, end);

function harness(responses = {}, confirmAnswer = true) {
  const confirms = [], toasts = [], spaces = [], opened = [];
  const elements = new Map();
  const listeners = {};
  const get = id => {
    if (!elements.has(id)) {
      elements.set(id, {
        id, hidden: false, checked: false, value: '', textContent: '', children: [],
        replaceChildren(...children) { this.children = children; },
        addEventListener(type, fn) { listeners[`${id}:${type}`] = fn; },
      });
    }
    return elements.get(id);
  };
  const calls = [];
  const context = {
    $: get,
    el: (tag, attrs, ...children) => ({ tag, attrs, children: children.filter(c => c !== null) }),
    when: ms => `t${ms}`,
    statusChip: status => ({ chip: status }),
    getSelection: () => ({ selectAllChildren() {} }),
    navigator: { clipboard: { writeText: async () => {} } },
    window: { terrariumConfirm: async (text) => { confirms.push(text); return confirmAnswer; } },
    toast(text) { toasts.push(text); },
    currentSpace: 'personal',
    pluginList: { plugins: [{ id: 'todo', pages: [{ id: 'main' }] }] },
    async setSpace(id) { spaces.push(id); context.currentSpace = id; },
    renderSpaceChrome() {}, renderHome() {}, loadBadges() {},
    async openPage(plugin, page, options) { opened.push({ plugin, page, item: options.item }); },
    async call(command, args) {
      calls.push({ command, args });
      const r = responses[command];
      if (r instanceof Error) throw r;
      return typeof r === 'function' ? r(args) : r;
    },
  };
  vm.runInNewContext(code, context);
  return { context, get, listeners, calls, confirms, toasts, spaces, opened };
}

const state = {
  available: true, enabled: true, listening: true, socket: '/data/control.sock', error: null,
  clients: [{ id: 'c1', name: 'Claude', scope: 'read', created_ms: 1, last_used_ms: null }],
  audit: [{ at_ms: 5, client: 'Claude', what: 'todo.tasks', kind: 'query', ok: true },
    { at_ms: 6, client: 'Claude', what: 'todo.create', kind: 'action', ok: false, code: 'forbidden' }],
};

test('the control card is hidden where it is not available', () => {
  const h = harness();
  h.context.renderControl({ ...state, available: false });
  assert.equal(h.get('control-card').hidden, true);
});

test('clients and recent activity are listed', () => {
  const h = harness();
  h.context.renderControl(state);
  assert.equal(h.get('control-card').hidden, false);
  assert.equal(h.get('control-enabled').checked, true);
  assert.match(h.get('control-socket').textContent, /Listening at \/data\/control.sock/);
  const [client] = h.get('control-clients').children;
  assert.equal(client.children[0].children[0].children[0], 'Claude');
  assert.match(client.children[0].children[1].children[0], /Read only · added t1 · never used/);
  const audit = h.get('control-audit').children;
  assert.equal(audit.length, 2);
  assert.equal(audit[1].children[1].chip, 'failed');
  assert.equal(audit[1].children[2].children[0], 'forbidden');
});

test('a new client shows its token once and Done clears it', async () => {
  const h = harness({ control_add_client: { token: 'trm_secret', state } });
  h.get('control-name').value = '  Agent ';
  h.get('control-scope').value = 'read';
  await h.listeners['control-add:submit']({ preventDefault() {} });
  assert.deepEqual(JSON.parse(JSON.stringify(h.calls[0])), { command: 'control_add_client', args: { name: 'Agent', scope: 'read' } });
  assert.equal(h.confirms.length, 0);
  assert.equal(h.get('control-token').hidden, false);
  assert.equal(h.get('control-token-value').textContent, 'trm_secret');
  assert.equal(h.get('control-name').value, '');
  h.listeners['control-token-done:click']();
  assert.equal(h.get('control-token-value').textContent, '');
  assert.equal(h.get('control-token').hidden, true);
});

test('errors are shown, not thrown', async () => {
  const h = harness({ control_set_enabled: new Error('socket path too long'), control_state: state });
  await h.listeners['control-enabled:change']({ target: { checked: true } });
  assert.equal(h.get('control-error').hidden, false);
  assert.match(h.get('control-error').textContent, /socket path too long/);
  // The checkbox follows the saved state.
  assert.equal(h.get('control-enabled').checked, true);
});

test('a client that can change data needs a confirmation first', async () => {
  const h = harness({ control_add_client: { token: 'trm_x', state } }, false);
  h.get('control-name').value = 'Agent';
  h.get('control-scope').value = 'change';
  await h.listeners['control-add:submit']({ preventDefault() {} });
  assert.equal(h.confirms.length, 1);
  assert.match(h.confirms[0], /add, change and remove/);
  assert.equal(h.calls.length, 0);
});

test('ui.open and ui.notify reach the window', async () => {
  const h = harness();
  await h.context.controlUi({ action: 'notify', text: 'Report ready' });
  assert.deepEqual(h.toasts, ['Report ready']);
  await h.context.controlUi({ action: 'open', space: 'work', plugin: 'todo', page: 'main', item: 'abc' });
  assert.deepEqual(h.spaces, ['work']);
  assert.deepEqual(JSON.parse(JSON.stringify(h.opened)), [{ plugin: 'todo', page: 'main', item: 'abc' }]);
  // Unknown plugins and pages are ignored.
  await h.context.controlUi({ action: 'open', plugin: 'nope', page: 'main' });
  await h.context.controlUi({ action: 'open', plugin: 'todo', page: 'nope' });
  assert.equal(h.opened.length, 1);
});

test('activity shows dry runs and how many rows changed', () => {
  const h = harness();
  h.context.renderControl({ ...state, audit: [
    { at_ms: 1, client: 'A', what: 'todo.create', kind: 'action', ok: true, dry_run: true, change_count: 1 },
    { at_ms: 2, client: 'A', what: 'todo.clear_done', kind: 'action', ok: true, change_count: 3 },
  ] });
  const [dry, real] = h.get('control-audit').children;
  assert.match(dry.children[0].children[1].children[0], /dry run, nothing kept · 1 change$/);
  assert.match(real.children[0].children[1].children[0], /3 changes$/);
});
