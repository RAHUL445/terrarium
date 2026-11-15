// Docked detail panel: on a wide window, a plugin page's item sheet (the
// sheet its `entry` opens) shows beside the list or Today instead of over it.
// The renderer asks a handle whether docking is possible and moves its sheet
// into the dock; the shell decides which view's sheet is visible.
(function (root) {
  "use strict";

  /** Window width at which item sheets dock (narrower windows keep the sheet). */
  const WIDE_QUERY = "(min-width: 1200px)";
  /** Width the list (or Today) keeps beside a docked sheet. */
  const MIN_LIST = 560;
  /** Docked widths: regular sheets, and large ones (they may shrink to MIN_LARGE). */
  const DOCK_WIDTH = 360;
  const MAX_LARGE = 640;
  const MIN_LARGE = 480;
  /** `size: "panel"` sheets: a fixed task-panel width. */
  const PANEL_WIDTH = 460;
  /** Room a page keeps for the list beside a panel (its own sidebar included), or the panel overlays instead. */
  const MIN_LIST_PANEL = 760;

  /**
   * Width of a docked sheet when `room` px are shared by the list and the
   * dock, or 0 when docking would leave the list narrower than MIN_LIST.
   * Large sheets prefer 40% of the room (480–640px) and shrink toward
   * MIN_LARGE before giving up.
   */
  function dockWidth(room, large) {
    if (!(room > 0)) return 0;
    const spare = room - MIN_LIST;
    if (large === "panel") return room - MIN_LIST_PANEL >= PANEL_WIDTH ? PANEL_WIDTH : 0;
    if (!large) return spare >= DOCK_WIDTH ? DOCK_WIDTH : 0;
    const preferred = Math.min(MAX_LARGE, Math.max(MIN_LARGE, Math.round(room * 0.4)));
    const width = Math.min(preferred, spare);
    return width >= MIN_LARGE ? Math.floor(width) : 0;
  }

  const isLarge = (dialog) => (dialog?.classList?.contains?.("pp-sheet-panel") ? "panel" : Boolean(dialog?.classList?.contains?.("pp-sheet-large")));

  // Sheets that do not depend on a list row: the page's own sheets.
  function sheetsOf(node, out = []) {
    if (!node || typeof node !== "object") return out;
    if (Array.isArray(node)) {
      for (const child of node) sheetsOf(child, out);
      return out;
    }
    if (node.type === "sheet") {
      out.push(node);
      return out;
    }
    if (node.type === "list") return out;
    for (const value of Object.values(node)) if (value && typeof value === "object") sheetsOf(value, out);
    return out;
  }

  /**
   * A copy of a page template that keeps only its sheets, so another view
   * (Today) can open the page's item sheet without drawing the page. Null
   * when the page has no item sheet for its `entry`.
   */
  function detailTemplate(template) {
    const bind = template?.entry?.bind;
    if (typeof bind !== "string" || !bind) return null;
    const sheets = sheetsOf(template.body);
    if (!sheets.some((sheet) => sheet.bind === bind && sheet.dock !== false)) return null;
    return { ...template, body: { type: "stack", children: sheets } };
  }

  /**
   * `container`: the dock element. `options.media`: a MediaQueryList for the
   * wide layout (defaults to WIDE_QUERY); `options.room()`: the width shared
   * by the list and the dock (defaults to the container's parent, `.main`,
   * which excludes the navigation rail and the companion); `options.observe`:
   * false to skip watching that width; `options.onChange(open)`.
   * Each owner (a view: "page", "today") gets its own `handle(owner)`; only
   * the active owner's sheet is shown.
   */
  function create(container, options = {}) {
    const media = options.media
      || (typeof root.matchMedia === "function" ? root.matchMedia(WIDE_QUERY) : null);
    const entries = new Map(); // dialog -> {owner, close, reflow}
    let active = null;
    const parent = () => container?.parentElement || container?.parentNode || null;
    const room = options.room || (() => {
      const node = parent();
      return node ? (node.clientWidth || node.getBoundingClientRect?.().width || 0) : 0;
    });
    // Docks only on a wide window, and only when the list keeps MIN_LIST.
    const widthFor = (large) => (container && media?.matches ? dockWidth(room(), large) : 0);

    function sync() {
      let shown = 0;
      let large = false;
      for (const [dialog, entry] of entries) {
        const on = entry.owner === active;
        dialog.hidden = !on;
        if (on) {
          shown++;
          large = large || isLarge(dialog);
        }
      }
      if (container) {
        container.hidden = shown === 0;
        container.dataset.size = large === "panel" ? "panel" : large ? "large" : "";
        const width = shown ? widthFor(large) : 0;
        if (width) container.style?.setProperty?.("--dock-width", `${width}px`);
      }
      options.onChange?.(shown > 0);
    }

    // Window crossing the breakpoint, or the room changing (window resize,
    // rail or companion opened/closed): each open sheet re-decides docked
    // vs. overlay and keeps its item open.
    let lastRoom = null;
    const reflow = () => {
      lastRoom = room();
      for (const entry of [...entries.values()]) entry.reflow?.();
      sync();
    };
    if (media?.addEventListener) media.addEventListener("change", reflow);
    else media?.addListener?.(reflow);
    if (options.observe !== false) {
      const target = parent();
      const onResize = () => {
        if (!entries.size) return;
        const now = room();
        if (now !== lastRoom) reflow();
      };
      if (target && typeof root.ResizeObserver === "function") new root.ResizeObserver(onResize).observe(target);
      else root.addEventListener?.("resize", onResize);
    }

    const api = {
      query: WIDE_QUERY,
      /**
       * True when a sheet (`large`: a `size: "large"` one) can dock now: a
       * wide window and enough room that the list keeps MIN_LIST.
       */
      available: (large = false) => widthFor(large === "panel" ? large : Boolean(large)) > 0,
      /** The docked width for a sheet now, or 0 (overlay instead). */
      width: (large = false) => widthFor(large === "panel" ? large : Boolean(large)),
      /** Shows `owner`'s docked sheet (null hides the dock). */
      setActive(owner) {
        active = owner || null;
        sync();
      },
      get active() { return active; },
      /** Whether `owner` (default: the active one) has a docked sheet. */
      isOpen(owner = active) {
        for (const entry of entries.values()) if (entry.owner === owner) return true;
        return false;
      },
      /** Asks `owner`'s docked sheets to close (they save first). */
      close(owner = active) {
        const list = [...entries.values()].filter((entry) => entry.owner === owner);
        for (const entry of list) entry.close?.();
        return list.length > 0;
      },
      /** The handle a renderer gets as `options.dock`. */
      handle(owner) {
        return {
          /** `dialog` (optional): the sheet asking, so large sheets measure their own width. */
          available: (dialog) => api.available(isLarge(dialog)),
          active: (dialog) => entries.get(dialog)?.owner === active,
          attach(dialog, hooks = {}) {
            // One item at a time per owner.
            for (const [other, entry] of [...entries]) if (other !== dialog && entry.owner === owner) entry.close?.();
            entries.set(dialog, { owner, close: hooks.close, reflow: hooks.reflow });
            lastRoom = room();
            container?.append(dialog);
            sync();
            return true;
          },
          detach(dialog) {
            if (!entries.delete(dialog)) return;
            if (dialog.parentNode === container) dialog.remove?.();
            dialog.hidden = false;
            sync();
          },
        };
      },
    };
    return api;
  }

  const api = { WIDE_QUERY, MIN_LIST, dockWidth, sheetsOf, detailTemplate, create };
  if (typeof module !== "undefined" && module.exports) module.exports = api;
  else root.TerrariumDock = api;
})(typeof window === "undefined" ? globalThis : window);
