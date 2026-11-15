const { test } = require('node:test');
const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { resolve, join } = require('node:path');
const { rendererHarness } = require('./helpers/renderer-dom.cjs');

const domNodes = (root, predicate, out = []) => {
  if (predicate(root)) out.push(root);
  for (const child of root.children || []) if (typeof child !== 'string') domNodes(child, predicate, out);
  return out;
};
const text = node => typeof node === 'string' ? node : (node.textContent || '') + (node.children || []).map(text).join('');
const hasClass = (node, name) => String(node.className || '').split(/\s+/).includes(name);
const byClass = (root, name) => domNodes(root, node => hasClass(node, name));
const turn = () => new Promise(done => setImmediate(done));

function memoryStorage(seed = {}) {
  const data = new Map(Object.entries(seed));
  return { data, getItem: key => data.has(key) ? data.get(key) : null, setItem: (key, value) => data.set(key, String(value)) };
}

function harness({ mac = true, wide = true, storage = memoryStorage() } = {}) {
  const ui = rendererHarness({
    window: { terrariumStorage: storage },
    navigator: { platform: mac ? 'MacIntel' : 'Win32', userAgent: '' },
    matchMedia: () => ({ matches: wide }),
  });
  return { ...ui, storage };
}

const bridge = (extra = {}) => ({
  plugin: 'demo', today: () => '2026-11-14', toast() {}, changed() {},
  query: async (name) => name === 'lists' ? [{ id: 'a', name: 'Work', color: '#0f766e', open: 4 }, { id: 'b', name: 'errands', color: '', open: 0 }] : [],
  action: async () => ({}),
  ...extra,
});

function page(content, state = { collapsed: false }) {
  return { state: { ...state, nav: 'inbox' }, body: {
    type: 'workspace', label: 'Views', collapsible: true, collapsed: 'state.collapsed',
    navigation: { type: 'stack', children: [
      { type: 'nav', label: 'Smart', items: [
        { label: 'Inbox', icon: 'inbox', count: 3, tooltip: 'Capture here.', selected: '{{state.nav == inbox}}', action: { set: { 'state.nav': 'inbox' } } },
        { label: 'Later', icon: 'calendar', count: 0, selected: '{{state.nav == later}}', action: { set: { 'state.nav': 'later' } } },
      ] },
      { type: 'nav', title: 'Lists', query: 'lists', item: { label: '{{row.name}}', color: '{{row.color}}', count: '{{row.open}}', key: '{{row.id}}' } },
    ] },
    content,
  } };
}
const headContent = { type: 'stack', gap: 12, children: [
  { type: 'stack', direction: 'row', align: 'center', children: [{ type: 'heading', level: 2, text: 'Inbox' }] },
  { type: 'text', text: 'rows' },
] };

async function mount(ui, template, options) {
  const view = ui.renderer.mount(ui.root, template, bridge(), options);
  await view.ready;
  await turn();
  return { view, node: byClass(ui.root, 'pp-workspace')[0] };
}

test('the sidebar toggle starts the content head row and names itself with the shortcut', async () => {
  const ui = harness();
  const { view, node } = await mount(ui, page(headContent));
  const collapse = byClass(node, 'pp-workspace-collapse');
  assert.equal(collapse.length, 1, 'one toggle, not a floating corner button too');
  const head = byClass(node, 'pp-workspace-content')[0].children[1].children[0];
  assert.equal(head.children[0], collapse[0], 'first in the head row, left of the title');
  assert.equal(head.children[1].tag, 'h2');
  assert.equal(byClass(node, 'pp-workspace-nav')[0].children.some(child => hasClass(child, 'pp-workspace-collapse')), false);
  assert.equal(node.dataset.collapsePlace, 'head');
  assert.equal(collapse[0].attrs['aria-label'], 'Hide sidebar');
  assert.equal(collapse[0].title, 'Hide sidebar (⌘\\)');
  assert.equal(collapse[0].attrs['aria-expanded'], 'true');
  await collapse[0].fire('click');
  assert.equal(node.dataset.collapsed, 'true');
  assert.equal(view.state.collapsed, true);
  assert.equal(collapse[0].attrs['aria-label'], 'Show sidebar');
  assert.equal(collapse[0].title, 'Show sidebar (⌘\\)');
  assert.equal(collapse[0].attrs['aria-expanded'], 'false');
  view.dispose();

  const other = harness({ mac: false });
  const plain = await mount(other, page({ type: 'text', text: 'content' }));
  const content = byClass(plain.node, 'pp-workspace-content')[0];
  assert.equal(plain.node.dataset.collapsePlace, 'float');
  assert.deepEqual(content.children.map(child => child.className.split(' ').at(-1)), ['pp-workspace-toggle', 'pp-text', 'pp-workspace-collapse']);
  assert.equal(byClass(content, 'pp-workspace-collapse')[0].title, 'Hide sidebar (Ctrl+\\)');
  plain.view.dispose();
});

