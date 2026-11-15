// FIX-14: folders pin to the left rail and open in place there.
const { test } = require('node:test');
const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const vm = require('node:vm');
const pins = require('../pins.js');

class Node {
  constructor(tag) { this.tag = tag; this.value = ''; this.children = []; this.attrs = {}; this.dataset = {}; this.listeners = {}; this.classes = new Set(); this.classList = { add: c => this.classes.add(c), toggle: (c, on) => on ? this.classes.add(c) : this.classes.delete(c) }; }
  set className(value) { this.classes = new Set(String(value).split(/\s+/).filter(Boolean)); }
  get className() { return [...this.classes].join(' '); }
  append(...nodes) { for (const node of nodes) { if (node && typeof node === 'object') node.parent = this; this.children.push(node); } }
  replaceChildren(...nodes) { this.children = []; this.append(...nodes); }
  setAttribute(name, value) {
    this.attrs[name] = String(value);
    if (name.startsWith('data-')) this.dataset[name.slice(5).replace(/-([a-z])/g, (_, c) => c.toUpperCase())] = String(value);
  }
  getAttribute(name) { return this.attrs[name] ?? null; }
  removeAttribute(name) { delete this.attrs[name]; }
  addEventListener(type, fn) { (this.listeners[type] ||= []).push(fn); }
  click(event = {}) { for (const fn of this.listeners.click || []) fn({ detail: 1, ...event }); }
  focus() { this.focused = true; }
  get textContent() { return this.children.map(c => typeof c === 'string' ? c : c.textContent).join(''); }
  set textContent(value) { this.children = [String(value)]; }
  get tagName() { return this.tag.toUpperCase(); }
  get options() { return this.children.filter(c => c.tag === 'option'); }
  get childNodes() { return this.children; }
  prepend(...nodes) { for (const node of nodes.reverse()) { node.parent = this; this.children.unshift(node); } }
  querySelector() { return null; }
}
const all = root => [root, ...root.children.filter(c => c && typeof c === 'object').flatMap(all)];
const has = (node, name) => node.classes.has(name);

function el(tag, attrs = {}, ...children) {
  const node = new Node(tag);
  for (const [key, value] of Object.entries(attrs)) {
    if (key === 'class') node.className = value;
    else if (key.startsWith('on')) node.addEventListener(key.slice(2), value);
    else if (value === true) node.setAttribute(key, '');
    else if (value !== false && value != null) node.setAttribute(key, value);
  }
  for (const child of children) if (child != null && child !== false) node.append(child);
  return node;
}

function load(document) {
  const window = { TerrariumPins: pins };
  vm.runInNewContext(readFileSync(require.resolve('../pages.js'), 'utf8'), { window, document });
  return window.TerrariumPages;
}

function makePages({ sidebar = 'expanded', stored = {} } = {}) {
  const document = { documentElement: { dataset: { sidebar } }, activeElement: null };
  const Pages = load(document);
  const p = Object.create(Pages.prototype);
  const storage = new Map(Object.entries(stored));
  const calls = [];
  const nodes = { 'page-nav': new Node('div') };
  Object.assign(p, {
    el, space: 'a', pinStates: new Map(), currentId: null,
    pinStorage: { getItem: k => storage.has(k) ? storage.get(k) : null, setItem: (k, v) => storage.set(k, v) },
    pages: [
      { id: 'p1', title: 'Reading list', folder_id: 'fbooks', position: 1 },
      { id: 'p2', title: 'Novels', folder_id: 'fnested', position: 0 },
      { id: 'p3', title: 'Loose page' },
    ],
    folders: [
      { id: 'fbooks', title: 'Books', parent_id: null },
      { id: 'fnested', title: 'Fiction', parent_id: 'fbooks' },
      { id: 'fempty', title: 'Empty', parent_id: null },
    ],
    plugins: [{ id: 'todo', name: 'Tasks', pages: [{ id: 'main' }] }],
    narrow: { matches: false },
    $: id => nodes[id] || null,
    o: { icon: name => el('svg', { 'data-icon': name }), toast: m => calls.push(['toast', m]), show: id => calls.push(['page', id]), openApp: a => calls.push(['app', a.id]), onPinsChange() {} },
    browsePages: options => calls.push(['library', options]),
    managePins: () => calls.push(['manage']),
  });
  return { p, calls, storage, nav: nodes['page-nav'], document };
}
const toggles = nav => all(nav).filter(n => n.dataset.folderPin || n.dataset.folder);
const pageRows = nav => all(nav).filter(n => n.dataset.userPage);

