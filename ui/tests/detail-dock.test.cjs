// FIX-11c: docked item details, autosave on blur, remembered disclosures,
// selected rows, and the To-do task panel.
const { test } = require('node:test');
const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { resolve } = require('node:path');
const { rendererHarness } = require('./helpers/renderer-dom.cjs');
const dockApi = require('../detail-dock.js');
const today = require('../today.js');

const turn = () => new Promise(r => setImmediate(r));
const packages = process.env.TERRA_PLUGIN_SOURCE || resolve(__dirname, '../../../terrarium-plugins/packages');
const todoPage = JSON.parse(readFileSync(resolve(packages, 'tasks/ui/page-main.json'), 'utf8'));
const todoManifest = JSON.parse(readFileSync(resolve(packages, 'tasks/manifest.json'), 'utf8'));
const walk = (node, out = []) => {
  if (Array.isArray(node)) { for (const n of node) walk(n, out); return out; }
  if (!node || typeof node !== 'object') return out;
  out.push(node);
  for (const value of Object.values(node)) if (value && typeof value === 'object') walk(value, out);
  return out;
};
const flatten = node => [node, ...(node.children || []).filter(n => typeof n !== 'string').flatMap(flatten)];

// ---- Dock module ----------------------------------------------------------

test('detailTemplate keeps the page sheets only and needs an item sheet', () => {
  const template = { state: { open: '', q: '' }, entry: { bind: 'state.open' }, body: { type: 'stack', children: [
    { type: 'list', query: 'rows', item: { type: 'sheet', bind: 'state.row_sheet' } },
    { type: 'workspace', content: [{ type: 'sheet', bind: 'state.nested' }] },
    { type: 'sheet', bind: 'state.open', title: 'Task' },
    { type: 'sheet', bind: 'state.creating' },
  ] } };
  const detail = dockApi.detailTemplate(template);
  assert.deepEqual(detail.body.children.map(s => s.bind), ['state.nested', 'state.open', 'state.creating']);
  assert.deepEqual(detail.state, template.state);
  assert.equal(detail.entry, template.entry);
  assert.equal(dockApi.detailTemplate({ ...template, entry: { bind: 'state.missing' } }), null);
  assert.equal(dockApi.detailTemplate({ body: template.body }), null);
  const optOut = structuredClone(template); optOut.body.children[2].dock = false;
  assert.equal(dockApi.detailTemplate(optOut), null);
});

function fakeNode(tag = 'div') {
  const node = { tag, hidden: false, dataset: {}, children: [], parentNode: null, classes: new Set(),
    classList: { contains: c => node.classes.has(c) },
    append(child) { child.parentNode = node; node.children.push(child); },
    remove() { const p = node.parentNode; if (p) p.children = p.children.filter(c => c !== node); node.parentNode = null; } };
  return node;
}

test('the dock shows only the active view, sizes for large sheets and closes or reflows owners', () => {
  const container = fakeNode('aside');
  let listener = null;
  const media = { matches: true, addEventListener: (type, f) => { listener = f; } };
  const changes = [];
  const dock = dockApi.create(container, { media, room: () => 1400, onChange: open => changes.push(open) });
  assert.equal(dock.available(), true);
  dock.setActive('page');
  const page = dock.handle('page'), todayHandle = dock.handle('today');
  const a = fakeNode('dialog'), b = fakeNode('dialog');
  b.classes.add('pp-sheet-large');
  const calls = [];
  page.attach(a, { close: () => calls.push('close a'), reflow: () => calls.push('reflow a') });
  assert.equal(container.hidden, false);
  assert.equal(a.parentNode, container);
  assert.equal(page.active(a), true);
  todayHandle.attach(b, { close: () => calls.push('close b'), reflow: () => calls.push('reflow b') });
  assert.equal(b.hidden, true, 'another view keeps its sheet hidden');
  assert.equal(container.dataset.size, '');
  dock.setActive('today');
  assert.equal(a.hidden, true);
  assert.equal(b.hidden, false);
  assert.equal(container.dataset.size, 'large');
  assert.equal(dock.isOpen('today'), true);
  assert.equal(dock.close('today'), true);
  listener();
  assert.deepEqual(calls, ['close b', 'reflow a', 'reflow b']);
  // A second sheet from the same owner replaces the first.
  const c = fakeNode('dialog');
  page.attach(c, { close: () => calls.push('close c') });
  assert.equal(calls.at(-1), 'close a', 'the earlier page sheet is asked to close');
  todayHandle.detach(b);
  assert.equal(b.parentNode, null);
  assert.equal(container.hidden, true, 'nothing left for Today');
  dock.setActive(null);
  assert.equal(changes.at(-1), false);
  media.matches = false;
  assert.equal(dock.available(), false);
});

