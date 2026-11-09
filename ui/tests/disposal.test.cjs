const { test } = require('node:test');
const assert = require('node:assert/strict');
const { rendererHarness, listTemplate } = require('./helpers/renderer-dom.cjs');
const turn = () => new Promise(resolve => setImmediate(resolve));
function clock() {
  const timers = new Map(); let id = 0;
  const add = (fn, repeat) => { timers.set(++id, { fn, repeat }); return id; };
  return { timers, setTimeout: fn => add(fn, false), setInterval: fn => add(fn, true),
    clearTimeout: id => timers.delete(id), clearInterval: id => timers.delete(id),
    tick() { for (const [key, value] of [...timers]) { if (!value.repeat) timers.delete(key); value.fn(); } } };
}
const bridge = { today: () => '2026-11-04', query: async () => [], action: async () => ({}), changed() {}, toast() {} };

test('disposal cancels debounce callbacks and is idempotent', async () => {
  const time = clock(); const ui = rendererHarness(time);
  const view = ui.renderer.mount(ui.root, { state: { q: '' }, body: { type: 'search-field', bind: 'state.q' } }, bridge);
  await view.ready;
  const input = ui.root.children[0].children[1]; input.value = 'Late search'; input.dispatchEvent({ type: 'input' });
  assert.equal(time.timers.size, 1);
  view.dispose(); view.dispose(); time.tick();
  assert.equal(time.timers.size, 0);
  assert.equal(view.state.q, '');
  assert.equal(view.watchers.length + view.lists.length + view.cleanups.length, 0);
});

test('removing a conditional region cancels its timer while the view stays active', async () => {
  const time = clock(); const ui = rendererHarness(time);
  const view = ui.renderer.mount(ui.root, { state: { show: true, q: '' }, body: {
    type: 'search-field', bind: 'state.q', if: '{{state.show}}',
  } }, bridge);
  await view.ready;
  const input = ui.root.children[0].children[0].children[1]; input.value = 'Late'; input.dispatchEvent({ type: 'input' });
  view.setState('state.show', false); time.tick();
  assert.equal(time.timers.size, 0); assert.equal(view.state.q, ''); view.dispose();
});

test('replacing list rows releases old row autosave timers', async () => {
  const time = clock(); const ui = rendererHarness(time); let revision = 0;
  const template = listTemplate(); template.body.item.autosave = 800;
  const view = ui.renderer.mount(ui.root, template, { ...bridge, query: async () => [{ title: `Revision ${revision}` }] });
  await view.ready;
  const form = ui.root.querySelector('form'); form.dispatchEvent({ type: 'input' });
  assert.equal(time.timers.size, 1);
  revision++; await view.refresh();
  assert.equal(time.timers.size, 0); view.dispose();
});

test('disposed query responses cannot rebuild detached rows', async () => {
  const ui = rendererHarness(); let finish;
  const view = ui.renderer.mount(ui.root, listTemplate(), { ...bridge, query: () => new Promise(resolve => { finish = resolve; }) });
  const allocations = ui.allocations(); view.dispose(); finish([{ title: 'Late result' }]); await view.ready;
  assert.equal(ui.allocations(), allocations);
});

test('disposing during a render batch cancels its yield and settles readiness', async () => {
  const ui = rendererHarness();
  const view = ui.renderer.mount(ui.root, listTemplate(), { ...bridge, query: async () => Array.from({ length: 500 }, (_, title) => ({ title })) });
  await new Promise(resolve => setTimeout(resolve, 0));
  const allocations = ui.allocations(); view.dispose(); await view.ready;
  assert.equal(ui.allocations(), allocations); assert.equal(view.timers.size, 0);
});

test('recording blocks unmount; forced resource disposal stops tracks and intervals without saving to another space', async () => {
  const time = clock(); let stopped = 0, actions = 0;
  class Recorder {
    static isTypeSupported() { return false; }
    constructor() { this.state = 'inactive'; this.events = {}; }
    addEventListener(type, run) { this.events[type] = run; }
    start() { this.state = 'recording'; }
    stop() { this.state = 'inactive'; this.events.stop?.(); }
  }
  const stream = { getTracks: () => [{ stop() { stopped++; } }] };
  const ui = rendererHarness({ ...time, navigator: { mediaDevices: { getUserMedia: async () => stream } }, MediaRecorder: Recorder });
  const view = ui.renderer.mount(ui.root, { body: { type: 'audio-recorder', action: { op: 'capture_audio' } } }, { ...bridge, action: async () => { actions++; } });
  await view.ready;
  await ui.root.children[0].children[0].fire('click');
  assert.equal(await view.flush({ unmount: true }), false);
  assert.equal(await view.flush(), true, 'a retained widget may record while another page opens');
  assert.equal(time.timers.size, 1);
  view.dispose(); await turn();
  assert.equal(time.timers.size, 0); assert.ok(stopped > 0); assert.equal(actions, 0);
});

test('a microphone permission response arriving after disposal releases the stream', async () => {
  let finish, stopped = 0;
  const ui = rendererHarness({ navigator: { mediaDevices: { getUserMedia: () => new Promise(resolve => { finish = resolve; }) } }, MediaRecorder: class {} });
  const view = ui.renderer.mount(ui.root, { body: { type: 'audio-recorder' } }, bridge); await view.ready;
  const click = ui.root.children[0].children[0].fire('click');
  view.dispose(); finish({ getTracks: () => [{ stop() { stopped++; } }] }); await click;
  assert.equal(stopped, 1); assert.equal(view.timers.size, 0);
});
