const { test } = require('node:test');
const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { resolve } = require('node:path');
const { rendererHarness } = require('./helpers/renderer-dom.cjs');

const packages = process.env.TERRA_PLUGIN_SOURCE || resolve(__dirname, '../../../terrarium-plugins/packages');
const templatePath = resolve(packages, 'todo/ui/page-main.json');
const template = JSON.parse(readFileSync(templatePath, 'utf8'));
const walk = (node, visit, parent = null) => {
  if (!node || typeof node !== 'object') return;
  visit(node, parent);
  for (const child of node.children || []) walk(child, visit, node);
  for (const key of ['navigation', 'content', 'item', 'trailing']) {
    const child = node[key];
    if (Array.isArray(child)) child.forEach(value => walk(value, visit, node));
    else if (child && typeof child === 'object') walk(child, visit, node);
  }
};
const nodes = (root, predicate) => {
  const found = [];
  walk(root, node => { if (predicate(node)) found.push(node); });
  return found;
};
const domNodes = (root, predicate, out = []) => {
  if (predicate(root)) out.push(root);
  for (const child of root.children || []) if (typeof child !== 'string') domNodes(child, predicate, out);
  return out;
};

function openTemplate() {
  const copy = structuredClone(template);
  Object.assign(copy.state, {
    open: 'task-1', manage: 'yes', edit_list: 'list-1',
    new_list: 'yes', new_group: 'list-1', new_status: 'list-1',
  });
  return copy;
}

function bridgeFor(calls) {
  return {
    today: () => '2026-11-12', toast() {}, changed() {},
    query: async (name, params = {}) => {
      if (name === 'task') return [{ id: params.id || 'task-1', title: 'Old task', list_id: 'list-1', group_id: null, status_id: null, planned_for: null, due: null }];
      if (name === 'task_lists') return [{ id: 'list-1', name: 'Work' }];
      if (name === 'list_detail') return [{ id: 'list-1', name: 'Work', color: '', archived: 0 }];
      if (name === 'group_scope' || name === 'status_scope') return [{ list_id: params.id || 'list-1' }];
      if (name === 'group_choices' || name === 'status_choices') calls.queries.push([name, params]);
      if (name === 'list_choices') return [{ value: 'list-1', label: 'Work' }];
      if (name === 'group_choices') return [{ value: 'group-1', label: 'Group' }];
      if (name === 'status_choices') return [{ value: 'status-1', label: 'Ready' }];
      return [];
    },
    action: async (op, args) => { calls.actions.push([op, args]); return { id: 'created', title: args.title }; },
  };
}

test('capture destination is explicit, Plan today changes planning only, and edit selects use row-scoped choices', async () => {
  const capture = nodes(template.body, node => node.type === 'form' && node.submit?.op === 'create')
    .find(form => form.submit.args.list_id === '{{form.list_id}}');
  assert.ok(capture);
  const destination = capture.children.find(node => node.type === 'select' && node.name === 'list_id');
  assert.equal(destination.options_query.query, 'list_choices');
  assert.equal(destination.value, '');

  const planningActions = nodes(template.body, node => node.label === 'Plan today');
  assert.ok(planningActions.length);
  for (const action of planningActions) {
    assert.equal(action.action.op, 'update');
    assert.equal(action.action.args.planned_for, '{{$today}}');
    assert.equal(Object.hasOwn(action.action.args, 'due'), false);
  }

  const calls = { queries: [], actions: [] };
  const ui = rendererHarness();
  const view = ui.renderer.mount(ui.root, openTemplate(), bridgeFor(calls));
  await view.ready;
  await new Promise(resolve => setImmediate(resolve));
  assert.ok(calls.queries.some(([name, params]) => name === 'group_choices' && params.list_id === 'list-1'));
  assert.ok(calls.queries.some(([name, params]) => name === 'status_choices' && params.list_id === 'list-1'));
  view.dispose();
});

test('capture and list/group/status management forms call their declared operations', async () => {
  const calls = { queries: [], actions: [] };
  const ui = rendererHarness();
  const view = ui.renderer.mount(ui.root, openTemplate(), bridgeFor(calls));
  await view.ready;

  const forms = domNodes(ui.root, node => node.tag === 'form');
  const submitByLabel = async (label, values) => {
    const form = forms.find(node => node.children.some(child => child.tag === 'button' && child.textContent === label));
    assert.ok(form, `mounted form for ${label}`);
    for (const input of domNodes(form, node => node.dataset?.ppName)) {
      if (Object.hasOwn(values, input.dataset.ppName)) input.value = values[input.dataset.ppName];
    }
    await form.fire('submit', { bubbles: true });
  };

  await submitByLabel('Add task', { title: 'Captured', list_id: 'list-1' });
  await submitByLabel('Create list', { name: 'Projects' });
  await submitByLabel('Create group', { name: 'Launch' });
  await submitByLabel('Create status', { name: 'Ready' });

  assert.ok(calls.actions.some(([op, args]) => op === 'create' && args.title === 'Captured' && args.list_id === 'list-1'));
  assert.ok(calls.actions.some(([op, args]) => op === 'create_list' && args.name === 'Projects'));
  assert.ok(calls.actions.some(([op, args]) => op === 'create_group' && args.name === 'Launch' && args.list_id === 'list-1'));
  assert.ok(calls.actions.some(([op, args]) => op === 'create_status' && args.name === 'Ready' && args.list_id === 'list-1'));
  view.dispose();
});
