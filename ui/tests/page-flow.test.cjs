const { test } = require('node:test');
const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { join } = require('node:path');
const vm = require('node:vm');
const grid = require('../grid.js');
const flow = require('../page-flow.js');

function prototype() {
  const window = { TerrariumPageFlow: flow };
  vm.runInNewContext(readFileSync(join(__dirname, '../pages.js'), 'utf8'), { window });
  return window.TerrariumPages.prototype;
}

const placed = (id, x, y, flow_order) => ({ id, plugin: 'todo', widget: 'view', x, y, w: 4, h: 3, flow_order, config: { query: id } });

test('flow ordering falls back to grid order and move returns new records without changing coordinates', () => {
  const input = [placed('c', 0, 4, null), placed('a', 4, 0, null), placed('b', 0, 0, null)];
  const before = structuredClone(input);
  assert.deepEqual(flow.order(input).map(p => p.id), ['b', 'a', 'c']);
  const moved = flow.move(input, 'c', -1);
  assert.deepEqual(flow.order(moved).map(p => p.id), ['b', 'c', 'a']);
  assert.deepEqual(moved.map(({ id, x, y }) => [id, x, y]), input.map(({ id, x, y }) => [id, x, y]));
  assert.deepEqual(input, before);
});

test('flow move uses explicit order first and leaves source records untouched', () => {
  const input = [placed('a', 0, 0, 2), placed('b', 4, 0, 0), placed('c', 0, 4, 1)];
  const moved = flow.move(input, 'c', -1);
  assert.deepEqual(flow.order(input).map(p => p.id), ['b', 'c', 'a']);
  assert.deepEqual(flow.order(moved).map(p => p.id), ['c', 'b', 'a']);
  assert.deepEqual(input.map(p => p.flow_order), [2, 0, 1]);
});

test('layout switches hint and handle instructions between flow and grid', () => {
  const page = Object.create(prototype());
  const placement = placed('one', 0, 0, 0);
  const handle = () => { const attrs = {}; return { attrs, setAttribute(name, value) { attrs[name] = value; }, title: '' }; };
  const move = handle(), resize = handle();
  const dims = { textContent: '' };
  const root = {
    classList: { toggle() {} }, style: {}, append() {},
  };
  const card = {
    parentNode: root, style: {}, dataset: {}, getAttribute: () => 'Tasks',
    querySelector(selector) { return ({ '.page-dims': dims, '.page-move': move, '.page-resize': resize, '.page-up': {}, '.page-down': {} })[selector]; },
  };
  const selector = { value: 'flow' }, hint = { hidden: false, textContent: '' }, empty = { hidden: true };
  page.draft = [placement]; page.placements = [placement]; page.cards = new Map([['one', card]]);
  page.grid = grid; page.page = { layout_mode: 'grid' }; page.narrow = { matches: false }; page.pack = () => {};
  page.$ = id => ({ widgets: root, 'page-layout-mode': selector, 'home-layout-hint': hint, 'home-layout-empty': empty }[id]);

  page.layout();
  assert.match(hint.textContent, /Move up\/Move down/);
  assert.doesNotMatch(hint.textContent, /Drag/);
  assert.match(move.attrs['aria-label'], /Move Tasks up or down/);
  assert.match(move.title, /↑\/↓/);
  assert.match(resize.attrs['aria-label'], /Resize Tasks is unavailable/);
  assert.match(resize.title, /grid layout/);

  selector.value = 'grid';
  page.layout();
  assert.match(hint.textContent, /Drag a widget/);
  assert.match(move.attrs['aria-label'], /Drag, or use arrow keys/);
  assert.equal(move.title, 'Drag to move · arrows move · Shift+arrows resize');
  assert.match(resize.attrs['aria-label'], /Resize Tasks\. Drag/);
  assert.equal(resize.title, 'Drag to resize');
});