test('pins accept and normalize folder pins with real folder ids', () => {
  const state = pins.normalize({ pins: [{ kind: 'folder', id: 'fk3m9xq2pz', extra: 1 }, { kind: 'folder', id: 'fk3m9xq2pz' }, { kind: 'folder', id: 'bad id' }, { kind: 'shelf', id: 'x' }] });
  assert.deepEqual(state.pins, [{ kind: 'folder', id: 'fk3m9xq2pz' }]);
  assert.equal(pins.key({ kind: 'folder', id: 'f1' }), 'folder:f1');
  const toggled = pins.toggle(pins.normalize(null), { kind: 'folder', id: 'f1' });
  assert.deepEqual(toggled.pins, [{ kind: 'folder', id: 'f1' }]);
  assert.deepEqual(pins.toggle(toggled, { kind: 'folder', id: 'f1' }).pins, []);
});

test('the page library pins a folder to the rail as {kind:"folder"}', async () => {
  const window = {};
  vm.runInNewContext(readFileSync(require.resolve('../page-library.js'), 'utf8'), { window, document: { createElement: tag => new Node(tag) } });
  const { p } = makePages();
  const host = new Node('root');
  const library = window.TerrariumPageLibrary.mount(host, {
    space: 'a',
    service: async name => name === 'folders_list' ? [{ id: 'fbooks', space: 'a', title: 'Books' }] : [],
    isPinned: id => p.isPinned({ kind: 'folder', id: String(id) }),
    onTogglePin: id => p.togglePin({ kind: 'folder', id: String(id) }),
  });
  await library.ready;
  const find = () => all(host).find(n => has(n, 'pl-folder-pin'));
  assert.equal(find().textContent, 'Pin to rail');
  assert.equal(find().attrs['aria-pressed'], 'false');
  find().click();
  assert.deepEqual(p.pins().pins, [{ kind: 'folder', id: 'fbooks' }]);
  assert.equal(find().textContent, 'Unpin from rail');
  assert.equal(find().attrs['aria-pressed'], 'true');
  find().click();
  assert.deepEqual(p.pins().pins, []);
});

test('the library opened at a folder expands its ancestors and marks it', async () => {
  const window = {};
  vm.runInNewContext(readFileSync(require.resolve('../page-library.js'), 'utf8'), { window, document: { createElement: tag => new Node(tag) } });
  const host = new Node('root');
  const library = window.TerrariumPageLibrary.mount(host, {
    space: 'a', focusFolder: 'fnested',
    service: async name => name === 'folders_list' ? [{ id: 'fbooks', space: 'a', title: 'Books' }, { id: 'fnested', space: 'a', title: 'Fiction', parent_id: 'fbooks' }] : [],
  });
  await library.ready;
  const folders = all(host).filter(n => has(n, 'pl-folder'));
  assert.equal(folders.length, 2);
  assert.ok(folders.every(n => n.open));
  assert.ok(has(folders.find(n => n.children[0].textContent === 'Fiction'), 'pl-folder-focus'));
  assert.ok(!has(folders.find(n => n.children[0].textContent === 'Books'), 'pl-folder-focus'));
});

test('the rail renders a pinned folder with its pages and subfolders and toggles aria-expanded', () => {
  const { p, nav } = makePages();
  p.togglePin({ kind: 'folder', id: 'fbooks' });
  let [toggle] = toggles(nav);
  assert.equal(toggle.dataset.folderPin, 'fbooks');
  assert.equal(toggle.attrs['aria-expanded'], 'false');
  assert.ok(toggle.textContent.includes('Books'));
  const list = all(nav).find(n => n.attrs.id === toggle.attrs['aria-controls']);
  assert.ok('hidden' in list.attrs);
  toggle.click();
  [toggle] = toggles(nav);
  assert.equal(toggle.attrs['aria-expanded'], 'true');
  const children = all(nav).find(n => n.attrs.id === toggle.attrs['aria-controls']);
  assert.ok(!('hidden' in children.attrs));
  assert.deepEqual(pageRows(nav).map(n => n.dataset.userPage), ['p1']);
  assert.ok(pageRows(nav).every(n => has(n, 'nav-child') && has(n, 'nav-item')));
  const sub = toggles(nav).find(n => n.dataset.folder === 'fnested');
  assert.equal(sub.attrs['aria-expanded'], 'false');
  sub.click();
  assert.deepEqual(pageRows(nav).map(n => n.dataset.userPage), ['p2', 'p1']);
  toggles(nav)[0].click();
  assert.equal(toggles(nav)[0].attrs['aria-expanded'], 'false');
  assert.deepEqual(pageRows(nav), []);
});

test('an empty folder shows "No pages" and a missing folder stays out of the rail', () => {
  const { p, nav } = makePages({ stored: { 'pp-rail-folders:a': '["fempty"]' } });
  p.savePins(pins.normalize({ pins: [{ kind: 'folder', id: 'fempty' }, { kind: 'folder', id: 'fgone' }, { kind: 'page', id: 'p3' }] }));
  assert.deepEqual(toggles(nav).map(n => n.dataset.folderPin), ['fempty']);
  assert.ok(all(nav).some(n => has(n, 'nav-folder-empty') && n.textContent === 'No pages'));
  assert.deepEqual(pageRows(nav).map(n => n.dataset.userPage), ['p3']);
  p.folders[2].archived = true;
  p.navigation();
  assert.deepEqual(toggles(nav), []);
});

