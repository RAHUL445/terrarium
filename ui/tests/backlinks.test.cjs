const { test } = require('node:test');
const assert = require('node:assert/strict');
const { rendererHarness } = require('./helpers/renderer-dom.cjs');
const turn = () => new Promise(resolve => setImmediate(resolve));
const walk = n => typeof n === 'string' ? [] : [n, ...n.children.flatMap(walk)];
const matching = (root, cls) => walk(root).filter(n => n.className?.split(' ').includes(cls));
const bridge = extra => ({ today: () => '2026-11-11', plugin: 'todo', ...extra });
const item = 'todo/tasks/t1';
const note = 'notes/notes/n1';
const bookmark = 'bookmarks/bookmarks/b1';
const relation = { id: 'l1', source: item, target: bookmark, kind: 'related', other: bookmark, direction: 'out' };
const hit = (ref, kind = 'Note', state = 'ok') => ({ ref, kind, state, title: ref === bookmark ? 'A bookmark' : 'A note', icon: 'note', open: { plugin: ref.split('/')[0], page: 'main', item: ref.split('/')[2] } });
const template = (extra = {}) => ({ body: { type: 'backlinks', ref: item, ...extra } });
function commands(calls, extra = {}) {
  return async (kind, id, args) => {
    calls.push({ kind, id, args });
    if (extra[id]) return extra[id](args);
    if (id === 'ref.backlinks') return { mentions: [note] };
    if (id === 'link.list') return [relation];
    if (id === 'ref.resolve') return args.refs.map(ref => hit(ref, ref === bookmark ? 'Bookmark' : 'Note'));
    if (id === 'link.remove') return { removed: 1 };
    assert.fail(id);
  };
}

test('rows get $ref when the list names its table without mutating query rows', async () => {
  for (const table of ['notes', undefined, 'bad/table']) {
    const ui = rendererHarness(); const row = Object.freeze({ id: 'n1', title: 'A' });
    const view = ui.renderer.mount(ui.root, { body: { type: 'list', query: 'rows', table, item: { type: 'text', text: '{{row.$ref}}' } } }, bridge({ plugin: 'notes', query: async () => [row] }));
    await view.ready;
    assert.equal(matching(ui.root, 'pp-text')[0].textContent || '', table === 'notes' ? 'notes/notes/n1' : '');
    assert.equal(row.$ref, undefined); view.dispose();
  }
});

test('sheet rows get $ref in initial and refreshed scopes without spurious rebuilds', async () => {
  const ui = rendererHarness(); let row = Object.freeze({ id: 't1', title: 'A' });
  const view = ui.renderer.mount(ui.root, { state: { open: 't1' }, body: { type: 'sheet', table: 'tasks', query: 'one', bind: 'state.open', refresh: true, children: [{ type: 'text', text: '{{row.$ref}}: {{row.title}}' }] } }, bridge({ query: async () => [row] }));
  await view.ready; await turn();
  const first = matching(ui.root, 'pp-text')[0]; assert.equal(first.textContent, 'todo/tasks/t1: A');
  await view.refresh(); assert.equal(matching(ui.root, 'pp-text')[0], first);
  row = Object.freeze({ id: 't1', title: 'B' }); await view.refresh();
  assert.equal(matching(ui.root, 'pp-text')[0].textContent, 'todo/tasks/t1: B');
  assert.equal(row.$ref, undefined); view.dispose();
});

