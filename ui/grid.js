// Widget grid layout math for pages. Pure functions over plain placement
// objects `{ id, x, y, w, h }` in 12-column grid units; no DOM access.
(function (root) {
  "use strict";
  const COLS = 12;
  const MAX_ROWS = 400;
  const DEFAULT_SIZE = { default: [4, 3], min: [2, 2], max: [12, 12] };

  const int = (value, fallback) => (Number.isFinite(Number(value)) ? Math.round(Number(value)) : fallback);
  const clampNum = (value, lo, hi) => Math.min(hi, Math.max(lo, value));

  // A widget's declared size limits, completed with defaults and made consistent.
  function limits(size) {
    const s = size && typeof size === "object" ? size : {};
    const pair = (value, fallback) => Array.isArray(value) && value.length === 2
      ? [clampNum(int(value[0], fallback[0]), 1, COLS), clampNum(int(value[1], fallback[1]), 1, 12)]
      : [...fallback];
    const min = pair(s.min, DEFAULT_SIZE.min);
    const max = pair(s.max, DEFAULT_SIZE.max);
    max[0] = Math.max(max[0], min[0]); max[1] = Math.max(max[1], min[1]);
    const def = pair(s.default, DEFAULT_SIZE.default);
    return {
      min, max,
      default: [clampNum(def[0], min[0], max[0]), clampNum(def[1], min[1], max[1])],
    };
  }

  // Keeps a placement inside the grid and within its size limits.
  function clamp(item, size) {
    const lim = limits(size);
    const w = clampNum(int(item.w, lim.default[0]), lim.min[0], lim.max[0]);
    const h = clampNum(int(item.h, lim.default[1]), lim.min[1], lim.max[1]);
    const x = clampNum(int(item.x, 0), 0, COLS - w);
    const y = clampNum(int(item.y, 0), 0, MAX_ROWS);
    return { ...item, x, y, w, h };
  }

  function collides(a, b) {
    return a.id !== b.id && a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;
  }

  const byPosition = (a, b) => a.y - b.y || a.x - b.x || String(a.id).localeCompare(String(b.id));

  // The highest row at or above `item.y` where `item` fits among `placed`,
  // or the first free row below when it overlaps and nothing above is free.
  function settle(item, placed) {
    const next = { ...item };
    for (let y = 0; y <= item.y; y += 1) {
      if (!placed.some((other) => collides({ ...next, y }, other))) { next.y = y; return next; }
    }
    let hit;
    while ((hit = placed.find((other) => collides(next, other)))) next.y = hit.y + hit.h;
    return next;
  }

  // Moves every placement as far up as it can go, in reading order.
  function compact(items) {
    const placed = [];
    // Overlapping input (merged edits) is pushed below what is already placed.
    for (const item of [...items].sort(byPosition)) placed.push(settle(item, placed));
    const order = new Map(items.map((item, index) => [item.id, index]));
    return placed.sort((a, b) => order.get(a.id) - order.get(b.id));
  }

  // Puts `id` at `rect` (clamped to `size`), pushes overlapped placements
  // down, then compacts. Returns a new array in the original order.
  function place(items, id, rect, size) {
    const target = items.find((item) => item.id === id);
    if (!target) return items.map((item) => ({ ...item }));
    const moved = clamp({ ...target, ...rect }, size);
    // Others settle around the moved placement held in place, so moving
    // onto a neighbour lets it slide into the space left behind.
    const settled = [];
    for (const item of items.filter((other) => other.id !== id).sort(byPosition)) settled.push(settle(item, [moved, ...settled]));
    const final = settle(moved, settled);
    // The moved placement may rise into space the others avoided: settle
    // them again around where it ended up so no gap is left behind.
    const again = [];
    for (const item of settled.sort(byPosition)) again.push(settle(item, [final, ...again]));
    const byId = new Map([final, ...again].map((item) => [item.id, item]));
    return items.map((item) => byId.get(item.id));
  }

  // The first free spot, in reading order, for a new placement of `w`×`h`.
  function firstFit(items, w, h) {
    w = clampNum(int(w, 4), 1, COLS); h = Math.max(1, int(h, 3));
    for (let y = 0; y <= MAX_ROWS; y += 1) {
      for (let x = 0; x + w <= COLS; x += 1) {
        const probe = { id: "\u0000probe", x, y, w, h };
        if (!items.some((item) => collides(probe, item))) return { x, y, w, h };
      }
    }
    return { x: 0, y: bottom(items), w, h };
  }

  function bottom(items) {
    return items.reduce((max, item) => Math.max(max, item.y + item.h), 0);
  }

  // Narrow screens: one column, reading order.
  function stack(items) {
    return [...items].sort(byPosition);
  }

  // Size class a template can adapt to.
  function sizeClass(item) {
    if (item.w <= 3 || item.h <= 2) return "compact";
    if (item.w >= 8) return "expanded";
    return "regular";
  }

  // Keyboard editing: arrow keys move by one cell, Shift resizes.
  function nudge(items, id, key, resize, size) {
    const item = items.find((entry) => entry.id === id);
    if (!item) return items;
    const delta = { ArrowLeft: [-1, 0], ArrowRight: [1, 0], ArrowUp: [0, -1], ArrowDown: [0, 1] }[key];
    if (!delta) return items;
    const rect = resize
      ? { w: item.w + delta[0], h: item.h + delta[1] }
      : { x: item.x + delta[0], y: Math.max(0, item.y + delta[1]) };
    // Moving down onto a neighbour swaps with it, or compaction undoes the move.
    if (!resize && delta[1] === 1) {
      const below = items.filter((other) => other.id !== id && other.y === item.y + item.h
        && other.x < item.x + item.w && item.x < other.x + other.w).sort(byPosition)[0];
      if (below) rect.y = item.y + below.h;
    }
    return place(items, id, rect, size);
  }

  const api = { COLS, limits, clamp, collides, compact, place, firstFit, bottom, stack, sizeClass, nudge };
  if (typeof module !== "undefined" && module.exports) module.exports = api;
  else root.TerrariumGrid = api;
})(typeof window === "undefined" ? globalThis : window);
