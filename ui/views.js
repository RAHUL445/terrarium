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

  function prepareRows(rows, view, config, focus, timezone) {
    const s = settings(view, config);
    const allColumns = view?.columns || [];
    const byKey = new Map(allColumns.map((column) => [column.key, column]));
    const enumColumns = columns(view, ["enum"]);
    const searchable = new Set([view?.title_column, ...allColumns.filter((column) => ["string", "text", "enum"].includes(column?.type)).map((column) => column.key)]);
    const needle = String(focus || "").trim().toLowerCase();
    let out = (Array.isArray(rows) ? rows : []).map((row) => {
      const next = { ...(row || {}) };
      for (const column of enumColumns) next[`${column.key}__label`] = enumLabel(column, next[column.key]);
      for (const column of columns(view, ["date"])) {
        const value = next[column.key];
        let label = value ?? "", day = value ?? "";
        if (typeof value === "number" || (typeof value === "string" && /^\d{4}-\d\d-\d\dT/.test(value))) {
          const date = new Date(value);
          if (Number.isFinite(date.getTime())) {
            const options = timezone ? { timeZone: timezone } : {};
            label = new Intl.DateTimeFormat(undefined, { ...options, dateStyle: "medium", timeStyle: "short" }).format(date);
            const parts = new Intl.DateTimeFormat("en-CA", { ...options, year: "numeric", month: "2-digit", day: "2-digit" }).formatToParts(date);
            const part = kind => parts.find(p => p.type === kind).value;
            day = `${part("year")}-${part("month")}-${part("day")}`;
          } else { label = "Invalid date"; day = ""; }
        }
        next[`${column.key}__label`] = label;
        next[`${column.key}__day`] = day;
      }
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
    if (s.sort && !view.pagination) {
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
    const rowAction = view.row_action ? { ...view.row_action.action, label: view.row_action.label, success: view.row_action.success, followup: view.row_action.followup } : null;
    const shown = visibleColumns(view, config);
    const explicitFields = typeof config?.fields === "string";
    const dateColumn = shown.find(column => column.type === "date");
    const enumColumns = columns(view, ["enum"]);
    const chips = shown.filter(column => column.type === "enum" || (explicitFields && column.key !== titleColumn && column !== dateColumn))
      .map(column => ({ text: ["enum", "date"].includes(column.type) ? `{{row.${column.key}__label}}` : `{{row.${column.key}}}` }));
    const item = {
      type: "item",
      title,
      ...(dateColumn ? { subtitle: `{{row.${dateColumn.key}__label}}` } : {}),
      ...(chips.length ? { chips } : {}),
      action: open,
      ...(rowAction ? { row_action: rowAction } : {}),
    };
    if (s.layout === "table") {
      const tableColumns = shown.map((column) => ({
        key: ["enum", "date"].includes(column.type) ? `${column.key}__label` : column.key,
        label: column.label,
        type: column.type === "enum" ? "string" : column.type,
      }));
      const sort = s.sort && ["enum", "date"].includes((view.columns || []).find((column) => column.key === s.sort)?.type) ? `${s.sort}__label` : s.sort;
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
          server_sorted: Boolean(view.pagination),
          ...(rowAction ? { row_action: rowAction } : {}),
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
      // A column marked `board: "matrix"` (four values) draws as a 2×2
      // grid, so a saved Eisenhower view reopens as the matrix.
      const matrix = groupColumn?.board === "matrix" && boardColumns.length === 4;
      return {
        state: {},
        body: {
          type: "list",
          layout: matrix ? "matrix" : "board",
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
          date: `${s.date}__day`,
          title: titleColumn,
          empty: "Nothing this month.",
          ...(rowAction ? { row_action: rowAction } : {}),
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

  function wrapBridge(base, entry, config, focus, paging, period) {
    return Object.assign({}, base, {
      query: async (name, params) => {
        if (name !== entry.query) return base.query(name, params);
        const values = filterParams(entry.view, config);
        const scoped = Object.assign({}, params, values);
        let timezone;
        const queryFocus = Object.hasOwn(entry.view.params || {}, "page_focus");
        if (queryFocus) scoped.page_focus = String(focus || "");
        if (period) {
          const bounds = typeof period === "function" ? period() : period;
          if (bounds) {
            if (!entry.view.temporal) throw new Error("This data source cannot follow a page review period.");
            timezone = bounds.timezone;
            scoped[entry.view.temporal.start_param] = bounds.start_ms;
            scoped[entry.view.temporal.end_param] = bounds.end_ms;
          }
        }
        if (entry.view.pagination) {
          if (focus && !queryFocus) throw new Error("This paged source does not support page focus. Clear this widget's focus to show it.");
          const s = settings(entry.view, config);
          if (s.sort && !(entry.view.sorts || []).length) throw new Error("This view now uses query-side sorting. Customize it and use its Sort filter instead.");
          for (const sort of entry.view.sorts || []) {
            if (s.sort) {
              if (!sort.values.includes(s.sort)) throw new Error("This sort is unavailable. Customize this view to choose a supported sort.");
              scoped[sort.param] = s.sort;
            }
            if (sort.direction_param) scoped[sort.direction_param] = s.dir;
          }
          const p = entry.view.pagination;
          const size = paging?.size || p.default_page_size || 100;
          scoped[p.page_size_param] = size + 1;
          scoped[p.offset_param] = paging?.offset || 0;
          const rows = await base.query(name, scoped);
          if (!Array.isArray(rows)) throw new Error("The data source returned invalid rows.");
          if (paging) { paging.more = rows.length > size; paging.count = Math.min(rows.length, size); paging.changed?.(); }
          return prepareRows(rows.slice(0, size), entry.view, config, "", timezone);
        }
        return prepareRows(await base.query(name, scoped), entry.view, config, queryFocus ? "" : focus, timezone);
      },
    });
  }

  function filterParams(view, config) {
    let filters = {};
    if (config?.filters) {
      if (typeof config.filters === "string") {
        try { filters = JSON.parse(config.filters); } catch (_) { throw new Error("Saved filters are invalid. Edit this view to repair them."); }
      } else if (config.filters && typeof config.filters === "object" && !Array.isArray(config.filters)) {
        filters = { ...config.filters };
      } else {
        throw new Error("Saved filters are invalid. Edit this view to repair them.");
      }
    }
    if (!filters || typeof filters !== "object" || Array.isArray(filters)) throw new Error("View filters must be an object.");
    const allowed = new Set((view.filters || []).map(f => f.param));
    for (const [key, value] of Object.entries(filters)) {
      if (!allowed.has(key) || (value !== null && typeof value === "object")) throw new Error(`Filter ${key} is no longer available. Edit this view to repair it.`);
      const definition = (view.filters || []).find(f => f.param === key);
      if (definition?.type === "number" && value !== "" && value != null) {
        if (!Number.isFinite(Number(value))) throw new Error(`${definition.label} must be a number.`);
        filters[key] = Number(value);
      }
      if (definition?.type === "boolean" && value !== "" && value != null) {
        if (![true, false, "true", "false"].includes(value)) throw new Error(`${definition.label} must be true or false.`);
        filters[key] = value === true || value === "true";
      }
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
    const pagination = options.entry.view.pagination;
    const paging = pagination ? { size: pagination.default_page_size || 100, offset: 0, more: false, count: 0 } : null;
    let previous, nextButton, count, busy = false;
    const updatePager = () => {
      if (!paging || !previous) return;
      previous.disabled = busy || paging.offset === 0;
      nextButton.disabled = busy || !paging.more;
      count.textContent = paging.count ? `Items ${paging.offset + 1}–${paging.offset + paging.count}${paging.more ? " · more available" : ""}` : "No items on this page";
      if (["board", "calendar"].includes(options.config?.layout)) count.textContent += " · showing this batch only";
    };
    if (paging) paging.changed = updatePager;
    const load = async (reset = true) => {
      if (paging && reset) paging.offset = 0;
      busy = true; updatePager();
      const seq = ++revision;
      try {
        await validateChoices(options.entry.view, options.config, options.bridge);
        const entry = await resolveChoices(options.entry, options.bridge);
        if (dead || seq !== revision) return;
        const next = JSON.stringify(entry.view);
        if (child && signature === next) { await child.refresh(); return; }
        child?.dispose(); body.replaceChildren(); signature = next;
        let content = body;
        if (paging) {
          content = document.createElement("div"); body.append(content);
          const controls = document.createElement("nav"); controls.className = "pp-view-pagination"; controls.setAttribute("aria-label", "View result pages");
          previous = document.createElement("button"); previous.type = "button"; previous.textContent = "Previous";
          nextButton = document.createElement("button"); nextButton.type = "button"; nextButton.textContent = "Next";
          count = document.createElement("span"); count.setAttribute("aria-live", "polite");
          previous.addEventListener("click", () => { if (busy || !paging.offset) return; paging.offset = Math.max(0, paging.offset - paging.size); return load(false); });
          nextButton.addEventListener("click", () => { if (busy || !paging.more) return; paging.offset += paging.size; return load(false); });
          controls.append(previous, count, nextButton); body.append(controls); updatePager();
        }
        child = (options.mount || root.TerrariumRenderer.mount)(content, template(entry, options.config, options.size),
          wrapBridge(options.bridge, entry, options.config, options.focus, paging, options.period), { snapshot: options.snapshot, size: options.size,
            page: { focus: options.focus || "", title: options.pageTitle || "" } });
        await child.ready;
      } catch (error) {
        if (dead || seq !== revision) return;
        child?.dispose(); child = null; signature = "";
        const message = document.createElement("p"); message.className = "pp-error";
        message.textContent = String(error); body.replaceChildren(message);
      } finally {
        if (seq === revision) { busy = false; updatePager(); }
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
