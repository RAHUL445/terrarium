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
    let initialized = false;
    let recordRequest = 0;
    let refreshRequest = 0;
    let filterValues = {};
    let filterControls = new Map();
    let filterChoices = new Map();
    let validControls = false;

    const rootNode = element("div", "pp-stack");
    rootNode.style.flexDirection = "column";
    rootNode.style.gap = "12px";
    const toolbar = element("div", "pp-stack");
    toolbar.style.flexWrap = "wrap";
    toolbar.style.gap = "8px";
    const savedSelect = element("select", "pp-input");
    savedSelect.setAttribute("aria-label", "Saved view");
    savedSelect.style.maxWidth = "240px";
    const newButton = element("button", "pp-button", "New view");
    newButton.type = "button";
    toolbar.append(savedSelect, newButton);

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
    const filtersArea = element("div", "pp-stack");
    filtersArea.style.flexWrap = "wrap";
    filtersArea.style.gap = "8px";

    const note = element("p", "muted", "Saving changes updates every page that uses this shared view.");
    const error = element("p", "error");
    error.setAttribute("role", "alert");
    error.hidden = true;
    const actions = element("div", "pp-stack");
    actions.style.flexWrap = "wrap";
    actions.style.gap = "8px";
    const applyButton = makeButton("Apply", "pp-button pp-primary", actions);
    const saveButton = makeButton("Save changes", "pp-button", actions);
    const saveAsButton = makeButton("Save as new", "pp-button", actions);
    const removeButton = makeButton("Delete view", "pp-button", actions);
    const previewNode = element("div", "pp-view-library-preview");
    node.replaceChildren(rootNode);
    rootNode.append(toolbar, controls, filtersArea, note, error, actions, previewNode);

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
        if (control.multiple) value = Array.from(control.selectedOptions, (item) => item.value).join(",");
        else value = control.value;
        const defaultValue = entryResult.entry.view.params?.[definition.param];
        // An empty selection clears a non-empty query default; otherwise it means all.
        if (value !== "" || (defaultValue != null && String(defaultValue) !== "")) filters[definition.param] = value;
      }
      return savedConfig(
        pluginId,
        queryField.select.value,
        layoutField.select.value,
        sortField.select.value,
        dirField.select.value,
        groupField.select.value,
        dateField.select.value,
        JSON.stringify(filters),
      );
    }
    function destroyPreview() {
      preview?.dispose?.();
      preview = null;
      previewNode.replaceChildren();
    }
    async function renderPreview(config) {
      const generationAtStart = ++generation;
      destroyPreview();
      const found = viewEntry(config);
      if (found.error) { setError(found.error); return false; }
      try {
        const mounted = VIEWS().mount(previewNode, {
          entry: found.entry,
          config,
          bridge,
          focus: "",
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
        option(select, "", defaultChoice ? `Default (${defaultChoice.label})` : "All");
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
        select.value = selectedValues[0] || "";
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
        const field = element("label", "pp-field");
        field.append(element("span", "pp-label", definition.label || definition.param));
        const control = element(isChoiceFilter(definition) ? "select" : "input", "pp-input");
        if (control.tagName === "INPUT") control.type = "text";
        if (definition.multiple && control.tagName === "SELECT") control.setAttribute("aria-label", `${definition.label || definition.param} (multiple)`);
        else control.setAttribute("aria-label", definition.label || definition.param);
        const saved = filterValues[definition.param];
        const selected = saved == null || saved === "" ? [] : definition.multiple ? String(saved).split(",") : [String(saved)];
        if (control.tagName === "SELECT") fillChoiceSelect(control, filterChoices.get(definition.param) || [], selected, Boolean(definition.multiple), view.params?.[definition.param]);
        else control.value = saved == null ? "" : String(saved);
        control.addEventListener(control.tagName === "SELECT" ? "change" : "input", () => {
          filterValues[definition.param] = control.multiple ? Array.from(control.selectedOptions, (item) => item.value).join(",") : control.value;
        });
        filterControls.set(definition.param, control);
        field.append(control);
        filtersArea.append(field);
      }
      const columns = Array.isArray(view.columns) ? view.columns : [];
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
      labels(sortField, () => true);
      labels(groupField, (column) => column.type === "enum");
      labels(dateField, (column) => column.type === "date");
      filterValues = filterDefaults(view, defs, parsed.values);
      titleInput.value = currentRecord?.title || "";
      const settings = VIEWS().settings(view, config);
      layoutField.select.value = settings.layout;
      sortField.select.value = settings.sort;
      dirField.select.value = settings.dir;
      groupField.select.value = settings.group;
      dateField.select.value = settings.date;
      const supports = new Set(VIEWS().layouts(view));
      layoutField.field.hidden = supports.size < 2;
      groupField.field.hidden = !columns.some((column) => column.type === "enum");
      dateField.field.hidden = !columns.some((column) => column.type === "date");
      saveButton.disabled = !selectedId || !selectedAvailable;
      removeButton.disabled = !selectedId || !selectedAvailable;
      validControls = true;
      return true;
    }
    function setDraft(config, record) {
      recordRequest++;
      currentRecord = record || null;
      selectedId = record?.id || "";
      selectedAvailable = Boolean(record);
      draftConfig = { ...(config || {}) };
      initialized = true;
      titleInput.value = record?.title || "";
      return buildControls(draftConfig).then((ok) => ok && renderPreview(draftConfig));
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
        return await renderPreview(config);
      } catch (e) { setError(String(e?.message || e)); return false; }
    }
    async function save(asNew) {
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
        currentRecord = record;
        selectedId = record.id;
        draftConfig = { ...record.config };
        await refresh();
        savedSelect.value = selectedId;
        await setDraft(record.config, record);
        toast(updating ? "Updated shared view" : "Saved new view", "ok");
        return true;
      } catch (e) { setError(String(e?.message || e)); return false; }
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
    newButton.addEventListener("click", () => { setDraft(defaultConfig(), null); });
    queryField.select.addEventListener("change", () => {
      const next = { ...defaultConfig(), query: queryField.select.value };
      setDraft(next, currentRecord);
    });
    applyButton.addEventListener("click", apply);
    saveButton.addEventListener("click", () => save(false));
    saveAsButton.addEventListener("click", () => save(true));
    removeButton.addEventListener("click", remove);

    const ready = refresh();
    return {
      ready,
      refresh,
      flush: async () => preview?.flush ? preview.flush() : true,
      dispose() {
        if (disposed) return;
        disposed = true;
        generation++;
        destroyPreview();
        node.replaceChildren();
      },
    };
  }

  const api = { mount, readFilters, isChoiceFilter, filterDefaults, savedConfig };
  if (typeof module !== "undefined" && module.exports) module.exports = api;
  else root.TerrariumViewLibrary = api;
})(typeof window === "undefined" ? globalThis : window);