test('duplicating a flow block makes a new draft ID, shares references by value, and reserves grid space', () => {
  const page = Object.create(prototype());
  const source = placed('saved-1', 0, 0, 0);
  source.config = { saved_view: 'shared-view', nested: { sort: 'newest' } };
  page.draft = [source]; page.placements = [source]; page.page = { layout_mode: 'flow' }; page.added = 0;
  page.$ = id => id === 'page-layout-mode' ? { value: 'flow' } : ({ append() {} });
  page.grid = grid; page.spec = () => ({ size: 'regular' });
  page.cards = new Map(); page.card = placement => { const card = { placement, classList: { add() {} }, scrollIntoView() {}, querySelector() { return null; } }; page.cards.set(placement.id, card); return card; };
  page.layout = () => {}; page.o = { toast() {} };
  assert.equal(page.duplicatePlacement(source.id), true);
  const copy = page.draft[1];
  assert.notEqual(copy.id, source.id);
  assert.match(copy.id, /^new-/);
  assert.notEqual(copy.config, source.config);
  assert.equal(JSON.stringify(copy.config), JSON.stringify(source.config));
  assert.equal(copy.config.saved_view, 'shared-view');
  assert.equal(copy.flow_order, 1);
  assert.notDeepEqual([copy.x, copy.y], [source.x, source.y]);
  assert.deepEqual([source.x, source.y], [0, 0]);
});

test('Cancel restores only blocks whose draft config or focus changed', () => {
  const page = Object.create(prototype());
  const saved = [placed('one', 0, 0, 0), placed('two', 4, 0, 1)];
  const changedDraft = saved.map(p => ({ ...p, config: { ...p.config } }));
  changedDraft[0].config.query = 'unsaved-query';
  page.placements = saved; page.draft = changedDraft; page.page = { layout_mode: 'grid' }; page.draftIcon = null;
  const selector = { value: 'flow' };
  page.$ = id => id === 'page-layout-mode' ? selector : ({ append() {} });
  page.cards = new Map(); page.views = new Map(); page.pending = new Map(); page.io = null;
  const disposed = [];
  for (const p of saved) {
    const old = { id: p.id, replaceWith(next) { this.replaced = next; } };
    page.cards.set(p.id, old);
    page.views.set(p.id, { dispose() { disposed.push(p.id); } });
  }
  page.card = p => { const card = { id: p.id, renderedQuery: p.config.query }; page.cards.set(p.id, card); return card; };
  page.drop = () => {};
  page.layout = () => {}; page.chrome = () => {};
  page.cancel();
  assert.equal(selector.value, 'grid');
  assert.equal(page.cards.get('one').renderedQuery, 'one');
  assert.equal(page.cards.get('two').id, 'two');
  assert.deepEqual(disposed, ['one']);
});

test('Done saves flow order and coordinates before requesting page_layout', async () => {
  const page = Object.create(prototype());
  page.page = { id: 'page-1', title: 'Page', focus: '', period: null, icon: null, layout_mode: 'grid', system: false };
  page.space = 'space-1'; page.draftIcon = null;
  page.draft = [placed('saved-1', 2, 5, 0), placed('new-1', 6, 8, 1)];
  const elements = {
    'page-layout-mode': { value: 'flow' }, 'page-title-input': { value: 'Page' }, 'page-focus-input': { value: '' },
  };
  page.$ = id => elements[id]; page.readPeriod = () => null; page.flush = async () => true;
  const calls = [];
  page.service = async (name, args) => { calls.push([name, args]); if (name === 'pages_list') return []; };
  page.render = async () => true; page.o = { toast() {} };
  await page.done();
  assert.deepEqual(calls.map(([name]) => name), ['page_save', 'page_layout', 'pages_list']);
  assert.equal(calls[1][1].page, 'page-1');
  assert.equal(calls[1][1].mode, 'flow');
  assert.deepEqual(calls[0][1].placements.map(p => [p.x, p.y, p.flow_order]), [[2, 5, 0], [6, 8, 1]]);
});

test('page_layout is not requested when placement saving fails', async () => {
  const page = Object.create(prototype());
  page.page = { id: 'page-1', title: 'Page', focus: '', period: null, icon: null, layout_mode: 'grid', system: false };
  page.space = 'space-1'; page.draftIcon = null; page.draft = [placed('saved-1', 2, 5, 0)];
  const elements = { 'page-layout-mode': { value: 'flow' }, 'page-title-input': { value: 'Page' }, 'page-focus-input': { value: '' } };
  page.$ = id => elements[id]; page.readPeriod = () => null; page.flush = async () => true;
  const calls = []; page.service = async (name) => { calls.push(name); if (name === 'page_save') throw new Error('save rejected'); };
  const errors = []; page.o = { toast: (_message, kind) => errors.push(kind) };
  await page.done();
  assert.deepEqual(calls, ['page_save']);
  assert.deepEqual(errors, ['error']);
  assert.notEqual(page.draft, null);
});
