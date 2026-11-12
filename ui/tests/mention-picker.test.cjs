const { test } = require('node:test');
const assert = require('node:assert/strict');
const { rendererHarness } = require('./helpers/renderer-dom.cjs');
const wait = () => new Promise(resolve => setTimeout(resolve, 200));
const hits = [
  { ref: 'todo/tasks/t1', title: 'Buy lamp', kind: 'Task' },
  { ref: 'notes/notes/n1', title: 'Lamp ideas', kind: 'Note' },
  { title: 'Fallback hit', kind: 'Habit' },
  { ref: 'todo/tasks/../../x', title: 'Invalid' },
];
const walk = node => typeof node === 'string' ? [] : [node, ...node.children.flatMap(walk)];
async function setup(command = async () => ({ results: hits })) {
  const ui = rendererHarness();
  const view = ui.renderer.mount(ui.root, { state: {}, body: { type: 'form', children: [{ type: 'markdown-editor', name: 'body', value: '' }] } }, { today: () => '2026-11-11', command });
  await view.ready;
  const nodes = walk(ui.root);
  const area = nodes.find(n => n.tag === 'textarea');
  const picker = nodes.find(n => n.className === 'pp-mention');
  const type = async value => { area.value = value; area.setSelectionRange(value.length, value.length); await area.fire('input'); };
  return { ...ui, view, area, picker, type };
}

test('typing @ and text searches every plugin', async () => {
  const calls = []; const ui = await setup(async (...args) => { calls.push(args); return { results: hits }; });
  await ui.type('See @l'); await ui.type('See @lam'); await wait();
  assert.equal(calls.length, 1); assert.deepEqual(JSON.parse(JSON.stringify(calls[0])), ['query', 'search.query', { q: 'lam', limit: 8 }]);
  assert.equal(ui.picker.hidden, false); assert.deepEqual(ui.picker.children.map(n => n.textContent), ['Buy lamp', 'Lamp ideas']);
  assert.equal(ui.picker.attrs.role, 'listbox'); assert.equal(ui.area.attrs['aria-controls'], ui.picker.id);
  assert.equal(ui.area.attrs['aria-expanded'], 'true');
  ui.view.dispose();
});

test('enter inserts a terrarium link and emits the bubbling input autosave needs', async () => {
  const ui = await setup(); let input = 0;
  ui.root.querySelector('form').addEventListener('input', () => input++);
  await ui.type('- See @lam'); await wait();
  let prevented = 0;
  await ui.area.fire('keydown', { key: 'Enter', preventDefault: () => prevented++ });
  assert.equal(ui.area.value, '- See [Buy lamp](terrarium:todo/tasks/t1) ');
  assert.equal(input, 1); assert.equal(prevented, 1); assert.equal(ui.picker.hidden, true);
  assert.equal(ui.area.selectionStart, ui.area.value.length); assert.equal(ui.area.focused, true);
  ui.view.dispose();
});

test('arrows wrap, tab chooses and escape closes without changing text', async () => {
  const ui = await setup(); await ui.type('See @lam'); await wait();
  await ui.area.fire('keydown', { key: 'ArrowDown' });
  assert.equal(ui.picker.children[1].attrs['aria-selected'], 'true');
  assert.equal(ui.area.attrs['aria-activedescendant'], ui.picker.children[1].id);
  await ui.area.fire('keydown', { key: 'ArrowDown' }); assert.equal(ui.picker.children[0].attrs['aria-selected'], 'true');
  await ui.area.fire('keydown', { key: 'ArrowUp' }); assert.equal(ui.picker.children[1].attrs['aria-selected'], 'true');
  await ui.area.fire('keydown', { key: 'Escape' }); assert.equal(ui.picker.hidden, true); assert.equal(ui.area.value, 'See @lam');
  await ui.type('See @lam'); await wait(); await ui.area.fire('keydown', { key: 'Tab' });
  assert.equal(ui.area.value, 'See [Buy lamp](terrarium:todo/tasks/t1) '); ui.view.dispose();
});

