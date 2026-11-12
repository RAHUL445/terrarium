const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const { rendererHarness } = require('./helpers/renderer-dom.cjs');
const source = fs.readFileSync(path.join(__dirname, '../app.js'), 'utf8');
const code = source.slice(source.indexOf('// ---- History ----'), source.indexOf('// ---- End history ----'));
const turn = () => new Promise(resolve => setImmediate(resolve));
const plain = value => JSON.parse(JSON.stringify(value));

function harness(reply, confirm = true) {
  const calls = [], toasts = [], confirmations = [];
  let refreshed = 0;
  class Element {
    constructor(tag, attrs = {}, children = []) { this.tag = tag; this.attrs = attrs; this.children = children; this.open = false; this.isConnected = true; }
    append(...children) { this.children.push(...children); }
    replaceChildren(...children) { this.children = children; }
    setAttribute(k, v) { this.attrs[k] = v; }
    removeAttribute(k) { delete this.attrs[k]; }
    focus() { document.activeElement = this; }
    showModal() { this.open = true; }
    close() { this.open = false; queueMicrotask(() => this.attrs.onclose?.()); }
    async click() { await this.attrs.onclick?.(); }
  }
  const document = { body: new Element('body'), activeElement: new Element('button') };
  const opener = document.activeElement;
  const context = { document, currentSpace: 'personal', Date,
    el: (tag, attrs, ...children) => new Element(tag, attrs, children),
    call: async (command, args) => { calls.push(plain({ command, args })); return reply(args); },
    toast: (...args) => toasts.push(args), refreshPluginViews: () => refreshed++,
    window: { terrariumConfirm: async (...args) => { confirmations.push(plain(args)); return typeof confirm === 'function' ? confirm() : confirm; } },
  };
  vm.runInNewContext(code, context);
  const nodes = () => { const walk = node => [node, ...node.children.filter(c => c instanceof Element).flatMap(walk)]; return walk(document.body); };
  return { context, calls, toasts, confirmations, document, opener, nodes,
    get: id => nodes().find(n => n.attrs.id === id),
    button: text => nodes().find(n => n.tag === 'button' && n.children.includes(text)),
    versions: () => nodes().filter(n => n.attrs.class === 'history-version'),
    open: (spec = {}) => context.openHistory('todo', 'personal', { table: 'tasks', id: 'r1', title: 'Pay rent', ...spec }),
    refreshed: () => refreshed };
}
const now = new Date(2026, 10, 11, 14, 2).getTime();
const versions = [
  { id: 'new', at_ms: now, device: 'd1', columns: ['title'] },
  { id: 'old', at_ms: now - 180000, device: 'd2', columns: ['body', 'title'], created: true },
];
function reply(args) {
  if (args.id === 'item.restore_version') return { restored: args.args.version, columns: ['title'] };
  if (!args.args.version) return { versions, this_device: 'd1' };
  return { values: { other_field: null, title: '<img src=x>', body: 'x'.repeat(2001), done: true, empty: '', no: false }, in_trash: true };
}

test('history labels use local days, device labels and readable change summaries', () => {
  const h = harness(reply), label = h.context.historyLabel;
  const created = label({ device: 'd1', at_ms: now - 60000, created: true, columns: ['title'] }, 'd1', now);
  assert.equal(created.who, 'This device'); assert.equal(created.what, 'Created'); assert.match(created.when, /^Today 14:01/);
  assert.equal(label({ device: 'd2', at_ms: now, columns: ['title', 'completed_at', 'a', 'b', 'c'] }, 'd1', now).what, 'Changed title, completed at, a, b and 1 more');
  assert.equal(label(versions[1], 'd1', now).who, 'Another device');
  const yesterday = new Date(now); yesterday.setDate(yesterday.getDate() - 1);
  assert.match(label({ at_ms: yesterday.getTime(), trashed: true }, '', now).when, /^Yesterday/);
  assert.equal(label({ restored: true, at_ms: now }, '', now, { in_trash: true }).what, 'Restored from trash');
  assert.equal(label({ restored: true, at_ms: now }, '', now, { in_trash: false }).what, 'Restored an earlier version');
  assert.equal(label({ restored: true, at_ms: now }, '', now).what, 'Restored an earlier version');
  assert.equal(label({ trashed: true, at_ms: now }, '', now).what, 'Moved to trash');
  assert.ok(!label({ at_ms: now - 3 * 86400000 }, '', now).when.startsWith('Today'));
});

