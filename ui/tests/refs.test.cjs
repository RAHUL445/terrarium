const { test } = require('node:test');
const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const vm = require('node:vm');
const { rendererHarness } = require('./helpers/renderer-dom.cjs');
const tick = () => new Promise(resolve => setImmediate(resolve));
const hit = { ref: 'todo/tasks/t1', state: 'ok', kind: 'Task', icon: 'check', title: 'Buy lamp', done: true, open: { plugin: 'todo', page: 'main', item: 't1' } };
const bridge = extra => ({ today: () => '2026-11-11', ...extra });
const walk = node => typeof node === 'string' ? [] : [node, ...node.children.flatMap(walk)];
const text = node => typeof node === 'string' ? node : (node.textContent || '') + node.children.map(text).join('');

test('a ref chip shows the current title and opens the item', async () => {
  const ui = rendererHarness(); const opened = []; const calls = [];
  const view = ui.renderer.mount(ui.root, { state: { id: 't1' }, body: { type: 'ref', ref: 'todo/tasks/{{state.id}}' } }, bridge({
    command: async (...args) => { calls.push(args); return [hit]; }, openRef: r => opened.push(r),
  }));
  await view.ready; await tick();
  const chip = ui.root.children[0];
  assert.equal(calls.length, 1); assert.equal(calls[0][0], 'query'); assert.equal(calls[0][1], 'ref.resolve');
  assert.equal(chip.children[1].textContent, 'Buy lamp');
  assert.match(chip.children[1].className, /pp-done/);
  assert.equal(chip.children[0].children[0].attrs.d, 'm5 12 5 5 9-10');
  assert.equal(chip.title, 'Task: Buy lamp'); assert.equal(chip.attrs['aria-label'], 'Open Task: Buy lamp');
  await chip.fire('click'); assert.deepEqual(opened, [hit]);
  view.bridge.command = async () => [{ ...hit, title: 'New title', icon: 'bookmark', done: false }];
  await view.refresh(); await tick();
  assert.equal(chip.children[1].textContent, 'New title');
  assert.equal(chip.children[1].className, 'pp-ref-title');
  assert.notEqual(chip.children[0].children[0].attrs.d, undefined);
  view.dispose();
});

test('states missing, trash and unknown are drawn without opening', async () => {
  const ui = rendererHarness(); let opens = 0;
  const states = ['missing', 'trash', 'unknown'];
  const view = ui.renderer.mount(ui.root, { body: { type: 'stack', children: states.map(state => ({ type: 'ref', ref: `todo/tasks/${state}`, label: 'Label' })) } }, bridge({
    command: async () => states.map(state => ({ ref: `todo/tasks/${state}`, state })), openRef: () => opens++,
  }));
  await view.ready; await tick();
  const chips = walk(ui.root).filter(n => n.className?.startsWith('pp-ref ' ) || n.className === 'pp-ref');
  assert.deepEqual(chips.map(n => n.children[1].textContent), ['Missing', 'In trash', 'Label']);
  for (const chip of chips) { assert.equal(chip.disabled, true); await chip.fire('click'); }
  assert.equal(opens, 0); view.dispose();
});

test('refs are resolved in one batch and cached until refresh', async () => {
  const ui = rendererHarness(); const calls = [];
  const view = ui.renderer.mount(ui.root, { body: { type: 'list', query: 'rows', item: { type: 'ref', ref: 'todo/tasks/{{row.id}}' } } }, bridge({
    query: async () => ['t1', 't2', 't1', 't3', 't2'].map(id => ({ id })),
    command: async (kind, id, args) => { calls.push([...args.refs]); return args.refs.map(ref => ({ ...hit, ref })); },
  }));
  await view.ready; await tick();
  assert.deepEqual(calls, [['todo/tasks/t1', 'todo/tasks/t2', 'todo/tasks/t3']]);
  view.refs.chip('todo/tasks/t1'); await tick(); assert.equal(calls.length, 1);
  await view.refresh(); await tick(); assert.equal(calls.length, 2); view.dispose();
});

test('an empty ref draws nothing', async () => {
  const ui = rendererHarness(); const view = ui.renderer.mount(ui.root, { body: { type: 'ref', ref: 'notes/notes/' } }, bridge());
  await view.ready; assert.equal(ui.root.children.length, 0); view.dispose();
});

test('markdown renders terrarium links as chips and keeps http links', () => {
  const ui = rendererHarness(); const marker = { tag: 'marker', children: [] }; let args;
  const input = 'See [Lamp](terrarium:todo/tasks/t1) and [site](https://example.com)';
  const md = ui.renderer.markdown(input, { ref: (...a) => { args = a; return marker; } });
  assert.deepEqual(args, ['todo/tasks/t1', 'Lamp']); assert.ok(walk(md).includes(marker));
  assert.equal(walk(md).find(n => n.tag === 'a').href, 'https://example.com');
  const plain = ui.renderer.markdown(input);
  assert.match(text(plain), /Lamp/); assert.equal(walk(plain).filter(n => n.tag === 'a').length, 1);
  assert.ok(walk(plain).every(n => !n.href?.startsWith('terrarium:')));
});