test('collapsed navigation entries carry icon, dot or letter glyphs, badges and rail tooltips', async () => {
  const ui = harness();
  const { view, node } = await mount(ui, page(headContent));
  const items = byClass(node, 'pp-nav-item');
  assert.deepEqual(items.map(item => item.dataset.ppGlyph), ['icon', 'icon', 'dot', 'letter']);
  assert.equal(text(byClass(items[3], 'pp-nav-letter')[0]), 'E');
  assert.equal(byClass(items[2], 'pp-nav-letter').length, 0);
  assert.equal(byClass(items[2], 'pp-nav-dot')[0].style['--pp-dot'], '#0f766e');
  assert.deepEqual(items.map(item => byClass(item, 'pp-nav-count').map(text).join('')), ['3', '', '4', '']);
  assert.equal(items[0].attrs['aria-current'], 'page');
  assert.equal(items[0].title, 'Capture here.');
  await byClass(node, 'pp-workspace-collapse')[0].fire('click');
  assert.deepEqual(items.map(item => item.title), ['Inbox (3)', 'Later', 'Work (4)', 'errands']);
  await items[1].fire('click');
  assert.equal(view.state.nav, 'later');
  assert.equal(items[1].attrs['aria-current'], 'page');
  await byClass(node, 'pp-workspace-collapse')[0].fire('click');
  assert.equal(items[0].title, 'Capture here.');
  assert.equal(items[1].attrs.title, undefined);
  view.dispose();
});

test('the collapsed choice is kept per device and page, and wins over the template default', async () => {
  const storage = memoryStorage();
  const first = harness({ storage });
  const a = await mount(first, page(headContent), { location: 'personal/page/demo/main' });
  assert.equal(a.node.dataset.collapsed, 'false');
  await byClass(a.node, 'pp-workspace-collapse')[0].fire('click');
  assert.equal(storage.data.get('terrarium.workspace.personal/page/demo/main.0'), 'collapsed');
  a.view.dispose();

  const again = harness({ storage });
  const b = await mount(again, page(headContent), { location: 'personal/page/demo/main' });
  assert.equal(b.node.dataset.collapsed, 'true');
  assert.equal(b.view.state.collapsed, true, 'the state binding follows the stored choice');
  b.view.setState('state.collapsed', false);
  assert.equal(b.node.dataset.collapsed, 'false', 'plugin state still drives the sidebar');
  assert.equal(storage.data.get('terrarium.workspace.personal/page/demo/main.0'), 'expanded');
  b.view.dispose();

  const elsewhere = harness({ storage });
  const c = await mount(elsewhere, page(headContent, { collapsed: true }), { location: 'work/page/demo/main' });
  assert.equal(c.node.dataset.collapsed, 'true', 'another space falls back to the binding default');
  c.view.dispose();
});