test('renderer history action resolves the row and tolerates a missing bridge method', async () => {
  for (const supported of [true, false]) {
    const h = rendererHarness(), calls = [];
    const bridge = { today: () => '2026-11-11', query: async () => [{ id: 'r1', title: 'Pay rent' }], ...(supported ? { history: spec => calls.push(plain(spec)) } : {}) };
    const view = h.renderer.mount(h.root, { body: { type: 'list', query: 'tasks', item: { type: 'button', label: 'History', action: { history: { table: 'tasks', id: '{{row.id}}', title: '{{row.title}}' } } } } }, bridge);
    await view.ready; await turn();
    const walk = n => [n, ...n.children.flatMap(walk)];
    await walk(h.root).find(n => n.tag === 'button').fire('click');
    assert.deepEqual(calls, supported ? [{ table: 'tasks', id: 'r1', title: 'Pay rent' }] : []);
    view.dispose();
  }
});

test('dialog loads scoped versions and safely renders values with changed fields first', async () => {
  const h = harness(reply); await h.open();
  assert.deepEqual(h.calls[0], { command: 'command', args: { kind: 'query', id: 'item.history', space: 'personal', args: { plugin: 'todo', table: 'tasks', id: 'r1', limit: 100 } } });
  assert.equal(h.get('history-title').textContent, 'History · Pay rent');
  assert.equal(h.versions()[0].attrs['aria-current'], 'true');
  assert.match(h.versions()[0].children[0].children[0], /Current/);
  assert.equal(h.button('Restore this version'), undefined);
  const fields = h.get('history-detail').children.filter(n => n.attrs?.class === 'history-field');
  assert.deepEqual(fields.map(n => n.children[0].children[0]), ['title', 'other field', 'body', 'done', 'empty', 'no']);
  assert.equal(fields[0].children[1].children[0], '<img src=x>');
  assert.equal(fields[3].children[1].children[0], 'Yes');
  assert.equal(fields[4].children[1].children[0], '—');
  assert.equal(fields[5].children[1].children[0], 'No');
  assert.equal(fields[2].children[1].children[0].length, 2001);
  await h.button('Show all').click();
  assert.equal(fields[2].children[1].textContent, 'x'.repeat(2001));
});

test('restore confirms, writes the selected version, refreshes views and reloads', async () => {
  const h = harness(reply); await h.open(); await h.versions()[1].click();
  assert.equal(h.versions()[0].attrs['aria-current'], undefined);
  assert.equal(h.versions()[1].attrs['aria-current'], 'true');
  await h.button('Restore this version').click();
  assert.equal(h.confirmations[0][1].title, 'Restore version');
  assert.match(h.confirmations[0][0], /What’s there now stays in history/);
  assert.deepEqual(h.calls.find(c => c.args.kind === 'action').args, { kind: 'action', id: 'item.restore_version', space: 'personal', args: { plugin: 'todo', table: 'tasks', id: 'r1', version: 'old' } });
  assert.deepEqual(h.toasts, [['Restored']]); assert.equal(h.refreshed(), 1);
  assert.equal(h.calls.filter(c => c.args.args.limit).length, 2);
});

test('cancelled restore makes no write; write errors toast and permit retry', async () => {
  for (const confirm of [false, true]) {
    const h = harness(args => { if (args.kind === 'action') throw Error('cannot restore'); return reply(args); }, confirm);
    await h.open(); await h.versions()[1].click(); await h.button('Restore this version').click();
    assert.equal(h.calls.some(c => c.args.kind === 'action'), confirm);
    assert.deepEqual(h.toasts, confirm ? [['Error: cannot restore', 'error']] : []);
    assert.equal(h.button('Restore this version').disabled, false);
  }
});

