// Opt-in local diagnostics. Records timing labels only, never command arguments,
// query results, space names or editor text. No storage or network export.
(function (root) {
  function createProfiler(enabled, clock = () => performance.now(), report = record => console.debug('[terrarium-perf]', JSON.stringify(record))) {
    return {
      enabled,
      record(name, metrics) {
        if (enabled) { try { report({ name, ...metrics }); } catch (_) {} }
      },
      measureSync(name, run) {
        if (!enabled) return run();
        const start = clock();
        let ok = false;
        try { const result = run(); ok = true; return result; }
        finally {
          try { report({ name, durationMs: +(clock() - start).toFixed(2), ok }); } catch (_) {}
        }
      },
      async measure(name, run) {
        if (!enabled) return run();
        const start = clock();
        let ok = false;
        try {
          const result = await run();
          ok = true;
          return result;
        } finally {
          // Diagnostics must never change the result of an app operation.
          try { report({ name, durationMs: +(clock() - start).toFixed(2), ok }); } catch (_) {}
        }
      },
    };
  }
  if (typeof module !== 'undefined' && module.exports) module.exports = createProfiler;
  else root.TerrariumPerformance = createProfiler(new URLSearchParams(root.location.search).get('profile') === '1');
})(typeof window !== 'undefined' ? window : globalThis);