test('backlinks merge mentions and relations, grouped by kind', async () => {
  const ui = rendererHarness(); const calls = []; const opened = [];
  const view = ui.renderer.mount(ui.root, template(), bridge({ command: commands(calls), openRef: r => opened.push(r) }));
  await view.ready; await turn();
  assert.deepEqual(matching(ui.root, 'pp-group').map(n => n.textContent), ['Bookmark', 'Note']);
  assert.equal(matching(ui.root, 'pp-backlinks')[0].attrs['aria-busy'], 'false');
  assert.equal(walk(ui.root).find(n => n.tag === 'h3').textContent, 'Linked from');
  const removes = walk(ui.root).filter(n => n.attrs?.['aria-label'] === 'Remove link'); assert.equal(removes.length, 1);
  const chips = matching(ui.root, 'pp-ref'); assert.deepEqual(chips.map(n => n.children[1].textContent), ['A bookmark', 'A note']);
  assert.equal(removes[0].parent, chips[0].parent);
  assert.equal(calls.filter(c => c.id === 'ref.resolve').length, 1, 'group response is reused by chips');
  await chips[1].fire('click'); assert.equal(opened[0].ref, note);
  assert.deepEqual(Object.keys(view.state), []); view.dispose();
});

test('relations win over duplicate mentions', async () => {
  const ui = rendererHarness(); const calls = [];
  const view = ui.renderer.mount(ui.root, template(), bridge({ command: commands(calls, { 'ref.backlinks': () => ({ mentions: [bookmark, bookmark] }) }) }));
  await view.ready; await turn();
  assert.equal(matching(ui.root, 'pp-ref').length, 1);
  assert.equal(walk(ui.root).filter(n => n.attrs?.['aria-label'] === 'Remove link').length, 1);
  assert.deepEqual([...calls.find(c => c.id === 'ref.resolve').args.refs], [bookmark]); view.dispose();
});

test('removing a relation calls link.remove and reloads', async () => {
  const ui = rendererHarness(); const calls = []; let removed = false, changed = 0;
  const view = ui.renderer.mount(ui.root, template(), bridge({ changed: () => changed++, command: commands(calls, {
    'link.list': () => removed ? [] : [relation], 'link.remove': () => { removed = true; return { removed: 1 }; },
  }) }));
  await view.ready;
  await walk(ui.root).find(n => n.attrs?.['aria-label'] === 'Remove link').fire('click');
  const call = calls.find(c => c.id === 'link.remove'); assert.equal(call.kind, 'action');
  assert.deepEqual(JSON.parse(JSON.stringify(call.args)), { source: item, target: bookmark, kind: 'related' });
  assert.equal(changed, 1); assert.equal(calls.filter(c => c.id === 'ref.backlinks').length, 2);
  assert.equal(matching(ui.root, 'pp-ref').length, 1);
  await view.refresh(); assert.equal(calls.filter(c => c.id === 'ref.backlinks').length, 3); view.dispose();
});

test('nothing links shows the empty text, and a bad ref renders nothing without calls', async () => {
  for (const ref of [item, '', '../bad', 'notes/notes/']) {
    const ui = rendererHarness(); const calls = [];
    const view = ui.renderer.mount(ui.root, template({ ref, empty: 'No links', title: 'Connections' }), bridge({ command: commands(calls, { 'ref.backlinks': () => ({ mentions: [] }), 'link.list': () => [] }) }));
    await view.ready;
    if (ref === item) {
      assert.equal(matching(ui.root, 'pp-muted')[0].textContent, 'No links'); assert.equal(walk(ui.root).find(n => n.tag === 'h3').textContent, 'Connections');
      assert.equal(calls.some(c => c.id === 'ref.resolve'), false);
    } else { assert.equal(ui.root.children.length, 0); assert.equal(calls.length, 0); }
    view.dispose();
  }
});

test('missing and trash use Other, unknown results are dropped', async () => {
  const ui = rendererHarness(); const calls = [];
  const states = ['missing', 'trash', 'unknown'];
  const view = ui.renderer.mount(ui.root, template(), bridge({ command: commands(calls, {
    'ref.backlinks': () => ({ mentions: states.map(s => `notes/notes/${s}`) }), 'link.list': () => [],
    'ref.resolve': ({ refs }) => refs.map(ref => hit(ref, 'Note', ref.split('/')[2])),
  }) }));
  await view.ready; await turn();
  assert.deepEqual(matching(ui.root, 'pp-group').map(n => n.textContent), ['Other']);
  assert.deepEqual(matching(ui.root, 'pp-ref-title').map(n => n.textContent), ['Missing', 'In trash']); view.dispose();
});

