const { test } = require('node:test');
const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { join } = require('node:path');
const vm = require('node:vm');

// Minimal DOM for exercising real renderer submit handlers with delayed storage.
function capture(action, snapshot) {
  class Node {
    constructor(tag) {
      this.tag = tag; this.children = []; this.dataset = {}; this.listeners = {};
      this.value = this.defaultValue = ''; this.style = {}; this.attrs = {};
      this.classList = { add() {}, toggle() {}, remove() {} };
    }
    append(...children) { for (const child of children) { child.parent = this; this.children.push(child); } }
    replaceChildren(...children) { this.children = []; this.append(...children); }
    setAttribute(key, value) { this.attrs[key] = value; }
    removeAttribute(key) { delete this.attrs[key]; }
    addEventListener(name, callback) { (this.listeners[name] ||= []).push(callback); }
    closest() { return null; }
    dispatchEvent(event) { for (const listener of this.listeners[event.type] || []) listener(event); }
    querySelectorAll(selector) {
      const nodes = this.children.flatMap(child => [child, ...child.querySelectorAll(selector)]);
      if (selector === 'form[data-pp-draft]') return nodes.filter(n => n.tag === 'form' && n.attrs['data-pp-draft']);
      if (selector === '[data-pp-name]') return nodes.filter(n => n.dataset.ppName);
      return [];
    }
    querySelector(selector) { return this.querySelectorAll(selector)[0] || null; }
    async submit() { for (const callback of this.listeners.submit) await callback({ preventDefault() {} }); }
  }
  const window = {};
  vm.runInNewContext(readFileSync(join(__dirname, '../renderer.js'), 'utf8'), {
    window, structuredClone, Event: class { constructor(type) { this.type = type; } }, document: { createElement: tag => new Node(tag) },
  });
  const root = new Node('root');
  const view = window.TerrariumRenderer.mount(root, { body: {
    type: 'form', inline: true, submit: { op: 'create', args: { body: '{{form.body}}' }, reset: true },
    children: [{ type: 'text-field', name: 'body', label: 'New entry' }],
  } }, { today: () => '2026-11-04', action, changed() {}, toast() {} }, { snapshot });
  const form = root.children[0];
  return { view, form, input: form.querySelector('[data-pp-name]'), button: form.children[1], status: form.children[2] };
}

test('pending capture retains text and blocks duplicate submission until success', async () => {
  let finish, calls = 0;
  const ui = capture(() => { calls++; return new Promise(resolve => { finish = resolve; }); });
  ui.input.value = 'Keep this thought';
  const pending = ui.form.submit();
  assert.equal(ui.input.value, 'Keep this thought');
  assert.equal(ui.button.disabled, true);
  assert.equal(ui.form.attrs['aria-busy'], 'true');
  const detail = { waits: [] };
  ui.form.dispatchEvent({ type: 'pp-flush', detail });
  assert.equal(detail.waits.length, 1, 'navigation must wait for the capture');
  await ui.form.submit();
  assert.equal(calls, 1);
  finish({}); await pending;
  assert.equal(ui.input.value, '');
  assert.equal(ui.button.disabled, false);
  assert.equal(ui.status.textContent, 'Saved on this device');
  assert.equal(ui.form.attrs['aria-busy'], undefined);
  ui.input.value = 'Next entry';
  for (const listener of ui.form.listeners.input) listener();
  assert.equal(ui.status.textContent, '');
});

test('failed capture preserves entry and supports retry', async () => {
  let fail = true;
  const ui = capture(async () => { if (fail) throw Error('offline'); return {}; });
  ui.input.value = 'Recoverable draft';
  await ui.form.submit();
  assert.equal(ui.input.value, 'Recoverable draft');
  assert.match(ui.status.textContent, /still here; try again/);
  assert.equal(ui.button.disabled, false);
  fail = false; await ui.form.submit();
  assert.equal(ui.input.value, '');
});

test('edits during a successful save remain available as the next draft', async () => {
  let finish;
  const ui = capture(() => new Promise(resolve => { finish = resolve; }));
  ui.input.value = 'First entry';
  const pending = ui.form.submit();
  ui.input.value = 'New draft';
  finish({}); await pending;
  assert.equal(ui.input.value, 'New draft');
  assert.match(ui.status.textContent, /new draft is ready/);
});


test('capture drafts restore through the actual renderer snapshot contract', async () => {
  const first = capture(async () => ({}));
  await first.view.ready;
  first.input.value = 'Unsaved capture';
  const second = capture(async () => ({}), first.view.snapshot());
  await second.view.ready;
  assert.equal(second.input.value, 'Unsaved capture');
  assert.equal(second.button.disabled, undefined);
});

test('restoring a draft never overwrites typing during initial loading', async () => {
  const first = capture(async () => ({}));
  await first.view.ready;
  first.input.value = 'Old draft';
  const second = capture(async () => ({}), first.view.snapshot());
  second.input.value = 'Fresh typing';
  second.form.dispatchEvent({ type: 'input' });
  await second.view.ready;
  assert.equal(second.input.value, 'Fresh typing');
});
