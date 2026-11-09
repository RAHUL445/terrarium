// User pages: a grid of widget placements per page. The host owns pages
// and their layout (kept in the synced `_pages` store); plugins only
// contribute the widgets they declare, each mounted with its own bridge.
(function () {
  "use strict";
  const GAP = 16;
  const ROW = 72;
  const ARROWS = ["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown"];
  const MAX_PLACEMENTS = 40;
  const BUILT_IN = { id: "_host", name: "Built in", pages: [], widgets: [], icon: "layers", color: "#64748B" };

  window.TerrariumPages = class {
    constructor(options) {
      this.o = options;
      this.el = options.el;
      this.grid = window.TerrariumGrid;
      this.space = null;
      this.pages = [];
      this.currentId = null;
      this.page = null;
      this.placements = [];
      this.draft = null;
      this.catalog = new Map();
      this.views = new Map();
      this.cards = new Map();
      this.seq = 0;
      this.added = 0;
      this.narrow = matchMedia("(max-width: 760px)");
      this.narrow.addEventListener("change", () => { this.layout(); if (this.o.onChange) this.o.onChange(); else this.navigation(); });
      this.$("page-edit").addEventListener("click", () => this.edit());
      this.$("page-cancel").addEventListener("click", () => this.cancel());
      this.$("page-done").addEventListener("click", () => this.done());
      this.$("page-delete").addEventListener("click", () => this.removePage());
      this.$("page-duplicate").addEventListener("click", () => this.duplicate());
      this.pending = new Map();
      this.io = null;
      this.mac = /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent || "");
      this.$("page-new").addEventListener("click", () => this.create());
      document.querySelectorAll("[data-page-add]").forEach((button) => button.addEventListener("click", () => this.openCatalog()));
      this.$("page-add-close").addEventListener("click", () => this.$("page-add-dialog").close());
      this.$("page-config-cancel").addEventListener("click", () => this.$("page-config-dialog").close());
      this.$("page-move-left").addEventListener("click", () => this.movePage(-1));
      this.$("page-move-right").addEventListener("click", () => this.movePage(1));
      this.$("page-icon").addEventListener("click", () => this.toggleIcons());
      this.$("page-add-search").addEventListener("input", () => this.filterCatalog());
      for (const id of ["page-title-input", "page-focus-input"]) {
        this.$(id).addEventListener("keydown", (event) => {
          if (event.key === "Enter") { event.preventDefault(); this.done(); }
        });
      }
      this.$("page-focus-chip").addEventListener("click", () => { this.edit(); this.$("page-focus-input").focus(); });
      // ⌘1–9 (Ctrl elsewhere) opens the page at that place in the tabs.
      document.addEventListener("keydown", (event) => {
        if (!(this.mac ? event.metaKey : event.ctrlKey) || event.altKey || event.shiftKey || !/^[1-9]$/.test(event.key)) return;
        const page = this.pages[Number(event.key) - 1];
        if (!page || document.querySelector("dialog[open]")) return;
        event.preventDefault();
        this.o.show(page.id);
      });
      document.addEventListener("keydown", (event) => {
        if (event.key !== "Escape" || !this.editing || document.querySelector("dialog[open]")) return;
        if (!this.$("page-icon-menu").hidden) { this.toggleIcons(false); this.$("page-icon").focus(); return; }
        event.preventDefault();
        this.discard();
      });
      document.addEventListener("pointerdown", (event) => {
        if (!this.$("page-icon-menu").hidden && !event.target.closest("#page-icon-menu, #page-icon")) this.toggleIcons(false);
      });
    }

    $(id) { return document.getElementById(id); }
    service(name, args) { return this.o.call("service", { name, args }); }
    get editing() { return this.draft !== null; }
    get home() { return this.pages.find((page) => page.system) || null; }
    isHome() { return !this.page || this.page.system; }
    title() { return this.page?.title || "Home"; }
    list() { return this.draft || this.placements; }
    mounted() { return [...this.views.values()]; }
    key(placement) { return `${placement.plugin}/${placement.widget}`; }
    sessionKey(id) { return `${this.space}/placement/${id}`; }
    // Whether a widget can filter its items by a project, tag or word.
    follows(entry) { return !!entry?.template?.page?.focus; }
    filterable(entry) { return this.follows(entry) || Object.values(entry?.template?.page?.config || {}).some((f) => f?.type === "choice" && f.filter !== false); }
    // A placement's own filter ("" = all items), else the page's focus.
    ownFocus(placement) { return typeof placement.focus === "string"; }
    focusOf(placement) { return this.ownFocus(placement) ? placement.focus : (this.page?.focus || ""); }

    // Widgets every plugin declares, with templates, for placing and mounting.
    async loadCatalog(plugins) {
      const entries = await Promise.all(plugins.flatMap((plugin) => plugin.widgets.map(async (widget) => {
        try {
          const template = await this.o.call("view_template", { plugin: plugin.id, kind: "widget", id: widget.id });
          return { plugin, widget, template };
        } catch (error) {
          return { plugin, widget, error };
        }
      })));
      this.catalog = new Map(entries.map((entry) => [`${entry.plugin.id}/${entry.widget.id}`, entry]));
      // The host's own widgets: content comes from their settings.
      try {
        for (const widget of await this.service("page_widgets")) {
          this.catalog.set(`${BUILT_IN.id}/${widget.id}`, { plugin: BUILT_IN, widget: { id: widget.id, title: widget.title }, template: { page: widget.page }, builtIn: true });
        }
      } catch (_) { /* an older backend: plugin widgets only */ }
    }

    // The first Home layout comes from the device's previous Home choices.
    seed() {
      const entries = [...this.catalog.values()].sort((a, b) => (a.template?.home?.order ?? 100) - (b.template?.home?.order ?? 100));
      const ids = entries.map((entry) => `${entry.plugin.id}/${entry.widget.id}`);
      const defaults = entries.filter((entry) => !entry.template?.home?.hidden && !entry.error).map((entry) => `${entry.plugin.id}/${entry.widget.id}`);
      const legacy = this.o.legacyHome?.() || { home: null, homeWide: [] };
      const chosen = window.TerrariumLayout.selected(legacy.home, ids, defaults);
      const placed = [];
      for (const id of chosen) {
        const entry = this.catalog.get(id);
        const lim = this.grid.limits(entry.template?.page?.size);
        const w = legacy.homeWide.includes(id) ? 12 : Math.max(lim.default[0], Math.min(6, lim.max[0]));
        const spot = this.grid.firstFit(placed, w, lim.default[1]);
        placed.push({ id, plugin: entry.plugin.id, widget: entry.widget.id, ...spot, config: {} });
      }
      return placed.map(({ id, ...rest }) => rest);
    }

    reset() {
      this.seq++;
      this.unmount();
      this.space = null; this.pages = []; this.page = null; this.placements = []; this.draft = null; this.currentId = null;
    }

    unmount() {
      for (const [id, view] of this.views) { this.o.session.save(this.sessionKey(id), view); view.dispose(); }
      this.views.clear(); this.cards.clear();
      this.pending.clear();
      this.io?.disconnect();
    }

    // Plugin widgets mount when their card nears the visible area, so a
    // long page does not run every widget's queries at once.
    later(id, card, mount) {
      if (typeof IntersectionObserver !== "function") { mount(); return; }
      this.io ||= new IntersectionObserver((entries) => {
        for (const entry of entries) {
          if (!entry.isIntersecting) continue;
          const key = entry.target.dataset.placement;
          const run = this.pending.get(key);
          this.io.unobserve(entry.target);
          if (!run || this.cards.get(key) !== entry.target) continue;
          this.pending.delete(key);
          run();
        }
      }, { root: this.$("content"), rootMargin: "400px 0px" });
      this.pending.set(id, mount);
      this.io.observe(card);
    }

    async flush() {
      return (await Promise.all(this.mounted().map((view) => view.flush({ unmount: true })))).every((ok) => ok !== false);
    }

    // Loads the space's pages (creating Home once) and shows the current one.
    async load(space, plugins) {
      const seq = ++this.seq;
      if (space !== this.space) { this.currentId = null; this.draft = null; }
      this.space = space;
      await this.loadCatalog(plugins);
      if (seq !== this.seq) return false;
      let pages = await this.service("pages_list", { space });
      if (!pages.some((page) => page.system)) {
        await this.service("page_home", { space, seed: this.seed() });
        pages = await this.service("pages_list", { space });
      }
      if (seq !== this.seq) return false;
      this.pages = pages;
      if (!pages.some((page) => page.id === this.currentId)) this.currentId = this.home?.id || null;
      return this.render(seq);
    }

    async open(id) {
      if (this.editing && !(await this.confirmLeave())) return false;
      this.draft = null;
      this.currentId = id;
      return this.render();
    }

    // Leaving edit mode drops the draft; ask first when it has changes.
    async confirmLeave() {
      if (!this.editing) return true;
      if (this.dirty() && !(await this.ask(`Discard your changes to “${this.page?.title || "this page"}”?`, { title: "Unsaved changes", confirm: "Discard", cancel: "Keep editing", danger: true }))) return false;
      this.cancel();
      return true;
    }

    async discard() {
      if (await this.confirmLeave()) this.$("page-edit").focus({ preventScroll: true });
    }

    ask(message, options) {
      return window.terrariumConfirm ? window.terrariumConfirm(message, options) : Promise.resolve(window.confirm(message));
    }

    // Whether the draft differs from what is saved.
    dirty() {
      if (!this.editing) return false;
      const shape = (list) => JSON.stringify(list.map((p) => [p.id, p.plugin, p.widget, p.x, p.y, p.w, p.h, p.config || {}, p.focus ?? null]).sort());
      const title = this.$("page-title-input").value.trim();
      return shape(this.draft) !== shape(this.placements)
        || (title !== "" && title !== this.page?.title)
        || this.$("page-focus-input").value.trim() !== (this.page?.focus || "")
        || (this.draftIcon ?? null) !== (this.page?.icon ?? null);
    }

    async render(seq = ++this.seq) {
      const id = this.currentId;
      if (!id) return false;
      const state = await this.service("page_get", { page: id });
      if (seq !== this.seq) return false;
      this.unmount();
      this.page = state.page;
      // Edits merged from other devices may overlap or leave gaps.
      this.placements = this.grid.compact(state.placements.map((p) => this.grid.clamp(p, this.spec(p).size)));
      if (this.draft) this.draft = this.draft.filter((p) => p.id.startsWith("new-") || this.placements.some((q) => q.id === p.id));
      const root = this.$("widgets");
      root.replaceChildren(...this.list().map((placement) => this.card(placement)));
      this.layout();
      this.chrome();
      return true;
    }

    // A placement's card: header, body (the mounted widget) and edit handles.
    card(placement) {
      const entry = this.catalog.get(this.key(placement));
      const name = entry?.widget.title || placement.widget;
      const body = this.el("div", { class: "widget-body" });
      const head = this.el("div", { class: "widget-head" });
      let filterBar = "";
      if (entry?.builtIn) {
        head.append(this.el("h2", {}, name));
      } else if (entry) {
        const open = entry.plugin.pages.length
          ? this.el("button", { type: "button", class: "link-button", onclick: () => this.o.openApp(entry.plugin) }, `Open ${entry.plugin.name}`)
          : this.el("span", { class: "muted" }, entry.plugin.name);
        head.append(this.o.appTile(entry.plugin, "xs"), this.el("h2", {}, name));
        // A widget with its own filters shows them; one following the page
        // focus shows a mark while the page has one.
        const chip = this.filterChip(placement, entry);
        if (chip) {
          filterBar = this.el("div", { class: "page-filter-bar" }, this.el("button", {
            type: "button", class: "page-filter-chip", title: `${this.describe(entry, placement.config, this.ownFocus(placement) ? placement.focus : undefined)}. Click to change.`,
            "aria-label": `${name} filters: ${chip}. Change`,
            onclick: () => { if (!this.editing) this.edit(); this.openSettings(placement.id); },
          }, this.o.icon("target"), this.el("span", {}, chip)));
        } else if (this.follows(entry) && this.page?.focus) {
          head.append(this.el("span", { class: "page-focus-mark", title: `Showing items matching the page focus “${this.page.focus}”`, "aria-label": `Filtered by page focus ${this.page.focus}` }, this.o.icon("target")));
        }
        head.append(open);
      } else {
        head.append(this.el("h2", {}, name));
      }
      const move = this.el("button", { type: "button", class: "page-handle page-move", "aria-label": `Move ${name}. Drag, or use arrow keys; Shift with arrows resizes.`, title: "Drag to move · arrows move · Shift+arrows resize" }, this.o.icon("grip"));
      const resize = this.el("button", { type: "button", class: "page-handle page-resize", "aria-label": `Resize ${name}. Drag, or use arrow keys.`, title: "Drag to resize" });
      const tool = (cls, label, icon, onclick) => this.el("button", { type: "button", class: `page-tool ${cls}`, "aria-label": label, title: label, onclick }, this.o.icon(icon));
      const tools = this.el("div", { class: "page-tools", role: "group", "aria-label": `${name} tools` },
        tool("page-up", `Move ${name} up`, "chevron-up", () => this.step(placement.id, -1)),
        tool("page-down", `Move ${name} down`, "chevron-down", () => this.step(placement.id, 1)),
        entry && (Object.keys(entry.template?.page?.config || {}).length || this.follows(entry)) ? tool("page-settings", `Settings for ${name}`, "sliders", () => this.openSettings(placement.id)) : "",
        tool("page-remove", `Remove ${name}`, "trash", () => this.removePlacement(placement.id)));
      head.prepend(move);
      const dims = this.el("span", { class: "page-dims", "aria-hidden": "true" });
      const section = this.el("section", { class: "card widget page-widget", "aria-label": name }, head, filterBar, body, tools, dims, resize);
      section.dataset.placement = placement.id;
      // The header leaves room for the tools floating over its right end.
      section.style.setProperty("--page-tools", String(tools.children.length - 2));
      this.drag(move, placement.id, false);
      this.drag(head, placement.id, false);
      this.drag(resize, placement.id, true);
      this.cards.set(placement.id, section);
      if (!entry) {
        body.append(this.el("p", { class: "muted" }, `Unavailable: ${placement.plugin} is not installed in this space. The widget comes back when it is.`));
      } else if (entry.error) {
        body.append(this.el("p", { class: "error" }, String(entry.error)));
      } else if (entry.builtIn) {
        section.classList.add("page-builtin", `page-builtin-${placement.widget}`);
        this.views.set(placement.id, this.builtIn(body, placement, entry));
      } else {
        body.classList.add("widget-pending");
        body.setAttribute("aria-busy", "true");
        this.later(placement.id, section, () => {
          body.classList.remove("widget-pending");
          body.removeAttribute("aria-busy");
          const view = this.o.mount(body, entry.template, this.o.bridge(entry.plugin.id), {
            snapshot: this.o.session.get(this.sessionKey(placement.id)),
            config: placement.config || {},
            page: { focus: this.focusOf(placement), title: this.page?.title || "" },
            size: this.grid.sizeClass(placement),
          });
          view.sessionKey = this.sessionKey(placement.id);
          this.views.set(placement.id, view);
        });
      }
      return section;
    }

    // Built-in widgets are drawn by the shell from their settings only.
    builtIn(body, placement, entry) {
      const config = {};
      for (const [name, field] of Object.entries(entry.template.page?.config || {})) config[name] = field.default;
      Object.assign(config, placement.config || {});
      const view = { refresh() {}, flush: async () => true, dispose() { if (this.timer) clearInterval(this.timer); this.timer = null; } };
      const open = (url) => /^https?:\/\//.test(url) ? this.o.call("open_external", { url }).catch((e) => this.o.toast(String(e), "error")) : null;
      if (placement.widget === "text") {
        body.append(config.text?.trim()
          ? window.TerrariumRenderer.markdown(config.text, { open })
          : this.el("p", { class: "muted" }, "Empty text. Edit the page and open this widget's settings to write here."));
      } else if (placement.widget === "link") {
        const url = /^https?:\/\//.test(config.url || "") ? config.url : "";
        let host = "";
        try { host = url ? new URL(url).host : ""; } catch (_) {}
        body.append(url
          ? this.el("button", { type: "button", class: "page-link", title: url, onclick: () => open(url) }, this.el("strong", {}, config.label || host), host && config.label ? this.el("span", { class: "muted" }, host) : "")
          : this.el("p", { class: "muted" }, "No address yet. Edit the page and open this widget's settings."));
      } else if (placement.widget === "clock") {
        const time = this.el("div", { class: "page-clock-time", role: "timer", "aria-live": "off" });
        const date = this.el("div", { class: "muted" });
        const tick = () => {
          const now = new Date();
          time.textContent = now.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit", ...(config.seconds ? { second: "2-digit" } : {}) });
          date.textContent = config.date ? now.toLocaleDateString(undefined, { weekday: "long", day: "numeric", month: "long" }) : "";
        };
        tick();
        view.timer = setInterval(tick, config.seconds ? 1000 : 10000);
        body.append(time, date);
      }
      return view;
    }

    // What a widget's template declares for pages: `page: { size, config }`.
    spec(placement) {
      return this.catalog.get(this.key(placement))?.template?.page || {};
    }

    size(id) {
      const placement = this.list().find((p) => p.id === id);
      return placement ? this.spec(placement).size : undefined;
    }

    // Replaces a card so its widget remounts with new settings.
    remount(id) {
      const placement = this.list().find((p) => p.id === id);
      const old = this.cards.get(id);
      if (!placement || !old) return;
      this.views.get(id)?.dispose();
      this.views.delete(id);
      this.pending.delete(id);
      this.io?.unobserve(old);
      const next = this.card(placement);
      old.replaceWith(next);
      this.layout();
      next.querySelector(".page-settings")?.focus({ preventScroll: true });
    }

    // Widget settings, in two groups: Filters (choices from the widget's own
    // data, a keyword and the page focus) and Display (the rest). A live line
    // at the top says what the widget will show.
    openSettings(id) {
      if (!this.editing) return;
      const placement = this.draft.find((p) => p.id === id);
      const entry = placement && this.catalog.get(this.key(placement));
      if (!entry) return;
      const fields = Object.entries(entry.template?.page?.config || {});
      const summary = this.el("p", { class: "page-settings-summary", "aria-live": "polite" });
      const controls = new Map();
      const filters = [];
      const display = [];
      let focus = null;
      const read = () => Object.fromEntries([...controls].map(([name, control]) => [name, control.read()]).filter(([, value]) => value !== undefined));
      const update = () => { summary.textContent = this.describe(entry, read(), focus ? focus.read() : null); };
      for (const [name, field] of fields) {
        const control = this.control(name, field, placement.config?.[name] ?? field.default, entry, () => update());
        controls.set(name, control);
        (field.type === "choice" && field.filter !== false ? filters : display).push(control.row);
      }
      focus = this.follows(entry) ? this.focusControls(placement, () => update()) : null;
      if (focus) filters.push(...focus.rows);
      const section = (title, rows, extra) => this.el("section", { class: "page-settings-group" },
        this.el("div", { class: "page-settings-group-head" }, this.el("h3", {}, title), extra || ""), ...rows);
      const reset = filters.length ? this.el("button", {
        type: "button", class: "link-button", onclick: () => {
          for (const [, control] of controls) control.clear?.();
          focus?.clear();
          update();
        },
      }, "Clear filters") : null;
      this.$("page-config-title").textContent = `${entry.widget.title} settings`;
      this.$("page-config-fields").replaceChildren(
        ...(filters.length ? [summary, section("Filters", filters, reset)] : []),
        ...(display.length ? [section(filters.length ? "Display" : "Options", display)] : []));
      update();
      const dialog = this.$("page-config-dialog");
      this.$("page-config-form").onsubmit = (event) => {
        event.preventDefault();
        if (focus) {
          const value = focus.read();
          if (value === undefined) delete placement.focus;
          else placement.focus = value;
        }
        placement.config = read();
        dialog.close();
        this.remount(id);
      };
      dialog.showModal();
      this.$("page-config-fields").querySelector("input:not([type=radio]):not([type=checkbox]), select, button.page-choice-chip, input:checked")?.focus();
    }

    // Options a widget lists for a choice: rows of its own named query.
    async choices(entry, query) {
      if (typeof query !== "string" || !query) return [];
      const rows = await this.o.bridge(entry.plugin.id).query(query, {}).catch(() => []);
      const seen = new Set();
      const out = [];
      for (const row of rows || []) {
        const value = String(row?.value ?? Object.values(row || {}).find((v) => typeof v === "string") ?? "").trim();
        if (!value || value.includes(",") || value.length > 60 || seen.has(value.toLowerCase())) continue;
        seen.add(value.toLowerCase());
        out.push({ value, label: String(row.label ?? value), count: Number.isFinite(row.count) ? row.count : null });
      }
      return out;
    }

    // One setting's control. Returns { row, read(), clear?() }.
    control(name, field, value, entry, changed) {
      const label = field.label || name;
      const help = field.help ? this.el("span", { class: "page-config-help" }, field.help) : "";
      const row = (control, cls = "page-config-field") => this.el("div", { class: cls }, this.el("span", { class: "page-config-label", id: `pp-cfg-${name}` }, label), control, help);
      if (field.type === "enum" && (field.values || []).length <= 4) {
        // Few options read best side by side.
        const group = this.el("div", { class: "page-segmented", role: "radiogroup", "aria-labelledby": `pp-cfg-${name}` },
          ...(field.values || []).map((option, index) => this.el("label", {},
            this.el("input", { type: "radio", name: `pp-cfg-${name}`, value: option, checked: value === option, onchange: changed }),
            this.el("span", {}, field.labels?.[index] || option))));
        return { row: row(group), read: () => group.querySelector("input:checked")?.value ?? field.default };
      }
      if (field.type === "enum") {
        const select = this.el("select", { "aria-labelledby": `pp-cfg-${name}`, onchange: changed }, ...(field.values || []).map((option, index) => this.el("option", { value: option, selected: value === option }, field.labels?.[index] || option)));
        return { row: row(select), read: () => select.value };
      }
      if (field.type === "boolean") {
        const input = this.el("input", { type: "checkbox", role: "switch", checked: value === true, onchange: changed });
        return { row: this.el("label", { class: "page-switch" }, input, this.el("span", {}, label), help), read: () => input.checked };
      }
      if (field.type === "number") {
        const input = this.el("input", { type: "number", value: value ?? "", min: field.min, max: field.max, "aria-labelledby": `pp-cfg-${name}`, oninput: changed });
        return { row: row(input), read: () => (input.value === "" ? undefined : Number(input.value)) };
      }
      if (field.type === "choice" && !field.multiple) {
        const select = this.el("select", { "aria-labelledby": `pp-cfg-${name}`, onchange: changed }, this.el("option", { value: "" }, field.any || "Any"));
        const fill = (options) => {
          const list = value && !options.some((o) => o.value === value) ? [{ value, label: value }, ...options] : options;
          select.replaceChildren(this.el("option", { value: "" }, field.any || "Any"), ...list.map((o) => this.el("option", { value: o.value, selected: o.value === value }, o.count ? `${o.label} (${o.count})` : o.label)));
          changed();
        };
        fill([]);
        this.choices(entry, field.suggest).then(fill);
        return { row: row(select), read: () => select.value, clear: () => { select.value = ""; } };
      }
      if (field.type === "choice") {
        // Several picks: chips that toggle, with a search once the list is long.
        const picked = new Set(String(value || "").split(",").map((v) => v.trim()).filter(Boolean));
        const list = this.el("div", { class: "page-choice-list", role: "group", "aria-labelledby": `pp-cfg-${name}` });
        const search = this.el("input", { type: "search", class: "page-choice-search", placeholder: `Search ${label.toLowerCase()}`, "aria-label": `Search ${label}`, hidden: true });
        const count = this.el("span", { class: "page-choice-count muted" });
        let options = [];
        const draw = () => {
          const query = search.value.trim().toLowerCase();
          const all = [...[...picked].filter((v) => !options.some((o) => o.value === v)).map((v) => ({ value: v, label: v })), ...options];
          const shown = all.filter((o) => !query || o.label.toLowerCase().includes(query));
          list.replaceChildren(...shown.map((o) => this.el("button", {
            type: "button", class: "page-choice-chip", "aria-pressed": String(picked.has(o.value)),
            onclick: (event) => {
              if (picked.has(o.value)) picked.delete(o.value); else picked.add(o.value);
              event.currentTarget.setAttribute("aria-pressed", String(picked.has(o.value)));
              count.textContent = picked.size ? `${picked.size} selected` : "";
              changed();
            },
          }, field.prefix && !o.label.startsWith(field.prefix) ? `${field.prefix}${o.label}` : o.label, o.count ? this.el("span", { class: "page-choice-n" }, String(o.count)) : "")));
          if (!all.length) list.append(this.el("span", { class: "muted" }, field.empty || "Nothing to choose from yet."));
          else if (!shown.length) list.append(this.el("span", { class: "muted" }, "No match."));
          search.hidden = all.length <= 10;
          count.textContent = picked.size ? `${picked.size} selected` : "";
        };
        search.addEventListener("input", draw);
        draw();
        this.choices(entry, field.suggest).then((found) => { options = found; draw(); changed(); });
        const box = this.el("div", { class: "page-choice" }, this.el("div", { class: "page-choice-tools" }, search, count), list);
        return { row: row(box), read: () => [...picked].join(", "), clear: () => { picked.clear(); search.value = ""; draw(); } };
      }
      const input = field.multiline ? this.el("textarea", { rows: "8", maxlength: field.max_length || 200, "aria-labelledby": `pp-cfg-${name}`, oninput: changed }) : this.el("input", { type: "text", value: value ?? "", maxlength: field.max_length || 200, "aria-labelledby": `pp-cfg-${name}`, oninput: changed });
      if (field.multiline) input.value = value ?? "";
      return { row: row(input), read: () => input.value };
    }

    // Keyword and page focus for widgets that can filter by text. Reads as
    // the placement's focus: undefined follows the page, "" ignores it.
    focusControls(placement, changed) {
      const pageFocus = this.page?.focus || "";
      const own = this.ownFocus(placement) ? placement.focus : null;
      const keyword = this.el("input", { type: "search", maxlength: 40, autocomplete: "off", placeholder: "Any text", "aria-labelledby": "pp-cfg-keyword", value: own || "" });
      const follow = this.el("input", { type: "checkbox", role: "switch", checked: own === null });
      const followText = this.el("span", {});
      const sync = () => {
        const typed = keyword.value.trim() !== "";
        follow.disabled = typed;
        followText.textContent = typed ? "Keyword replaces the page focus"
          : pageFocus ? `Follow page focus “${pageFocus}”` : "Follow page focus (none set)";
      };
      keyword.addEventListener("input", () => { sync(); changed(); });
      follow.addEventListener("change", changed);
      sync();
      return {
        rows: [
          this.el("div", { class: "page-config-field" }, this.el("span", { class: "page-config-label", id: "pp-cfg-keyword" }, "Keyword"), keyword, this.el("span", { class: "page-config-help" }, "Also matches titles and text.")),
          this.el("label", { class: "page-switch" }, follow, followText),
        ],
        read: () => {
          const text = keyword.value.trim();
          if (text) return text;
          return follow.checked ? undefined : "";
        },
        clear: () => { keyword.value = ""; follow.checked = true; sync(); },
      };
    }

    // "Tagged a or b · Project: Work · “kw”": what a widget's filters show.
    describe(entry, config, focus) {
      const parts = this.filterParts(entry, config, focus);
      if (parts.length) return `Filtered by ${parts.join(" · ")}`;
      return focus === "" ? "No filters: ignores the page focus" : "No filters";
    }

    filterParts(entry, config, focus) {
      const parts = [];
      for (const [name, field] of Object.entries(entry?.template?.page?.config || {})) {
        if (field.type !== "choice" || field.filter === false || !config?.[name]) continue;
        const picks = String(config[name]).split(",").map((v) => v.trim()).filter(Boolean);
        if (!picks.length) continue;
        const shown = picks.map((v) => field.prefix && !v.startsWith(field.prefix) ? `${field.prefix}${v}` : v);
        parts.push(`${field.label || name}: ${shown.join(field.multiple ? " or " : ", ")}`);
      }
      const pageFocus = this.page?.focus || "";
      if (focus === undefined) { if (pageFocus && this.follows(entry)) parts.push(`page focus “${pageFocus}”`); }
      else if (focus) parts.push(`“${focus}”`);
      return parts;
    }

    // A short label for a card's own filters, or "" when it has none.
    filterChip(placement, entry) {
      const own = this.ownFocus(placement);
      const parts = this.filterParts(entry, placement.config, own ? placement.focus : null);
      if (parts.length) return parts.map((part) => part.replace(/^[^:]+: /, "")).join(" · ");
      return own && placement.focus === "" ? "All items" : "";
    }

    // Positions cards from the current list; narrow screens stack them.
    layout() {
      const root = this.$("widgets");
      const list = this.list();
      const order = new Map(this.grid.stack(list).map((p, index) => [p.id, index]));
      const last = list.length - 1;
      for (const placement of list) {
        const card = this.cards.get(placement.id);
        if (!card) continue;
        card.style.gridColumn = `${placement.x + 1} / span ${placement.w}`;
        card.style.gridRow = `${placement.y + 1} / span ${placement.h}`;
        card.style.order = String(order.get(placement.id));
        card.dataset.size = this.grid.sizeClass(placement);
        const dims = card.querySelector(".page-dims");
        if (dims) dims.textContent = `${placement.w} × ${placement.h}`;
        const up = card.querySelector(".page-up");
        const down = card.querySelector(".page-down");
        if (up) up.disabled = order.get(placement.id) === 0;
        if (down) down.disabled = order.get(placement.id) === last;
      }
      root.classList.toggle("editing", this.editing);
      // Spare rows below the last widget give room to drag into.
      root.style.minHeight = this.editing && list.length ? `${(this.grid.bottom(list) + 2) * (ROW + GAP) - GAP}px` : "";
      const empty = list.length === 0;
      this.$("home-layout-empty").hidden = !empty;
      this.$("home-layout-hint").hidden = !this.editing || empty;
    }

    chrome() {
      const editing = this.editing;
      this.$("page-edit").hidden = editing;
      this.$("page-editbar").hidden = !editing;
      this.$("page-delete").hidden = this.isHome();
      const order = this.pages.filter((page) => !page.system);
      const index = order.findIndex((page) => page.id === this.page?.id);
      this.$("page-order").hidden = this.isHome() || order.length < 2;
      this.$("page-move-left").disabled = index <= 0;
      this.$("page-move-right").disabled = index < 0 || index >= order.length - 1;
      this.$("page-icon").replaceChildren(this.o.icon((editing ? this.draftIcon : this.page?.icon) || (this.isHome() ? "home" : "layers")));
      this.$("page-icon").hidden = this.isHome();
      const chip = this.$("page-focus-chip");
      chip.hidden = editing || !this.page?.focus;
      if (!chip.hidden) {
        chip.replaceChildren(this.o.icon("target"), this.el("span", {}, `Focus · ${this.page.focus}`));
        chip.title = "Widgets that support it show only items matching this. Click to change.";
      }
      if (!editing) this.toggleIcons(false);

      this.o.onChange?.();
    }

    navigation() {
      const items = this.pages.filter((page) => !page.system).map((page) => this.el("button", {
        type: "button", class: "nav-item", "data-user-page": page.id, title: page.title, "aria-label": page.title,
        onclick: () => this.o.show(page.id),
      }, this.o.icon(page.icon || "layers"), this.el("span", {}, page.title)));
      this.$("page-nav").replaceChildren(...items);
      const tabs = this.pages.map((page, index) => this.el("button", {
        type: "button", class: "page-tab", "data-user-page": page.id, onclick: () => this.o.show(page.id),
        title: index < 9 ? `${page.title} (${this.mac ? "⌘" : "Ctrl+"}${index + 1})` : page.title,
      }, this.o.icon(page.icon || (page.system ? "home" : "layers")), this.el("span", {}, page.title)));
      tabs.push(this.el("button", { type: "button", class: "page-tab page-tab-new", "aria-label": "New page", title: "New page", onclick: () => this.create() }, this.o.icon("plus")));
      this.$("page-tabs").replaceChildren(...tabs);
      this.$("page-tabs").hidden = this.pages.length < 2 && !this.narrow.matches;
    }

    edit() {
      if (this.editing) return;
      this.draft = this.placements.map((p) => ({ ...p, config: { ...(p.config || {}) } }));
      this.draftIcon = this.page?.icon ?? null;
      this.$("page-title-input").value = this.page?.title || "";
      this.$("page-focus-input").value = this.page?.focus || "";
      this.layout();
      this.chrome();
      this.$("page-title-input").focus();
    }

    cancel() {
      if (!this.editing) return;
      for (const p of this.draft) if (p.id.startsWith("new-")) this.drop(p.id);
      for (const p of this.placements) if (!this.cards.has(p.id)) this.$("widgets").append(this.card(p));
      this.draft = null;
      this.draftIcon = null;
      this.layout();
      this.chrome();
    }

    drop(id) {
      this.views.get(id)?.dispose();
      this.views.delete(id);
      this.pending.delete(id);
      if (this.cards.get(id)) this.io?.unobserve(this.cards.get(id));
      this.cards.get(id)?.remove();
      this.cards.delete(id);
    }

    async done() {
      if (!this.editing) return;
      const page = this.page.id;
      const title = this.$("page-title-input").value.trim();
      try {
        const meta = {};
        if (title && title !== this.page.title) meta.title = title;
        if (!this.isHome() && (this.draftIcon ?? null) !== (this.page.icon ?? null)) meta.icon = this.draftIcon;
        const focus = this.$("page-focus-input").value.trim();
        if (focus !== (this.page.focus || "")) meta.focus = focus;
        if (Object.keys(meta).length) await this.service("page_update", { page, ...meta });
        if (!(await this.flush())) return;
        const placements = this.draft.map((p) => ({ ...p, id: p.id.startsWith("new-") ? "" : p.id }));
        await this.service("page_save", { page, placements });
        this.draft = null;
        this.pages = await this.service("pages_list", { space: this.space });
        await this.render();
        this.o.toast("Page saved");
      } catch (error) {
        this.o.toast(String(error), "error");
      }
    }

    removePlacement(id) {
      if (!this.editing) return;
      const order = this.grid.stack(this.draft);
      const index = order.findIndex((p) => p.id === id);
      const removed = this.draft.find((p) => p.id === id);
      if (!removed) return;
      const name = this.catalog.get(this.key(removed))?.widget.title || removed.widget;
      const page = this.page?.id;
      this.draft = this.draft.filter((p) => p.id !== id);
      this.drop(id);
      this.draft = this.grid.compact(this.draft);
      this.layout();
      // Keep keyboard focus on the page: the next widget, or Add widget.
      const next = order[index + 1] || order[index - 1];
      (next ? this.cards.get(next.id)?.querySelector(".page-move") : this.$("page-editbar").querySelector("[data-page-add]"))?.focus({ preventScroll: true });
      this.o.toast(`Removed “${name}”`, "ok", { label: "Undo", run: () => this.restorePlacement(page, removed) });
    }

    // Puts a removed widget back where it was, while that edit is still open.
    restorePlacement(page, placement) {
      if (!this.editing || this.page?.id !== page) {
        this.o.toast("That edit has ended. Edit the page and add the widget again.", "error");
        return true;
      }
      if (this.draft.some((p) => p.id === placement.id)) return true;
      if (this.draft.length >= MAX_PLACEMENTS) { this.o.toast(`A page holds up to ${MAX_PLACEMENTS} widgets.`, "error"); return true; }
      this.draft = this.grid.place([...this.draft, { ...placement }], placement.id, { x: placement.x, y: placement.y, w: placement.w, h: placement.h }, this.size(placement.id) ?? this.spec(placement).size);
      this.$("widgets").append(this.card(placement));
      this.layout();
      this.cards.get(placement.id)?.querySelector(".page-move")?.focus({ preventScroll: true });
      return true;
    }

    // Phones show one column: move a widget before or after its neighbour.
    step(id, direction) {
      if (!this.editing) return;
      const order = this.grid.stack(this.draft);
      const index = order.findIndex((p) => p.id === id);
      const other = order[index + direction];
      if (!other) return;
      const [mover, target] = direction < 0 ? [order[index], other] : [other, order[index]];
      this.draft = this.grid.place(this.draft, mover.id, { x: target.x, y: target.y }, this.size(mover.id));
      this.layout();
      const card = this.cards.get(id);
      const same = card?.querySelector(direction < 0 ? ".page-up" : ".page-down");
      (same?.disabled ? card.querySelector(direction < 0 ? ".page-down" : ".page-up") : same)?.focus({ preventScroll: true });
      card?.scrollIntoView({ block: "nearest" });
    }

    // Icon choice for the page, kept in the draft until Done.
    toggleIcons(force) {
      const menu = this.$("page-icon-menu");
      const show = force ?? menu.hidden;
      if (!show || !this.editing) { menu.hidden = true; this.$("page-icon").setAttribute("aria-expanded", "false"); return; }
      const current = this.draftIcon || "layers";
      menu.replaceChildren(...(this.o.pageIcons || ["layers"]).map((name) => this.el("button", {
        type: "button", class: "page-icon-choice", role: "option", "aria-selected": String(name === current), "aria-label": name, title: name,
        onclick: () => { this.draftIcon = name; this.toggleIcons(false); this.chrome(); this.$("page-icon").focus(); },
      }, this.o.icon(name))));
      menu.hidden = false;
      this.$("page-icon").setAttribute("aria-expanded", "true");
      menu.querySelector('[aria-selected="true"]')?.focus();
    }

    // Moves the current page earlier or later among the user's pages.
    async movePage(direction) {
      const order = this.pages.filter((page) => !page.system);
      const index = order.findIndex((page) => page.id === this.page?.id);
      const target = index + direction;
      if (index < 0 || target < 0 || target >= order.length) return;
      [order[index], order[target]] = [order[target], order[index]];
      try {
        await Promise.all(order.map((page, i) => page.position === i + 1 ? null : this.service("page_update", { page: page.id, position: i + 1 })));
        this.pages = await this.service("pages_list", { space: this.space });
        this.chrome();
        this.$(direction < 0 ? "page-move-left" : "page-move-right").focus();
      } catch (error) {
        this.o.toast(String(error), "error");
      }
    }

    add(key) {
      const entry = this.catalog.get(key);
      if (!entry) return;
      if (!this.editing) this.edit();
      const lim = this.grid.limits(entry.template?.page?.size);
      const placement = { id: `new-${++this.added}`, plugin: entry.plugin.id, widget: entry.widget.id, ...this.grid.firstFit(this.draft, lim.default[0], lim.default[1]), config: {} };
      this.draft.push(placement);
      this.$("widgets").append(this.card(placement));
      this.layout();
      this.cards.get(placement.id)?.classList.add("page-new-widget");
      this.cards.get(placement.id)?.scrollIntoView({ block: "nearest", behavior: "smooth" });
      this.cards.get(placement.id)?.querySelector(".page-move")?.focus({ preventScroll: true });
      // Widgets that can filter ask which items to show right away.
      if (this.filterable(entry)) this.openSettings(placement.id);
    }

    openCatalog() {
      const groups = new Map();
      for (const [key, entry] of this.catalog) {
        if (entry.error) continue;
        if (!groups.has(entry.plugin.id)) groups.set(entry.plugin.id, { plugin: entry.plugin, items: [] });
        groups.get(entry.plugin.id).items.push({ key, entry });
      }
      const used = new Map();
      for (const p of this.list()) used.set(this.key(p), (used.get(this.key(p)) || 0) + 1);
      const full = this.list().length >= MAX_PLACEMENTS;
      const list = this.$("page-add-list");
      // Built-in widgets last: plugins are what people usually look for.
      const ordered = [...groups.values()].sort((a, b) => (a.plugin === BUILT_IN) - (b.plugin === BUILT_IN));
      list.replaceChildren(...ordered.map(({ plugin, items }) => this.el("section", { class: "page-add-group" },
        this.el("h3", {}, plugin === BUILT_IN ? this.o.icon("layers") : this.o.appTile(plugin, "xs"), plugin.name),
        this.el("div", { class: "page-add-items" }, ...items.map(({ key, entry }) => {
          const lim = this.grid.limits(entry.template?.page?.size);
          const count = used.get(key) || 0;
          const button = this.el("button", {
            type: "button", class: "page-add-item", disabled: full, "data-search": `${entry.widget.title} ${plugin.name}`.toLowerCase(),
            onclick: () => { this.$("page-add-dialog").close(); this.add(key); },
          },
          this.el("strong", {}, entry.widget.title),
          this.el("span", { class: "muted" }, `${lim.default[0]} × ${lim.default[1]}${Object.keys(entry.template?.page?.config || {}).length ? " · has settings" : ""}${this.filterable(entry) ? " · has filters" : ""}`),
          count ? this.el("span", { class: "page-add-count" }, count === 1 ? "On page" : `On page ×${count}`) : "");
          return button;
        })))));
      if (!groups.size) list.append(this.el("p", { class: "muted" }, "No plugin offers widgets yet."));
      list.append(this.el("p", { class: "muted page-add-none", hidden: true }, "No widget matches."));
      if (full) list.prepend(this.el("p", { class: "error" }, `A page holds up to ${MAX_PLACEMENTS} widgets. Remove one to add another.`));
      this.$("page-add-search").value = "";
      this.$("page-add-dialog").showModal();
      this.$("page-add-search").focus();
    }

    filterCatalog() {
      const query = this.$("page-add-search").value.trim().toLowerCase();
      let shown = 0;
      for (const group of this.$("page-add-list").querySelectorAll(".page-add-group")) {
        let any = false;
        for (const item of group.querySelectorAll(".page-add-item")) {
          const match = !query || item.dataset.search.includes(query);
          item.hidden = !match;
          any ||= match;
          if (match) shown++;
        }
        group.hidden = !any;
      }
      const none = this.$("page-add-list").querySelector(".page-add-none");
      if (none) none.hidden = shown > 0;
    }

    async create() {
      if (!this.space) return;
      if (this.editing && !(await this.confirmLeave())) return;
      try {
        const page = await this.service("page_create", { space: this.space, title: "New page" });
        this.pages = await this.service("pages_list", { space: this.space });
        await this.o.show(page.id);
        this.edit();
        this.$("page-title-input").select();
      } catch (error) {
        this.o.toast(String(error), "error");
      }
    }

    // Deleting is undoable for a few seconds instead of asking first: the
    // saved page and its layout are kept and recreated on Undo.
    async removePage() {
      if (!this.page || this.page.system) return;
      const title = this.page.title;
      try {
        const saved = await this.service("page_get", { page: this.page.id });
        const space = this.space;
        await this.service("page_remove", { page: this.page.id });
        this.draft = null;
        this.draftIcon = null;
        this.pages = await this.service("pages_list", { space });
        await this.o.show(this.home?.id);
        this.o.toast(`Deleted “${title}”`, "ok", { label: "Undo", run: () => this.restorePage(space, saved) });
      } catch (error) {
        this.o.toast(String(error), "error");
      }
    }

    async restorePage(space, saved) {
      try {
        const page = await this.copyPage(space, saved, saved.page.title);
        await this.service("page_update", { page: page.id, position: saved.page.position });
        if (this.space !== space) { this.o.toast(`Restored “${saved.page.title}”`); return true; }
        this.pages = await this.service("pages_list", { space });
        await this.o.show(page.id);
        this.o.toast(`Restored “${saved.page.title}”`);
        return true;
      } catch (error) {
        this.o.toast(String(error), "error");
        return false;
      }
    }

    // A new page with the same icon, layout and widget settings.
    async copyPage(space, saved, title) {
      const page = await this.service("page_create", { space, title, icon: saved.page.icon || null, ...(saved.page.focus ? { focus: saved.page.focus } : {}) });
      const placements = saved.placements.map(({ plugin, widget, x, y, w, h, config, focus }) => ({ id: "", plugin, widget, x, y, w, h, config: config || {}, focus: focus ?? null }));
      if (placements.length) await this.service("page_save", { page: page.id, placements });
      return page;
    }

    // Copies the saved page; unsaved edits are discarded first (after asking).
    async duplicate() {
      if (!this.page || !this.space) return;
      if (this.editing && !(await this.confirmLeave())) return;
      const space = this.space;
      try {
        const saved = await this.service("page_get", { page: this.page.id });
        const title = `${saved.page.title} copy`.slice(0, 40);
        const page = await this.copyPage(space, saved, title);
        this.pages = await this.service("pages_list", { space });
        await this.o.show(page.id);
        this.edit();
        this.$("page-title-input").select();
        this.o.toast(`Duplicated as “${title}”`);
      } catch (error) {
        this.o.toast(String(error), "error");
      }
    }

    // Pointer dragging on a move or resize handle, plus keyboard editing.
    drag(handle, id, resize) {
      let start = null;
      handle.addEventListener("pointerdown", (event) => {
        if (!this.editing || event.button !== 0 || this.narrow.matches) return;
        // The header drags too, but its own buttons stay buttons.
        const control = event.target.closest("button, a, input, select, textarea");
        if (control && control !== handle) return;
        const placement = this.draft.find((p) => p.id === id);
        if (!placement) return;
        event.preventDefault();
        handle.setPointerCapture(event.pointerId);
        const bounds = this.$("widgets").getBoundingClientRect();
        start = { x: event.clientX, y: event.clientY, base: this.draft.map((p) => ({ ...p })), placement: { ...placement }, col: (bounds.width + GAP) / this.grid.COLS };
        this.cards.get(id)?.classList.add("page-dragging");
      });
      handle.addEventListener("pointermove", (event) => {
        if (!start) return;
        const dx = Math.round((event.clientX - start.x) / start.col);
        const dy = Math.round((event.clientY - start.y) / (ROW + GAP));
        const p = start.placement;
        const rect = resize ? { w: p.w + dx, h: p.h + dy } : { x: p.x + dx, y: Math.max(0, p.y + dy) };
        this.draft = this.grid.place(start.base, id, rect, this.size(id));
        this.layout();
        const content = this.$("content").getBoundingClientRect();
        if (event.clientY > content.bottom - 48) this.$("content").scrollTop += 16;
        else if (event.clientY < content.top + 48) this.$("content").scrollTop -= 16;
      });
      const end = () => {
        if (!start) return;
        start = null;
        this.cards.get(id)?.classList.remove("page-dragging");
        this.sizeChanged(id);
      };
      handle.addEventListener("pointerup", end);
      handle.addEventListener("pointercancel", end);
      handle.addEventListener("keydown", (event) => {
        if (!this.editing || !ARROWS.includes(event.key)) return;
        event.preventDefault();
        this.draft = this.grid.nudge(this.draft, id, event.key, resize || event.shiftKey, this.size(id));
        this.layout();
        this.sizeChanged(id);
        handle.focus({ preventScroll: true });
        handle.scrollIntoView({ block: "nearest" });
      });
    }

    // Size classes change what a template shows; the view learns on Done.
    sizeChanged(id) {
      const placement = this.list().find((p) => p.id === id);
      const card = this.cards.get(id);
      if (placement && card) card.dataset.size = this.grid.sizeClass(placement);
    }
  };
})();