test('open folders persist per space on the device and survive unavailable storage', () => {
  const { p, storage, nav } = makePages();
  p.togglePin({ kind: 'folder', id: 'fbooks' });
  toggles(nav)[0].click();
  assert.deepEqual(JSON.parse(storage.get('pp-rail-folders:a')), ['fbooks']);
  const again = makePages({ stored: { 'pp-rail-folders:a': storage.get('pp-rail-folders:a'), 'pp-pins:a': storage.get('pp-pins:a') } });
  again.p.navigation();
  assert.equal(toggles(again.nav)[0].attrs['aria-expanded'], 'true');
  const broken = makePages();
  broken.p.pinStorage = { getItem() { throw Error('no'); }, setItem() { throw Error('no'); } };
  broken.p.o.toast = () => {};
  broken.p.togglePin({ kind: 'folder', id: 'fbooks' });
  toggles(broken.nav)[0].click();
  assert.equal(toggles(broken.nav)[0].attrs['aria-expanded'], 'true');
});

test('the current page auto-expands its pinned folder chain and marks the page', () => {
  const { p, nav } = makePages();
  p.togglePin({ kind: 'folder', id: 'fbooks' });
  p.currentId = 'p2';
  p.navigation();
  assert.equal(toggles(nav).find(n => n.dataset.folderPin === 'fbooks').attrs['aria-expanded'], 'true');
  assert.equal(toggles(nav).find(n => n.dataset.folder === 'fnested').attrs['aria-expanded'], 'true');
  assert.ok(pageRows(nav).some(n => n.dataset.userPage === 'p2'));
  // A later collapse sticks while the same page stays current.
  toggles(nav)[0].click();
  p.navigation();
  assert.equal(toggles(nav)[0].attrs['aria-expanded'], 'false');
  // markNav marks rail rows by data-user-page, so children share it.
  const app = readFileSync(require.resolve('../app.js'), 'utf8');
  assert.match(app, /item\.dataset\.userPage\) active = currentView === "home" && pages\?\.currentId === item\.dataset\.userPage/);
});

test('⌘ shortcut on a folder pin toggles it, and the collapsed rail opens the library at it', () => {
  const { p, nav, calls, document } = makePages();
  p.savePins(pins.normalize({ pins: [{ kind: 'page', id: 'p3' }, { kind: 'folder', id: 'fbooks' }] }));
  const pin = p.pins().pins[1];
  p.openPin(pin);
  assert.equal(toggles(nav)[0].attrs['aria-expanded'], 'true');
  p.openPin(pin);
  assert.equal(toggles(nav)[0].attrs['aria-expanded'], 'false');
  p.openPin(p.pins().pins[0]);
  assert.deepEqual(calls.at(-1), ['page', 'p3']);
  document.documentElement.dataset.sidebar = 'fixed';
  p.openPin(pin);
  assert.equal(JSON.stringify(calls.at(-1)), JSON.stringify(['library', { folder: 'fbooks' }]));
  toggles(nav)[0].click();
  assert.equal(JSON.stringify(calls.at(-1)), JSON.stringify(['library', { folder: 'fbooks' }]));
  assert.equal(toggles(nav)[0].attrs['aria-expanded'], 'false');
  p.folders = [];
  p.openPin(pin);
  assert.deepEqual(calls.at(-1), ['manage']);
});

test('the pin manager lists folders with their path, and the rail menu and styles cover folder rows', () => {
  const { p } = makePages();
  const content = new Node('div');
  const dialog = new Node('dialog'); dialog.showModal = () => {};
  delete p.managePins;
  p.$ = id => ({ 'page-pins-content': content, 'page-pins-dialog': dialog, 'page-nav': new Node('div') })[id];
  p.managePins();
  const text = all(content).map(n => n.textContent);
  assert.ok(text.includes('Available pages, folders and apps'));
  assert.ok(text.includes('Pin Books / Fiction (folder)'));
  all(content).find(n => n.textContent === 'Pin Books / Fiction (folder)').click();
  assert.deepEqual(p.pins().pins, [{ kind: 'folder', id: 'fnested' }]);
  assert.ok(all(content).some(n => n.textContent === 'Books / Fiction'));
  const app = readFileSync(require.resolve('../app.js'), 'utf8');
  assert.match(app, /#page-nav \[data-folder-pin\]/);
  assert.match(app, /kind: "folder", id: target\.dataset\.folderPin/);
  const css = readFileSync(require.resolve('../app.css'), 'utf8');
  assert.match(css, /\.nav-folder-children \{[^}]*padding-left: 12px/);
  assert.match(css, /\.nav-folder-children\[hidden\] \{ display: none; \}/);
  assert.match(css, /:root\[data-sidebar="fixed"\] \.nav-folder-children \{ display: none; \}/);
});
