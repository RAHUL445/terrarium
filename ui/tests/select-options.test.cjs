const { test } = require('node:test');
const assert = require('node:assert/strict');
const { rendererHarness } = require('./helpers/renderer-dom.cjs');

const findAll = (node, predicate, out = []) => {
  if (predicate(node)) out.push(node);
  for (const child of node.children || []) if (typeof child !== 'string') findAll(child, predicate, out);
  return out;
};
const select = root => findAll(root, node => node.tag === 'select')[0];
const optionValues = node => node.children.map(option => option.value);
const tick = async () => { await new Promise(resolve => setImmediate(resolve)); };

function formTemplate() {
  return { body: { type: 'form', submit: { op: 'save' }, children: [
    { type: 'text-field', name: 'list_id', value: 'one' },
    { type: 'select', name: 'group_id', bind: 'state.group', options: [{ value: '', label: 'Choose' }], options_query: {
      query: 'groups', params: { list_id: '{{form.list_id}}' }, value: 'id', label: 'name',
    } },
  ] } };
}

test('options query receives its row scope', async () => {
  const ui = rendererHarness();
  const received = [];
  const view = ui.renderer.mount(ui.root, { body: { type: 'list', query: 'rows', item: { type: 'select', options_query: {
    query: 'choices', params: { row_id: '{{row.id}}' }, value: 'id', label: 'name',
  } } } }, { today: () => '2026-11-12', query: async (name, params) => {
    received.push([name, params]);
    return name === 'rows' ? [{ id: 'row-a' }] : [{ id: 'choice-a', name: 'Choice A' }];
  } });
  await view.ready;
  assert.ok(received.some(([name, params]) => name === 'choices' && params.row_id === 'row-a'));
  view.dispose();
});

test('form parameter changes fetch matching choices and clear stale selection', async () => {
  const ui = rendererHarness();
  const calls = [];
  const view = ui.renderer.mount(ui.root, formTemplate(), { today: () => '2026-11-12', query: async (_name, params) => {
    calls.push(params.list_id);
    return [{ id: `${params.list_id}-group`, name: params.list_id }];
  } });
  await view.ready;
  const form = findAll(ui.root, node => node.tag === 'form')[0];
  const text = findAll(form, node => node.tag === 'input')[0];
  const choice = select(form);
  choice.value = 'one-group';
  text.value = 'two';
  await form.fire('input', { bubbles: true });
  await tick();
  assert.ok(calls.includes('two'));
  assert.equal(choice.value, '');
  assert.deepEqual(optionValues(choice), ['', 'two-group']);
  view.dispose();
});

test('late earlier response is ignored and same parameter refresh retains the choice', async () => {
  const ui = rendererHarness();
  const pending = [];
  const view = ui.renderer.mount(ui.root, formTemplate(), { today: () => '2026-11-12', query: (_name, params) => new Promise(resolve => pending.push({ key: params.list_id, resolve })) });
  const form = findAll(ui.root, node => node.tag === 'form')[0];
  const text = findAll(form, node => node.tag === 'input')[0];
  const choice = select(form);
  await tick();
  pending.find(request => request.key === 'one').resolve([{ id: 'one-group', name: 'One' }]);
  await view.ready;
  text.value = 'two';
  await form.fire('input', { bubbles: true });
  await tick();
  pending.find(request => request.key === 'two').resolve([{ id: 'two-group', name: 'Two' }]);
  await tick();
  pending.find(request => request.key === 'one').resolve([{ id: 'one-group', name: 'One' }]);
  await tick();
  assert.deepEqual(optionValues(choice), ['', 'two-group']);
  choice.value = 'two-group';
  await choice.fire('change');
  const refreshing = view.refresh();
  await tick();
  pending.filter(request => request.key === 'two').at(-1).resolve([{ id: 'two-group', name: 'Two refreshed' }]);
  await refreshing;
  await tick();
  assert.equal(choice.value, 'two-group');
  view.dispose();
});

test('initial selection omitted from results remains submit-able as unavailable', async () => {
  const ui = rendererHarness();
  const view = ui.renderer.mount(ui.root, { state: { group: 'archived' }, body: { type: 'select', bind: 'state.group', options_query: {
    query: 'groups', params: {}, value: 'id', label: 'name',
  } } }, { today: () => '2026-11-12', query: async () => [{ id: 'active', name: 'Active' }] });
  await view.ready;
  const choice = select(ui.root);
  assert.equal(choice.value, 'archived');
  assert.deepEqual(optionValues(choice), ['active', 'archived']);
  assert.equal(choice.children[1].textContent, 'Unavailable selection');
  view.dispose();
});
