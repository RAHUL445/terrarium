// FIX-22: nav cleanup — foldable groups, count tone, row menus that apply, rail dots.
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

const group = (extra = {}) => ({ type: 'nav', title: 'More', collapsible: true, collapsed: true,
  items: [{ label: 'Done', key: 'done', count: '3', count_tone: 'danger', action: {} }, { label: 'Trash', key: 'trash', action: {} }], ...extra });

test('a collapsible nav group folds from its title and the device remembers it', async () => {
  const store = new Map();
  const window = { terrariumStorage: { getItem: k => store.get(k) ?? null, setItem: (k, v) => store.set(k, v) } };
  {
    const ui = rendererHarness({ window });
    const view = ui.renderer.mount(ui.root, { body: group() }, bridge());
    await view.ready;
    const [fold] = byClass(ui.root, 'pp-nav-fold');
    const [nav] = byClass(ui.root, 'pp-nav');
    assert.equal(fold.attrs['aria-expanded'], 'false');
    assert.equal(nav.dataset.folded, 'true');
    await fold.fire('click');
    assert.equal(fold.attrs['aria-expanded'], 'true');
    assert.equal(nav.dataset.folded, 'false');
    view.dispose();
  }
  const ui = rendererHarness({ window });
  const view = ui.renderer.mount(ui.root, { body: group() }, bridge());
  await view.ready;
  assert.equal(byClass(ui.root, 'pp-nav')[0].dataset.folded, 'false', 'opened state survives a reload');
  view.dispose();
});

test('a plain nav title stays a label, and count_tone tints the count', async () => {
  const ui = rendererHarness();
  const view = ui.renderer.mount(ui.root, { body: group({ collapsible: false }) }, bridge());
  await view.ready;
  assert.equal(byClass(ui.root, 'pp-nav-fold').length, 0);
  assert.match(byClass(ui.root, 'pp-nav-count')[0].className, /pp-tone-danger/);
  view.dispose();
});

test('a row menu whose entries all fail `if` shows no button', async () => {
  const ui = rendererHarness();
  const spec = { type: 'nav', query: 'q', item: { label: '{{row.name}}', key: '{{row.id}}', action: {},
    menu: [{ label: 'Rename', if: '{{row.list_id}}', action: {} }] } };
  const rows = [{ id: 'inbox', name: 'Inbox', list_id: '' }, { id: 'a', name: 'Alpha', list_id: 'a' }];
  const view = ui.renderer.mount(ui.root, { body: spec }, bridge({ query: async () => rows }));
  await view.ready;
  assert.equal(byClass(ui.root, 'pp-nav-menu').length, 1);
  view.dispose();
});

test('the collapsed rail shows a dot instead of a pill over the icon; Collapse leaves Settings alone', () => {
  assert.match(css, /data-collapsed="true"\] \.pp-nav-count \{[^}]*width: 7px;[^}]*font-size: 0/);
  assert.match(css, /data-sidebar="expanded"\] \.sidebar > \.rail-collapse \{ position: static;/);
});

test('tasks nav: plain names, Inbox first in Lists, collapsible More, one count rule', () => {
  const page = JSON.parse(readFileSync(join(packages, 'tasks', 'ui', 'page-main.json'), 'utf8'));
  const deep = (v, out = []) => { if (v && typeof v === 'object') { out.push(v); Object.values(v).forEach(x => deep(x, out)); } return out; };
  const navs = deep(page.body).filter(n => n.type === 'nav');
  const labels = navs.flatMap(n => (n.items || []).map(i => i.label));
  assert.ok(labels.includes('Due') && labels.includes('Anytime'));
  assert.ok(!labels.includes('Due / overdue') && !labels.includes('Ready to pick'));
  const due = navs.flatMap(n => n.items || []).find(i => i.label === 'Due');
  assert.equal(due.count_tone, 'danger');
  const lists = navs.find(n => n.title === 'Lists');
  assert.equal(lists.query, 'nav_lists');
  assert.equal(lists.empty, undefined);
  const more = navs.find(n => n.title === 'More');
  assert.equal(more.collapsible, true);
  assert.deepEqual(more.items.map(i => i.label).slice(-2), ['Archive', 'Trash']);
});

test('nav row menu floats over the count so list counts line up with menu-less rows', () => {
  // `.pp-menu { position: relative }` comes later in app.css; the nav rule must out-rank it,
  // or the hidden ⋯ button stays in flow and pushes the count left (Work 12 vs Inbox 4).
  const rule = css.match(/^\.pp-nav-row > \.pp-menu\.pp-nav-menu \{([^}]*)\}/m);
  assert.ok(rule, 'nav menu rule must use .pp-nav-row > .pp-menu.pp-nav-menu');
  assert.match(rule[1], /position:\s*absolute/);
  assert.doesNotMatch(css, /^\.pp-nav-menu \{[^}]*position:\s*absolute/m);
});
