(function (root) {
  "use strict";

  function viewsOf(plugins) {
    const out = [];
    for (const plugin of plugins || []) {
      if (!plugin || typeof plugin !== "object") continue;
      const views = plugin.views && typeof plugin.views === "object" ? plugin.views : {};
      for (const [query, view] of Object.entries(views)) {
        if (typeof query !== "string" || !query || !view || typeof view !== "object") continue;
        out.push({ plugin, query, view });
      }
    }
    return out;
  }

  function find(plugins, pluginId, query) {
    return viewsOf(plugins).find((entry) => entry.plugin?.id === pluginId && entry.query === query) || null;
  }

  function columns(view, types) {
    const wanted = new Set(types || []);
    return (view?.columns || []).filter((column) => wanted.has(column?.type));
  }

  function layouts(view) {
    const out = ["list", "table"];
    if (columns(view, ["enum"]).length) out.push("board");
    if (columns(view, ["date"]).length) out.push("calendar");
    return out;
  }

  function settings(view, config) {
    const safe = config && typeof config === "object" ? config : {};
    const allColumns = view?.columns || [];
    const keys = new Set(allColumns.map((column) => column?.key).filter((key) => typeof key === "string" && key));
    const enumColumns = columns(view, ["enum"]);
    const dateColumns = columns(view, ["date"]);
    const allowedLayouts = layouts(view);
    const layout = allowedLayouts.includes(safe.layout) ? safe.layout : "list";
    const sort = keys.has(safe.sort) ? safe.sort : "";
    const dir = safe.dir === "desc" ? "desc" : "asc";
    const groupable = layout === "board" ? enumColumns : columns(view, ["enum", "string"]);
    let group = keys.has(safe.group) && groupable.some((column) => column.key === safe.group) ? safe.group : "";
    let date = keys.has(safe.date) && dateColumns.some((column) => column.key === safe.date) ? safe.date : "";
    if (layout === "board" && !group) group = enumColumns[0]?.key || "";
    if (layout === "calendar" && !date) date = dateColumns[0]?.key || "";
    return { layout, sort, dir, group, date };
  }

  function layoutFor(config, sizeClass) {
    if (sizeClass === "compact") return "list";
    const layout = config && typeof config === "object" ? config.layout : null;
    return typeof layout === "string" && layout ? layout : "list";
  }

  function visibleColumns(view, config) {
    const all = view.columns || [];
    if (typeof config?.fields !== "string") return all;
    const selected = new Set(config.fields.split(",").filter(Boolean));
    const unknown = [...selected].filter(key => !all.some(column => column.key === key));
    if (unknown.length) throw new Error("Some display fields are unavailable. Customize this view to choose its fields again.");
    return all.filter(column => column.key === (view.title_column || "title") || selected.has(column.key));
  }

  function compareValues(a, b, type) {
    const kind = String(type || "string");
    const numberLike = kind === "integer" || kind === "number";
    const av = numberLike ? Number(a) : a;
    const bv = numberLike ? Number(b) : b;
    const aEmpty = a == null || a === "" || (numberLike && Number.isNaN(av));
    const bEmpty = b == null || b === "" || (numberLike && Number.isNaN(bv));
    if (aEmpty || bEmpty) return aEmpty === bEmpty ? 0 : (aEmpty ? 1 : -1);
    if (numberLike) return av - bv;
    if (kind === "boolean") return Number(Boolean(av)) - Number(Boolean(bv));
    return String(av).localeCompare(String(bv), undefined, { numeric: true, sensitivity: "base" });
  }

  function sortRows(rows, key, type, dir) {
    const order = dir === "desc" ? "desc" : "asc";
    const isEmpty = (value) => {
      if (value == null || value === "") return true;
      if (type === "integer" || type === "number") return Number.isNaN(Number(value));
      return false;
    };
    return rows
      .map((row, index) => ({ row, index }))
      .sort((left, right) => {
        const leftEmpty = isEmpty(left.row?.[key]);
        const rightEmpty = isEmpty(right.row?.[key]);
        if (leftEmpty || rightEmpty) return leftEmpty === rightEmpty ? left.index - right.index : (leftEmpty ? 1 : -1);
        const cmp = compareValues(left.row?.[key], right.row?.[key], type);
        if (cmp === 0) return left.index - right.index;
        return order === "asc" ? cmp : -cmp;
      })
      .map((entry) => entry.row);
  }

  function enumLabel(column, value) {
    value = value ?? "";
    const values = Array.isArray(column?.values) ? column.values : [];
    const labels = Array.isArray(column?.labels) ? column.labels : [];
    const index = values.findIndex((item) => item === value);
    if (index < 0) return String(value);
    return labels[index] || String(value);
  }

  function prepareRows(rows, view, config, focus) {
    const s = settings(view, config);
    const allColumns = view?.columns || [];
    const byKey = new Map(allColumns.map((column) => [column.key, column]));
    const enumColumns = columns(view, ["enum"]);
    const searchable = new Set([view?.title_column, ...allColumns.filter((column) => ["string", "text", "enum"].includes(column?.type)).map((column) => column.key)]);
    const needle = String(focus || "").trim().toLowerCase();
    let out = (Array.isArray(rows) ? rows : []).map((row) => {
      const next = { ...(row || {}) };
      for (const column of enumColumns) next[`${column.key}__label`] = enumLabel(column, next[column.key]);
      return next;
    });
    if (needle) {
      out = out.filter((row) => {
        for (const key of searchable) {
          if (!key) continue;
          const value = row[key];
          if (value != null && String(value).toLowerCase().includes(needle)) return true;
          const column = byKey.get(key);
          if (column?.type === "enum" && String(row[`${key}__label`] || "").toLowerCase().includes(needle)) return true;
        }
        return false;
      });
    }
    if (s.sort) {
      const type = byKey.get(s.sort)?.type || "string";
      out = sortRows(out, s.sort, type, s.dir);
    }
    if (s.group) {
      const type = byKey.get(s.group)?.type || "string";
      out = sortRows(out, s.group, type, "asc");
    }
    return out;
  }

  function template(entry, config, sizeClass) {
    const view = entry.view || {};
    const s = settings(view, Object.assign({}, config, { layout: layoutFor(config, sizeClass) }));
    const params = view.params && typeof view.params === "object" ? view.params : {};
    const titleColumn = typeof view.title_column === "string" && view.title_column ? view.title_column : "title";
    const title = `{{row.${titleColumn}}}`;
    const open = { navigate: { item: "{{row.id}}" } };
    const shown = visibleColumns(view, config);
    const explicitFields = typeof config?.fields === "string";
    const dateColumn = shown.find(column => column.type === "date");
    const enumColumns = columns(view, ["enum"]);
    const chips = shown.filter(column => column.type === "enum" || (explicitFields && column.key !== titleColumn && column !== dateColumn))
      .map(column => ({ text: column.type === "enum" ? `{{row.${column.key}__label}}` : `{{row.${column.key}}}` }));
    const item = {
      type: "item",
      title,
      ...(dateColumn ? { subtitle: `{{row.${dateColumn.key}}}` } : {}),
      ...(chips.length ? { chips } : {}),
      action: open,
    };
    if (s.layout === "table") {
      const tableColumns = shown.map((column) => ({
        key: column.type === "enum" ? `${column.key}__label` : column.key,
        label: column.label,
        type: column.type === "enum" ? "string" : column.type,
      }));
      const sort = s.sort && (view.columns || []).find((column) => column.key === s.sort)?.type === "enum" ? `${s.sort}__label` : s.sort;
      return {
        state: {},
        body: {
          type: "table",
          query: entry.query,
          params,
          label: view.title,
          columns: tableColumns,
          sort,
          dir: s.dir,
          empty: "No items match this view. Adjust its filters or capture a new item.",
          empty_focus: "Nothing matches “{{$page.focus}}”.",
          action: open,
        },
      };
    }
    if (s.layout === "board") {
      const groupColumn = (view.columns || []).find((column) => column.key === s.group) || columns(view, ["enum"])[0];
      const boardColumns = (groupColumn?.values || []).map((value, index) => ({
        value,
        label: groupColumn?.labels?.[index] || value,
      }));
      return {
        state: {},
        body: {
          type: "list",
          layout: "board",
          query: entry.query,
          params,
          label: view.title,
          column: groupColumn?.key || "",
          columns: boardColumns,
          ...(groupColumn?.move ? { move: groupColumn.move } : {}),
          item,
        },
      };
    }
    if (s.layout === "calendar") {
      return {
        state: {},
        body: {
          type: "calendar",
          query: entry.query,
          params,
          label: view.title,
          date: s.date,
          title: titleColumn,
          empty: "Nothing this month.",
          action: open,
        },
      };
    }
    return {
      state: {},
      body: {
        type: "list",
        query: entry.query,
        params,
        ...(s.group ? { group: enumColumns.some(c => c.key === s.group) ? `${s.group}__label` : s.group } : {}),
        empty: "No items match this view. Adjust its filters or capture a new item.",
        empty_focus: "Nothing matches “{{$page.focus}}”.",
        item,
      },
    };
  }

  function wrapBridge(base, entry, config, focus) {
    return Object.assign({}, base, {
      query: async (name, params) => {
        if (name !== entry.query) return base.query(name, params);
        const values = filterParams(entry.view, config);
        const scoped = Object.assign({}, params, values);
        const queryFocus = Object.hasOwn(entry.view.params || {}, "page_focus");
        if (queryFocus) scoped.page_focus = String(focus || "");
        return prepareRows(await base.query(name, scoped), entry.view, config, queryFocus ? "" : focus);
      },
    });
  }

  function filterParams(view, config) {
    let filters = {};
    if (config?.filters) {
      try { filters = JSON.parse(config.filters); } catch (_) { throw new Error("Saved filters are invalid. Edit this view to repair them."); }
    }
    if (!filters || typeof filters !== "object" || Array.isArray(filters)) throw new Error("View filters must be an object.");
    const allowed = new Set((view.filters || []).map(f => f.param));
    for (const [key, value] of Object.entries(filters)) {
      if (!allowed.has(key) || (value !== null && typeof value === "object")) throw new Error(`Filter ${key} is no longer available. Edit this view to repair it.`);
    }
    return filters;
  }

  async function resolveChoices(entry, bridge) {
    const view = structuredClone(entry.view);
    await Promise.all((view.columns || []).map(async column => {
      if (!column.choices_query) return;
      const rows = await bridge.query(column.choices_query, {});
      column.values = rows.map(r => String(r.value ?? ""));
      column.labels = rows.map(r => String(r.label ?? r.value ?? ""));
    }));
    return { ...entry, view };
  }

  async function validateChoices(view, config, bridge) {
    const filters = filterParams(view, config);
    await Promise.all((view.filters || []).map(async definition => {
      const value = filters[definition.param];
      if (value == null || value === "") return;
      let choices = definition.values || [];
      if (definition.choices_query) {
        choices = (await bridge.query(definition.choices_query, {})).map(row => String(row.value ?? ""));
      } else if (!choices.length) return;
      const selected = definition.multiple ? String(value).split(",") : [String(value)];
      if (selected.some(item => !choices.includes(item))) {
        throw new Error(`${definition.label} contains an unavailable choice. Edit this view to repair it.`);
      }
    }));
  }

  // Used by both page placements and the host view library inside plugin pages.
  function mount(body, options) {
    body.classList.add("pp-data-view");
    body.dataset.density = options.config?.density === "compact" ? "compact" : "comfortable";
    let child = null, dead = false, revision = 0, signature = "";
    const load = async () => {
      const seq = ++revision;
      try {
        await validateChoices(options.entry.view, options.config, options.bridge);
        const entry = await resolveChoices(options.entry, options.bridge);
        if (dead || seq !== revision) return;
        const next = JSON.stringify(entry.view);
        if (child && signature === next) { await child.refresh(); return; }
        child?.dispose(); body.replaceChildren(); signature = next;
        child = (options.mount || root.TerrariumRenderer.mount)(body, template(entry, options.config, options.size),
          wrapBridge(options.bridge, entry, options.config, options.focus), { snapshot: options.snapshot, size: options.size,
            page: { focus: options.focus || "", title: options.pageTitle || "" } });
        await child.ready;
      } catch (error) {
        if (dead || seq !== revision) return;
        child?.dispose(); child = null; signature = "";
        const message = document.createElement("p"); message.className = "pp-error";
        message.textContent = String(error); body.replaceChildren(message);
      }
    };
    const handle = { refresh: load, flush: async (...args) => child ? child.flush(...args) : true,
      snapshot: () => child?.snapshot(),
      dispose() { dead = true; revision++; child?.dispose(); } };
    handle.ready = load();
    return handle;
  }

  const api = { viewsOf, find, columns, layouts, settings, layoutFor, visibleColumns, compareValues, prepareRows, template, wrapBridge, filterParams, resolveChoices, validateChoices, mount };
  if (typeof module !== "undefined" && module.exports) module.exports = api;
  else root.TerrariumViews = api;
})(typeof window === "undefined" ? globalThis : window);
