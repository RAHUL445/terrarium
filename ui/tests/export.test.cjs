const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

const source = fs.readFileSync(path.join(__dirname, '../app.js'), 'utf8');
const start = source.indexOf('// ---- Settings → Spaces → Export ----');
const end = source.indexOf('// ---- Spaces ----', start);
assert.ok(start >= 0 && end > start);
const code = source.slice(start, end);

function harness({ isPhone = false, response = { path: '/x.zip', bytes: 2048 } } = {}) {
  const elements = new Map(), calls = [], toasts = [];
  const get = id => {
    if (!elements.has(id)) elements.set(id, {
      disabled: false, checked: false, hidden: false, children: [], listeners: {},
      replaceChildren(...children) { this.children = children; },
      addEventListener(type, listener) { this.listeners[type] = listener; },
    });
    return elements.get(id);
  };
  vm.runInNewContext(code, {
    $: get,
    el: (tag, attrs, ...children) => ({ tag, attrs, children }),
    formatSize: b => b + ' B',
    toast: (text, kind) => toasts.push({ text, kind }),
    isPhone,
    currentSpace: 'work',
    async call(command, args) {
      calls.push({ command, args: JSON.parse(JSON.stringify(args)) });
      if (response instanceof Error) throw response;
      return typeof response === 'function' ? response(command) : response;
    },
  });
  return { get, calls, toasts, click: () => get('export-run').listeners.click() };
}

test('export calls the app with the space and trash choice', async () => {
  const h = harness();
  h.get('export-trash').checked = true;
  const pending = h.click();
  assert.equal(h.get('export-run').disabled, true);
  assert.deepEqual(h.get('export-result').children, []);
  await pending;
  assert.deepEqual(h.calls[0], {
    command: 'export_space', args: { space: 'work', includeTrash: true },
  });
  assert.deepEqual(h.toasts, [{ text: 'Export saved (2048 B)', kind: undefined }]);
  const [saved, , button] = h.get('export-result').children;
  assert.equal(saved.children[0], 'Saved to /x.zip');
  assert.equal(button.children[0], 'Show in folder');
  await button.attrs.onclick();
  assert.deepEqual(h.calls[1], { command: 'reveal_export', args: { path: '/x.zip' } });
  assert.equal(h.get('export-run').disabled, false);
});

test('errors are shown', async () => {
  const h = harness({ response: new Error('Export failed') });
  h.get('export-result').children = ['previous export'];
  await h.click();
  assert.deepEqual(h.toasts, [{ text: 'Error: Export failed', kind: 'error' }]);
  assert.equal(h.get('export-run').disabled, false);
  assert.deepEqual(h.get('export-result').children, []);
});

test('reveal errors are shown', async () => {
  const h = harness({ response: command => {
    if (command === 'reveal_export') throw new Error('Only exports can be shown.');
    return { path: '/x.zip', bytes: 2048 };
  } });
  await h.click();
  await h.get('export-result').children[2].attrs.onclick();
  assert.deepEqual(h.toasts[1], { text: 'Error: Only exports can be shown.', kind: 'error' });
});

test('hidden on phones', () => {
  assert.equal(harness({ isPhone: true }).get('export-card').hidden, true);
  assert.equal(harness().get('export-card').hidden, false);
});
