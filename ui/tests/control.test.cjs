const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

const source = fs.readFileSync(path.join(__dirname, '../app.js'), 'utf8');
const start = source.indexOf('// ---- Settings → Developer → Command-line control ----');
const end = source.indexOf('function showSettings(section) {');
const code = source.slice(start, end);

function harness(responses = {}) {
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
    toast() {},
    getSelection: () => ({ selectAllChildren() {} }),
    navigator: { clipboard: { writeText: async () => {} } },
    window: { terrariumConfirm: async () => true },
    async call(command, args) {
      calls.push({ command, args });
      const r = responses[command];
      if (r instanceof Error) throw r;
      return typeof r === 'function' ? r(args) : r;
    },
  };
  vm.runInNewContext(code, context);
  return { context, get, listeners, calls };
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
  await h.listeners['control-add:submit']({ preventDefault() {} });
  assert.deepEqual(JSON.parse(JSON.stringify(h.calls[0])), { command: 'control_add_client', args: { name: 'Agent' } });
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
