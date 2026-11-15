(function (root) {
  "use strict";

  const VIEWS = () => root.TerrariumViews;

  function element(tag, className, text) {
    const node = document.createElement(tag);
    if (className) node.className = className;
    if (text != null) node.textContent = String(text);
    return node;
  }

  function option(select, value, label) {
    const node = element("option", "", label);
    node.value = value;
    select.append(node);
    return node;
  }

  function readFilters(raw, definitions) {
    if (raw == null || raw === "") return { values: {}, error: "" };
    if (typeof raw !== "string") return { values: {}, error: "Saved filters must be JSON text." };
    let parsed;
    try { parsed = JSON.parse(raw); }
    catch (_) { return { values: {}, error: "Saved filters are not valid JSON." }; }
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
      return { values: {}, error: "Saved filters must be a JSON object." };
    }
    const allowed = new Map((definitions || []).map((filter) => [filter.param, filter]));
    for (const [key, value] of Object.entries(parsed)) {
      if (!allowed.has(key)) return { values: parsed, error: `This view uses an unavailable filter: ${key}.` };
      if (!(value == null || ["string", "number", "boolean"].includes(typeof value))) {
        return { values: parsed, error: `Filter ${key} has an invalid value.` };
      }
    }
    return { values: parsed, error: "" };
  }

  function isChoiceFilter(definition) {
    return Boolean(definition?.choices_query || (Array.isArray(definition?.values) && definition.values.length > 0));
  }

  function filterDefaults(view, definitions, saved) {
    const defaults = {};
    const params = view?.params && typeof view.params === "object" ? view.params : {};
    for (const definition of definitions || []) {
      const source = Object.hasOwn(saved || {}, definition.param) ? saved[definition.param] : params[definition.param];
      if (source == null) continue;
      if (["string", "number", "boolean"].includes(typeof source)) defaults[definition.param] = String(source);
    }
    return defaults;
  }

  function savedConfig(plugin, query, layout, sort, dir, group, date, filters) {
    const config = { plugin, query, layout, dir, filters };
    for (const [key, value] of Object.entries({ sort, group, date })) {
      if (value) config[key] = value;
    }
    return config;
  }

  // Native checkboxes keep multiple selection usable with touch and keyboard.
  function choicePicker(label, choices, selected, onChange) {
    const node = element("details", "pp-choice-picker");
    const summary = element("summary", "pp-input");
    const panel = element("div", "pp-choice-panel");
    const search = element("input", "pp-input");
    search.type = "search"; search.placeholder = `Find ${label.toLowerCase()}…`;
    search.setAttribute("aria-label", `Find ${label.toLowerCase()}`);
    const clear = element("button", "pp-button pp-ghost", "Clear selection"); clear.type = "button";
    const list = element("div", "pp-choice-options");
    const values = new Set(selected), checks = [];
    const available = new Map(choices.map(choice => [String(choice.value), String(choice.label ?? choice.value)]));
    for (const value of selected) if (!available.has(value)) available.set(value, `Unavailable choice · ${value}`);
    const update = () => {
      summary.textContent = values.size ? `${label}: ${values.size} selected` : `${label}: Any`;
      for (const { input } of checks) input.checked = values.has(input.value);
    };
    for (const [value, title] of available) {
      const row = element("label", "pp-choice-option");
      const input = element("input"); input.type = "checkbox"; input.value = value;
      input.setAttribute("aria-label", `${label}: ${title}`);
      row.append(input, element("span", "", title)); list.append(row); checks.push({ row, input, title });
      input.addEventListener("change", () => { input.checked ? values.add(value) : values.delete(value); update(); onChange?.(); });
    }
    clear.addEventListener("click", () => { values.clear(); update(); onChange?.(); });
    search.addEventListener("input", () => {
      const query = search.value.trim().toLowerCase();
      for (const { row, title } of checks) row.hidden = !title.toLowerCase().includes(query);
    });
    node.addEventListener("keydown", event => {
      if (event.key === "Escape") { node.open = false; summary.focus(); event.stopPropagation(); }
    });
    panel.append(search, clear, list); node.append(summary, panel); update();
    return { node, read: () => [...values].join(",") };
  }

  function mount(node, spec, options) {
    const opts = options || {};
    const pluginId = String(opts.plugin || "");
    const preferredQuery = typeof spec?.query === "string" ? spec.query : "";
    const plugins = Array.isArray(opts.plugins) ? opts.plugins : [];
    const plugin = plugins.find((item) => item?.id === pluginId);
    const bridge = opts.bridge;
    const service = opts.service;
    const toast = typeof opts.toast === "function" ? opts.toast : () => {};
    let disposed = false;
    let generation = 0;
    let preview = null;
    let selectedId = "";
    let selectedAvailable = false;
    let currentRecord = null;
    let draftConfig = null;
    let appliedConfig = null;
    let initialized = false;
    let recordRequest = 0;
    let refreshRequest = 0;
    let filterValues = {};
    let filterControls = new Map();
    let filterChoices = new Map();
    let validControls = false;
    let fieldControls = [];
    let saving = false;
    let reuseShared = false;
    let groupChoiceRefresh = () => {};

    const rootNode = element("div", "pp-stack");
    rootNode.style.flexDirection = "column";
    rootNode.style.gap = "12px";
    const toolbar = element("div", "pp-stack");
    toolbar.style.flexWrap = "wrap";
    toolbar.style.gap = "8px";
    const savedSelect = element("select", "pp-input");
    savedSelect.setAttribute("aria-label", "Saved view");
    savedSelect.style.maxWidth = "240px";
    const newButton = element("button", "pp-button", opts.localEditor ? "Make independent copy" : "New view");
    newButton.type = "button";
    toolbar.append(savedSelect, newButton);
    const reuseButton = opts.localEditor ? makeButton("Use shared view", "pp-button", toolbar) : null;
    // A page that lists saved views itself opens one (`selected`) or starts a
    // new one from its current filters (`draft`); the picker is then hidden.
    const selectedOnOpen = typeof spec?.selected === "string" ? spec.selected : "";
    const draftOnOpen = spec?.draft && typeof spec.draft === "object" ? spec.draft : null;
    toolbar.hidden = Boolean(!opts.localEditor && (spec?.selected !== undefined || draftOnOpen));
    const summary = element("p", "pp-view-summary");
    const editor = element("details", "pp-view-editor");
    editor.append(element("summary", "pp-button pp-ghost", "Customize view"));
    const editorBody = element("div", "pp-view-editor-body");
    editor.append(editorBody);

    const controls = element("div", "pp-stack");
    controls.style.flexWrap = "wrap";
    controls.style.alignItems = "end";
    controls.style.gap = "8px";
    const titleField = element("label", "pp-field");
    const titleLabel = element("span", "pp-label", "View name");
    const titleInput = element("input", "pp-input");
    titleInput.type = "text";
    titleInput.maxLength = 40;
    titleInput.autocomplete = "off";
    titleField.append(titleLabel, titleInput);
    controls.append(titleField);

    const queryField = makeSelect("Data", controls);
    const layoutField = makeSelect("Layout", controls);
    const sortField = makeSelect("Sort by", controls);
    const dirField = makeSelect("Direction", controls);
    option(dirField.select, "asc", "Ascending");
    option(dirField.select, "desc", "Descending");
    const groupField = makeSelect("Group by", controls);
    const dateField = makeSelect("Calendar date", controls);
    layoutField.select.addEventListener("change", () => groupChoiceRefresh());
    const densityField = makeSelect("Density", controls);
    option(densityField.select, "comfortable", "Comfortable");
    option(densityField.select, "compact", "Compact");
    const fieldsArea = element("fieldset", "pp-view-fields");
    const filtersArea = element("div", "pp-stack");
    filtersArea.style.flexWrap = "wrap";
    filtersArea.style.gap = "8px";

    const note = element("p", "muted", opts.localEditor
      ? "Changes stay on this page when you press Apply. Preview changes uses the current page focus and review period. Save as shared view creates a reusable view."
      : "Saving changes updates every page that uses this shared view.");
    const error = element("p", "error");
    error.setAttribute("role", "alert");
    error.hidden = true;
    const actions = element("div", "pp-stack");
    actions.style.flexWrap = "wrap";
    actions.style.gap = "8px";
    const applyButton = makeButton(opts.localEditor ? "Preview changes" : "Apply", "pp-button pp-primary", actions);
    const saveButton = makeButton("Save changes", "pp-button", actions);
    const saveAsButton = makeButton(opts.localEditor ? "Save as shared view" : "Save as new", "pp-button", actions);
    const removeButton = makeButton("Delete view", "pp-button", actions);
    const resetButton = makeButton("Reset changes", "pp-button pp-ghost", actions);
    if (opts.localEditor) {
      saveButton.hidden = true;
      removeButton.hidden = true;
      applyButton.title = "Preview this local draft; page Apply saves it to this placement.";
    }
    const previewNode = element("div", "pp-view-library-preview");
    node.replaceChildren(rootNode);
    editorBody.append(controls, filtersArea, fieldsArea, note, actions);
    rootNode.append(toolbar, summary, editor, error, previewNode);

    function changed() {
      if (opts.localEditor) reuseShared = false;
      summary.textContent = "Unsaved changes · Apply to preview, or save to keep them.";
      if (opts.localEditor && validControls) {
        try { opts.onConfigChange?.(readControlConfig()); } catch (_) { /* partially edited controls */ }
      }
    }
    editor.addEventListener("input", changed);
    editor.addEventListener("change", changed);
    function describe(config) {
      const entry = viewEntry(config).entry;
      const filters = readFilters(config?.filters, entry?.view.filters || []).values;
      const active = (entry?.view.filters || []).filter(def => filters[def.param] != null && filters[def.param] !== "");
      summary.textContent = `${currentRecord?.title || "New view"} · ${config.layout || "list"} · ${active.length ? `${active.length} filters` : "Default filters"}`;
    }

    function makeSelect(label, parent) {
      const field = element("label", "pp-field");
      const caption = element("span", "pp-label", label);
      const select = element("select", "pp-input");
      field.append(caption, select);
      parent.append(field);
      return { field, select };
    }
    function makeButton(label, className, parent) {
      const button = element("button", className, label);
      button.type = "button";
      parent.append(button);
      return button;
    }
    function setError(message) {
      error.textContent = message || "";
      error.hidden = !message;
    }
    function unavailableMessage() {
      return `This saved view is unavailable in this space. Your draft is preserved; use Save as new to keep it.`;
    }
    function viewEntry(config) {
      if (!plugin) return { error: `Plugin “${pluginId || "(missing)"}” is unavailable.` };
      if (config?.plugin !== pluginId) return { error: `This view belongs to unavailable plugin “${config?.plugin || "(missing)"}”.` };
      const entry = VIEWS()?.find(plugins, pluginId, config?.query);
      if (!entry) return { error: `Query “${config?.query || "(missing)"}” is unavailable in ${plugin.name || pluginId}.` };
      return { entry };
    }
    function readControlConfig() {
      const entryResult = viewEntry(draftConfig);
      if (entryResult.error) throw new Error(entryResult.error);
      const filters = {};
      for (const definition of entryResult.entry.view.filters || []) {
        const control = filterControls.get(definition.param);
        if (!control) continue;
        let value;
        if (control.read) value = control.read();
        else if (control.multiple) value = Array.from(control.selectedOptions, (item) => item.value).join(",");
        else if (control.dataset.filterType === "boolean" && control.value !== "") value = control.value === "true";
        else if (control.type === "number" && control.value !== "") value = Number(control.value);
        else value = control.value;
        const defaultValue = entryResult.entry.view.params?.[definition.param];
        // An empty selection clears a non-empty query default; otherwise it means all.
        if (value !== "" || (defaultValue != null && String(defaultValue) !== "")) filters[definition.param] = value;
      }
      const config = savedConfig(
        pluginId,
        queryField.select.value,
        layoutField.select.value,
        sortField.select.value,
        dirField.select.value,
        groupField.select.value,
        dateField.select.value,
        JSON.stringify(filters),
      );
      config.density = densityField.select.value;
      config.fields = fieldControls.filter(input => input.checked).map(input => input.value).join(",");
      return config;
    }
    function destroyPreview() {
      preview?.dispose?.();
      preview = null;
      previewNode.replaceChildren();
    }
    async function renderPreview(config) {
      appliedConfig = { ...config };
      const generationAtStart = ++generation;
      destroyPreview();
      const found = viewEntry(config);
      if (found.error) { setError(found.error); return false; }
      try {
        const focus = typeof opts.focus === "function" ? opts.focus() : opts.focus;
        const mounted = VIEWS().mount(previewNode, {
          entry: found.entry,
          config,
          bridge,
          focus: focus == null ? "" : String(focus),
          period: opts.period || null,
          size: "regular",
        });
        preview = mounted;
        await mounted.ready;
        if (disposed || generationAtStart !== generation) {
          mounted.dispose?.();
          if (preview === mounted) preview = null;
          return false;
        }
        const firstPreviewChild = previewNode.firstElementChild;
        const renderError = firstPreviewChild?.classList.contains("pp-error") ? firstPreviewChild : null;
        if (renderError) { setError(renderError.textContent); return false; }
        setError(selectedId && !selectedAvailable ? unavailableMessage() : "");
        opts.onPreview?.(config);
        return true;
      } catch (e) {
        if (!disposed && generationAtStart === generation) setError(String(e?.message || e));
        return false;
      }
    }
    function fillChoiceSelect(select, choices, selectedValues, multiple, fallback) {
      select.replaceChildren();
      const hasEmptyChoice = choices.some((item) => item && item.value != null && String(item.value) === "");
      if (!multiple && !hasEmptyChoice) {
        const defaultChoice = choices.find(item => String(item.value) === String(fallback));
        if (!defaultChoice) option(select, "", "Any");
      }
      const known = new Set();
      for (const item of choices) {
        if (!item || item.value == null) continue;
        const value = String(item.value);
        if (known.has(value)) continue;
        known.add(value);
        option(select, value, item.label == null ? value : String(item.label));
      }
      for (const value of selectedValues) {
        if (known.has(value)) continue;
        const missing = option(select, value, `Unavailable choice · ${value}`);
        missing.dataset.missing = "true";
      }
      if (multiple) {
        select.multiple = true;
        for (const item of select.options) item.selected = selectedValues.includes(item.value);
      } else {
        select.multiple = false;
        select.value = selectedValues[0] || (hasEmptyChoice ? "" : String(fallback ?? ""));
      }
    }
    async function buildControls(config) {
      validControls = false;
      const found = viewEntry(config);
      if (found.error) { setError(found.error); return false; }
      const view = found.entry.view;
      const defs = Array.isArray(view.filters) ? view.filters : [];
      const parsed = readFilters(config.filters, defs);
      if (parsed.error) { setError(parsed.error); return false; }
      filterValues = filterDefaults(view, defs, parsed.values);
      const generationAtStart = ++generation;
      let loaded;
      try {
        loaded = await Promise.all(defs.map(async (definition) => {
          if (!definition.choices_query) {
            return [definition.param, (definition.values || []).map((value, index) => ({ value, label: definition.labels?.[index] || value }))];
          }
          const rows = await bridge.query(definition.choices_query, {});
          return [definition.param, (Array.isArray(rows) ? rows : []).map((row) => ({ value: row?.value, label: row?.label }))];
        }));
      } catch (e) {
        if (!disposed && generationAtStart === generation) setError(`Could not load filter choices: ${String(e?.message || e)}`);
        return false;
      }
      if (disposed || generationAtStart !== generation) return false;
      filterChoices = new Map(loaded);
      filterControls = new Map();
      filtersArea.replaceChildren();
      for (const definition of defs) {
        const field = element(definition.multiple ? "div" : "label", "pp-field");
        field.append(element("span", "pp-label", definition.label || definition.param));
        const booleanFilter = definition.type === "boolean";
        const nativeTypedFilter = definition.type === "date" || definition.type === "number";
        const control = element((isChoiceFilter(definition) && !nativeTypedFilter) || booleanFilter ? "select" : "input", "pp-input");
        control.dataset.filterType = definition.type || "";
        if (control.tagName === "INPUT") control.type = definition.type === "date" || definition.type === "number" ? definition.type : "text";
        if (definition.multiple && control.tagName === "SELECT") control.setAttribute("aria-label", `${definition.label || definition.param} (multiple)`);
        else control.setAttribute("aria-label", definition.label || definition.param);
        const saved = filterValues[definition.param];
        const selected = saved == null || saved === "" ? [] : definition.multiple ? String(saved).split(",") : [String(saved)];
        if (definition.multiple && isChoiceFilter(definition)) {
          const picker = choicePicker(definition.label || definition.param, filterChoices.get(definition.param) || [], selected, changed);
          filterControls.set(definition.param, picker);
          field.append(picker.node); filtersArea.append(field); continue;
        }
        if (control.tagName === "SELECT") {
          const choices = booleanFilter
            ? [{ value: "true", label: "Yes" }, { value: "false", label: "No" }]
            : filterChoices.get(definition.param) || [];
          fillChoiceSelect(control, choices, selected, Boolean(definition.multiple), view.params?.[definition.param]);
        }
        else control.value = saved == null ? "" : String(saved);
        control.addEventListener(control.tagName === "SELECT" ? "change" : "input", () => {
          filterValues[definition.param] = control.multiple ? Array.from(control.selectedOptions, (item) => item.value).join(",") : control.value;
        });
        filterControls.set(definition.param, control);
        field.append(control);
        filtersArea.append(field);
      }
      const columns = Array.isArray(view.columns) ? view.columns : [];
      fieldsArea.replaceChildren(element("legend", "", "Visible fields"));
      fieldControls = [];
      const chosenFields = typeof config.fields === "string" ? config.fields.split(",") : columns.map(column => column.key);
      for (const column of columns) {
        const label = element("label", "pp-choice-option");
        const input = element("input"); input.type = "checkbox"; input.value = column.key;
        input.disabled = column.key === (view.title_column || "title");
        input.checked = input.disabled || chosenFields.includes(column.key);
        input.setAttribute("aria-label", `Show ${column.label}`);
        label.append(input, element("span", "", column.label)); fieldsArea.append(label); fieldControls.push(input);
      }
      densityField.select.value = config.density === "compact" ? "compact" : "comfortable";
      queryField.select.replaceChildren();
      for (const entry of VIEWS().viewsOf([plugin])) option(queryField.select, entry.query, entry.view.title || entry.query);
      queryField.select.value = config.query;
      const labels = (field, predicate) => {
        field.select.replaceChildren();
        option(field.select, "", "None");
        for (const column of columns.filter(predicate)) option(field.select, column.key, column.label || column.key);
      };
      layoutField.select.replaceChildren(...VIEWS().layouts(view).map((layout) => {
        const item = element("option", "", layout[0].toUpperCase() + layout.slice(1)); item.value = layout; return item;
      }));
      const declaredSortValues = view.pagination || Array.isArray(view.sorts)
        ? new Set((Array.isArray(view.sorts) ? view.sorts : []).flatMap(sort => Array.isArray(sort?.values) ? sort.values.map(String) : []))
        : null;
      labels(sortField, column => !declaredSortValues || declaredSortValues.has(String(column.key)));
      labels(dateField, (column) => column.type === "date");
      filterValues = filterDefaults(view, defs, parsed.values);
      titleInput.value = currentRecord?.title || "";
      const settings = VIEWS().settings(view, config);
      layoutField.select.value = settings.layout;
      sortField.select.value = settings.sort;
      dirField.select.value = settings.dir;
      let groupInitialized = false;
      groupChoiceRefresh = () => {
        const board = layoutField.select.value === "board";
        labels(groupField, column => column.type === "enum" || (!board && column.type === "string"));
        groupField.select.value = VIEWS().settings(view, {
          ...config,
          layout: layoutField.select.value,
          group: groupInitialized ? groupField.select.value : settings.group,
        }).group;
        groupInitialized = true;
      };
      groupChoiceRefresh();
      dateField.select.value = settings.date;
      const supports = new Set(VIEWS().layouts(view));
      layoutField.field.hidden = supports.size < 2;
      groupField.field.hidden = !columns.some((column) => column.type === "enum" || column.type === "string");
      dateField.field.hidden = !columns.some((column) => column.type === "date");
      saveButton.disabled = !selectedId || !selectedAvailable;
      removeButton.disabled = !selectedId || !selectedAvailable;
      if (reuseButton) reuseButton.disabled = !selectedId || !selectedAvailable;
      newButton.disabled = false;
      saveAsButton.disabled = false;
      validControls = true;
      describe(config);
      if (opts.localEditor) opts.onConfigChange?.(readControlConfig());
      return true;
    }
    function setDraft(config, record, previewConfig = config) {
      recordRequest++;
      currentRecord = record || null;
      selectedId = record?.id || "";
      selectedAvailable = Boolean(record);
      if (!record) reuseShared = false;
      draftConfig = { ...(config || {}) };
      initialized = true;
      titleInput.value = record?.title || "";
      editor.open = !record;
      return buildControls(draftConfig).then((ok) => ok && renderPreview(previewConfig));
    }
    function configFromFields() {
      if (!validControls) throw new Error("Choose a valid plugin query and load its filter choices first.");
      const config = readControlConfig();
      return config;
    }
    async function loadRecord(id) {
      const request = ++recordRequest;
      if (!id) return setDraft(defaultConfig(), null);
      setError("");
      try {
        const record = await service("view_get", { id });
        if (disposed || request !== recordRequest) return false;
        if (!record || typeof record !== "object") throw new Error("Saved view was not returned.");
        if (opts.localEditor) reuseShared = false;
        return setDraft(record.config, record);
      } catch (e) {
        if (!disposed && request === recordRequest) setError(String(e?.message || e));
        return false;
      }
    }
    function defaultConfig() {
      const available = VIEWS().viewsOf([plugin]).filter((entry) => entry.plugin.id === pluginId);
      const first = available.find((entry) => entry.query === preferredQuery) || available[0];
      if (!first) return { plugin: pluginId, query: "" };
      return { plugin: pluginId, query: first.query, layout: "list", sort: "", dir: "asc", group: "", date: "", filters: "{}" };
    }
    // Only declared filters with plain values are kept; unknown layouts and
    // sorts fall back through the normal view settings.
    function configFromDraft(draft) {
      const config = defaultConfig();
      for (const key of ["layout", "sort", "dir", "group", "date", "density"]) {
        if (typeof draft[key] === "string" && draft[key]) config[key] = draft[key];
      }
      // `layouts` maps a page's own layout name to view settings, e.g.
      // {"kanban": {"layout": "board", "group": "board_lane"}}.
      const mapped = draft.layouts && typeof draft.layouts === "object" && typeof draft.layout === "string"
        && Object.hasOwn(draft.layouts, draft.layout) ? draft.layouts[draft.layout] : null;
      if (mapped && typeof mapped === "object") {
        for (const key of ["layout", "group", "sort", "dir", "date"]) {
          if (typeof mapped[key] === "string" && mapped[key]) config[key] = mapped[key];
        }
      }
      const definitions = new Set((VIEWS().find(plugins, pluginId, config.query)?.view.filters || []).map((filter) => filter.param));
      const filters = {};
      for (const [key, value] of Object.entries(draft.filters && typeof draft.filters === "object" ? draft.filters : {})) {
        if (definitions.has(key) && value != null && value !== "" && ["string", "number", "boolean"].includes(typeof value)) filters[key] = value;
      }
      config.filters = JSON.stringify(filters);
      return config;
    }
    async function startDraft(draft) {
      const ok = await setDraft(configFromDraft(draft), null);
      if (disposed) return false;
      if (typeof draft.title === "string" && !titleInput.value) titleInput.value = draft.title.slice(0, 40);
      summary.textContent = "New view · Name it, then Save as new to keep it.";
      return ok;
    }
    async function refresh() {
      if (disposed) return false;
      if (!plugin) { setError(`Plugin “${pluginId || "(missing)"}” is unavailable.`); return false; }
      const request = ++refreshRequest;
      try {
        const records = await service("views_list", {});
        if (disposed || request !== refreshRequest) return false;
        const own = (Array.isArray(records) ? records : []).filter((record) => record?.config?.plugin === pluginId);
        savedSelect.replaceChildren();
        option(savedSelect, "", "Choose a saved view");
        for (const record of own) option(savedSelect, String(record.id), record.title || "Untitled view");
        if (opts.localEditor && !initialized && opts.config?.saved_view) {
          const requestedId = String(opts.config.saved_view);
          if (!own.some(record => String(record.id) === requestedId)) {
            selectedId = requestedId;
            selectedAvailable = false;
            currentRecord = { id: requestedId, title: "Unavailable shared view" };
            reuseShared = true;
            initialized = true;
            option(savedSelect, requestedId, `Unavailable view · ${requestedId}`);
            savedSelect.value = requestedId;
            saveButton.disabled = true;
            removeButton.disabled = true;
            newButton.disabled = true;
            saveAsButton.disabled = true;
            if (reuseButton) reuseButton.disabled = true;
            setError(unavailableMessage());
            return false;
          }
        }
        const selectedRecord = own.find((record) => String(record.id) === String(selectedId));
        if (selectedId && !selectedRecord) {
          selectedAvailable = false;
          saveButton.disabled = true;
          removeButton.disabled = true;
          option(savedSelect, selectedId, `Unavailable view · ${currentRecord?.title || titleInput.value || selectedId}`);
          savedSelect.value = selectedId;
          setError(unavailableMessage());
          return false;
        }
        if (selectedRecord) {
          const wasUnavailable = !selectedAvailable;
          selectedAvailable = true;
          saveButton.disabled = !validControls;
          removeButton.disabled = false;
          if (wasUnavailable) setError("");
        }
        savedSelect.value = selectedRecord ? selectedId : "";
        if (selectedRecord) {
          // Keep the in-progress edits; list refresh is for external updates.
          return preview?.refresh ? preview.refresh().then(() => true) : true;
        }
        if (initialized) return preview?.refresh ? preview.refresh().then(() => true) : true;
        if (draftOnOpen) return startDraft(draftOnOpen);
        const snapshotFits = !selectedOnOpen || opts.snapshot?.selectedId === selectedOnOpen;
        if (selectedOnOpen && !snapshotFits) return loadRecord(selectedOnOpen);
        if (opts.snapshot?.config && !initialized && snapshotFits) {
          const snapshot = opts.snapshot;
          const record = own.find(record => record.id === snapshot.selectedId) || (snapshot.selectedId ? { id: snapshot.selectedId, title: snapshot.title, config: snapshot.config } : null);
          await setDraft(snapshot.config, record, snapshot.appliedConfig || snapshot.config);
          if (snapshot.selectedId && !own.some(record => record.id === snapshot.selectedId)) option(savedSelect, snapshot.selectedId, "Unavailable saved view");
          savedSelect.value = snapshot.selectedId || "";
          titleInput.value = snapshot.title || "";
          editor.open = Boolean(snapshot.open);
          if (snapshot.selectedId && !own.some(record => record.id === snapshot.selectedId)) {
            selectedAvailable = false; saveButton.disabled = true; removeButton.disabled = true; setError(unavailableMessage());
          }
          if (snapshot.dirty) changed();
          return true;
        }
        if (opts.localEditor && opts.config?.saved_view) {
          const record = own.find(item => String(item.id) === String(opts.config.saved_view));
          if (record) {
            const ok = await setDraft(record.config, record);
            reuseShared = Boolean(ok);
            return ok;
          }
          setError(unavailableMessage());
        }
        if (opts.localEditor && opts.config && !opts.config.saved_view) return setDraft(opts.config, null);
        return setDraft(defaultConfig(), null);
      } catch (e) {
        if (!disposed && request === refreshRequest) setError(String(e?.message || e));
        return false;
      }
    }
    async function apply() {
      try {
        const config = configFromFields();
        const parsed = readFilters(config.filters, viewEntry(config).entry?.view.filters || []);
        if (parsed.error) throw new Error(parsed.error);
        draftConfig = config;
        const ok = await renderPreview(config);
        if (ok) {
          summary.textContent = opts.localEditor
            ? "Previewing local changes · Press Apply to save them to this page."
            : "Previewing changes · Save changes or Save as new to keep them.";
          editor.open = false;
        }
        return ok;
      } catch (e) { setError(String(e?.message || e)); return false; }
    }
    async function save(asNew) {
      if (saving) return false;
      saving = true;
      saveButton.disabled = true; saveAsButton.disabled = true;
      try {
        const updating = !asNew && Boolean(selectedId);
        if (updating && !selectedAvailable) throw new Error("This saved view is unavailable. Use Save as new to keep the draft.");
        const config = configFromFields();
        const title = titleInput.value.trim();
        if (!title) throw new Error("Enter a name for this view.");
        if (!viewEntry(config).entry) throw new Error(viewEntry(config).error);
        const record = await service("view_save", {
          ...(asNew || !selectedId ? {} : { id: selectedId }),
          title,
          config,
        });
        if (disposed) return false;
        bridge.changed?.();
        if (opts.localEditor) {
          selectedId = record.id;
          currentRecord = record;
          selectedAvailable = true;
          reuseShared = false;
          await refresh();
          toast("Saved as shared view", "ok");
          return true;
        }
        currentRecord = record;
        selectedId = record.id;
        draftConfig = { ...record.config };
        await refresh();
        savedSelect.value = selectedId;
        await setDraft(record.config, record);
        toast(updating ? "Updated shared view" : "Saved new view", "ok");
        opts.onSaved?.(record);
        return true;
      } catch (e) { setError(String(e?.message || e)); editor.open = true; return false; }
      finally {
        saving = false;
        saveButton.disabled = !validControls || !selectedId || !selectedAvailable;
        saveAsButton.disabled = false;
      }
    }
    async function remove() {
      if (!selectedId || !selectedAvailable) return false;
      const id = selectedId;
      const accepted = root.terrariumConfirm
        ? await root.terrariumConfirm("Delete this saved view? Page references will remain unavailable.", { title: "Delete saved view?", confirm: "Delete view", danger: true })
        : typeof root.confirm === "function" && root.confirm("Delete this saved view? Page references will remain unavailable.");
      if (!accepted) return false;
      try {
        await service("view_remove", { id });
        if (disposed) return false;
        bridge.changed?.();
        await setDraft(defaultConfig(), null);
        toast("Deleted saved view", "ok");
        return refresh();
      } catch (e) { setError(String(e?.message || e)); return false; }
    }

    savedSelect.addEventListener("change", () => {
      if (!savedSelect.value) { savedSelect.value = selectedId; return; }
      loadRecord(savedSelect.value);
    });
    newButton.addEventListener("click", () => {
      if (opts.localEditor && selectedId && selectedAvailable) {
        reuseShared = false;
        summary.textContent = "Independent local copy · edits affect this page only.";
        return;
      }
      setDraft(defaultConfig(), null);
    });
    reuseButton?.addEventListener("click", () => {
      if (!selectedId || !selectedAvailable) return;
      reuseButton.disabled = true;
      setDraft(currentRecord.config, currentRecord).then(ok => {
        if (!ok || disposed) { reuseButton.disabled = !selectedAvailable; return; }
        reuseShared = true;
        summary.textContent = `Using shared view · ${currentRecord?.title || "Saved view"}. Changes to it appear on every page that uses it.`;
        reuseButton.disabled = false;
      });
    });
    resetButton.addEventListener("click", () => { setDraft(currentRecord?.config || defaultConfig(), currentRecord); });
    queryField.select.addEventListener("change", () => {
      const next = { ...defaultConfig(), query: queryField.select.value };
      if (opts.localEditor) savedSelect.value = "";
      setDraft(next, opts.localEditor ? null : currentRecord);
    });
    applyButton.addEventListener("click", apply);
    saveButton.addEventListener("click", () => save(false));
    saveAsButton.addEventListener("click", () => save(true));
    removeButton.addEventListener("click", remove);

    const ready = refresh();
    return {
      ready,
      refresh,
      select: (id) => {
        if (disposed || !id || String(id) === String(selectedId)) return Promise.resolve(false);
        return loadRecord(String(id));
      },
      config: () => opts.localEditor && reuseShared && selectedId ? { saved_view: selectedId } : configFromFields(),
      validate: () => Boolean(validControls && (!reuseShared || selectedAvailable)),
      flush: async () => preview?.flush ? preview.flush() : true,
      snapshot: () => ({ selectedId, title: titleInput.value, config: validControls ? readControlConfig() : draftConfig, appliedConfig,
        open: editor.open, dirty: summary.textContent.startsWith("Unsaved") || summary.textContent.startsWith("Previewing") }),
      dispose() {
        if (disposed) return;
        disposed = true;
        generation++;
        destroyPreview();
        node.replaceChildren();
      },
    };
  }

  const api = { mount, readFilters, isChoiceFilter, filterDefaults, savedConfig, choicePicker };
  if (typeof module !== "undefined" && module.exports) module.exports = api;
  else root.TerrariumViewLibrary = api;
})(typeof window === "undefined" ? globalThis : window);
