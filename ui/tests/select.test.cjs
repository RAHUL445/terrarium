const { test } = require('node:test');
const assert = require('node:assert/strict');
const { rendererHarness } = require('./helpers/renderer-dom.cjs');

function find(node, predicate) {
  if (predicate(node)) return node;
  for (const child of node.children || []) {
    const got = find(child, predicate);
    if (got) return got;
  }
  return null;
}

function findAll(node, predicate, out = []) {
  if (predicate(node)) out.push(node);
  for (const child of node.children || []) findAll(child, predicate, out);
  return out;
}

function taskTemplate() {
  return {
    body: {
      type: 'list',
      query: 'tasks',
      item: {
        type: 'item',
        title: '{{row.title}}',
        action: { op: 'open', args: { id: '{{row.id}}' } },
      },
      select: {
        label: 'Select tasks',
        actions: [
          { label: 'Complete', icon: 'check', op: 'toggle', args: { done: true }, success: '{{count}} tasks completed' },
          { label: 'Delete', icon: 'trash', variant: 'danger', op: 'delete', confirm: 'Move {{count}} tasks to the trash?', success: 'Moved to trash' },
        ],
      },
    },
  };
}

function boardTemplate() {
  return {
    body: {
      type: 'list',
      layout: 'board',
      label: 'Task board',
      query: 'tasks',
      column: 'lane',
      columns: [{ value: 'todo', label: 'To do' }, { value: 'done', label: 'Done' }],
      item: { type: 'text-field', name: 'title', value: '{{row.title}}' },
    },
  };
}

async function mountList({ rows, bulkResult, withBulk = true, window, template } = {}) {
  const ui = rendererHarness(window ? { window } : {});
  const bulkCalls = [];
  const actionCalls = [];
  const toasts = [];
  let changed = 0;
  const bridge = {
    today: () => '2026-11-11',
    query: async () => rows || [{ id: 'a', title: 'A' }, { id: 'b', title: 'B' }, { id: 'c', title: 'C' }],
    action: async (...args) => actionCalls.push(args),
    toast: (text, kind, action) => toasts.push([text, kind, action]),
    changed: () => { changed++; },
  };
  if (withBulk) {
    bridge.bulk = async (op, ids, args) => {
      bulkCalls.push([op, ids, args]);
      if (typeof bulkResult === 'function') return bulkResult(op, ids, args);
      if (bulkResult) return bulkResult;
      return { total: ids.length, done: ids.length, failed: 0, message: `${ids.length} done, 0 failed`, items: [] };
    };
  }
  const view = ui.renderer.mount(ui.root, template || taskTemplate(), bridge);
  await view.ready;
  return { ui, view, bulkCalls, actionCalls, toasts, changed: () => changed };
}

test('select mode shows checkboxes and counts', async () => {
  const { ui, actionCalls } = await mountList();
  const checks = findAll(ui.root, (n) => n.tag === 'input' && n.className === 'pp-select-check');
  assert.equal(checks.length, 3);
  assert.ok(checks.every((check) => check.hidden === true));
  await find(ui.root, (n) => n.tag === 'button' && n.textContent === 'Select').fire('click');
  assert.ok(checks.every((check) => check.hidden === false));
  await find(ui.root, (n) => n.tag === 'li' && n.dataset.ppId === 'a').fire('click');
  assert.equal(find(ui.root, (n) => n.tag === 'li' && n.dataset.ppId === 'a').dataset.ppSelected, 'true');
  assert.ok(!!find(ui.root, (n) => n.className === 'pp-select-count' && n.textContent === '1 selected'));
  assert.equal(actionCalls.length, 0);
});