test('docking depends on the room beside the list, not the window width', () => {
  const { dockWidth, MIN_LIST } = dockApi;
  assert.equal(MIN_LIST, 560);
  assert.equal(dockWidth(1400, false), 360);
  assert.equal(dockWidth(920, false), 360, 'list keeps exactly 560px');
  assert.equal(dockWidth(919, false), 0, 'too narrow: overlay sheet');
  assert.equal(dockWidth(1600, true), 640);
  assert.equal(dockWidth(1300, true), 520, '40% of the room');
  assert.equal(dockWidth(1080, true), 480, 'smaller rooms get 480px');
  assert.equal(dockWidth(1040, true), 480);
  assert.equal(dockWidth(1039, true), 0, 'below 480px a large sheet falls back');
  assert.equal(dockWidth(0, false), 0);
  assert.equal(dockWidth(1400, 'panel'), 460, 'panel sheets (FIX-23) dock at a fixed 460px');
  assert.equal(dockWidth(1220, 'panel'), 460);
  assert.equal(dockWidth(1219, 'panel'), 0, 'a page with a sidebar keeps 760px for its list, else the panel overlays');
  assert.equal(dockWidth(1034, 'panel'), 0);
  // Rails opening shrink the room: open sheets reflow to the overlay and back.
  let room = 1400;
  const container = fakeNode('aside');
  container.style = { props: {}, setProperty(k, v) { this.props[k] = v; } };
  const media = { matches: true, addEventListener() {} };
  const dock = dockApi.create(container, { media, room: () => room, observe: false });
  const handle = dock.handle('page');
  dock.setActive('page');
  const sheet = fakeNode('dialog'); sheet.classes.add('pp-sheet-large');
  assert.equal(handle.available(sheet), true);
  assert.equal(handle.available(), true);
  handle.attach(sheet, {});
  assert.equal(container.style.props['--dock-width'], '560px');
  room = 1000;
  assert.equal(handle.available(sheet), false);
  assert.equal(handle.available(), true, 'a regular sheet still fits');
  room = 900;
  assert.equal(dock.available(), false);
  media.matches = false; room = 2000;
  assert.equal(dock.available(), false, 'narrow windows never dock');
});

// ---- Renderer: docked sheet --------------------------------------------------

