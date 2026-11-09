// Dependency-free checks for shell state and the renderer's entry/save contracts.
// Browser scenarios (actual forms, dialogs and layout) are checked manually in the app.
const { test } = require('node:test');
const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { join } = require('node:path');
const vm = require('node:vm');
const ui = join(__dirname, '..');
const shell = readFileSync(join(ui, 'app.js'), 'utf8');
const statusCode = shell.slice(shell.indexOf('function updateTitleStatus()'), shell.indexOf('async function loadSyncStatus()'));
function status(value) {
  const nodes = new Map();
  const $ = (id) => {
    if (!nodes.has(id)) nodes.set(id, { dataset: {}, setAttribute(name, value) { this[name] = value; } });
    return nodes.get(id);
  };
  vm.runInNewContext(`${statusCode}; updateTitleStatus();`, { syncStatus: value, $, rails: null, ago: () => 'just now' });
  return { desktop: $('space-status'), phone: $('sync-mobile') };
}
const paired = { role: 'host', devices: [{ this_device: true }, { this_device: false }], last: null };
for (const [name, input, expected, tone] of [
  ['unpaired', { role: null, devices: [] }, 'Saved here · this device only', 'off'],
  ['host without a peer', { role: 'host', devices: [{ this_device: true }] }, 'Saved here · this device only', 'off'],
  ['before first sync', paired, 'Saved here · waiting for first sync', 'off'],
  ['transport failure', { ...paired, last: { error: 'offline' } }, 'Saved here · sync needs attention', 'error'],
  ['rejected batch', { ...paired, last: { ok: false, report: { rejected: ['bad'] } } }, 'Saved here · sync needs attention', 'error'],
  ['partial route failure', { ...paired, last: { ok: true, stores: { host: 'asleep' } } }, 'Saved here · some sync routes unavailable', 'off'],
  ['successful historical pass', { ...paired, last: { ok: true, at_ms: 1, report: { rejected: [] }, stores: {} } }, 'Last sync just now', 'ok'],
]) {
  test(`sync status: ${name}`, () => {
    const result = status(input);
    assert.equal(result.desktop.textContent, expected);
    assert.equal(result.desktop.dataset.state, tone);
    assert.equal(result.phone['aria-label'], `${expected}. Open Devices and sync`);
  });
}
const renderer = readFileSync(join(ui, 'renderer.js'), 'utf8');
function mount(template, item, forms = []) {
  const window = {};
  vm.runInNewContext(renderer, { window, structuredClone, CustomEvent: class { constructor(type, options) { this.type = type; this.detail = options.detail; } } });
  return window.TerrariumRenderer.mount({ replaceChildren() {}, querySelectorAll: () => forms }, template, { today: () => '2026-11-03' }, { item });
}
test('search entry opens only the state explicitly declared by the template', () => {
  const view = mount({ state: { open: '', q: '' }, entry: { bind: 'state.open' } }, 'item-42');
  assert.equal(view.state.open, 'item-42');
  assert.equal(view.state.q, '');
});
test('search IDs cannot create arbitrary state or prototype properties', () => {
  for (const bind of ['state.missing', 'state.__proto__', 'row.open', 'state.open.nested']) {
    const view = mount({ state: { open: '' }, entry: { bind } }, 'item-42');
    assert.equal(view.state.open, '');
    assert.equal(Object.keys(view.state).length, 1);
  }
  assert.equal(mount({ state: { open: '' } }, 'item-42').state.open, '');
});
test('navigation flush waits for every save and propagates a failure', async () => {
  const forms = [true, false].map((ok) => ({ dispatchEvent(event) { event.detail.waits.push(Promise.resolve(ok)); } }));
  assert.equal(await mount({ state: {} }, null, forms).flush(), false);
  assert.equal(await mount({ state: {} }, null, forms.slice(0, 1)).flush(), true);
  assert.equal(await mount({ state: {} }, null).flush(), true);
});

test('navigation flush also saves text typed during an in-flight save', async () => {
  let calls = 0;
  const form = {
    dataset: { ppUnsaved: 'true' },
    dispatchEvent(event) {
      calls++;
      event.detail.waits.push(Promise.resolve().then(() => {
        this.dataset.ppUnsaved = calls === 1 ? 'true' : 'false';
        return true;
      }));
    },
  };
  assert.equal(await mount({ state: {} }, null, [form]).flush(), true);
  assert.equal(calls, 2);
});
