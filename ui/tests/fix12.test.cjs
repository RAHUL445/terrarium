// FIX-12: Today is the home; the old Home widget page is the Dashboard.
const { test } = require('node:test');
const assert = require('node:assert/strict');
const vm = require('node:vm');
const { readFileSync } = require('node:fs');
const { join } = require('node:path');
const pins = require('../pins.js');
const library = require('../page-library.js');
const today = require('../today.js');

const ui = (file) => readFileSync(join(__dirname, '..', file), 'utf8');
const window = { TerrariumPins: pins };
vm.runInNewContext(ui('pages.js'), { window });
const Pages = window.TerrariumPages;

function memory() {
  const data = new Map();
  return { data, getItem: (k) => (data.has(k) ? data.get(k) : null), setItem: (k, v) => data.set(k, String(v)) };
}

function pagesFor(space, storage, list) {
  const p = Object.create(Pages.prototype);
  Object.assign(p, { space, pinStates: new Map(), pinStorage: storage, pages: list.map(Pages.dashboard), o: { toast() {} }, navigation() {} });
  return p;
}

test('the app always starts on Today', () => {
  const app = ui('app.js');
  const body = app.match(/function startView\(\) \{([^}]*)\}/)[1];
  assert.match(body, /^\s*return "today";\s*$/);
  // Only opening a user page shows the page view; no fallback lands there.
  assert.equal(app.match(/showView\("home"\)/g).length, 1);
  assert.ok(/async function showUserPage\(id\) \{[^]*?showView\("home"\)/.test(app));
  assert.doesNotMatch(app, /open-on-start/);
  assert.equal(today.readStart, undefined);
  assert.equal(today.writeStart, undefined);
});

test('the rail and tab bar have one front door: Today', () => {
  const html = ui('index.html');
  assert.doesNotMatch(html, /data-view="home"/);
  assert.doesNotMatch(html, /open-on-start/);
  assert.match(html, /class="nav-item" data-view="today"[^>]*title="Home \(⌘1 \/ Ctrl\+1\)"[^>]*><svg class="icon" aria-hidden="true" data-icon="home"><\/svg><span>Home<\/span>/);
  assert.match(html, /class="tab" data-view="today"/);
});

test('the system page is titled Dashboard unless the user renamed it', () => {
  assert.equal(Pages.dashboard({ id: 'h', system: true, title: 'Home' }).title, 'Dashboard');
  assert.equal(Pages.dashboard({ id: 'h', system: true, title: '' }).title, 'Dashboard');
  assert.equal(Pages.dashboard({ id: 'h', system: true, title: 'Desk' }).title, 'Desk');
  assert.equal(Pages.dashboard({ id: 'h', system: true, title: 'Home' }).icon, 'grid');
  assert.equal(Pages.dashboard({ id: 'p', title: 'Home' }).title, 'Home', 'ordinary pages keep their title');
  assert.equal(library.shownPage({ id: 'h', system: true, title: 'Home' }).title, 'Dashboard');
  const p = Object.create(Pages.prototype);
  p.page = Pages.dashboard({ id: 'h', system: true, title: 'Home' });
  assert.equal(p.title(), 'Dashboard');
  assert.equal(p.isDashboard(), true);
  p.page = null;
  assert.equal(p.title(), 'Dashboard');
});

test('the Dashboard is pinned once per space and a later unpin sticks', () => {
  const storage = memory();
  const list = [{ id: 'other', title: 'Other' }, { id: 'home1', system: true, title: 'Home' }];
  const p = pagesFor('a', storage, list);
  assert.equal(p.pinDashboardOnce(), true);
  assert.equal(p.isPinned({ kind: 'page', id: 'home1' }), true);
  assert.equal(JSON.parse(storage.getItem('pp-pins:a')).pins.length, 1);
  p.togglePin({ kind: 'page', id: 'home1' });
  assert.equal(p.isPinned({ kind: 'page', id: 'home1' }), false);
  // A later load on this device (fresh state) keeps the unpin.
  const again = pagesFor('a', storage, list);
  assert.equal(again.pinDashboardOnce(), false);
  assert.equal(again.isPinned({ kind: 'page', id: 'home1' }), false);
  // Another space gets its own one-time pin.
  const other = pagesFor('b', storage, [{ id: 'home2', system: true, title: 'Home' }]);
  assert.equal(other.pinDashboardOnce(), true);
  assert.equal(other.isPinned({ kind: 'page', id: 'home2' }), true);
});

test('an already pinned Dashboard is not pinned twice and other pins keep their order', () => {
  const storage = memory();
  storage.setItem('pp-pins:a', JSON.stringify({ version: 1, pins: [{ kind: 'app', id: 'todo' }, { kind: 'page', id: 'home1' }] }));
  const p = pagesFor('a', storage, [{ id: 'home1', system: true, title: 'Home' }]);
  p.pinDashboardOnce();
  assert.deepEqual(p.pins().pins, [{ kind: 'app', id: 'todo' }, { kind: 'page', id: 'home1' }]);
  assert.equal(storage.getItem('pp-dashboard-pinned:a'), '1');
});

test('user-facing labels say Dashboard, not Home', () => {
  const rails = ui('rails.js');
  assert.match(rails, /"Dashboard widget"/);
  assert.doesNotMatch(rails, /"Home widget"/);
  const todayJs = ui('today.js');
  assert.match(todayJs, /"Pin Dashboard"/);
  assert.match(todayJs, /"Edit Dashboard"/);
  // Phones have no sidebar, so Today's menu is how they reach the Dashboard.
  assert.match(todayJs, /label: "Open Dashboard"/);
  assert.match(ui('app.js'), /openDashboard: \(\) => \(pages\.home \? showUserPage\(pages\.home\.id\)/);
  assert.doesNotMatch(todayJs, /Home page/);
});