function dockedHarness({ available = true, rows = ['a', 'b', 'c'], template = null, failSave = false } = {}) {
  const toasts = [];
  const keydown = [];
  const document = {
    createElement: tag => new (rendererHarness().root.constructor)(tag),
    createElementNS: (ns, tag) => new (rendererHarness().root.constructor)(tag),
    createDocumentFragment: () => new (rendererHarness().root.constructor)('fragment'),
    addEventListener: (type, f) => { if (type === 'keydown') keydown.push(f); },
    removeEventListener: (type, f) => { const i = keydown.indexOf(f); if (i >= 0) keydown.splice(i, 1); },
    querySelector: () => null,
  };
  const h = rendererHarness({ document });
  const events = [];
  const dock = {
    available: () => available,
    active: () => true,
    attach: (dialog, hooks) => { events.push(['attach', dialog]); dock.hooks = hooks; },
    detach: dialog => events.push(['detach', dialog]),
  };
  const actions = [];
  const view = h.renderer.mount(h.root, template || { state: { open: '' }, entry: { bind: 'state.open' }, body: { type: 'stack', children: [
    { type: 'list', query: 'rows', item: { type: 'item', title: '{{row.title}}', action: { set: { 'state.open': '{{row.id}}' } } } },
    { type: 'sheet', bind: 'state.open', title: '{{row.title}}', query: 'row', params: { id: '{{state.open}}' },
      menu: { label: 'Task actions', items: [{ label: 'Delete', variant: 'danger', action: { op: 'delete', args: { id: '{{row.id}}' }, then: { 'state.open': '' } } }] },
      children: [{ type: 'form', autosave: true, submit: { op: 'save', args: { id: '{{row.id}}', title: '{{form.title}}' } },
        children: [{ type: 'text-field', name: 'title', value: '{{row.title}}' }] }] },
  ] } }, {
    today: () => '2026-11-14', changed() {}, toast: (text, kind) => toasts.push([text, kind]),
    query: async (name, params) => name === 'rows' ? rows.map(id => ({ id, title: id.toUpperCase() })) : [{ id: params.id, title: params.id.toUpperCase() }],
    action: async (op, args) => { actions.push([op, args]); if (failSave && op === 'save') throw new Error('nope'); return {}; },
  }, { dock });
  return { ...h, view, dock, events, actions, keydown, toasts,
    dialog: () => flatten(h.root).find(n => n.tag === 'dialog'),
    press: async (key, target = {}) => { const e = { key, target, defaultPrevented: false, preventDefault() { e.defaultPrevented = true; } }; for (const f of [...keydown]) await f(e); await turn(); await turn(); return e; },
    mains: () => flatten(h.root).filter(n => n.dataset?.ppSelect !== undefined) };
}

test('an item sheet docks when the host offers a dock, without a modal or autofocus', async () => {
  const ui = dockedHarness();
  await ui.view.ready; await turn();
  const dialog = ui.dialog();
  let modal = 0; dialog.showModal = () => { modal++; dialog.open = true; };
  ui.view.setState('state.open', 'a'); await turn(); await turn();
  assert.equal(dialog.open, true);
  assert.equal(modal, 0, 'not modal');
  assert.deepEqual(ui.events.map(e => e[0]), ['attach']);
  assert.equal(ui.keydown.length, 1, 'listens for J/K and Escape while docked');
  assert.equal(flatten(dialog).some(n => n.focused), false, 'keyboard stays on the list');
  assert.equal(ui.view.docked.has(dialog), true);
  const menu = flatten(dialog).find(n => n.attrs?.['aria-label'] === 'Task actions');
  assert.ok(menu, 'the sheet head has a ⋯ menu');
  const selected = ui.mains().find(n => n.dataset.ppSelect === 'a');
  assert.equal(selected.attrs['aria-current'], 'true');
  assert.equal(ui.mains().find(n => n.dataset.ppSelect === 'b').attrs['aria-current'], undefined);
  ui.view.dispose();
  assert.equal(ui.events.at(-1)[0], 'detach');
  assert.equal(ui.keydown.length, 0);
});

test('J/K change the docked item after saving the current one; Escape closes', async () => {
  const ui = dockedHarness();
  await ui.view.ready; await turn();
  ui.view.setState('state.open', 'a'); await turn(); await turn();
  const input = flatten(ui.dialog()).find(n => n.dataset?.ppName === 'title');
  input.value = 'Edited A';
  input.parent.dispatchEvent({ type: 'input' });
  await ui.press('j');
  await turn();
  assert.equal(ui.view.state.open, 'b');
  assert.deepEqual(JSON.parse(JSON.stringify(ui.actions[0])), ['save', { id: 'a', title: 'Edited A' }], 'the previous item saved before switching');
  assert.equal(flatten(ui.dialog()).find(n => n.dataset?.ppName === 'title').value, 'B');
  await ui.press('k');
  assert.equal(ui.view.state.open, 'a');
  await ui.press('k');
  assert.equal(ui.view.state.open, 'a', 'stays on the first row');
  const typing = await ui.press('j', { tagName: 'INPUT' });
  assert.equal(typing.defaultPrevented, false, 'typing in a field is left alone');
  assert.equal(ui.view.state.open, 'a');
  const esc = await ui.press('Escape');
  assert.equal(esc.defaultPrevented, true);
  await turn();
  assert.equal(ui.view.state.open, '');
  assert.equal(ui.dialog().open, false);
  assert.equal(ui.events.at(-1)[0], 'detach');
  ui.view.dispose();
});