test('resolve chunks run sequentially and the 500-entry cap prioritizes relations', async () => {
  const ui = rendererHarness(); const calls = []; let active = 0, maxActive = 0;
  const relations = Array.from({ length: 300 }, (_, i) => ({ ...relation, other: `bookmarks/bookmarks/b${i}` }));
  const mentions = Array.from({ length: 300 }, (_, i) => `notes/notes/n${i}`);
  const view = ui.renderer.mount(ui.root, template(), bridge({ command: commands(calls, {
    'ref.backlinks': () => ({ mentions }), 'link.list': () => relations,
    'ref.resolve': async ({ refs }) => { active++; maxActive = Math.max(maxActive, active); await turn(); active--; return refs.map(r => hit(r)); },
  }) }));
  await view.ready; await turn();
  const batches = calls.filter(c => c.id === 'ref.resolve').map(c => [...c.args.refs]);
  assert.deepEqual(batches.map(refs => refs.length), [200, 200, 100]); assert.equal(maxActive, 1);
  assert.equal(batches.flat().filter(r => r.startsWith('bookmarks/')).length, 300);
  assert.equal(matching(ui.root, 'pp-ref').length, 500); assert.equal(matching(ui.root, 'pp-muted').at(-1).textContent, '100 more not shown'); view.dispose();
});

test('old or disposed answers cannot redraw the node or seed the chip cache', async () => {
  const ui = rendererHarness(); const calls = []; let answer;
  const view = ui.renderer.mount(ui.root, template(), bridge({ command: commands(calls, { 'ref.resolve': () => new Promise(resolve => { answer = resolve; }) }) }));
  await turn(); const old = answer;
  view.bridge.command = commands(calls, { 'ref.backlinks': () => ({ mentions: [] }), 'link.list': () => [] });
  await view.refresh(); old([hit(note)]); await view.ready;
  assert.equal(matching(ui.root, 'pp-ref').length, 0); assert.equal(view.refs.cache.has(note), false);
  view.bridge.command = commands(calls, { 'ref.resolve': () => new Promise(resolve => { answer = resolve; }) });
  const pending = view.refresh(); await turn(); view.dispose(); answer([hit(note)]); await pending;
  assert.equal(matching(ui.root, 'pp-ref').length, 0); assert.equal(view.refs.cache.size, 0);
});

test('failed source queries are isolated and remove errors are shown', async () => {
  const ui = rendererHarness(); const calls = []; const errors = [];
  const view = ui.renderer.mount(ui.root, template(), bridge({ command: commands(calls, {
    'ref.backlinks': () => { throw new Error('index unavailable'); }, 'link.remove': () => { throw new Error('remove failed'); },
  }), toast: (...args) => errors.push(args) }));
  await view.ready;
  const remove = walk(ui.root).find(n => n.attrs?.['aria-label'] === 'Remove link'); await remove.fire('click');
  assert.match(errors[0][0], /remove failed/); assert.equal(errors[0][1], 'error'); assert.equal(remove.disabled, false);
  view.dispose();
});

test('release of a conditional region invalidates its pending backlinks request', async () => {
  const ui = rendererHarness(); let answer;
  const view = ui.renderer.mount(ui.root, { state: { show: true }, body: { type: 'backlinks', ref: item, if: '{{state.show}}' } }, bridge({ command: async (kind, id) => id === 'ref.backlinks' ? new Promise(resolve => { answer = resolve; }) : [] }));
  await turn(); view.setState('state.show', false); answer({ mentions: [note] }); await view.ready;
  assert.equal(matching(ui.root, 'pp-ref').length, 0); assert.equal(view.refs.cache.size, 0); view.dispose();
});
