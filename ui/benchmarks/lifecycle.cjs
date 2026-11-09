// node --expose-gc ui/benchmarks/lifecycle.cjs
// Actual renderer + synthetic DOM. Reachability evidence, not native RSS.
const { rendererHarness, listTemplate } = require('../tests/helpers/renderer-dom.cjs');
if (!global.gc) throw Error('Run with --expose-gc');
const turn = () => new Promise(resolve => setImmediate(resolve));
(async () => {
  let last;
  const ui = rendererHarness({ window: { TerrariumPerformance: {
    measureSync: (name, run) => run(), record: (name, metrics) => { last = metrics; },
  } } });
  const template = listTemplate();
  template.body = { type: 'stack', children: [{ type: 'search-field', bind: 'state.q' }, template.body] };
  const refs = [];
  global.gc(); const heapBefore = process.memoryUsage().heapUsed;
  async function cycle() {
    const view = ui.renderer.mount(ui.root, template, { today: () => '2026-11-04',
      query: async () => Array.from({ length: 30 }, (_, id) => ({ title: `Entry ${id}` })) });
    await view.ready;
    const input = ui.root.children[0].children[0].children[1];
    input.value = 'Pending search'; input.dispatchEvent({ type: 'input' });
    view.dispose(); ui.root.replaceChildren();
    return new WeakRef(view);
  }
  for (let i = 0; i < 200; i++) {
    refs.push(await cycle());
    if (i % 20 === 0) await turn();
  }
  for (let i = 0; i < 5; i++) { await turn(); global.gc(); }
  const retainedViews = refs.filter(ref => ref.deref()).length;
  const report = { cycles: 200, retainedViews, lifecycle: last,
    heapBefore, heapAfter: process.memoryUsage().heapUsed,
    limitation: 'Synthetic DOM and explicit GC; browser/native heap and RSS are not established by this check.' };
  console.log(JSON.stringify(report, null, 2));
  if (retainedViews || last.activeViews || last.activeTimers) process.exitCode = 1;
})();
