const test = require('node:test');
const assert = require('node:assert/strict');
const vm = require('node:vm');
const fs = require('node:fs');
const source = fs.readFileSync(require('node:path').join(__dirname, '../registry.js'), 'utf8');
function harness(listing, phone = false) {
  const nodes = new Map(), requests = [], badges = [{}];
  const $ = id => {
    if (!nodes.has(id)) nodes.set(id, { value: '', children: [], listeners: {}, addEventListener(t, fn) { this.listeners[t] = fn; }, replaceChildren(...c) { this.children = c; } });
    return nodes.get(id);
  };
  let reviewed, timer, at = 100;
  const window = {};
  vm.runInNewContext(source, { window, document: { querySelectorAll: s => s === '[data-registry-badge]' ? badges : [] },
    setTimeout: fn => { timer = fn; return 1; }, clearTimeout() {} });
  const view = window.TerrariumRegistry.create({ $, clock: () => at,
    el: (tag, attrs, ...children) => ({ tag, attrs, children }), icon: name => ({ tag: 'icon', name }),
    async call(command, args) { requests.push({ command, args: JSON.parse(JSON.stringify(args || {})) }); return command === 'registry_index' ? listing : { token: 'checked-review' }; },
    showReview: r => { reviewed = r; }, renderInstalled() {}, phone });
  return { view, $, nodes, requests, badges, reviewed: () => reviewed, expire() { at = 501; timer(); } };
}
const card = { id: 'notes', name: '<img src=x>', description: '<script>bad()</script>', publisher: 'aa-bb', icon: 'inbox', version: '0.10.0', capabilities: ['uses:tasks'], update: true, can_install: true, installed_version: '0.9.0' };
const listing = () => ({ plugins: [{ ...card }], configured: true, fresh: true, expires_ms: 500, automatic_checks: false });

test('empty configuration and phone have no refresh or stage request', async () => {
  const h = harness({ plugins: [], configured: false, fresh: false, message: 'No plugin list is set up' });
  await h.view.load();
  assert.equal(h.$('registry-message').textContent, 'No plugin list is set up');
  assert.equal(h.$('registry-check').disabled, true);
  await h.view.stage('notes', '0.10.0');
  assert.equal(h.requests.length, 1);
  assert.deepEqual(h.requests[0], { command: 'registry_index', args: { refresh: false } });
  const phone = harness(listing(), true); await phone.view.load(true); await phone.view.stage('notes', '0.10.0');
  assert.equal(phone.requests.length, 0);
  assert.equal(phone.$('registry-message').textContent, 'Plugin downloads are available on desktop.');
});

test('local search, safe text cards and manual-only Browse never fetch automatically', async () => {
  const h = harness(listing()); await h.view.load();
  assert.equal(h.$('registry-automatic').checked, false);
  const rendered = h.$('registry-list').children[0];
  assert.equal(rendered.children[0].children[1].children[0], card.name);
  assert.equal(rendered.children[1].children[0], card.description);
  h.$('registry-search').value = 'not-found'; h.$('registry-search').listeners.input();
  assert.equal(h.$('registry-list').children[0].children[0], 'No plugins found');
  assert.equal(h.requests.length, 1);
  h.$('registry-search').value = 'notes'; h.$('registry-search').listeners.input();
  assert.equal(h.$('registry-list').children.length, 1);
  await h.view.load(true);
  assert.deepEqual(h.requests.at(-1), { command: 'registry_index', args: { refresh: true } });
});

test('registry selection sends only id/version and opens existing consent review', async () => {
  const h = harness(listing()); await h.view.load(); await h.view.stage('notes', '0.10.0');
  assert.deepEqual(h.requests.at(-1), { command: 'registry_stage', args: { id: 'notes', version: '0.10.0' } });
  assert.equal(h.reviewed().token, 'checked-review');
  assert.equal(h.requests.some(r => r.command === 'confirm_install'), false);
});

test('badges expire and stale, yanked, incompatible and development rows stay disabled', async () => {
  const h = harness(listing()); await h.view.load();
  assert.equal(h.badges[0].textContent, '1'); assert.equal(h.view.updateFor('notes').version, '0.10.0');
  h.expire(); assert.equal(h.view.updateFor('notes'), null); assert.equal(h.badges[0].hidden, true);
  assert.equal(h.$('registry-message').textContent, 'This list is out of date.');
  await h.view.stage('notes', '0.10.0'); assert.equal(h.requests.length, 1);
  for (const warning of ['Update Terrarium to install this version.', 'Remove the development folder to install this plugin.', 'This version was withdrawn by its publisher.']) {
    const list = listing(); list.plugins[0].can_install = false; list.plugins[0].warning = warning;
    const other = harness(list); await other.view.load();
    assert.equal(other.$('registry-list').children[0].children.at(-1).attrs.disabled, true);
  }
});