test('select all and run an action', async () => {
  const { ui, bulkCalls, toasts, changed } = await mountList();
  await find(ui.root, (n) => n.tag === 'button' && n.textContent === 'Select').fire('click');
  await find(ui.root, (n) => n.tag === 'button' && n.textContent === 'Select all').fire('click');
  assert.equal(find(ui.root, (n) => n.className === 'pp-select-count').textContent, '3 selected');
  await find(ui.root, (n) => n.tag === 'button' && n.textContent === 'Complete').fire('click');
  assert.equal(JSON.stringify(bulkCalls), JSON.stringify([['toggle', ['a', 'b', 'c'], { done: true }]]));
  assert.deepEqual(toasts.map(([text, kind]) => [text, kind]), [['3 tasks completed', 'ok']]);
  const checks = findAll(ui.root, (n) => n.tag === 'input' && n.className === 'pp-select-check');
  assert.ok(checks.every((check) => check.hidden === true));
  assert.equal(changed(), 1);
});

test('failed items stay selected', async () => {
  const bulkResult = {
    total: 3,
    done: 2,
    failed: 1,
    message: '2 done, 1 failed',
    items: [{ id: 'a', ok: true }, { id: 'b', ok: true }, { id: 'c', ok: false, error: { code: 'not-found' } }],
  };
  const { ui, toasts } = await mountList({ bulkResult });
  await find(ui.root, (n) => n.tag === 'button' && n.textContent === 'Select').fire('click');
  await find(ui.root, (n) => n.tag === 'button' && n.textContent === 'Select all').fire('click');
  await find(ui.root, (n) => n.tag === 'button' && n.textContent === 'Complete').fire('click');
  assert.deepEqual(toasts.map(([text, kind]) => [text, kind]), [['2 done, 1 failed', 'error']]);
  assert.equal(find(ui.root, (n) => n.className === 'pp-select-list').dataset.selecting, 'true');
  assert.equal(find(ui.root, (n) => n.className === 'pp-select-count').textContent, '1 selected');
  assert.equal(find(ui.root, (n) => n.tag === 'li' && n.dataset.ppId === 'c').dataset.ppSelected, 'true');
});

test('undo runs the undo op on the items that worked', async () => {
  const template = taskTemplate();
  template.body.select.actions[1].undo = { op: 'restore', success: 'Restored {{count}} tasks' };
  const { ui, bulkCalls, toasts } = await mountList({
    template,
    window: { terrariumConfirm: async () => true },
    bulkResult: (op) => {
      if (op === 'delete') {
        return {
          total: 3,
          done: 2,
          failed: 1,
          message: '2 done, 1 failed',
          items: [{ id: 'a', ok: true }, { id: 'b', ok: true }, { id: 'c', ok: false }],
        };
      }
      return {
        total: 2,
        done: 2,
        failed: 0,
        message: '2 done',
        items: [{ id: 'a', ok: true }, { id: 'b', ok: true }],
      };
    },
  });
  await find(ui.root, (n) => n.tag === 'button' && n.textContent === 'Select').fire('click');
  await find(ui.root, (n) => n.tag === 'button' && n.textContent === 'Select all').fire('click');
  await find(ui.root, (n) => n.tag === 'button' && n.textContent === 'Delete').fire('click');
  assert.equal(toasts[0][2].label, 'Undo');
  const ok = await toasts[0][2].run();
  assert.equal(ok, true);
  assert.equal(bulkCalls[1][0], 'restore');
  assert.deepEqual(bulkCalls[1][1], ['a', 'b']);
  assert.deepEqual({ ...bulkCalls[1][2] }, {});
  assert.deepEqual(toasts[1], ['Restored 2 tasks', 'ok', undefined]);
});

test('no undo, no button', async () => {
  const { ui, toasts } = await mountList();
  await find(ui.root, (n) => n.tag === 'button' && n.textContent === 'Select').fire('click');
  await find(ui.root, (n) => n.tag === 'button' && n.textContent === 'Select all').fire('click');
  await find(ui.root, (n) => n.tag === 'button' && n.textContent === 'Complete').fire('click');
  assert.equal(toasts[0][2], null);
});

