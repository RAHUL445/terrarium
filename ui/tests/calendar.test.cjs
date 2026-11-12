const { test } = require('node:test');
const assert = require('node:assert/strict');
const { rendererHarness } = require('./helpers/renderer-dom.cjs');

const all = (node) => [node, ...node.children.flatMap(all)];
const byClass = (root, name) => all(root).filter((node) => (node.className || '').split(' ').includes(name));

function bridge(overrides = {}) {
  return {
    today: () => '2026-11-07',
    query: async () => [],
    action: async () => ({}),
    changed() {},
    toast() {},
    ...overrides,
  };
}

test('month grid starts on Monday and covers six weeks', () => {
  const ui = rendererHarness();
  const november = ui.renderer.monthGrid(2026, 11);
  assert.equal(november.length, 42);
  assert.equal(november[0].day, '2026-10-26');
  assert.equal(november[41].day, '2026-12-06');
  assert.equal(november.filter((entry) => entry.inMonth).length, 30);

  const february = ui.renderer.monthGrid(2026, 2);
  assert.equal(february[0].day, '2026-01-26');
});

test('dayKey accepts dates and date-times only', () => {
  const ui = rendererHarness();
  assert.equal(ui.renderer.dayKey('2026-11-07'), '2026-11-07');
  assert.equal(ui.renderer.dayKey('2026-11-07T10:00'), '2026-11-07');
  assert.equal(ui.renderer.dayKey(''), null);
  assert.equal(ui.renderer.dayKey(null), null);
  assert.equal(ui.renderer.dayKey('7 Nov'), null);
});

test('rows land on their day and params see the visible month', async () => {
  const ui = rendererHarness();
  const calls = [];
  const actions = [];
  const template = {
    state: {},
    body: {
      type: 'calendar',
      query: 'tasks',
      params: { from: '{{$month_start}}', to: '{{$month_end}}' },
      date: 'due',
      title: 'title',
      label: 'Tasks',
      action: { op: 'open', args: { id: '{{row.id}}' } },
    },
  };
  const view = ui.renderer.mount(ui.root, template, bridge({
    query: async (query, params) => {
      calls.push([query, params]);
      return [
        { id: 'a', title: 'First', due: '2026-11-03' },
        { id: 'b', title: 'Second', due: '2026-11-03T10:00' },
        { id: 'c', title: 'Third', due: '2026-11-30' },
        { id: 'z', title: 'Ignore', due: null },
      ];
    },
    action: async (op, args) => {
      actions.push([op, args]);
      return {};
    },
  }));
  await view.ready;

  assert.equal(calls[0][1].from, '2026-11-01');
  assert.equal(calls[0][1].to, '2026-11-30');
  const nov3 = byClass(ui.root, 'pp-cal-daybtn').find((node) => node.dataset.day === '2026-11-03');
  assert.ok(nov3.attrs['aria-label'].endsWith(', 2 items'));
  assert.equal(byClass(ui.root, 'pp-cal-entry').length, 3);

  await byClass(ui.root, 'pp-cal-entry')[0].fire('click');
  assert.equal(actions.length, 1);
  assert.equal(actions[0][0], 'open');
  assert.equal(actions[0][1].id, 'a');
  view.dispose();
});

test('more than three entries collapse into a more button', async () => {
  const ui = rendererHarness();
  const rows = ['a', 'b', 'c', 'd', 'e'].map((id) => ({ id, title: id, due: '2026-11-04' }));
  const view = ui.renderer.mount(ui.root, {
    state: {},
    body: { type: 'calendar', query: 'tasks', date: 'due', title: 'title', label: 'Tasks' },
  }, bridge({ query: async () => rows }));
  await view.ready;
  assert.equal(byClass(ui.root, 'pp-cal-entry').length, 3);
  const more = byClass(ui.root, 'pp-cal-more')[0];
  assert.equal(more.textContent, '+2 more');
  view.dispose();
});

test('next month re-queries', async () => {
  const ui = rendererHarness();
  const calls = [];
  const view = ui.renderer.mount(ui.root, {
    state: {},
    body: {
      type: 'calendar',
      query: 'tasks',
      params: { from: '{{$month_start}}', to: '{{$month_end}}' },
      date: 'due',
      title: 'title',
      label: 'Tasks',
    },
  }, bridge({
    query: async (_query, params) => {
      calls.push(params);
      return [];
    },
  }));
  await view.ready;
  const next = all(ui.root).find((node) => node.tag === 'button' && node.textContent === 'Next month');
  await next.fire('click');
  assert.equal(calls[1].from, '2026-12-01');
  assert.equal(calls[1].to, '2026-12-31');
  view.dispose();
});

test('selecting a day lists its items', async () => {
  const ui = rendererHarness();
  const view = ui.renderer.mount(ui.root, {
    state: {},
    body: { type: 'calendar', query: 'tasks', date: 'due', title: 'title', label: 'Tasks', action: { op: 'open', args: { id: '{{row.id}}' } } },
  }, bridge({
    query: async () => [
      { id: 'a', title: 'One', due: '2026-11-03' },
      { id: 'b', title: 'Two', due: '2026-11-03' },
    ],
  }));
  await view.ready;

  const third = byClass(ui.root, 'pp-cal-daybtn').find((node) => node.dataset.day === '2026-11-03');
  await third.fire('click');
  const heading = byClass(ui.root, 'pp-cal-list-heading')[0];
  assert.ok((heading.textContent || '').trim().length > 0);
  assert.equal(byClass(ui.root, 'pp-item-main').length, 2);

  const fifth = byClass(ui.root, 'pp-cal-daybtn').find((node) => node.dataset.day === '2026-11-05');
  await fifth.fire('click');
  const empty = byClass(ui.root, 'pp-empty').find((node) => node.textContent === 'Nothing on this day.');
  assert.ok(empty);
  view.dispose();
});