test('brackets and newlines in titles cannot break the link', async () => {
  const ui = await setup(async () => ({ results: [{ ...hits[0], title: 'a]b(c[\n\r' }] }));
  await ui.type('See @lam'); await wait(); await ui.area.fire('keydown', { key: 'Enter' });
  assert.equal(ui.area.value, 'See [a b(c](terrarium:todo/tasks/t1) '); ui.view.dispose();
});

test('no picker for emails, a bare @, invalid tokens or more than 40 characters', async () => {
  let calls = 0; const ui = await setup(async () => { calls++; return { results: hits }; });
  for (const value of ['mail a@b', 'x @', '@'+ 'a'.repeat(41), '@a[b', '@a(b', '@a@b', '@a b']) await ui.type(value);
  await wait(); assert.equal(calls, 0); assert.equal(ui.picker.hidden, true); ui.view.dispose();
});

test('old answers are ignored', async () => {
  let old; const ui = await setup((kind, id, { q }) => q === 'first' ? new Promise(resolve => { old = resolve; }) : Promise.resolve({ results: [hits[1]] }));
  await ui.type('@first'); await wait(); await ui.type('@second'); await wait();
  assert.equal(ui.picker.children[0].textContent, 'Lamp ideas');
  old({ results: [hits[0]] }); await wait(); assert.equal(ui.picker.children[0].textContent, 'Lamp ideas'); ui.view.dispose();
});

test('mouse and touch choose without losing focus and preserve text after the caret', async () => {
  for (const event of ['mousedown', 'click']) {
    const ui = await setup(); await ui.type('See @lam later'); ui.area.setSelectionRange(8, 8); await ui.area.fire('input'); await wait();
    let prevented = false;
    await ui.picker.children[0].fire(event, { pointerType: 'touch', preventDefault() { prevented = true; } });
    assert.equal(ui.area.value, 'See [Buy lamp](terrarium:todo/tasks/t1)  later');
    if (event !== 'click') assert.equal(prevented, true);
    assert.equal(ui.picker.hidden, true); ui.view.dispose();
  }
});

test('blur, preview, invalidated input and disposal close or cancel pending results', async () => {
  const ui = await setup(); await ui.type('@lam'); await wait();
  await ui.area.fire('blur'); await wait(); assert.equal(ui.picker.hidden, true);
  await ui.type('@lam'); await wait();
  const preview = walk(ui.root).find(n => n.className?.includes('pp-md-mode'));
  await preview.fire('click'); assert.equal(ui.picker.hidden, true);
  await preview.fire('click'); await ui.type('@lam'); await ui.type('no mention'); await wait(); assert.equal(ui.picker.hidden, true);
  let finish; ui.view.bridge.command = () => new Promise(resolve => { finish = resolve; });
  await ui.type('@lam'); await wait(); ui.view.dispose(); finish({ results: hits }); await wait();
  assert.equal(ui.picker.hidden, true); assert.equal(ui.view.timers.size, 0);
});

test('empty and failed searches show no matches and prevent list continuation', async () => {
  const ui = await setup(async () => { throw new Error('offline'); });
  await ui.type('- @lam'); await wait(); assert.equal(ui.picker.children[0].textContent, 'No matching items');
  await ui.area.fire('keydown', { key: 'Enter' }); assert.equal(ui.area.value, '- @lam'); ui.view.dispose();
});


test('disposal cancels a pending debounce and invalid input invalidates an in-flight search', async () => {
  let calls = 0;
  const ui = await setup(async () => { calls++; return { results: hits }; });
  await ui.type('@lamp'); ui.view.dispose(); await wait(); assert.equal(calls, 0);
  let finish;
  const next = await setup(() => new Promise(resolve => { finish = resolve; }));
  await next.type('@lamp'); await wait(); await next.type('plain text');
  finish({ results: hits }); await wait(); assert.equal(next.picker.hidden, true); next.view.dispose();
});
