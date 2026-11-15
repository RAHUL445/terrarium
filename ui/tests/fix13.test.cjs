// FIX-13: lighter task rows — circle done control and a tidy meta line.
const { test } = require('node:test');
const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { join, resolve } = require('node:path');
const { rendererHarness } = require('./helpers/renderer-dom.cjs');

const css = readFileSync(join(__dirname, '..', 'app.css'), 'utf8');
const packages = process.env.TERRA_PLUGIN_SOURCE || resolve(__dirname, '../../../terrarium-plugins/packages');
const bridge = (extra = {}) => ({ today: () => '2026-11-12', query: async () => [], action: async () => ({}), changed() {}, toast() {}, ...extra });
const walk = (node, out = []) => {
  if (!node || typeof node === 'string') return out;
  out.push(node);
  for (const child of node.children || []) walk(child, out);
  return out;
};
const text = node => typeof node === 'string' ? node : (node.textContent || '') + (node.children || []).map(text).join('');
const byClass = (root, name) => walk(root).filter(node => String(node.className || '').split(/\s+/).includes(name));
const rule = (selector) => {
  const at = css.indexOf(`${selector} {`);
  assert.notEqual(at, -1, selector);
  return css.slice(at, css.indexOf('}', at));
};

test('the done control is a 16px ring in a 32px hit area, 44px on touch', () => {
  const base = rule('input.pp-item-check');
  assert.match(base, /appearance: none/);
  assert.match(base, /--pp-check-hit: 32px/);
  assert.match(base, /border-radius: 50%/);
  const ring = rule('input.pp-item-check::before');
  assert.match(ring, /width: 16px; height: 16px; border-radius: 50%/);
  assert.match(ring, /1\.5px solid var\(--check-color/);
  assert.match(css, /input\.pp-item-check:hover::after \{ opacity: \.55; \}/);
  assert.match(css, /input\.pp-item-check:focus-visible::after \{ opacity: \.55; \}/);
  assert.match(rule('input.pp-item-check:checked::before'), /background: var\(--check-color, var\(--accent-strong\)\)/);
  assert.match(rule('input.pp-item-check:checked::after'), /border-color: #fff; opacity: 1/);
  assert.match(css, /@media \(pointer: coarse\) \{ input\.pp-item-check \{ --pp-check-hit: 44px; \} \}/);
  const forced = css.slice(css.indexOf('@media (forced-colors: active) {\n  input.pp-item-check'));
  assert.match(forced, /appearance: auto/);
  assert.match(forced, /input\.pp-item-check::before, input\.pp-item-check::after \{ content: none; \}/);
  assert.doesNotMatch(css, /border-radius: 6px; background: var\(--surface\); cursor: pointer; \}/, 'the rounded square is gone');
});

test('Today uses the same ring, with a green ring for habits', () => {
  assert.match(rule('.today-check'), /border-radius: 50%/);
  assert.match(rule('.today-check'), /--today-hit: 32px/);
  assert.match(rule('.today-check::before'), /width: 16px; height: 16px; border-radius: 50%/);
  assert.match(rule('.today-check[aria-checked="true"]::after'), /border-color: #fff/);
  assert.match(rule('.today-ring'), /--today-ring: var\(--ok\)/);
  assert.match(css, /@media \(pointer: coarse\) \{ \.today-check \{ --today-hit: 44px; \} \}/);
  assert.doesNotMatch(css, /\.today-check \{[^}]*border-radius: 4px/);
});

test('item rows keep a real checkbox with the ring class', async () => {
  const ui = rendererHarness();
  const view = ui.renderer.mount(ui.root, { body: { type: 'item', layout: 'line', title: 'Focus Work', flag: 2,
    check: { checked: false, action: { op: 'toggle' } } } }, bridge());
  await view.ready;
  const [input] = byClass(ui.root, 'pp-item-check');
  assert.equal(input.tag, 'input');
  assert.equal(input.type, 'checkbox');
  assert.equal(input.attrs['aria-label'], 'Mark done: Focus Work');
  assert.ok(byClass(ui.root, 'pp-flag-2').length, 'priority colours the ring');
  view.dispose();
});

test('task dates use the UI font, not monospace', () => {
  const time = rule('.pp-meta-time');
  assert.doesNotMatch(time, /--mono/);
  assert.match(time, /tabular-nums/);
});

test('short-time chips drop :00 and join the date with a dot', async () => {
  const ui = rendererHarness();
  const view = ui.renderer.mount(ui.root, { body: { type: 'stack', children: ['14:00', '14:30', '10 Dec 14:00', 'Today 09:05', 'Tomorrow'].map(t => (
    { type: 'item', layout: 'line', title: 'x', chips: [{ text: t, style: 'time', format: 'short-time' }] })) } }, bridge());
  await view.ready;
  assert.deepEqual(byClass(ui.root, 'pp-meta-time').map(text), ['2pm', '2:30pm', '10 Dec · 2pm', 'Today · 9:05am', 'Tomorrow']);
  view.dispose();
});

test('an icon_only chip shows its icon and names itself', async () => {
  const ui = rendererHarness();
  const view = ui.renderer.mount(ui.root, { body: { type: 'item', layout: 'line', title: 'x',
    chips: [{ text: 'Monthly', icon: 'repeat', icon_only: true, label: 'Repeats monthly' }, { text: 'Daily', icon: 'repeat' }] } }, bridge());
  await view.ready;
  const [bare, full] = byClass(ui.root, 'pp-meta');
  assert.match(bare.className, /pp-meta-icon/);
  assert.equal(text(bare), '');
  assert.equal(bare.title, 'Repeats monthly');
  assert.equal(bare.attrs['aria-label'], 'Repeats monthly');
  assert.equal(bare.attrs.role, 'img');
  assert.equal(text(full), 'Daily');
  view.dispose();
});

test('a text chip with a label uses it as its tooltip, and the lock icon draws', async () => {
  const ui = rendererHarness();
  const view = ui.renderer.mount(ui.root, { body: { type: 'item', layout: 'line', title: 'x',
    chips: [{ text: 'Blocked', icon: 'lock', label: 'Waiting on Budget' }, { text: '2/5' }] } }, bridge());
  await view.ready;
  const [blocked, plain] = byClass(ui.root, 'pp-meta');
  assert.equal(text(blocked), 'Blocked');
  assert.equal(blocked.title, 'Waiting on Budget');
  const svg = blocked.children.find(c => c.tag === 'svg');
  assert.ok(svg, 'the lock icon is drawn');
  assert.match(svg.children[0].attrs.d, /^M6 11h12v10H6z/);
  assert.ok(!plain.title);
  view.dispose();
});

test('a chip equal to its group heading is hidden; ungrouped lists keep it', async () => {
  const rows = [
    { id: 1, title: 'Movie ticket', list_name: 'Inbox', g: 'Inbox' },
    { id: 2, title: 'Report', list_name: 'Work', g: 'Planned today' },
  ];
  const item = { type: 'item', layout: 'line', title: '{{row.title}}', chips: [{ text: '{{row.list_name}}', style: 'source' }] };
  const grouped = rendererHarness();
  const view = grouped.renderer.mount(grouped.root, { body: { type: 'list', query: 'tasks', group: 'g', item } }, bridge({ query: async () => rows }));
  await view.ready;
  assert.deepEqual(byClass(grouped.root, 'pp-group').map(text), ['Inbox', 'Planned today']);
  assert.deepEqual(byClass(grouped.root, 'pp-meta-source').map(text), ['Work']);
  view.dispose();

  const flat = rendererHarness();
  const plain = flat.renderer.mount(flat.root, { body: { type: 'list', query: 'tasks', item } }, bridge({ query: async () => rows }));
  await plain.ready;
  assert.deepEqual(byClass(flat.root, 'pp-meta-source').map(text), ['Inbox', 'Work']);
  plain.dispose();
});

test('To-do rows declare date first, an icon-only repeat, and tags last', () => {
  const template = JSON.parse(readFileSync(resolve(packages, 'tasks/ui/page-main.json'), 'utf8'));
  const items = [];
  const find = (node) => {
    if (!node || typeof node !== 'object') return;
    if (node.type === 'item' && node.layout === 'line' && node.chips) items.push(node);
    for (const value of Object.values(node)) Array.isArray(value) ? value.forEach(find) : find(value);
  };
  find(template.body);
  assert.ok(items.length >= 3);
  for (const item of items) {
    const fields = item.chips.map(c => c.field);
    if (item.slots) {
      // Calm rows: fixed columns, state icons, date, time, tags, then list.
      assert.deepEqual(item.slots.map(slot => slot.name), ['state', 'date', 'time', 'tags', 'list']);
      const slotOf = (field) => item.chips.find(c => c.field === field).slot;
      assert.equal(slotOf('due'), 'date');
      assert.equal(slotOf('repeat'), 'state');
      assert.equal(slotOf('tags'), 'tags');
      assert.equal(slotOf('list'), 'list');
      assert.equal(item.chips.find(c => c.field === 'due'), item.chips.filter(c => c.slot === 'date')[0]);
    } else {
      assert.equal(fields[0], 'due');
      assert.equal(fields.at(-1), 'tags');
      assert.ok(fields.indexOf('repeat') < fields.indexOf('tags'));
    }
    const due = item.chips.find(c => c.field === 'due');
    assert.equal(due.format, 'short-time');
    assert.equal(due.style, 'time');
    const repeat = item.chips.find(c => c.field === 'repeat');
    assert.equal(repeat.icon_only, true);
    assert.equal(repeat.icon, 'repeat');
    assert.match(repeat.label, /^Repeats /);
  }
});
