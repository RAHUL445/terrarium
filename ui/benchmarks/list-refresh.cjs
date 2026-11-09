// Run with: node ui/benchmarks/list-refresh.cjs
// Synthetic DOM: repeatable JS work/retention evidence, not native paint time.
const { performance } = require('node:perf_hooks');
const { rendererHarness, listTemplate } = require('../tests/helpers/renderer-dom.cjs');

(async () => {
  const harness = rendererHarness();
  const rows = Array.from({ length: 500 }, (_, id) => ({ id, title: `Entry ${id}` }));
  let queries = 0;
  const start = performance.now();
  const view = harness.renderer.mount(harness.root, listTemplate(), {
    today: () => '2026-11-04', query: async () => { queries++; return rows; },
  });
  await view.ready;
  const mountedMs = performance.now() - start;
  const initialWatchers = view.watchers.length;
  const before = harness.allocations();
  const refreshStart = performance.now();
  for (let i = 0; i < 20; i++) await view.refresh();
  console.log(JSON.stringify({ rows: rows.length, refreshes: 20, queries,
    mountedMs: +mountedMs.toFixed(2), refreshMs: +(performance.now() - refreshStart).toFixed(2),
    initialWatchers, finalWatchers: view.watchers.length,
    refreshAllocations: harness.allocations() - before }, null, 2));
})();
