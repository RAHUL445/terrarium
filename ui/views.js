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
    let group = keys.has(safe.group) && enumColumns.some((column) => column.key === safe.group) ? safe.group : "";
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
    if (value == null || value === "") return "";
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
    const dateColumn = columns(view, ["date"])[0];
    const enumColumns = columns(view, ["enum"]);
    const chips = enumColumns.map((column) => ({ text: `{{row.${column.key}__label}}` }));
    const item = {
      type: "item",
      title,
      ...(dateColumn ? { subtitle: `{{row.${dateColumn.key}}}` } : {}),
      ...(chips.length ? { chips } : {}),
      action: open,
    };
    if (s.layout === "table") {
      const tableColumns = (view.columns || []).map((column) => ({
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
          empty: "Nothing to show.",
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
        ...(s.group ? { group: s.group } : {}),
        empty: "Nothing to show.",
        empty_focus: "Nothing matches “{{$page.focus}}”.",
        item,
      },
    };
  }

  function wrapBridge(base, entry, config, focus) {
    return Object.assign({}, base, {
      query: async (name, params) => prepareRows(await base.query(name, params), entry.view, config, focus),
    });
  }

  const api = { viewsOf, find, columns, layouts, settings, layoutFor, compareValues, prepareRows, template, wrapBridge };
  if (typeof module !== "undefined" && module.exports) module.exports = api;
  else root.TerrariumViews = api;
})(typeof window === "undefined" ? globalThis : window);
