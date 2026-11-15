// Pure helpers for ordered, single-column page layouts.
(function (root) {
  "use strict";

  function gridOrder(placements) {
    return [...placements].sort((a, b) => (a.y - b.y) || (a.x - b.x) || String(a.id).localeCompare(String(b.id)));
  }

  function order(placements) {
    const grid = gridOrder(placements);
    const rank = new Map(grid.map((placement, index) => [placement.id, index]));
    return [...placements].sort((a, b) => {
      const ao = Number.isInteger(a.flow_order) ? a.flow_order : rank.get(a.id);
      const bo = Number.isInteger(b.flow_order) ? b.flow_order : rank.get(b.id);
      return (ao - bo) || (rank.get(a.id) - rank.get(b.id));
    });
  }

  // Returns fresh records and never changes the saved or input draft records.
  function move(placements, id, direction) {
    const sorted = order(placements);
    const index = sorted.findIndex((placement) => placement.id === id);
    const target = index + Math.sign(direction);
    if (index < 0 || target < 0 || target >= sorted.length || !direction) return placements.map((p) => ({ ...p }));
    [sorted[index], sorted[target]] = [sorted[target], sorted[index]];
    const positions = new Map(sorted.map((placement, rank) => [placement.id, rank]));
    return placements.map((placement) => ({ ...placement, flow_order: positions.get(placement.id) }));
  }

  const api = Object.freeze({ order, move });
  if (typeof module !== "undefined" && module.exports) module.exports = api;
  if (root) root.TerrariumPageFlow = api;
})(typeof window !== "undefined" ? window : globalThis);