test('empty history and query failures stay inside the dialog', async () => {
  const empty = harness(() => ({ versions: [], this_device: 'd1' })); await empty.open();
  assert.equal(empty.get('history-versions').children[0].children[0], 'No history for this item yet.');
  const failed = harness(() => { throw Error('unavailable'); }); await failed.open();
  assert.equal(failed.get('history-versions').children[0].attrs.role, 'alert');
  const detail = harness(args => { if (args.args.version) throw Error('expired'); return reply(args); }); await detail.open();
  assert.equal(detail.get('history-detail').children[0].attrs.role, 'alert');
  assert.match(detail.get('history-detail').children[0].children[0], /expired/);
});

test('Escape and Close return focus, reuse one dialog and ignore late responses', async () => {
  const h = harness(reply); await h.open();
  const dialog = h.document.body.children[0];
  let prevented = false;
  dialog.attrs.oncancel({ preventDefault() { prevented = true; } }); await turn();
  assert.equal(prevented, true); assert.equal(dialog.open, false); assert.equal(h.document.activeElement, h.opener);
  await h.open(); assert.equal(h.document.body.children.length, 1);
  await h.nodes().find(n => n.attrs['aria-label'] === 'Close').click(); await turn();
  assert.equal(h.document.activeElement, h.opener);
  let finish;
  const slow = harness(args => args.args.version === 'old' ? new Promise(resolve => { finish = resolve; }) : reply(args));
  await slow.open(); const pending = slow.versions()[1].click();
  await slow.versions()[0].click(); finish({ values: { title: 'stale' } }); await pending;
  assert.ok(!slow.nodes().some(n => n.children.includes('stale')));
});

test('space changes and closing during confirmation prevent stale writes', async () => {
  let finish;
  const h = harness(reply, () => new Promise(resolve => { finish = resolve; }));
  await h.open(); await h.versions()[1].click();
  const pending = h.button('Restore this version').click();
  h.context.currentSpace = 'work'; h.context.closeHistory(); finish(true); await pending;
  assert.equal(h.calls.some(c => c.args.kind === 'action'), false);
  await h.open(); assert.equal(h.document.body.children[0].open, false);
  assert.match(source, /closeHistory\(\);\s*currentSpace = id;/);
});

test('bridge fixes plugin and space identity and ignores calls after changing spaces', async () => {
  const opened = [];
  const context = { window: {}, toast() {}, localToday() {}, currentSpace: 'personal', openHistory: (...args) => opened.push(plain(args)) };
  vm.runInNewContext(source.slice(source.indexOf('function bridge(plugin)'), source.indexOf('// ---- History ----')), context);
  const bridge = context.bridge('todo');
  const spec = { table: 'tasks', id: 'r1', title: 'Pay rent', plugin: 'notes', space: 'work' };
  bridge.history(spec);
  assert.deepEqual(opened, [['todo', 'personal', spec]]);
  context.currentSpace = 'work'; bridge.history(spec);
  assert.equal(opened.length, 1);
});

test('an old list response cannot replace a reopened item', async () => {
  let finish;
  const h = harness(args => args.args.id === 'slow' ? new Promise(resolve => { finish = resolve; }) : reply(args));
  const pending = h.open({ id: 'slow', title: 'Old item' });
  h.context.closeHistory();
  await h.open({ id: 'r1', title: 'New item' });
  finish({ versions: [], this_device: 'd1' }); await pending;
  assert.equal(h.get('history-title').textContent, 'History · New item');
  assert.equal(h.versions().length, 2);
  assert.equal(h.document.body.children[0].open, true);
});

test('retention explains an expired Current without requesting its values', async () => {
  const h = harness(args => args.args.version ? assert.fail('expired preview requested') : {
    versions: [{ ...versions[0], at_ms: Date.now() - 31 * 86400000 }], this_device: 'd1', keeps_days: 30,
  });
  await h.open();
  assert.equal(h.get('history-retention').textContent, 'Showing the last 30 days');
  assert.match(h.versions()[0].children[0].children[0], /Current/);
  assert.match(h.get('history-detail').children[0].children[0], /preview is unavailable/);
  assert.equal(h.button('Restore this version'), undefined);
  assert.equal(h.calls.length, 1);
});