test('failed undo keeps the button', async () => {
  const template = taskTemplate();
  template.body.select.actions[1].undo = { op: 'restore', success: 'Restored {{count}} tasks' };
  const { ui, toasts } = await mountList({
    template,
    window: { terrariumConfirm: async () => true },
    bulkResult: (op) => {
      if (op === 'delete') {
        return {
          total: 3,
          done: 2,
          failed: 1,
          message: '2 done, 1 failed',
          items: [{ id: 'a', ok: true }, { id: 'b', ok: true }, { id: 'c', ok: false }],
        };
      }
      throw new Error('restore failed');
    },
  });
  await find(ui.root, (n) => n.tag === 'button' && n.textContent === 'Select').fire('click');
  await find(ui.root, (n) => n.tag === 'button' && n.textContent === 'Select all').fire('click');
  await find(ui.root, (n) => n.tag === 'button' && n.textContent === 'Delete').fire('click');
  const ok = await toasts[0][2].run();
  assert.equal(ok, false);
  assert.deepEqual(toasts[1], ['Error: restore failed', 'error', undefined]);
});

test('confirm is asked with the count', async () => {
  const asked = [];
  const { ui, bulkCalls } = await mountList({
    window: {
      terrariumConfirm: async (question) => {
        asked.push(question);
        return false;
      },
    },
  });
  await find(ui.root, (n) => n.tag === 'button' && n.textContent === 'Select').fire('click');
  await find(ui.root, (n) => n.tag === 'li' && n.dataset.ppId === 'a').fire('click');
  await find(ui.root, (n) => n.tag === 'li' && n.dataset.ppId === 'b').fire('click');
  await find(ui.root, (n) => n.tag === 'button' && n.textContent === 'Delete').fire('click');
  assert.deepEqual(asked, ['Move 2 tasks to the trash?']);
  assert.equal(bulkCalls.length, 0);
});

test('over 500 disables actions', async () => {
  const rows = Array.from({ length: 501 }, (_, i) => ({ id: `row-${i}`, title: `Row ${i}` }));
  const { ui } = await mountList({ rows });
  await find(ui.root, (n) => n.tag === 'button' && n.textContent === 'Select').fire('click');
  await find(ui.root, (n) => n.tag === 'button' && n.textContent === 'Select all').fire('click');
  assert.equal(find(ui.root, (n) => n.className === 'pp-select-note').textContent, 'Select up to 500 items at a time.');
  assert.equal(find(ui.root, (n) => n.tag === 'button' && n.textContent === 'Complete').disabled, true);
  assert.equal(find(ui.root, (n) => n.tag === 'button' && n.textContent === 'Delete').disabled, true);
});

test('lists without select are unchanged', async () => {
  const ui = rendererHarness();
  const view = ui.renderer.mount(ui.root, boardTemplate(), {
    today: () => '2026-11-11',
    query: async () => [{ title: 'First', lane: 'todo' }],
    bulk: async () => ({ total: 1, done: 1, failed: 0, message: 'ok', items: [] }),
  });
  await view.ready;
  assert.equal(ui.root.children[0].tag, 'ul');
});

test('no bulk in bridge, no bar', async () => {
  const { ui } = await mountList({ withBulk: false });
  assert.equal(ui.root.children[0].tag, 'ul');
  assert.equal(find(ui.root, (n) => n.className === 'pp-select-bar'), null);
});

test('cmd click starts select mode', async () => {
  const { ui, actionCalls } = await mountList();
  const b = find(ui.root, (n) => n.tag === 'li' && n.dataset.ppId === 'b');
  await b.fire('click', { metaKey: true });
  assert.equal(find(ui.root, (n) => n.className === 'pp-select-list').dataset.selecting, 'true');
  assert.equal(b.dataset.ppSelected, 'true');
  assert.equal(find(ui.root, (n) => n.className === 'pp-select-count').textContent, '1 selected');
  assert.equal(actionCalls.length, 0);
});

test('shift click selects a range', async () => {
  const { ui } = await mountList();
  const a = find(ui.root, (n) => n.tag === 'li' && n.dataset.ppId === 'a');
  const c = find(ui.root, (n) => n.tag === 'li' && n.dataset.ppId === 'c');
  await a.fire('click', { metaKey: true });
  await c.fire('click', { shiftKey: true });
  assert.equal(find(ui.root, (n) => n.className === 'pp-select-count').textContent, '3 selected');
});

