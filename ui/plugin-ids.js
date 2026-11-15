// Renamed plugins. A plugin lists the ids it had before in `former_ids`;
// its data, refs and synced rows keep the first of them (its storage id).
// The UI names it by its current id and moves this device's own
// preferences over once, keeping the old keys so an older build still
// finds them.
(function (root) {
  "use strict";

  /** [former id, current id] for every renamed plugin. */
  function renames(plugins) {
    const out = [];
    for (const p of Array.isArray(plugins) ? plugins : []) {
      for (const former of Array.isArray(p?.former_ids) ? p.former_ids : []) {
        if (typeof former === "string" && former && former !== p.id) out.push([former, p.id]);
      }
    }
    return out;
  }

  /** The current id of the plugin `id` names. */
  function canonical(plugins, id) {
    for (const [from, to] of renames(plugins)) if (id === from) return to;
    return id;
  }

  /** The plugin `id` names, by current or former id. */
  function find(plugins, id) {
    const current = canonical(plugins, id);
    return (Array.isArray(plugins) ? plugins : []).find((p) => p?.id === current) || null;
  }

  /** The id item refs name `plugin` by: its storage id, so older builds resolve them. */
  function refId(plugin) {
    const former = Array.isArray(plugin?.former_ids) ? plugin.former_ids[0] : null;
    return typeof former === "string" && former ? former : plugin?.id;
  }

  /** `value` (an id or a `plugin/...` key) with `from` renamed to `to`, or null if it names another plugin. */
  function renamed(value, from, to) {
    if (value === from) return to;
    if (typeof value === "string" && value.startsWith(`${from}/`)) return to + value.slice(from.length);
    return null;
  }

  /** `list` with each entry that names `from` followed by its renamed copy. */
  function withRenamed(list, from, to) {
    if (!Array.isArray(list)) return list;
    const out = [];
    for (const item of list) {
      out.push(item);
      const next = renamed(item, from, to);
      if (next !== null && !list.includes(next) && !out.includes(next)) out.push(next);
    }
    return out;
  }

  function json(storage, key) {
    try { return JSON.parse(storage.getItem(key) || "null"); } catch (_) { return null; }
  }

  /**
   * Copies this device's preferences for renamed plugins to their current
   * ids, once per rename. Old keys and entries stay for rollback. `keys`
   * lists the storage keys present (without any profile prefix).
   * Returns the number of keys written.
   */
  function migrate(storage, keys, plugins) {
    let written = 0;
    const set = (key, value) => {
      try { storage.setItem(key, value); written++; } catch (_) {}
    };
    const all = Array.isArray(keys) ? keys : [];
    for (const [from, to] of renames(plugins)) {
      const marker = `pp-renamed:${from}:${to}`;
      try { if (storage.getItem(marker)) continue; } catch (_) { continue; }
      for (const key of all) {
        if (key.startsWith("pp-pins:")) {
          const value = json(storage, key);
          if (!Array.isArray(value?.pins)) continue;
          // Replaced in place: a pin left under the former id would show as
          // "unavailable" and take a shortcut slot.
          const hasTo = value.pins.some((p) => p?.kind === "app" && p.id === to);
          let changed = false;
          const pins = [];
          for (const pin of value.pins) {
            if (pin?.kind === "app" && pin.id === from) {
              changed = true;
              if (!hasTo && !pins.some((p) => p?.kind === "app" && p.id === to)) pins.push({ kind: "app", id: to });
            } else pins.push(pin);
          }
          if (changed) set(key, JSON.stringify({ ...value, pins }));
        } else if (key.startsWith("pp-layout:") || key.startsWith("pp-today:")) {
          const value = json(storage, key);
          if (!value || typeof value !== "object") continue;
          const next = { ...value };
          for (const field of ["nav", "widgets", "actions", "home", "homeWide", "folded", "plugins"]) {
            if (Array.isArray(value[field])) next[field] = withRenamed(value[field], from, to);
          }
          if (JSON.stringify(next) !== JSON.stringify(value)) set(key, JSON.stringify(next));
        } else if (key.startsWith("pp-location:")) {
          const value = json(storage, key);
          if (value?.plugin === from) set(key, JSON.stringify({ ...value, plugin: to }));
        } else if (key === "quick-capture-action" || key === "share-target") {
          let value = null;
          try { value = storage.getItem(key); } catch (_) {}
          const next = renamed(value, from, to);
          if (next !== null) set(key, next);
        } else if (key.startsWith("terrarium.workspace.") && key.includes(`/page/${from}/`)) {
          const next = key.replace(`/page/${from}/`, `/page/${to}/`);
          if (all.includes(next)) continue;
          let value = null;
          try { value = storage.getItem(key); } catch (_) {}
          if (value !== null) set(next, value);
        }
      }
      set(marker, "1");
    }
    return written;
  }

  const api = { renames, canonical, find, refId, migrate };
  if (typeof module !== "undefined" && module.exports) module.exports = api;
  else root.TerrariumPluginIds = api;
})(typeof window === "undefined" ? globalThis : window);