test('picking another row while the open item cannot save keeps it open and says so (FIX-23)', async () => {
  const ui = dockedHarness({ failSave: true });
  await ui.view.ready; await turn();
  ui.view.setState('state.open', 'a'); await turn(); await turn();
  const input = flatten(ui.dialog()).find(n => n.dataset?.ppName === 'title');
  input.value = 'Edited A';
  input.parent.dispatchEvent({ type: 'input' });
  ui.view.setState('state.open', 'b'); await turn(); await turn(); await turn();
  assert.equal(ui.view.state.open, 'a', 'stays on the item that did not save');
  assert.ok(ui.toasts.some(([text, kind]) => kind === 'error' && /stays open/.test(text)), 'the user is told why');
  ui.view.dispose();
});

test('a narrow list keeps titles: low-priority slots drop and the title keeps half the row (FIX-23)', () => {
  const css = readFileSync(resolve(__dirname, '../app.css'), 'utf8');
  assert.match(css, /@container \(max-width: 560px\) \{[^@]*\.pp-slot-tags, \.pp-item-calm \.pp-slot-list, \.pp-item-calm \.pp-slot-time \{ display: none; \}/);
  assert.match(css, /\.pp-item-calm \.pp-item-line-text \{ flex: 1 1 50%; min-width: 50%; \}/);
  assert.match(css, /@container \(max-width: 340px\)/);
});

test('a narrow window keeps the modal sheet and moves between modes on reflow', async () => {
  const ui = dockedHarness({ available: false });
  await ui.view.ready; await turn();
  const dialog = ui.dialog();
  let modal = 0; dialog.showModal = () => { modal++; dialog.open = true; };
  ui.view.setState('state.open', 'a'); await turn(); await turn();
  assert.equal(modal, 1);
  assert.equal(ui.events.length, 0);
  ui.view.dispose();
  const wide = dockedHarness({ available: true });
  await wide.view.ready; await turn();
  const d2 = wide.dialog();
  let modal2 = 0; d2.showModal = () => { modal2++; d2.open = true; };
  wide.view.setState('state.open', 'a'); await turn(); await turn();
  assert.equal(modal2, 0);
  wide.dock.available = () => false;
  wide.dock.hooks.reflow();
  assert.equal(modal2, 1, 'shrinking the window turns the docked sheet into the usual one');
  assert.equal(wide.events.at(-1)[0], 'detach');
  wide.view.dispose();
});

test('sheets without an entry bind, or without a dock, stay modal', async () => {
  const ui = dockedHarness({ template: { state: { creating: '' }, entry: { bind: 'state.open' }, body: {
    type: 'sheet', bind: 'state.creating', title: 'New', children: [{ type: 'text', text: 'x' }] } } });
  await ui.view.ready;
  const dialog = ui.dialog();
  let modal = 0; dialog.showModal = () => { modal++; dialog.open = true; };
  ui.view.setState('state.creating', 'yes'); await turn(); await turn();
  assert.equal(modal, 1);
  assert.equal(ui.events.length, 0);
  ui.view.dispose();
});

// ---- Renderer: autosave, remembered sections, clock ---------------------------

test('autosave saves on blur without waiting for the delay', async () => {
  const h = rendererHarness();
  const actions = [];
  const view = h.renderer.mount(h.root, { state: {}, body: { type: 'form', autosave: 5000,
    submit: { op: 'update', args: { id: 't', title: '{{form.title}}' } }, children: [{ type: 'text-field', name: 'title', value: 'Old' }] } },
  { today: () => '2026-11-14', changed() {}, toast() {}, query: async () => [], action: async (op, args) => { actions.push([op, args]); return {}; } });
  await view.ready;
  const form = h.root.querySelector('form');
  const input = flatten(form).find(n => n.dataset?.ppName === 'title');
  input.value = 'New';
  form.dispatchEvent({ type: 'input' });
  assert.equal(actions.length, 0);
  form.dispatchEvent({ type: 'focusout' });
  await turn();
  assert.deepEqual(JSON.parse(JSON.stringify(actions)), [['update', { id: 't', title: 'New' }]]);
  assert.equal(form.querySelector('.pp-save-status').textContent, 'Saved on this device');
  form.dispatchEvent({ type: 'focusout' });
  await turn();
  assert.equal(actions.length, 1, 'nothing pending, nothing saved');
  view.dispose();
});

test('autosave forms in a sheet report in the sheet header, not under their fields', async () => {
  const ui = dockedHarness();
  await ui.view.ready; await turn();
  ui.view.setState('state.open', 'a'); await turn(); await turn();
  const dialog = ui.dialog();
  const slot = dialog.querySelector('.pp-sheet-status');
  assert.ok(slot, 'the sheet head has one status slot');
  assert.equal(slot.textContent || '', '');
  const form = dialog.querySelector('form');
  const input = flatten(form).find(n => n.dataset?.ppName === 'title');
  input.value = 'Renamed';
  form.dispatchEvent({ type: 'input' });
  assert.equal(slot.textContent, 'Changes waiting to save…');
  assert.equal(await ui.view.flush(), true);
  assert.equal(slot.textContent, 'Saved on this device');
  // The form keeps its own (CSS-hidden unless an error) line for screen
  // layouts outside sheets.
  assert.equal(form.querySelector('.pp-save-status').textContent, 'Saved on this device');
  // Another item starts with an empty header status.
  ui.view.setState('state.open', 'b'); await turn(); await turn(); await turn();
  assert.equal(slot.textContent, '');
  ui.view.dispose();
});

test('a collapsible section with remember keeps its open state on this device', async () => {
  const store = new Map();
  const window = { terrariumStorage: { getItem: k => store.get(k) ?? null, setItem: (k, v) => store.set(k, v) } };
  const template = { state: {}, body: { type: 'section', title: 'More', collapsible: true, remember: 'task-more', children: [{ type: 'text', text: 'x' }] } };
  const bridge = { plugin: 'todo', today: () => '2026-11-14', query: async () => [], action: async () => ({}), changed() {}, toast() {} };
  let h = rendererHarness({ window });
  let view = h.renderer.mount(h.root, template, bridge);
  let details = h.root.children[0];
  assert.equal(details.tag, 'details');
  assert.equal(details.open, false);
  details.open = true;
  details.dispatchEvent({ type: 'toggle' });
  assert.equal(store.get('terrarium.section.todo.task-more'), 'open');
  view.dispose();
  h = rendererHarness({ window });
  view = h.renderer.mount(h.root, template, bridge);
  assert.equal(h.root.children[0].open, true, 'reopens expanded');
  view.dispose();
  const broken = rendererHarness({ window: { terrariumStorage: { getItem() { throw Error('denied'); }, setItem() { throw Error('denied'); } } } });
  const v3 = broken.renderer.mount(broken.root, template, bridge);
  broken.root.children[0].dispatchEvent({ type: 'toggle' });
  assert.equal(broken.root.children[0].open, false);
  v3.dispose();
});

test('templates can read the clock as $now', async () => {
  const h = rendererHarness();
  const view = h.renderer.mount(h.root, { state: {}, body: { type: 'text', text: 'x' } }, { today: () => '2026-11-14', query: async () => [], action: async () => ({}) });
  const before = Date.now();
  assert.ok(view.scope().$now >= before && view.scope().$now <= Date.now());
  view.dispose();
});

// ---- Today ----------------------------------------------------------------

function todayHarness(extra = {}) {
  const nodes = [];
  class Node {
    constructor(tag) { this.tag = tag; this.children = []; this.attrs = {}; this.dataset = {}; this.listeners = {}; this.parent = null; this.value = ''; this.tabIndex = -1; this.classes = new Set(); nodes.push(this);
      this.classList = { toggle: (c, on) => (on ? this.classes.add(c) : this.classes.delete(c)) }; }
    set className(v) { this.attrs.class = v; } get className() { return this.attrs.class || ''; }
    set textContent(v) { this.children = [String(v)]; } get textContent() { return this.children.map(c => typeof c === 'string' ? c : c.textContent).join(''); }
    get hidden() { return 'hidden' in this.attrs; } set hidden(v) { if (v) this.attrs.hidden = ''; else delete this.attrs.hidden; }
    get disabled() { return 'disabled' in this.attrs; } set disabled(v) { if (v) this.attrs.disabled = ''; else delete this.attrs.disabled; }
    setAttribute(k, v) { this.attrs[k] = String(v); } getAttribute(k) { return this.attrs[k] ?? null; } removeAttribute(k) { delete this.attrs[k]; }
    append(...ns) { for (const n of ns) { if (n == null) continue; if (typeof n === 'object') n.parent = this; this.children.push(n); } }
    replaceChildren(...ns) { this.children = []; this.append(...ns); }
    addEventListener(t, f) { (this.listeners[t] ||= []).push(f); }
    focus() { for (let n = this; n; n = n.parent) for (const f of n.listeners.focusin || []) f({ target: this }); }
    contains(n) { for (let x = n; x; x = x.parent) if (x === this) return true; return false; }
    all() { return this.children.filter(c => typeof c === 'object').flatMap(c => [c, ...c.all()]); }
    querySelectorAll(sel) { return this.all().filter(n => sel === '.today-row' ? n.className.split(' ').includes('today-row') : n.tag === sel); }
    querySelector(sel) { return this.querySelectorAll(sel)[0] || null; }
  }
  const el = (tag, attrs = {}, ...children) => {
    const n = new Node(tag);
    for (const [k, v] of Object.entries(attrs)) {
      if (k.startsWith('on')) n.addEventListener(k.slice(2), v);
      else if (k === 'hidden' || k === 'disabled') { if (v) n.attrs[k] = ''; }
      else if (v !== false && v != null) n.setAttribute(k, v);
    }
    for (const c of children) if (c != null && c !== false) n.append(typeof c === 'object' ? c : String(c));
    return n;
  };
  const opened = [];
  let shown = null, closed = 0;
  const plugin = { id: 'todo', name: 'To-do', requires: ['today-v1'], operations: ['toggle', 'update'], today: [{
    id: 'my-day', section: 'my_day', query: 'today_my_day', params: {}, limit: 20,
    row: { key: '{{row.id}}', kind: 'task', title: '{{row.title}}', open: { navigate: { item: '{{row.id}}' } } } }] };
  const storage = new Map();
  const deps = {
    el, icon: () => el('svg'), toast() {}, plugins: () => [plugin], space: () => 'personal', visible: () => true, now: () => new Date(2026, 10, 14, 12).getTime(),
    storage: { getItem: k => storage.get(k) ?? null, setItem: (k, v) => storage.set(k, v) },
    query: async () => [{ id: 't1', title: 'One' }, { id: 't2', title: 'Two' }], action: async () => ({}),
    open: async (pluginId, target) => { opened.push([pluginId, target]); shown = { plugin: pluginId, item: target.item }; view.selected(); },
    openExternal() {}, changed() {},
    detail: () => shown, closeDetail: () => { closed++; shown = null; view.selected(); },
    ...extra,
  };
  const root = el('section');
  const view = today.mount(root, deps);
  return { view, root, opened, get closed() { return closed; } };
}

test('Today marks the docked row, J/K show the next record and Escape closes it', async () => {
  const h = todayHarness();
  await h.view.refresh();
  const rows = h.root.querySelectorAll('.today-row');
  assert.equal(rows.length, 2);
  h.view.handle('next');
  h.view.handle('open');
  await turn();
  assert.deepEqual(h.opened, [['todo', { item: 't1' }]]);
  assert.equal(rows[0].classes.has('is-selected'), true);
  assert.equal(rows[0].attrs['aria-current'], 'true');
  h.view.handle('next');
  await turn();
  assert.deepEqual(h.opened.at(-1), ['todo', { item: 't2' }], 'J opens the next row beside Today');
  assert.equal(rows[0].classes.has('is-selected'), false);
  assert.equal(rows[1].classes.has('is-selected'), true);
  assert.equal(h.view.handle('escape'), true);
  assert.equal(h.closed, 1);
  assert.equal(rows[1].classes.has('is-selected'), false);
  h.view.handle('prev');
  await turn();
  assert.equal(h.opened.length, 2, 'without a docked record J/K only move focus');
  assert.equal(h.view.handle('escape'), false);
  h.view.dispose();
});

// ---- To-do task panel and saved views -------------------------------------

test('the To-do task panel autosaves, shows the main fields first and keeps the rest under More', () => {
  const sheet = todoPage.body.children.find(n => n.type === 'sheet' && n.bind === todoPage.entry.bind);
  assert.ok(sheet, 'the item sheet is the page entry');
  const nodes = walk(sheet.children);
  const forms = nodes.filter(n => n.type === 'form');
  assert.ok(forms.every(f => f.autosave === true || f.inline), 'every field form autosaves (steps add inline)');
  assert.equal(nodes.some(n => n.submit_label === 'Save'), false);
  const more = sheet.children.find(n => n.type === 'section' && n.title === 'More');
  assert.equal(more.collapsible, true);
  assert.equal(more.remember, 'task-more');
  const named = list => walk(list).filter(n => n.name).map(n => n.name);
  const before = sheet.children.slice(0, sheet.children.indexOf(more));
  // FIX-23: header (title), properties (list, status, plan, due, estimate, priority, tags), notes, then the collapsible sections: Subtasks, Steps, Time (Add time form), Fields.
  assert.deepEqual(named(before).filter(n => n !== 'text'), ['title', 'list_id', 'status_id', 'planned_for', 'due', 'minutes', 'priority', 'tags', 'notes', 'title', 'title', 'duration', 'date', 'note', 'value', 'value', 'value', 'value', 'value', 'value']);
  const part = title => n => (n.title || '').replace('{{row.subtasks_title}}', 'Subtasks') === title;
  const order = ['Subtasks', 'Steps', 'Time', 'Fields', 'Dependencies'].map(title => before.findIndex(part(title)));
  assert.ok(order.every((at, i) => at >= 0 && (i === 0 || at > order[i - 1])), 'Subtasks, Steps, Time, Fields, Dependencies in that order');
  assert.ok(['Subtasks', 'Steps', 'Time', 'Fields', 'Dependencies'].every(title => before.find(part(title)).collapsible === true), 'each part folds');
  assert.equal(sheet.size, 'panel');
  assert.deepEqual(named(more.children), ['repeat', 'remind_time', 'urgent', 'important', 'in_progress', 'group_id', 'sort_order']);
  assert.ok(walk(more.children).some(n => n.type === 'backlinks'));
  assert.ok(walk(more.children).some(n => n.action?.history));
  // In progress saves on its own so other edits never reset a custom status.
  const progress = forms.find(f => f.children.some(c => c.name === 'in_progress'));
  assert.deepEqual(Object.keys(progress.submit.args), ['id', 'in_progress']);
  // Delete lives in the panel's ⋯ menu.
  // The only other Delete is "Move to trash" in the banner an archived task shows (FIX-19).
  assert.deepEqual(nodes.filter(n => n.action?.op === 'delete').map(n => n.label), ['Move to trash']);
  const remove = sheet.menu.items.find(i => i.label === 'Delete');
  assert.equal(remove.action.op, 'delete');
  assert.equal(remove.action.undo.op, 'restore');
  // Mark done is the round check in the header (the title form's neighbour).
  const check = nodes.find(n => n.type === 'checkbox' && n.action?.op === 'toggle');
  assert.equal(check.style, 'round');
  assert.equal(check.if, '{{!row.archived}}');
  // A subtask's "Parent: …" link (a live `task` list) comes first; then the header, List/Status,
  // Plan/Due/Estimate, Priority/Tags and Notes sit in one 10px stack (no sheet gap between them).
  assert.equal(sheet.children[0].query, 'task');
  const fields = sheet.children[1];
  assert.equal(fields.type, 'stack');
  assert.equal(fields.gap, 10);
  assert.deepEqual(fields.children.map(c => c.type), ['stack', 'list', 'stack', 'form', 'form']);
  const title = nodes.find(n => n.name === 'title' && n.style === 'title');
  assert.equal(title.label_hidden, true);
  assert.equal(title.required, true);
  assert.equal(nodes.find(n => n.name === 'notes').placeholder, 'Add notes');
  // Empty autosave status lines take no row; in a sheet the header shows the status.
  const css = readFileSync(resolve(__dirname, '../app.css'), 'utf8');
  assert.match(css, /\.pp-form > \.pp-save-status:empty, \.pp-sheet \.pp-form > \.pp-save-status:not\(\.pp-tone-danger\) \{ display: none; \}/);
});

test('To-do lists pass the clock, undated tasks drop the due chip, and saved views keep Kanban/Eisenhower', () => {
  const lists = walk(todoPage.body).filter(n => n.type === 'list' && n.query === 'workspace_tasks');
  assert.equal(lists.length, 3);
  for (const list of lists) assert.equal(list.params.now, '{{$now}}');
  // `now` keeps its position; newer optional params (field_values) follow it.
  assert.equal(todoManifest.queries.workspace_tasks.params.indexOf('now'), 14);
  assert.match(todoManifest.queries.workspace_tasks.sql, /WHEN due IS NULL THEN NULL WHEN d = 0/);
  for (const id of ['my-day', 'due']) assert.equal(todoManifest.today.find(s => s.id === id).params.now, '{{$now}}');
  const draft = walk(todoPage.body).find(n => n.type === 'view-library' && n.id === 'new-view').draft;
  // Keys are the page's own layout values (state.view), so both map.
  assert.deepEqual(draft.layouts, { kanban: { layout: 'board', group: 'board_lane' }, matrix: { layout: 'board', group: 'matrix_lane' } });
  for (const key of Object.keys(draft.layouts)) assert.ok(walk(todoPage.body).some(n => n.if === `{{state.view == ${key}}}`), key);
  const columns = todoManifest.queries.workspace_tasks.view.columns;
  const keys = columns.map(c => c.key);
  assert.ok(keys.includes('board_lane') && keys.includes('matrix_lane'));
  // Board-by-Quadrant reopens as the 2×2 matrix; Stage stays a Kanban board.
  const views = require('../views.js');
  const entry = { query: 'workspace_tasks', view: todoManifest.queries.workspace_tasks.view };
  assert.equal(views.template(entry, { layout: 'board', group: 'matrix_lane' }).body.layout, 'matrix');
  assert.equal(views.template(entry, { layout: 'board', group: 'board_lane' }).body.layout, 'board');
  assert.equal(columns.find(c => c.key === 'matrix_lane').board, 'matrix');
  // include_archived (FIX-19) is appended after `now`.
  assert.equal(todoManifest.queries.tasks.params.indexOf('now'), 4, 'the Home widget query takes the clock');
  assert.equal(todoManifest.queries.tasks.params.at(-1), 'include_archived');
  const widget = JSON.parse(readFileSync(resolve(packages, 'tasks/ui/widget-today.json'), 'utf8'));
  assert.equal(widget.body.params.now, '{{$now}}');
});
