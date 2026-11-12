const {test} = require('node:test');
const assert = require('node:assert/strict');
const {rendererHarness} = require('./helpers/renderer-dom.cjs');
const template = require('../../crates/runtime/tests/fixtures/declarative/ui/page-main.json');
const nodes = root => [root, ...root.children.flatMap(nodes)];
test('reading list fixture renders and checkbox sends a typed desired state', async () => {
  const ui = rendererHarness(); const actions = []; let done = false;
  const view = ui.renderer.mount(ui.root, template, {
    today: () => '2026-11-11', changed: () => {}, toast: message => { throw Error(message); },
    query: async name => name === (done ? 'done' : 'open') ? [{id:'reading-1',title:'Read a chapter',done:done ? 1 : 0}] : [],
    action: async (op,args) => { actions.push({op,args}); done=args.done; return null; },
  });
  await view.ready;
  const checkbox = nodes(ui.root).find(n => n.tag === 'input' && n.type === 'checkbox');
  assert.ok(checkbox); assert.equal(checkbox.checked,false);
  checkbox.checked=true;await checkbox.fire('change');
  assert.equal(actions[0].op,'toggle');assert.equal(actions[0].args.id,'reading-1');assert.equal(actions[0].args.done,true);
  await view.refresh();
  assert.equal(nodes(ui.root).filter(n => n.tag === 'input' && n.type === 'checkbox').length,1);
  view.dispose();
});
