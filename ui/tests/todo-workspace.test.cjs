const { test } = require('node:test');
const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { resolve } = require('node:path');
const { rendererHarness } = require('./helpers/renderer-dom.cjs');

const packages = process.env.TERRA_PLUGIN_SOURCE || resolve(__dirname, '../../../terrarium-plugins/packages');
const templatePath = resolve(packages, 'tasks/ui/page-main.json');
const template = JSON.parse(readFileSync(templatePath, 'utf8'));
const walk = (node, visit, parent = null) => {
  if (!node || typeof node !== 'object') return;
  visit(node, parent);
  for (const child of node.children || []) walk(child, visit, node);
  for (const key of ['navigation', 'content', 'item', 'trailing', 'actions']) {
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
    today: () => '2026-11-12', toast(...args) { (calls.toasts ||= []).push(args); }, changed() {},
    query: async (name, params = {}) => {
      if (name === 'task') return [{ id: params.id || 'task-1', title: 'Old task', list_id: 'list-1', group_id: null, status_id: null, planned_for: null, due: null }];
      if (name === 'workspace_tasks') return calls.taskRows || [];
      if (name === 'steps') {
        (calls.queries ||= []).push([name, params]);
        return calls.stepRows?.[params.task_id] || [];
      }
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
  assert.equal(destination.bind, 'state.capture_list');
  assert.equal(template.state.capture_list, '');
  assert.deepEqual(destination.options, [{ value: '', label: 'Inbox' }]);

  const planningActions = nodes(template.body, node => node.label === 'Plan today');
  assert.ok(planningActions.length);
  for (const action of planningActions) {
    assert.equal(action.action.op, 'update');
    assert.equal(action.action.args.planned_for, '{{$today}}');
    assert.equal(Object.hasOwn(action.action.args, 'due'), false);
    assert.deepEqual(action.action.undo, {
      op: 'update',
      args: { id: '{{row.id}}', planned_for: '{{row.planned_for}}' },
      success: 'Restored previous plan',
    });
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
      if (!Object.hasOwn(values, input.dataset.ppName)) continue;
      input.value = values[input.dataset.ppName];
      if (input.tag === 'select') await input.fire('change');
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

test('inline steps load only the expanded task, use step and parent ids, and planning undo restores date or null', async () => {
  const calls = {
    queries: [], actions: [],
    taskRows: [
      { id: 'task-1', title: 'Dated task', planned_for: '2026-11-10', done: false, list_id: 'list-1', tags: '' },
      { id: 'task-2', title: 'Unplanned task', planned_for: null, done: false, list_id: 'list-1', tags: '' },
    ],
    stepRows: {
      'task-1': [{ id: 'step-1', title: 'First step', done: false }],
      'task-2': [{ id: 'step-2', title: 'Second step', done: false }],
    },
  };
  const ui = rendererHarness();
  const copy = openTemplate();
  copy.state.open = '';
  copy.state.inline_steps = '';
  const view = ui.renderer.mount(ui.root, copy, bridgeFor(calls));
  await view.ready;
  await new Promise(resolve => setImmediate(resolve));
  assert.deepEqual(calls.queries.filter(([name]) => name === 'steps'), []);

  const buttons = () => domNodes(ui.root, node => node.tag === 'button');
  // Row actions sit in one quiet "..." menu per row; items mount when it opens.
  const rowMenus = () => buttons().filter(node => /^More for /.test(node.attrs?.['aria-label'] || ''));
  assert.equal(rowMenus().length, 2);
  assert.equal(buttons().filter(node => node.textContent === 'Show steps').length, 0);
  let opened = null;
  const menuItems = (label) => domNodes(opened.parent, node => node.attrs?.role === 'menuitem' && domNodes(node, n => n.textContent === label).length > 0);
  const openMenu = async (n) => { opened = rowMenus()[n]; await opened.fire('click'); await new Promise(resolve => setImmediate(resolve)); };
  await openMenu(1);
  const show = menuItems('Show steps')[0];
  assert.ok(show, `second task menu has Show steps; buttons=${buttons().map(node => node.textContent).join('|')}`);
  assert.equal(menuItems('Plan today').length, 1);
  assert.equal(menuItems('Remove from today').length, 0);
  assert.equal(rowMenus()[1].parent.parent.className, 'pp-item-actions');
  assert.match(rowMenus()[1].parent.parent.parent.className, /\bpp-item-line\b/);
  await show.fire('click');
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(JSON.stringify(calls.queries.filter(([name]) => name === 'steps')), JSON.stringify([['steps', { task_id: 'task-2' }]]));

  const stepCheck = domNodes(ui.root, node => node.tag === 'input' && node.attrs?.['aria-label'] === 'Mark done: Second step')[0];
  assert.ok(stepCheck, 'expanded row renders the second task step checkbox');
  stepCheck.checked = true;
  await stepCheck.fire('change');
  assert.ok(calls.actions.some(([op, args]) => op === 'toggle_step' && args.id === 'step-2' && args.done === true));

  const addForm = domNodes(ui.root, node => node.tag === 'form').find(form =>
    form.children.some(child => child.tag === 'button' && child.textContent === 'Add step'));
  assert.ok(addForm, 'expanded row has an inline Add step form');
  const title = domNodes(addForm, node => node.dataset?.ppName === 'title')[0];
  title.value = 'New subtask';
  await addForm.fire('submit', { bubbles: true });
  assert.ok(calls.actions.some(([op, args]) => op === 'add_step' && args.task_id === 'task-2' && args.title === 'New subtask'));
  assert.deepEqual(calls.queries.filter(([name]) => name === 'steps').map(([, params]) => params.task_id), ['task-2']);

  await openMenu(1);
  const hide = menuItems('Hide steps')[0];
  assert.ok(hide, 'expanded row menu has a collapse item');
  assert.equal(menuItems('Show steps').length, 0);
  await hide.fire('click');
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(calls.queries.filter(([name]) => name === 'steps').length, 1);

  for (const [index, label] of [[0, 'Plan today'], [1, 'Plan today']]) {
    await openMenu(index);
    const plan = menuItems(label)[0];
    assert.ok(plan, `row ${index} menu has ${label}`);
    await plan.fire('click');
    const undo = calls.toasts.at(-1)?.[2];
    assert.equal(undo?.label, 'Undo');
    await undo.run();
  }
  assert.ok(calls.actions.some(([op, args]) => op === 'update' && args.id === 'task-1' && args.planned_for === '2026-11-10'));
  assert.ok(calls.actions.some(([op, args]) => op === 'update' && args.id === 'task-2' && args.planned_for === null));
  view.dispose();
});