test('a malicious ref is plain text', () => {
  const ui = rendererHarness(); const input = '[x](terrarium:todo/tasks/a b) and [x](terrarium:../../etc)';
  const md = ui.renderer.markdown(input, { ref: () => assert.fail('invalid ref') });
  assert.equal(text(md), input); assert.equal(walk(md).filter(n => n.tag === 'a').length, 0);
});

test('batches split at 200 and errors or absent commands preserve labels', async () => {
  const ui = rendererHarness(); const sizes = [];
  const view = ui.renderer.mount(ui.root, { body: { type: 'stack', children: Array.from({ length: 201 }, (_, i) => ({ type: 'ref', ref: `todo/tasks/t${i}`, label: 'Label' })) } }, bridge({
    command: async (kind, id, { refs }) => { sizes.push(refs.length); throw new Error('offline'); },
  }));
  await view.ready; await tick(); assert.deepEqual(sizes, [200, 1]);
  assert.equal(ui.root.children[0].children[0].children[1].textContent, 'Label');
  delete view.bridge.command; await view.refresh(); await tick(); view.dispose();
});

test('disposed views and replies from a previous refresh cannot change chips', async () => {
  const ui = rendererHarness(); let answer;
  const view = ui.renderer.mount(ui.root, { body: { type: 'ref', ref: hit.ref, label: 'Label' } }, bridge({ command: () => new Promise(resolve => { answer = resolve; }) }));
  await view.ready; const old = answer;
  view.bridge.command = async () => [{ ...hit, title: 'Current' }];
  await view.refresh(); await tick(); old([hit]); await tick();
  const chip = ui.root.children[0]; assert.equal(chip.children[1].textContent, 'Current');
  view.bridge.command = () => new Promise(resolve => { answer = resolve; });
  await view.refresh(); view.dispose(); answer([hit]); await tick();
  assert.equal(chip.children[1].textContent, 'Current');
});

test('bridge forwards only allowed host commands and guards the captured space', async () => {
  const source = readFileSync(require.resolve('../app.js'), 'utf8');
  const calls = []; const opens = []; let closes = 0;
  const context = { window: {}, currentSpace: 'personal', rails: { closeDrawer: () => closes++ }, call: (...a) => { calls.push(a); return Promise.resolve(); }, openPage: (...a) => opens.push(a), toast() {}, localToday() {} };
  vm.createContext(context);
  vm.runInContext(source.slice(source.indexOf('const HOST_BRIDGE_COMMANDS'), source.indexOf('\nfunction refreshPluginViews')), context);
  const b = vm.runInContext('bridge("notes")', context);
  assert.equal(b.plugin, 'notes'); await b.command('query', 'ref.resolve', { refs: [hit.ref] });
  assert.equal(calls[0][0], 'command'); assert.equal(calls[0][1].space, 'personal');
  await assert.rejects(b.command('action', 'todo.create', {}), /Not allowed/); assert.equal(calls.length, 1);
  await b.command('query', 'search.query', { q: 'lamp', limit: 8 });
  assert.equal(calls.length, 2); assert.equal(calls[1][1].id, 'search.query'); assert.equal(calls[1][1].space, 'personal');
  for (const [kind, id] of [['query', 'ref.backlinks'], ['query', 'link.list'], ['action', 'link.remove'], ['action', 'link.add']]) {
    await b.command(kind, id, {}); assert.equal(calls.at(-1)[1].space, 'personal'); assert.equal(calls.at(-1)[1].id, id);
  }
  await assert.rejects(b.command('action', 'plugin.uninstall', {}), /Not allowed/);
  b.openRef(hit); assert.equal(opens.length, 1); assert.equal(closes, 1);
  assert.equal(opens[0][0], 'todo'); assert.equal(opens[0][1], 'main'); assert.equal(opens[0][2].item, 't1');
  context.currentSpace = 'work'; b.openRef(hit); b.openRef({}); assert.equal(opens.length, 1);
});

for (const type of ['markdown', 'markdown-editor']) test(`${type} resolves its chips through the view bridge`, async () => {
  const ui = rendererHarness(); let calls = 0;
  const view = ui.renderer.mount(ui.root, { body: { type, text: '[Lamp](terrarium:todo/tasks/t1)', value: '[Lamp](terrarium:todo/tasks/t1)', name: 'body', start: 'preview' } }, bridge({
    command: async () => { calls++; return [hit]; },
  }));
  await view.ready; await tick();
  assert.equal(calls, 1); assert.match(text(ui.root), /Buy lamp/);
  assert.equal(walk(ui.root).find(n => n.className === 'pp-ref').disabled, false);
  view.dispose();
});