test('⌘\\ / Ctrl+\\ toggles the page sidebar, except while typing or below 900px', async () => {
  const ui = harness();
  const { view, node } = await mount(ui, page(headContent));
  const shortcut = ui.renderer.sidebarShortcut;
  const key = (extra) => ({ key: '\\', code: 'Backslash', metaKey: true, ctrlKey: false, target: { tagName: 'BODY' }, ...extra });
  assert.equal(shortcut(key(), view), true);
  assert.equal(node.dataset.collapsed, 'true');
  for (const target of [{ tagName: 'INPUT' }, { tagName: 'TEXTAREA' }, { tagName: 'DIV', isContentEditable: true }]) {
    assert.equal(shortcut(key({ target }), view), false, target.tagName);
  }
  assert.equal(shortcut(key({ metaKey: false, ctrlKey: true }), view), false, 'Ctrl+\\ is not the macOS shortcut');
  assert.equal(shortcut(key({ key: 'k' , code: 'KeyK' }), view), false);
  assert.equal(node.dataset.collapsed, 'true');
  assert.equal(shortcut(key(), view), true);
  assert.equal(node.dataset.collapsed, 'false');
  view.dispose();
  assert.equal(shortcut(key(), view), false, 'a disposed page ignores it');

  const pc = harness({ mac: false });
  const other = await mount(pc, page(headContent));
  assert.equal(pc.renderer.sidebarShortcut(key({ metaKey: false, ctrlKey: true }), other.view), true);
  assert.equal(other.node.dataset.collapsed, 'true');
  other.view.dispose();

  const app = readFileSync(join(__dirname, '../app.js'), 'utf8');
  assert.match(app, /currentView === "plugin"[^\n]*\n\s*&& window\.TerrariumRenderer\.sidebarShortcut\(e, views\.page\)/);
  assert.match(app, /location: locationKey\(pluginId, pageId\)/);

  const none = harness();
  const flat = await mount(none, { body: { type: 'workspace', navigation: { type: 'text', text: 'n' }, content: { type: 'text', text: 'c' } } });
  assert.equal(none.renderer.sidebarShortcut(key(), flat.view), false, 'not collapsible: nothing to toggle');
  flat.view.dispose();
});

test('below 900px the View menu holds the navigation and the sidebar toggle is hidden', async () => {
  const ui = harness({ wide: false });
  const { view, node } = await mount(ui, page(headContent));
  assert.equal(ui.renderer.sidebarShortcut({ key: '\\', metaKey: true, target: {} }, view), false);
  assert.equal(node.dataset.collapsed, 'false');
  const toggle = byClass(node, 'pp-workspace-toggle')[0];
  await toggle.fire('click');
  assert.equal(node.dataset.navOpen, 'true');
  assert.equal(toggle.attrs['aria-expanded'], 'true');
  view.dispose();

  const css = readFileSync(join(__dirname, '../app.css'), 'utf8');
  const block = (query) => {
    const start = css.indexOf(`@media (${query}) {`, css.indexOf('.pp-workspace { position: relative;'));
    assert.ok(start >= 0, query);
    return css.slice(start, css.indexOf('\n}\n', start));
  };
  const narrow = block('max-width: 899px');
  assert.match(narrow, /\.pp-workspace \.pp-workspace-collapse \{ display: none; \}/);
  assert.match(narrow, /\.pp-workspace \.pp-workspace-toggle \{ display: inline-flex; \}/);
  assert.match(narrow, /\.pp-workspace\[data-nav-open="true"\] > \.pp-workspace-nav \{ display: flex; position: absolute;/);
  const wide = block('min-width: 900px');
  assert.match(wide, /\.pp-workspace\[data-collapsed="true"\] \{ grid-template-columns: 44px minmax\(0, 880px\);/);
  // The rail lives only in the desktop block; nothing outside it hides the nav.
  const outside = css.replace(wide, '');
  assert.doesNotMatch(outside, /data-collapsed="true"\][^{]*\{[^}]*display: none/);
  assert.doesNotMatch(css, /margin-bottom: -32px/);
  assert.match(css, /@media \(prefers-reduced-motion: reduce\) \{\n  \.pp-workspace \{ transition: none; \}/);
});

test('To-do puts the sidebar toggle before its view title', async () => {
  const packages = process.env.TERRA_PLUGIN_SOURCE || resolve(__dirname, '../../../terrarium-plugins/packages');
  const template = JSON.parse(readFileSync(resolve(packages, 'tasks/ui/page-main.json'), 'utf8'));
  const ui = harness();
  const view = ui.renderer.mount(ui.root, template, bridge({ plugin: 'todo', listViews: async () => [] }), { hideTitle: true, location: 'personal/page/todo/main' });
  await view.ready;
  const collapse = byClass(ui.root, 'pp-workspace-collapse')[0];
  assert.equal(collapse.parent.children[0], collapse);
  assert.ok(byClass(collapse.parent, 'pp-heading-title').length, 'the head row holds the title');
  view.dispose();
});
