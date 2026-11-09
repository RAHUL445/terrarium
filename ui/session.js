// Navigation metadata is device-local. Draft bodies remain in this app session.
(function (root) {
  "use strict";
  class Session {
    constructor(storage) { this.storage = storage; this.drafts = new Map(); }
    read(space) {
      try {
        const value = JSON.parse(this.storage.getItem(`pp-location:${space}`) || "null");
        if (!value || !["home", "apps", "settings", "plugin"].includes(value.view)) return null;
        const id = v => typeof v === "string" && v.length < 512 ? v : null;
        return { view: value.view, plugin: id(value.plugin), page: id(value.page), item: id(value.item), settings: id(value.settings), scroll: Number.isFinite(value.scroll) ? Math.max(0, Math.min(value.scroll, 10000000)) : 0 };
      } catch (_) { return null; }
    }
    write(space, location) {
      try { this.storage.setItem(`pp-location:${space}`, JSON.stringify(location)); } catch (_) {}
    }
    save(key, view) { if (view?.snapshot) this.drafts.set(key, view.snapshot()); }
    get(key) { return this.drafts.get(key); }
  }
  root.TerrariumSession = Session;
  if (typeof module !== "undefined") module.exports = Session;
})(typeof window === "undefined" ? globalThis : window);
