// Device-local layout preferences, independently scoped to each space.
(function (root) {
  "use strict";
  const unique = (items) => Array.isArray(items)
    ? [...new Set(items.filter((id) => typeof id === "string" && id.length <= 180))].slice(0, 200) : null;
  function normalize(value) {
    const v = value && typeof value === "object" ? value : {};
    return { version: 1, nav: unique(v.nav), widgets: unique(v.widgets), actions: unique(v.actions), folded: unique(v.folded) || [], home: unique(v.home), homeWide: unique(v.homeWide) || [] };
  }
  function selected(value, available, defaults = available) {
    const allowed = new Set(available);
    return (value === null ? defaults : value).filter((id) => allowed.has(id));
  }
  function toggle(value, id, enabled) {
    return enabled ? [...new Set([...value, id])] : value.filter((item) => item !== id);
  }
  function move(value, id, delta) {
    const next = [...value], from = next.indexOf(id), to = from + delta;
    if (from >= 0 && to >= 0 && to < next.length) [next[from], next[to]] = [next[to], next[from]];
    return next;
  }
  function place(value, id, before) {
    if (id === before || !value.includes(id) || !value.includes(before)) return [...value];
    const next = value.filter((item) => item !== id);
    next.splice(value.indexOf(before), 0, id);
    return next;
  }
  function read(storage, space) {
    try { return normalize(JSON.parse(storage.getItem(`pp-layout:${space}`) || "null")); }
    catch (_) { return normalize(null); }
  }
  function write(storage, space, value) {
    try { storage.setItem(`pp-layout:${space}`, JSON.stringify(normalize(value))); return true; }
    catch (_) { return false; }
  }
  const api = { normalize, selected, toggle, move, place, read, write };
  if (typeof module !== "undefined" && module.exports) module.exports = api;
  else root.TerrariumLayout = api;
})(typeof window === "undefined" ? globalThis : window);
