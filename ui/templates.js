// Templates are plugin metadata, independent of space and record data.
// Invalidate on every plugin reload, including same-version development edits.
(function (root) {
  class TemplateCache {
    constructor() { this.entries = new Map(); }
    clear() { this.entries.clear(); }
    get(args, load) {
      const key = JSON.stringify([args.plugin, args.kind, args.id]);
      if (this.entries.has(key)) return this.entries.get(key);
      const pending = Promise.resolve().then(load).catch(error => {
        if (this.entries.get(key) === pending) this.entries.delete(key);
        throw error;
      });
      this.entries.set(key, pending);
      return pending;
    }
  }
  if (typeof module !== 'undefined' && module.exports) module.exports = TemplateCache;
  else root.TerrariumTemplateCache = TemplateCache;
})(typeof window !== 'undefined' ? window : globalThis);
