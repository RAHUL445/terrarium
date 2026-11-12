const { test } = require('node:test');
const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { join } = require('node:path');
const { rendererHarness } = require('./helpers/renderer-dom.cjs');

function findForm(node) {
  if (node.type === 'form' && node.inline) return node;
  return (node.children || []).map(findForm).find(Boolean);
}

test('Bookmarks quick entry permits navigation and restores its unsaved draft', async () => {
  const packages = process.env.TERRA_PLUGIN_SOURCE || join(__dirname, '../../../terrarium-plugins/packages');
  const template = JSON.parse(readFileSync(join(packages,
    'bookmarks/ui/page-main.json'), 'utf8'));
  const form = findForm(template.body);
  assert.ok(form, 'exercise the actual Bookmarks quick-entry form');
  const body = { body: form };
  let saves = 0;
  const bridge = { today: () => '2026-11-12', action: async () => { saves++; }, toast() {} };
  const first = rendererHarness();
  const view = first.renderer.mount(first.root, body, bridge);
  await view.ready;
  const input = first.root.querySelector('[data-pp-name]');
  input.value = 'https://example.org #test';
  input.dispatchEvent({ type: 'input', bubbles: true });
  assert.equal(await view.flush({ unmount: true }), true);
  const snapshot = view.snapshot();
  view.dispose();
  const second = rendererHarness();
  const restored = second.renderer.mount(second.root, body, bridge, { snapshot });
  await restored.ready;
  assert.equal(second.root.querySelector('[data-pp-name]').value, 'https://example.org #test');
  assert.equal(saves, 0, 'leaving must not submit a quick-entry draft');
  restored.dispose();
});