test('escape leaves select mode', async () => {
  const { ui } = await mountList();
  await find(ui.root, (n) => n.tag === 'button' && n.textContent === 'Select').fire('click');
  await find(ui.root, (n) => n.tag === 'li' && n.dataset.ppId === 'a').fire('click');
  const wrap = find(ui.root, (n) => n.className === 'pp-select-list');
  await wrap.fire('keydown', { key: 'Escape' });
  assert.equal(wrap.dataset.selecting, 'false');
  assert.equal(find(ui.root, (n) => n.className === 'pp-select-count'), null);
  const checks = findAll(ui.root, (n) => n.tag === 'input' && n.className === 'pp-select-check');
  assert.ok(checks.every((check) => check.hidden === true));
});

test('cmd A selects all only in select mode', async () => {
  const { ui } = await mountList();
  const wrap = find(ui.root, (n) => n.className === 'pp-select-list');
  await wrap.fire('keydown', { key: 'a', metaKey: true });
  assert.equal(find(ui.root, (n) => n.className === 'pp-select-count'), null);
  await find(ui.root, (n) => n.tag === 'button' && n.textContent === 'Select').fire('click');
  await wrap.fire('keydown', { key: 'a', metaKey: true });
  assert.equal(find(ui.root, (n) => n.className === 'pp-select-count').textContent, '3 selected');
});

test('long press selects on touch', async () => {
  const { ui } = await mountList();
  const b = find(ui.root, (n) => n.tag === 'li' && n.dataset.ppId === 'b');
  await b.fire('pointerdown', { pointerType: 'touch', clientX: 5, clientY: 5 });
  await new Promise((resolve) => setTimeout(resolve, 550));
  assert.equal(find(ui.root, (n) => n.className === 'pp-select-list').dataset.selecting, 'true');
  assert.equal(b.dataset.ppSelected, 'true');
  await b.fire('click');
  assert.equal(b.dataset.ppSelected, 'true');
});

test('moving cancels long press', async () => {
  const { ui } = await mountList();
  const b = find(ui.root, (n) => n.tag === 'li' && n.dataset.ppId === 'b');
  await b.fire('pointerdown', { pointerType: 'touch', clientX: 5, clientY: 5 });
  await b.fire('pointermove', { clientX: 40, clientY: 5 });
  await new Promise((resolve) => setTimeout(resolve, 550));
  assert.equal(find(ui.root, (n) => n.className === 'pp-select-list').dataset.selecting, 'false');
  assert.equal(find(ui.root, (n) => n.className === 'pp-select-count'), null);
});

test('ask passes the value', async () => {
  const asks = [];
  const template = taskTemplate();
  template.body.select.actions = [
    { label: 'Add tag', op: 'add_tag', ask: { name: 'tag', label: 'Tag' } },
  ];
  const withValue = await mountList({
    template,
    window: {
      terrariumAsk: async (...args) => {
        asks.push(args);
        return 'work';
      },
    },
  });
  await find(withValue.ui.root, (n) => n.tag === 'button' && n.textContent === 'Select').fire('click');
  await find(withValue.ui.root, (n) => n.tag === 'button' && n.textContent === 'Select all').fire('click');
  await find(withValue.ui.root, (n) => n.tag === 'button' && n.textContent === 'Add tag').fire('click');
  assert.equal(asks[0][0], undefined);
  assert.equal(asks[0][1].label, 'Tag');
  assert.equal(asks[0][1].confirm, 'Add tag');
  assert.equal(withValue.bulkCalls.length, 1);
  assert.equal(withValue.bulkCalls[0][0], 'add_tag');
  assert.equal(withValue.bulkCalls[0][1].join(','), 'a,b,c');
  assert.equal(withValue.bulkCalls[0][2].tag, 'work');

  const withoutValue = await mountList({
    template,
    window: { terrariumAsk: async () => null },
  });
  await find(withoutValue.ui.root, (n) => n.tag === 'button' && n.textContent === 'Select').fire('click');
  await find(withoutValue.ui.root, (n) => n.tag === 'button' && n.textContent === 'Select all').fire('click');
  await find(withoutValue.ui.root, (n) => n.tag === 'button' && n.textContent === 'Add tag').fire('click');
  assert.equal(withoutValue.bulkCalls.length, 0);
});
