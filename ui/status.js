// Reduce scheduler history to current-space status, without exposing item contents.
(function (root) {
  "use strict";
  function summarize(runs, space) {
    const own = (Array.isArray(runs) ? runs : []).filter(run => run.space === space);
    const latest = new Map();
    for (const run of own) {
      const key = run.schedule || run.id;
      const previous = latest.get(key);
      if (!previous || (run.slot_ms || 0) >= (previous.slot_ms || 0)) latest.set(key, run);
    }
    return {
      active: own.filter(run => ["running", "waiting", "retry"].includes(run.status)).length,
      failed: [...latest.values()].filter(run => run.status === "failed").length,
    };
  }
  root.TerrariumStatus = { summarize };
  if (typeof module !== "undefined") module.exports = { summarize };
})(typeof window === "undefined" ? globalThis : window);
