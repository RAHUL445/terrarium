// Declarative UI v1 renderer. Turns a plugin's template (data, not code)
// into DOM. Rendering never calls plugin code: lists run the manifest's
// named queries through the host, and only actions invoke the plugin.
//
// Template: { "state": {...}, "body": node }.
// Node: { "type": ..., "if"?, "grow"?, "basis"?, ...props, "children"? }.
// Strings may hold {{path}} lookups into the scope: state.*, row.*,
// form.*, value, result (after an operation), $today, $page (focus and
// title of the page a widget is on; lists may give `empty_focus` text
// for when a focus filters everything out), $config (widget
// settings on a page) and $size (compact, regular or expanded). A string
// that is exactly "{{path}}" yields the raw value (number, boolean, null); otherwise lookups are joined as text.
// A whole string may also be "{{!path}}" (not) or "{{path == word}}" /
// "{{path != word}}", compared as text.
//
// All text goes through textContent / setAttribute: templates cannot
// inject markup or event handlers.
"use strict";

(function () {
  const profile = window.TerrariumPerformance || { measureSync: (name, run) => run() };
  const lifecycle = { activeViews: 0, activeTimers: 0, mountedTotal: 0, disposedTotal: 0 };
  const WHOLE = /^\{\{\s*(!?)\s*([\w$.]+)\s*(?:(==|!=)\s*([\w$.-]*)\s*)?\}\}$/;
  const PART = /\{\{\s*([\w$.]+)\s*\}\}/g;

  function lookup(scope, path) {
    let value = scope;
    for (const key of path.split(".")) {
      if (value == null) return undefined;
      value = value[key];
    }
    return value;
  }

  // A list's empty text; `empty_focus` replaces it while the page has a
  // focus, so a filtered widget can say nothing matches instead of "none yet".
  function emptyText(spec, scope) {
    return (spec.empty_focus && scope?.$page?.focus) ? spec.empty_focus : spec.empty;
  }

  function resolve(input, scope) {
    if (typeof input === "string") {
      const whole = input.match(WHOLE);
      if (whole) {
        const [, not, path, op, word] = whole;
        let value = lookup(scope, path);
        if (op) value = (String(value ?? "") === word) === (op === "==");
        return not ? !truthy(value) : value;
      }
      return input.replace(PART, (_, path) => {
        const v = lookup(scope, path);
        return v == null ? "" : String(v);
      });
    }
    if (Array.isArray(input)) return input.map((v) => resolve(v, scope));
    if (input && typeof input === "object") {
      const out = {};
      for (const [k, v] of Object.entries(input)) out[k] = resolve(v, scope);
      return out;
    }
    return input;
  }

  function truthy(v) {
    return !(v == null || v === false || v === 0 || v === "" || (Array.isArray(v) && v.length === 0));
  }

  function el(tag, className, text) {
    const node = document.createElement(tag);
    if (className) node.className = className;
    if (text != null) node.textContent = String(text);
    return node;
  }

  // The clipboard API where allowed, else a hidden text area and execCommand.
  async function copyText(text) {
    try {
      await navigator.clipboard.writeText(text);
      return true;
    } catch {
      const area = el("textarea");
      area.value = text;
      area.setAttribute("readonly", "");
      area.style.cssText = "position:fixed;opacity:0;pointer-events:none";
      document.body.append(area);
      area.select();
      const ok = document.execCommand("copy");
      area.remove();
      return ok;
    }
  }

  let uid = 0;
  const nextId = (prefix) => `${prefix}-${++uid}`;

  // One mounted template: its state, its lists, and its host bridge.
  class View {
    constructor(root, template, bridge, options = {}) {
      this.root = root;
      // hideTitle: the host draws the page title, so the template's first
      // top-level heading is left out.
      this.options = options;
      this.titleHidden = false;
      this.template = template;
      this.bridge = bridge; // { query(name, params), action(op, args), toast(msg, kind), today() }
      this.state = structuredClone(template.state || {});
      for (const [key, value] of Object.entries(options.snapshot?.state || {})) {
        if (Object.hasOwn(this.state, key)) this.state[key] = structuredClone(value);
      }
      // The template owns which state opens a search result. IDs stay data.
      const entry = template.entry?.bind?.match(/^state\.([a-zA-Z_]\w*)$/)?.[1];
      if (options.item != null && entry && Object.hasOwn(this.state, entry)) this.state[entry] = String(options.item);
      // Widget settings: declared defaults (`page.config`), then the
      // placement's own values. Settings are data for bindings only.
      this.config = {};
      for (const [name, field] of Object.entries(template.page?.config || {})) {
        if (field && typeof field === "object" && "default" in field) this.config[name] = field.default;
      }
      Object.assign(this.config, options.config || {});
      // The page a widget sits on: its focus (a project, tag or word) lets
      // widgets filter their own data. Blank outside pages (e.g. the rail).
      this.page = { focus: String(options.page?.focus || ""), title: String(options.page?.title || "") };
      this.lists = [];
      this.watchers = [];
      this.cleanups = [];
      this.timers = new Map();
      this.disposed = false;
      lifecycle.activeViews++; lifecycle.mountedTotal++;
    }

    schedule(run, delay, repeat = false, onCancel = null) {
      if (this.disposed) return null;
      const id = (repeat ? setInterval : setTimeout)(() => {
        if (!repeat) { this.timers.delete(id); lifecycle.activeTimers--; }
        if (!this.disposed) run();
      }, delay);
      this.timers.set(id, { repeat, onCancel });
      lifecycle.activeTimers++;
      return id;
    }

    cancel(id) {
      const timer = this.timers.get(id);
      if (!timer) return;
      (timer.repeat ? clearInterval : clearTimeout)(id);
      this.timers.delete(id); lifecycle.activeTimers--;
      timer.onCancel?.();
    }

    release(region) {
      for (const cleanup of region.cleanups || []) { try { cleanup(); } catch (_) {} }
      region.cleanups = []; region.watchers = []; region.lists = [];
    }

    dispose() {
      if (this.disposed) return;
      this.disposed = true;
      this.release(this);
      for (const id of this.timers.keys()) this.cancel(id);
      lifecycle.activeViews--; lifecycle.disposedTotal++;
      profile.record?.("lifecycle/dispose", { ...lifecycle });
    }

    mount() {
      this.titleHidden = false;
      this.root.replaceChildren();
      this.lists = [];
      this.watchers = [];
      profile.measureSync("render/mount", () => {
        const body = this.node(this.template.body, this.scope());
        if (body) this.root.append(body);
      });
      for (const watch of this.watchers) watch();
      return this.refresh().then(() => {
        if (this.disposed) return;
        profile.record?.("lifecycle/ready", { ...lifecycle });
        if (!this.options.snapshot) return;
        const forms = [...this.root.querySelectorAll("form[data-pp-draft]")];
        for (const [index, form] of forms.entries()) {
          if (form.dataset.ppEdited) continue;
          const fields = [...form.querySelectorAll("[data-pp-name]")];
          const saved = this.options.snapshot?.forms?.[index];
          if (!saved || saved.names !== fields.map(input => input.dataset.ppName).join("|")) continue;
          fields.forEach((input, i) => {
            input.value = saved.values[i].value;
            input.checked = saved.values[i].checked;
            input.dispatchEvent(new Event("pp-value"));
          });
          form.dispatchEvent(new Event("input", { bubbles: true }));
        }
      });
    }

    snapshot() {
      return { state: structuredClone(this.state), forms: [...this.root.querySelectorAll("form[data-pp-draft]")].map(form => {
        const fields = [...form.querySelectorAll("[data-pp-name]")];
        return { names: fields.map(input => input.dataset.ppName).join("|"), values: fields.map(input => ({ value: input.value, checked: input.checked })) };
      }) };
    }

    scope(extra = {}) {
      const today = this.bridge.today();
      // Monday = 0 … Sunday = 6, matching the plugin clock interface.
      const weekday = (new Date(`${today}T12:00:00`).getDay() + 6) % 7;
      // Add to a UTC ms timestamp to get local wall-clock ms (for SQL dates).
      const offset = -new Date().getTimezoneOffset() * 60000;
      return { state: this.state, $today: today, $weekday: weekday, $utc_offset_ms: offset, $config: this.config, $size: this.options.size || "regular", $page: this.page, ...extra };
    }

    setState(path, value) {
      if (this.disposed) return;
      const key = path.replace(/^state\./, "");
      if (this.state[key] === value) return;
      this.state[key] = value;
      for (const watch of this.watchers) watch();
      this.refresh();
    }

    refresh() {
      if (this.disposed) return Promise.resolve();
      return Promise.all(this.lists.map((list) => list()));
    }

    async flush({ unmount = false } = {}) {
      if (this.disposed) return true;
      if (unmount && this.root.querySelector?.('.pp-recorder[data-state="recording"], .pp-recorder[data-state="saving"]')) return false;
      // A user may keep typing while a save is in flight. Drain those edits
      // too before allowing the host to replace this view.
      const forms = [...this.root.querySelectorAll("form")];
      do {
        const detail = { waits: [] };
        for (const form of forms) form.dispatchEvent(new CustomEvent("pp-flush", { detail }));
        if ((await Promise.all(detail.waits)).some((ok) => ok === false)) return false;
      } while (forms.some((form) => form.dataset?.ppUnsaved === "true"));
      return true;
    }

    async act(action, scope) {
      if (!action || this.disposed) return false;
      if (action.navigate) {
        await this.bridge.openItem?.(String(resolve(action.navigate.item, scope) || ""));
        return true;
      }
      // Local state changes, e.g. {"set": {"state.selected": "{{row.id}}"}}.
      if (action.set) {
        for (const [path, value] of Object.entries(action.set)) this.setState(path, resolve(value, scope));
        if (!action.op && !action.open) return true;
      }
      // Copies text, e.g. {"copy": "{{row.copy_text}}", "success": "Copied"}.
      if (action.copy) {
        const ok = await copyText(String(resolve(action.copy, scope) ?? ""));
        this.bridge.toast(ok ? resolve(action.success || "Copied", scope) : "Couldn’t copy", ok ? "ok" : "error");
        return ok;
      }
      // Opens an http(s) address in the system browser, e.g. {"open": "{{row.url}}", "op": "visit"}.
      if (action.open) {
        const url = String(resolve(action.open, scope) || "");
        if (/^https?:\/\//i.test(url)) this.bridge.open(url);
        if (!action.op) return true;
      }
      const args = resolve(action.args || {}, scope);
      try {
        const result = await this.bridge.action(action.op, args);
        if (this.disposed) { this.bridge.changed(); return true; }
        const after = { ...scope, result };
        if (action.success) this.bridge.toast(resolve(action.success, after), "ok", action.undo ? {
          label: "Undo", run: () => this.act(resolve(action.undo, after), after),
        } : null);
        // State changes once the operation succeeded, e.g. closing a sheet.
        for (const [path, value] of Object.entries(action.then || {})) this.setState(path, resolve(value, after));
        this.bridge.changed();
        return true;
      } catch (e) {
        if (this.disposed) return false;
        this.bridge.toast(String(e), "error");
        return false;
      }
    }

    // Builds with fresh list and watcher registries, so a part of the view
    // that comes and goes (a sheet, a conditional) owns what it created.
    region(build) {
      const saved = [this.lists, this.watchers, this.cleanups];
      const region = { lists: [], watchers: [], cleanups: [] };
      [this.lists, this.watchers, this.cleanups] = [region.lists, region.watchers, region.cleanups];
      try {
        build();
      } finally {
        [this.lists, this.watchers, this.cleanups] = saved;
      }
      return region;
    }

    // A node whose `if` depends on state outside a list row is shown and
    // hidden as state changes.
    reactive(spec, scope) {
      const holder = el("div", "pp-if");
      const inner = { ...spec };
      delete inner.if;
      // Form conditions retain typed values while hidden, but hidden fields
      // are omitted from submission. They update without rebuilding controls.
      if (spec.if.includes("form.")) {
        const child = this.node(inner, scope);
        if (child) holder.append(child);
        const sync = () => { holder.hidden = !truthy(resolve(spec.if, { ...scope, state: this.state })); };
        this.watchers.push(sync);
        sync();
        return holder;
      }
      let shown = null;
      let region = { lists: [], watchers: [] };
      this.cleanups.push(() => this.release(region));
      const sync = () => {
        const want = truthy(resolve(spec.if, this.scope()));
        if (want !== shown) {
          shown = want;
          this.release(region);
          holder.replaceChildren();
          region = this.region(() => {
            const node = want ? this.node(inner, { ...scope, state: this.state }) : null;
            if (node) holder.append(node);
          });
        }
        for (const watch of region.watchers) watch();
      };
      this.watchers.push(sync);
      this.lists.push(() => Promise.all(region.lists.map((list) => list())));
      sync();
      return holder;
    }

    node(spec, scope) {
      if (!spec || typeof spec !== "object") return null;
      if ("if" in spec && typeof spec.if === "string" && (spec.if.includes("form.") || (!scope.row && spec.if.includes("state.")))) return this.reactive(spec, scope);
      if ("if" in spec && !truthy(resolve(spec.if, scope))) return null;
      const make = COMPONENTS[spec.type];
      const node = make ? make.call(this, spec, scope) : el("div", "pp-unsupported", `Unsupported component: ${spec.type}`);
      if (!node) return null;
      if (spec.grow != null) node.style.flexGrow = String(spec.grow);
      if (spec.basis != null) node.style.flexBasis = `${Number(spec.basis)}px`;
      if (spec.grow != null || spec.basis != null) node.style.minWidth = "0";
      return node;
    }

    children(parent, spec, scope) {
      for (const child of spec.children || []) {
        const node = this.node(child, scope);
        if (node) parent.append(node);
      }
      return parent;
    }
  }

  const COMPONENTS = {
    stack(spec, scope) {
      const node = el("div", "pp-stack");
      node.style.flexDirection = spec.direction === "row" ? "row" : "column";
      node.style.gap = `${Number(spec.gap ?? 8)}px`;
      if (spec.wrap) node.style.flexWrap = "wrap";
      const align = { start: "flex-start", center: "center", end: "flex-end", stretch: "stretch" };
      const justify = { start: "flex-start", center: "center", end: "flex-end", between: "space-between" };
      if (align[spec.align]) node.style.alignItems = align[spec.align];
      if (justify[spec.justify]) node.style.justifyContent = justify[spec.justify];
      return this.children(node, spec, scope);
    },

    card(spec, scope) {
      return this.children(el("div", "pp-card"), spec, scope);
    },

    section(spec, scope) {
      if (spec.collapsible) {
        const node = el("details", "pp-section pp-collapsible");
        // "open": starts expanded, e.g. "{{row.needs_check}}".
        if (spec.open !== undefined && truthy(resolve(spec.open, scope))) node.open = true;
        node.append(el("summary", "pp-section-title", resolve(spec.title, scope)));
        return this.children(node, spec, scope);
      }
      const node = el("section", "pp-section");
      if (spec.title) node.append(el("h2", "pp-section-title", resolve(spec.title, scope)));
      return this.children(node, spec, scope);
    },

    heading(spec, scope) {
      const level = Math.min(Math.max(Number(spec.level) || 1, 1), 3);
      if (level === 1 && this.options.hideTitle && !this.titleHidden) {
        this.titleHidden = true;
        return null;
      }
      return el(`h${level}`, `pp-heading pp-h${level}`, resolve(spec.text, scope));
    },

    text(spec, scope) {
      return el("p", `pp-text${spec.tone === "muted" ? " pp-muted" : ""}`, resolve(spec.text, scope));
    },

    badge(spec, scope) {
      return el("span", "pp-badge", resolve(spec.text, scope));
    },

    stat(spec, scope) {
      const node = el("div", "pp-stat");
      node.append(el("span", "pp-stat-value", resolve(spec.value, scope) ?? 0));
      node.append(el("span", "pp-stat-label", resolve(spec.label, scope)));
      // A change next to the number, e.g. "▲ 3 vs last week"; tone good / bad.
      const note = resolve(spec.note, scope);
      if (note) {
        const tone = resolve(spec.tone, scope);
        node.append(el("span", `pp-stat-note${tone === "good" || tone === "bad" ? ` pp-stat-${tone}` : ""}`, note));
      }
      return node;
    },

    divider() {
      return el("hr", "pp-divider");
    },

    empty(spec, scope) {
      return el("p", "pp-empty", resolve(spec.text, scope));
    },

    button(spec, scope) {
      const variant = ["primary", "danger", "ghost"].includes(spec.variant) ? ` pp-${spec.variant}` : spec.variant === "chip" ? " pp-chip" : "";
      const node = el("button", `pp-button${variant}`, resolve(spec.label, scope));
      if ("pressed" in spec) node.setAttribute("aria-pressed", String(truthy(resolve(spec.pressed, scope))));
      if (spec.icon && ICONS[spec.icon]) {
        node.prepend(icon(spec.icon));
        if (!spec.label) node.classList.add("pp-icon-button");
      }
      node.type = "button";
      if (spec.aria_label) node.setAttribute("aria-label", resolve(spec.aria_label, scope));
      node.addEventListener("click", async () => {
        if (spec.action?.confirm) {
          const question = resolve(spec.action.confirm, scope);
          const ok = window.terrariumConfirm
            ? await window.terrariumConfirm(question, { confirm: spec.label ? resolve(spec.label, scope) : "OK", danger: spec.variant === "danger" })
            : window.confirm(question);
          if (!ok) return;
        }
        node.disabled = true;
        // In a sheet, let its autosaving forms finish first so the
        // operation sees what was just typed.
        const sheet = node.closest(".pp-sheet");
        if (sheet) {
          const flush = { waits: [] };
          for (const f of sheet.querySelectorAll("form")) f.dispatchEvent(new CustomEvent("pp-flush", { detail: flush }));
          await Promise.all(flush.waits);
        }
        await this.act(spec.action, scope);
        node.disabled = false;
      });
      return node;
    },

    checkbox(spec, scope) {
      const node = el("label", "pp-checkbox");
      const input = el("input");
      input.type = "checkbox";
      input.checked = truthy(resolve(spec.checked, scope));
      const text = el("span", "", resolve(spec.label, scope));
      if (input.checked) text.classList.add("pp-done");
      input.addEventListener("change", async () => {
        const ok = await this.act(spec.action, { ...scope, value: input.checked });
        if (!ok) input.checked = !input.checked;
      });
      node.append(input, text);
      return node;
    },

    tabs(spec, scope) {
      const node = el("div", "pp-tabs");
      node.setAttribute("role", "tablist");
      if (spec.label) node.setAttribute("aria-label", resolve(spec.label, scope));
      const buttons = [];
      const sync = () => {
        const current = lookup({ state: this.state }, spec.bind);
        for (const [value, button] of buttons) {
          button.setAttribute("aria-selected", String(value === current));
          button.tabIndex = value === current ? 0 : -1;
        }
      };
      for (const option of spec.options || []) {
        const button = el("button", "pp-tab", resolve(option.label, scope));
        button.type = "button";
        button.setAttribute("role", "tab");
        button.addEventListener("click", () => this.setState(spec.bind, option.value));
        button.addEventListener("keydown", (e) => {
          if (e.key !== "ArrowRight" && e.key !== "ArrowLeft") return;
          const i = buttons.findIndex(([, b]) => b === button);
          const next = buttons[(i + (e.key === "ArrowRight" ? 1 : buttons.length - 1)) % buttons.length];
          next[1].focus();
          this.setState(spec.bind, next[0]);
        });
        buttons.push([option.value, button]);
        node.append(button);
      }
      this.watchers.push(sync);
      sync();
      return node;
    },

    "search-field"(spec, scope) {
      const id = nextId("pp-search");
      const node = el("div", "pp-field");
      const label = el("label", `pp-label${spec.label_hidden ? " pp-sr" : ""}`, resolve(spec.label || "Search", scope));
      label.htmlFor = id;
      const input = el("input", "pp-input");
      input.type = "search";
      input.id = id;
      input.placeholder = resolve(spec.placeholder || "Search", scope);
      input.value = lookup({ state: this.state }, spec.bind) ?? "";
      let timer;
      this.cleanups.push(() => this.cancel(timer));
      input.addEventListener("input", () => {
        this.cancel(timer);
        timer = this.schedule(() => this.setState(spec.bind, input.value), 150);
      });
      node.append(label, input);
      return node;
    },

    form(spec, scope) {
      const form = el("form", `pp-form${spec.inline ? " pp-form-inline" : ""}`);
      if (!spec.autosave) form.setAttribute("data-pp-draft", "true");
      form.addEventListener("input", () => { form.dataset.ppEdited = "true"; });
      scope = { ...scope, form: {} };
      const readValues = () => Object.fromEntries([...form.querySelectorAll("[data-pp-name]")]
        .filter((input) => !input.closest(".pp-if[hidden]"))
        .map((input) => [input.dataset.ppName, input.dataset.ppKind === "toggle" ? input.checked
          : input.dataset.ppKind === "number" ? (input.value === "" ? null : Number(input.value)) : input.value]));
      const region = this.region(() => this.children(form, spec, scope));
      this.cleanups.push(() => this.release(region));
      this.lists.push(() => Promise.all(region.lists.map((list) => list())));
      const syncForm = () => {
        // Read all fields for conditions (including a hidden field that may
        // be revealed again), then collect visible fields for submission.
        Object.assign(scope.form, Object.fromEntries([...form.querySelectorAll("[data-pp-name]")]
          .map((input) => [input.dataset.ppName, input.dataset.ppKind === "toggle" ? input.checked : input.value])));
        for (const watch of region.watchers) watch();
      };
      this.watchers.push(syncForm);
      form.addEventListener("input", syncForm);
      form.addEventListener("change", syncForm);
      syncForm();
      const submit = el("button", "pp-button pp-primary", resolve(spec.submit_label || "Save", scope));
      submit.type = "submit";
      if (spec.submit_icon && ICONS[spec.submit_icon]) {
        submit.setAttribute("aria-label", submit.textContent);
        submit.title = submit.textContent;
        submit.replaceChildren(icon(spec.submit_icon));
        submit.classList.add("pp-icon-button");
      }
      form.append(submit);
      let defaultsPending = false;
      let defaultsFailed = false;
      let defaultsSeq = 0;
      this.cleanups.push(() => { defaultsSeq++; });
      const defaults = spec.defaults;
      if (defaults) {
        const status = el("span", "pp-muted");
        status.setAttribute("role", "status");
        form.append(status);
        const loadDefaults = async () => {
          const seq = ++defaultsSeq;
          defaultsPending = true;
          defaultsFailed = false;
          submit.disabled = true;
          status.textContent = "Loading template choices…";
          try {
            const rows = await this.bridge.query(defaults.query, resolve(defaults.params || {}, scope));
            if (seq !== defaultsSeq) return;
            const row = rows[0] || {};
            for (const input of form.querySelectorAll("[data-pp-name]")) {
              const name = input.dataset.ppName;
              if (!Object.hasOwn(defaults.fields, name)) continue;
              input.value = String(row[defaults.fields[name]] || defaults.fallback?.[name] || "");
              input.dispatchEvent(new Event("pp-value"));
            }
            syncForm();
            status.textContent = "Template choices loaded. You can change them below.";
          } catch (_) {
            if (seq !== defaultsSeq) return;
            defaultsFailed = true;
            status.textContent = "Couldn’t load template choices. Choose the template again to retry.";
          } finally {
            if (seq === defaultsSeq) {
              defaultsPending = false;
              submit.disabled = defaultsFailed;
            }
          }
        };
        form.addEventListener("change", (e) => {
          if (e.target.dataset.ppName === defaults.field) loadDefaults();
        });
        loadDefaults();
      }
      if (spec.autosave) {
        // Saves quietly a moment after typing stops, and when the form is
        // flushed (its sheet closes). No button, no toast.
        submit.hidden = true;
        const status = el("span", "pp-save-status", "");
        status.setAttribute("aria-live", "polite");
        // Next to an editor's word count when there is one.
        const foot = form.querySelector(".pp-md-foot");
        if (foot) foot.prepend(status);
        else form.append(status);
        const retry = el("button", "pp-button pp-ghost", "Retry save");
        retry.type = "button";
        retry.hidden = true;
        form.append(retry);
        const delay = Number(spec.autosave) > 1 ? Number(spec.autosave) : 800;
        const read = () => {
          const values = {};
          for (const input of form.querySelectorAll("[data-pp-name]")) {
            values[input.dataset.ppName] = input.dataset.ppKind === "toggle" ? input.checked : input.value;
          }
          return values;
        };
        let saved = JSON.stringify(read());
        let timer = null;
        let running = Promise.resolve(true);
        const save = () => {
          this.cancel(timer);
          timer = null;
          const values = read();
          const text = JSON.stringify(values);
          if (text === saved) return running;
          saved = text;
          status.textContent = "Saving…";
          running = running.then(async () => {
            if (this.disposed) return true;
            const ok = await this.act({ op: spec.submit.op, args: spec.submit.args }, { ...scope, form: values });
            if (this.disposed) return ok;
            if (!ok) saved = "";
            if (JSON.stringify(read()) === text) {
              form.dataset.ppUnsaved = String(!ok);
              status.textContent = ok ? "Saved on this device" : "Not saved — keep this editor open and try again";
            }
            retry.hidden = ok;
            return ok;
          });
          return running;
        };
        retry.addEventListener("click", save);
        const later = () => {
          this.cancel(timer);
          form.dataset.ppUnsaved = "true";
          status.textContent = "Changes waiting to save…";
          timer = this.schedule(save, delay);
        };
        this.cleanups.push(() => this.cancel(timer));
        form.addEventListener("input", later);
        form.addEventListener("change", later);
        form.addEventListener("pp-flush", (e) => e.detail?.waits.push(save()));
        form.addEventListener("submit", (e) => (e.preventDefault(), save()));
        return form;
      }
      const feedback = el("span", "pp-form-feedback pp-muted");
      feedback.setAttribute("role", "status");
      feedback.setAttribute("aria-atomic", "true");
      form.append(feedback);
      form.addEventListener("input", () => {
        if (submit.disabled) return;
        feedback.textContent = "";
        feedback.classList.remove("pp-tone-danger");
      });
      let submission = null;
      form.addEventListener("pp-flush", event => {
        if (submission) event.detail?.waits.push(submission);
      });
      form.addEventListener("submit", async (e) => {
        e.preventDefault();
        if (defaultsPending || defaultsFailed || submit.disabled) return;
        syncForm();
        const values = readValues();
        const missing = [...form.querySelectorAll("[data-pp-required]")]
          .find((input) => !input.closest(".pp-if[hidden]") && !input.value);
        if (missing) {
          this.bridge.toast("Choose at least one data source", "error");
          missing.closest("fieldset")?.querySelector("button")?.focus();
          return;
        }
        const inputs = [...form.querySelectorAll("[data-pp-name]")];
        submit.disabled = true;
        form.setAttribute("aria-busy", "true");
        feedback.textContent = "Saving…";
        try {
          const ok = await (submission = this.act(spec.submit, { ...scope, form: values }));
          feedback.classList.toggle("pp-tone-danger", !ok);
          feedback.textContent = ok ? (spec.submit?.reset ? "Saved on this device" : "Done") : "Couldn’t save. Your entry is still here; try again.";
          // Keep the submitted text visible until success. If the user edits
          // while saving, leave the newer draft intact for the next submit.
          if (ok && spec.submit?.reset && JSON.stringify(readValues()) === JSON.stringify(values)) {
            for (const input of inputs) {
              if (input.dataset.ppKind === "toggle") input.checked = input.defaultChecked;
              else if (input.dataset.ppKind !== "choice") input.value = input.defaultValue;
            }
            for (const choice of form.querySelectorAll(".pp-choice")) choice.dispatchEvent(new Event("pp-reset"));
            syncForm();
            if (form.isConnected && form.contains(document.activeElement)) inputs.find((input) => input.type !== "hidden")?.focus();
          } else if (ok && spec.submit?.reset) {
            feedback.textContent = "Previous entry saved. Your new draft is ready to save.";
          }
        } finally {
          submission = null;
          submit.disabled = false;
          form.removeAttribute("aria-busy");
        }
      });
      return form;
    },

    "text-field"(spec, scope) {
      const node = field.call(this, spec, scope, "input", "text");
      // Suggestions from a query column, e.g. existing project names.
      if (spec.suggestions?.query) {
        const list = el("datalist");
        list.id = nextId("pp-suggest");
        node.querySelector("input").setAttribute("list", list.id);
        node.append(list);
        this.bridge
          .query(spec.suggestions.query, resolve(spec.suggestions.params || {}, this.scope()))
          .then((rows) => {
            if (this.disposed) return;
            for (const row of rows) {
              const opt = el("option");
              opt.value = String(row[spec.suggestions.column] ?? "");
              list.append(opt);
            }
          })
          .catch(() => {});
      }
      return node;
    },

    /**
     * Markdown text with a toolbar, list continuation and a live preview
     * whose checkboxes can be ticked: {name, label, value, rows, actions:
     * [{label, icon, op, arg}]}. An action gets the selected text, or the
     * current line, as `arg`.
     */
    "markdown-editor"(spec, scope) {
      const id = nextId("pp-md");
      const node = el("div", "pp-field pp-md");
      const label = el("label", "pp-label pp-sr", resolve(spec.label || spec.name, scope));
      label.htmlFor = id;
      const bar = el("div", "pp-md-bar");
      bar.setAttribute("role", "toolbar");
      bar.setAttribute("aria-label", "Formatting");
      const area = el("textarea", "pp-input pp-md-area");
      area.id = id;
      area.dataset.ppName = spec.name;
      area.dataset.ppKind = "text-area";
      area.rows = Number(spec.rows) || 16;
      if (spec.placeholder) area.placeholder = resolve(spec.placeholder, scope);
      area.value = String(resolve(spec.value, scope) ?? "");
      const preview = el("div", "pp-md-preview");
      preview.hidden = true;
      const foot = el("div", "pp-md-foot");
      const count = el("span", "pp-muted", "");
      foot.append(count);
      const changed = () => area.dispatchEvent(new Event("input", { bubbles: true }));
      const words = () => {
        const text = area.value.replace(/^\s*(#+|[-*>]|\d+\.)\s+(\[[ xX]\]\s+)?/gm, "");
        const n = (text.match(/[^\s*_`#>-]\S*/g) || []).length;
        count.textContent = `${n} ${n === 1 ? "word" : "words"}`;
      };
      area.addEventListener("input", words);
      words();

      const lineBounds = () => {
        const v = area.value;
        const start = v.lastIndexOf("\n", area.selectionStart - 1) + 1;
        let end = v.indexOf("\n", area.selectionEnd);
        if (end < 0) end = v.length;
        return [start, end];
      };
      const replace = (start, end, text, select = [start + text.length, start + text.length]) => {
        area.focus();
        area.setRangeText(text, start, end);
        area.setSelectionRange(...select);
        changed();
      };
      const wrap = (mark) => {
        const { selectionStart: a, selectionEnd: b, value } = area;
        const inner = value.slice(a, b) || "text";
        replace(a, b, `${mark}${inner}${mark}`, [a + mark.length, a + mark.length + inner.length]);
      };
      const prefix = (mark) => {
        const [start, end] = lineBounds();
        const lines = area.value.slice(start, end).split("\n");
        const all = lines.every((l) => l.startsWith(mark));
        const out = lines.map((l) => (all ? l.slice(mark.length) : mark + l.replace(/^(#+ |[-*] (\[[ x]\] )?|\d+\. )/, ""))).join("\n");
        replace(start, end, out);
      };
      const tools = [
        ["Heading", "H", () => prefix("## ")],
        ["Bold", "B", () => wrap("**")],
        ["Italic", "I", () => wrap("*")],
        ["Bulleted list", "•", () => prefix("- ")],
        ["Checklist", "☐", () => prefix("- [ ] ")],
        ["Numbered list", "1.", () => prefix("1. ")],
      ];
      for (const [name, text, run] of tools) {
        const b = el("button", "pp-button pp-ghost pp-md-tool", text);
        b.type = "button";
        b.setAttribute("aria-label", name);
        b.title = name;
        b.addEventListener("click", run);
        bar.append(b);
      }
      for (const action of spec.actions || []) {
        const b = el("button", "pp-button pp-ghost pp-md-tool pp-md-action", resolve(action.label, scope));
        b.type = "button";
        if (action.icon && ICONS[action.icon]) b.prepend(icon(action.icon));
        b.addEventListener("click", async () => {
          const { selectionStart: a, selectionEnd: z, value } = area;
          let text = value.slice(a, z);
          if (!text.trim()) {
            const [start, end] = lineBounds();
            text = value.slice(start, end);
          }
          if (!text.trim()) return this.bridge.toast("Put the cursor on a line first", "error");
          b.disabled = true;
          await this.act({ op: action.op, args: { [action.arg || "text"]: text.trim() }, success: action.success }, scope);
          b.disabled = false;
        });
        bar.append(b);
      }
      const toggle = el("button", "pp-button pp-ghost pp-md-tool pp-md-mode", "Preview");
      toggle.type = "button";
      toggle.setAttribute("aria-pressed", "false");
      const showPreview = (on) => {
        preview.hidden = !on;
        area.hidden = on;
        toggle.textContent = on ? "Edit" : "Preview";
        toggle.setAttribute("aria-pressed", String(on));
        for (const b of bar.querySelectorAll(".pp-md-tool:not(.pp-md-mode)")) b.disabled = on;
        if (on) {
          preview.replaceChildren(
            markdown(area.value, {
              open: (url) => this.bridge.open(url),
              toggle: (line) => {
                const lines = area.value.split("\n");
                lines[line] = lines[line].replace(/\[( |x)\]/, (_, c) => (c === "x" ? "[ ]" : "[x]"));
                area.value = lines.join("\n");
                changed();
                showPreview(true);
              },
            }),
          );
          if (!area.value.trim()) preview.append(el("p", "pp-muted", "Nothing to preview yet."));
        } else area.focus();
      };
      toggle.addEventListener("click", () => showPreview(preview.hidden));
      bar.append(toggle);
      if (spec.start === "preview" && area.value.trim()) showPreview(true);

      // Enter continues a list; Enter on an empty item ends it.
      area.addEventListener("keydown", (e) => {
        if (e.key !== "Enter" || e.shiftKey || e.isComposing) return;
        const [start] = lineBounds();
        const line = area.value.slice(start, area.selectionStart);
        const m = line.match(/^(\s*)([-*] \[[ x]\] |[-*] |(\d+)\. )(.*)$/);
        if (!m) return;
        e.preventDefault();
        if (!m[4].trim()) return replace(start, area.selectionStart, "");
        const next = m[3] ? `${Number(m[3]) + 1}. ` : m[2].replace("[x]", "[ ]");
        replace(area.selectionStart, area.selectionEnd, `\n${m[1]}${next}`);
      });
      node.append(label, bar, area, preview, foot);
      return node;
    },

    "text-area"(spec, scope) {
      return field.call(this, spec, scope, "textarea");
    },

    date(spec, scope) {
      return field.call(this, spec, scope, "input", "date");
    },

    time(spec, scope) {
      return field.call(this, spec, scope, "input", "time");
    },

    number(spec, scope) {
      const node = field.call(this, spec, scope, "input", "number");
      const input = node.querySelector("input");
      for (const key of ["min", "max", "step"]) if (key in spec) input[key] = String(spec[key]);
      return node;
    },

    select(spec, scope) {
      const id = nextId("pp-field");
      const node = el("div", "pp-field");
      const label = el("label", "pp-label", resolve(spec.label || spec.name, scope));
      label.htmlFor = id;
      const select = el("select", "pp-input");
      select.id = id;
      select.dataset.ppName = spec.name;
      for (const option of spec.options || []) {
        const opt = el("option", "", resolve(option.label, scope));
        opt.value = resolve(option.value, scope);
        select.append(opt);
      }
      const wanted = "value" in spec ? String(resolve(spec.value, scope) ?? "") : null;
      if (wanted != null) select.value = wanted;
      // More options from a query, after the fixed ones:
      // {"options_query": {"query": "skills", "value": "id", "label": "name"}}.
      const more = spec.options_query;
      if (more?.query) {
        this.bridge
          .query(more.query, resolve(more.params || {}, this.scope()))
          .then((rows) => {
            for (const row of rows) {
              const opt = el("option", "", String(row[more.label || "label"] ?? ""));
              opt.value = String(row[more.value || "value"] ?? "");
              select.append(opt);
            }
            if (wanted != null) select.value = wanted;
            if (select.selectedIndex < 0 && select.options.length) select.selectedIndex = 0;
            select.dispatchEvent(new Event("change", { bubbles: true }));
          })
          .catch(() => {});
      }
      node.append(label, select);
      return node;
    },

    /** One of a few options as chips, e.g. priority: {name, label, options, value, multiple?}. Multiple holds "0,2,4". */
    choice(spec, scope) {
      const node = el("fieldset", "pp-field pp-choice");
      node.append(el("legend", "pp-label", resolve(spec.label || spec.name, scope)));
      const hidden = el("input");
      hidden.type = "hidden";
      hidden.dataset.ppName = spec.name;
      hidden.dataset.ppKind = "choice";
      if (spec.required) hidden.dataset.ppRequired = "true";
      const multi = truthy(spec.multiple);
      const raw = resolve(spec.value, scope);
      const values = (spec.options || []).map((o) => String(resolve(o.value, scope)));
      const join = (set) => values.filter((v) => set.has(v)).join(",");
      const start = multi
        ? join(new Set((Array.isArray(raw) ? raw : String(raw ?? "").split(",")).map((v) => String(v).trim())))
        : String(raw ?? spec.options?.[0]?.value ?? "");
      hidden.value = start;
      const row = el("div", "pp-choice-row");
      const buttons = [];
      const sync = () => {
        const on = new Set(multi ? hidden.value.split(",") : [hidden.value]);
        for (const [value, b] of buttons) b.setAttribute("aria-pressed", String(on.has(value)));
      };
      for (const option of spec.options || []) {
        const value = String(resolve(option.value, scope));
        const b = el("button", `pp-chip${option.tone ? ` pp-tone-${option.tone}` : ""}`, resolve(option.label, scope));
        b.type = "button";
        b.addEventListener("click", () => {
          if (multi) {
            const on = new Set(hidden.value.split(",").filter(Boolean));
            on.has(value) ? on.delete(value) : on.add(value);
            hidden.value = join(on);
          } else hidden.value = value;
          sync();
          hidden.dispatchEvent(new Event("change", { bubbles: true }));
        });
        buttons.push([value, b]);
        row.append(b);
      }
      hidden.addEventListener("pp-value", sync);
      node.addEventListener("pp-reset", () => ((hidden.value = start), sync()));
      sync();
      node.append(hidden, row);
      return node;
    },

    /**
     * A list row: optional checkbox, title, subtitle and chips; clicking it
     * runs `action` (e.g. opening a sheet). {title, subtitle, avatar, done, flag,
     * check: {checked, action}, chips: [{text, tone, icon}], action}.
     */
    item(spec, scope) {
      const flag = Number(resolve(spec.flag, scope)) || 0;
      const done = truthy(resolve(spec.done, scope));
      const node = el("div", `pp-item${flag ? ` pp-flag-${Math.min(flag, 3)}` : ""}${done ? " pp-item-done" : ""}`);
      const title = String(resolve(spec.title, scope) ?? "");
      if (spec.check) {
        const input = el("input", "pp-item-check");
        input.type = "checkbox";
        input.checked = truthy(resolve(spec.check.checked, scope));
        input.setAttribute("aria-label", `${input.checked ? "Mark not done" : "Mark done"}: ${title}`);
        input.addEventListener("change", async () => {
          node.classList.toggle("pp-item-done", input.checked);
          const ok = await this.act(spec.check.action, { ...scope, value: input.checked });
          if (!ok) {
            input.checked = !input.checked;
            node.classList.toggle("pp-item-done", input.checked);
          }
        });
        node.append(input);
      }
      // A letter tile, e.g. a link's host: {"avatar": "{{row.host}}"}.
      const avatar = String(resolve(spec.avatar, scope) ?? "").replace(/^www\./, "");
      if (avatar) {
        let hue = 0;
        for (const c of avatar) hue = (hue * 31 + c.codePointAt(0)) % 360;
        const tile = el("span", "pp-avatar", [...avatar][0].toUpperCase());
        tile.style.setProperty("--hue", String(hue));
        tile.setAttribute("aria-hidden", "true");
        node.append(tile);
      }
      const main = el(spec.action ? "button" : "div", "pp-item-main");
      if (spec.action) {
        main.type = "button";
        main.addEventListener("click", () => this.act(spec.action, scope));
      }
      main.append(el("span", "pp-item-title", title));
      const subtitle = resolve(spec.subtitle, scope);
      if (truthy(subtitle)) main.append(el("span", "pp-item-subtitle", subtitle));
      const chips = (spec.chips || [])
        .map((c) => ({ text: resolve(c.text, scope), tone: resolve(c.tone, scope), icon: resolve(c.icon, scope) }))
        .filter((c) => truthy(c.text));
      if (chips.length) {
        const row = el("span", "pp-item-chips");
        for (const c of chips) {
          const chip = el("span", `pp-meta${c.tone ? ` pp-tone-${c.tone}` : ""}`);
          if (c.icon && ICONS[c.icon]) chip.append(icon(c.icon));
          chip.append(String(c.text));
          row.append(chip);
        }
        main.append(row);
      }
      node.append(main);
      for (const child of spec.trailing || []) {
        const n = this.node(child, scope);
        if (n) node.append(n);
      }
      return node;
    },

    /**
     * A panel over the page while `bind` holds a value, e.g. the id of the
     * row being edited: {bind, title, query?, params?, children}. With a
     * query, its first row is `row` for the children.
     */
    sheet(spec, scope) {
      const dialog = el("dialog", `pp-sheet${spec.size === "large" ? " pp-sheet-large" : ""}${spec.presentation === "modal" ? " pp-sheet-modal" : ""}`);
      const head = el("div", "pp-sheet-head");
      const heading = el("h2", "pp-sheet-title", resolve(spec.title, scope));
      heading.id = nextId("pp-sheet-title");
      dialog.setAttribute("aria-labelledby", heading.id);
      const close = el("button", "pp-button pp-ghost pp-icon-button");
      close.type = "button";
      close.setAttribute("aria-label", "Close");
      close.append(icon("close"));
      head.append(heading, close);
      const body = el("div", "pp-sheet-body");
      dialog.append(head, body);
      let openKey = null;
      let region = { lists: [], watchers: [] };
      let revision = 0;
      let closing = null;
      let internalCloses = 0;
      let dead = false;
      this.cleanups.push(() => {
        dead = true; revision++; openKey = null;
        this.release(region);
        if (dialog.open) { internalCloses++; dialog.close(); }
      });
      const current = () => lookup({ state: this.state }, spec.bind);
      let closeScope = null;
      const shut = () => {
        if (dead) return Promise.resolve(true);
        if (closing) return closing;
        if (openKey == null) return Promise.resolve(true);
        const key = openKey, version = revision, scope = closeScope;
        const forms = [...body.querySelectorAll("form")];
        close.disabled = true;
        closing = (async () => {
          // Keep the modal open/inert until autosave has drained. An old close
          // must never clear a newer sheet opened while a request was pending.
          do {
            const flush = { waits: [] };
            for (const form of forms) form.dispatchEvent(new CustomEvent("pp-flush", { detail: flush }));
            if ((await Promise.all(flush.waits)).some(ok => ok === false)) return false;
          } while (forms.some(form => form.dataset.ppUnsaved === "true"));
          if (dead || version !== revision) return false;
          if (spec.on_close && scope && !await this.act(spec.on_close, scope)) return false;
          if (dead || version !== revision) return false;
          openKey = null;
          revision++;
          if (current() === key) this.setState(spec.bind, "");
          if (dialog.open) { internalCloses++; dialog.close(); }
          return true;
        })().finally(() => { closing = null; close.disabled = false; });
        return closing;
      };
      close.addEventListener("click", shut);
      dialog.addEventListener("click", e => { if (e.target === dialog) shut(); });
      dialog.addEventListener("cancel", e => { e.preventDefault(); shut(); });
      dialog.addEventListener("close", () => {
        if (internalCloses) { internalCloses--; return; }
        if (!dead && truthy(current()) && dialog.isConnected) { dialog.showModal(); shut(); }
      });
      const build = async (key, version) => {
        let row;
        if (spec.query) {
          const rows = await this.bridge.query(spec.query, resolve(spec.params || {}, this.scope())).catch(() => []);
          if (dead || version !== revision) return;
          row = rows[0];
          if (!row) return this.setState(spec.bind, "");
        }
        if (dead || key !== openKey || version !== revision) return;
        const inner = this.scope(row ? { row } : {});
        closeScope = inner;
        this.release(region);
        body.replaceChildren();
        region = this.region(() => this.children(body, spec, inner));
        heading.textContent = resolve(spec.title, inner);
        await Promise.all(region.lists.map((list) => list()));
        if (dead || key !== openKey || version !== revision) return;
        if (!dialog.open) dialog.showModal();
        // Focus the first field with a keyboard and mouse; on a phone that would pop the keyboard up.
        if (matchMedia("(hover: hover)").matches) body.querySelector("input:not([type=hidden]), textarea")?.focus({ preventScroll: true });
      };
      this.watchers.push(() => {
        const key = current();
        if (truthy(key) && key !== openKey) {
          openKey = key;
          build(key, ++revision);
        } else if (!truthy(key)) shut();
        for (const watch of region.watchers) watch();
      });
      // With `refresh`, a change to the row (say a report rebuilt from a
      // button inside) redraws the sheet, unless a field in it has focus.
      this.lists.push(async () => {
        if (dead || !dialog.open || closing) return;
        if (spec.refresh && spec.query && openKey != null) {
          const rows = await this.bridge.query(spec.query, resolve(spec.params || {}, this.scope())).catch(() => []);
          if (dead) return;
          const typing = body.contains(document.activeElement) && document.activeElement.matches("input, textarea, select, [contenteditable]");
          if (rows[0] && JSON.stringify(rows[0]) !== JSON.stringify(closeScope?.row) && !typing) {
            const inner = this.scope({ row: rows[0] });
            closeScope = inner;
            const top = body.scrollTop;
            const fresh = el("div");
            this.release(region);
            region = this.region(() => this.children(fresh, spec, inner));
            body.replaceChildren(...fresh.childNodes);
            heading.textContent = resolve(spec.title, inner);
            await Promise.all(region.lists.map((list) => list()));
            body.scrollTop = top;
            return;
          }
        }
        await Promise.all(region.lists.map((list) => list()));
      });
      return dialog;
    },

    toggle(spec, scope) {
      const node = el("label", "pp-checkbox");
      const input = el("input");
      input.type = "checkbox";
      input.dataset.ppName = spec.name;
      input.dataset.ppKind = "toggle";
      input.checked = truthy(resolve(spec.value, scope));
      node.append(input, el("span", "", resolve(spec.label || spec.name, scope)));
      return node;
    },

    "ai-status"(spec, scope) {
      const node = el("p", "pp-muted", "Checking AI destination…");
      node.setAttribute("role", "status");
      this.bridge.aiStatus?.().then((status) => {
        if (this.disposed) return;
        const model = resolve(spec.model, scope) || status.model || "No model selected";
        const destination = `${status.provider}${status.host ? ` (${status.host})` : ""} · ${model}`;
        node.textContent = `Next AI request: ${destination}.${status.ready ? "" : ` Not ready: ${String(status.reason || "Check settings").replace(/[.!]+$/, "")}. Open Settings → AI to configure it.`}`;
      }).catch(() => { node.textContent = "AI destination unavailable. Check Settings → AI before sending."; });
      return node;
    },

    markdown(spec, scope) {
      return markdown(resolve(spec.text, scope), { open: (url) => this.bridge.open(url) });
    },

    /** Opens an http(s) URL in the system browser. */
    link(spec, scope) {
      const href = String(resolve(spec.href, scope) || "");
      const node = el("a", "pp-link", resolve(spec.text ?? spec.href, scope));
      node.href = /^https?:\/\//i.test(href) ? href : "#";
      node.addEventListener("click", (e) => {
        e.preventDefault();
        if (/^https?:\/\//i.test(href)) this.bridge.open(href);
        if (spec.action) this.act(spec.action, scope);
      });
      return node;
    },

    progress(spec, scope) {
      const value = Number(resolve(spec.value, scope)) || 0;
      const node = el("div", "pp-progress");
      node.setAttribute("role", "progressbar");
      node.setAttribute("aria-valuemin", "0");
      node.setAttribute("aria-valuemax", "100");
      node.setAttribute("aria-valuenow", String(Math.round(value)));
      if (spec.label) node.setAttribute("aria-label", resolve(spec.label, scope));
      const bar = el("div", "pp-progress-bar");
      bar.style.width = `${Math.max(0, Math.min(100, value))}%`;
      node.append(bar);
      return node;
    },

    /** Days with a count, e.g. habit check-ins: {query, params, weeks}. */
    "calendar-heatmap"(spec, scope) {
      const weeks = Number(spec.weeks) || 12;
      const node = el("div", "pp-heatmap");
      node.setAttribute("role", "img");
      node.setAttribute("aria-label", resolve(spec.label || "Activity by day", scope));
      const load = async () => {
        const rows = await this.bridge.query(spec.query, resolve(spec.params || {}, this.scope())).catch(() => []);
        if (this.disposed) return;
        const counts = new Map(rows.map((r) => [r.day, Number(r.count) || 0]));
        const max = Math.max(1, ...counts.values());
        const today = new Date(`${this.bridge.today()}T12:00:00`);
        // Start on the Monday `weeks` weeks back so columns are whole weeks.
        const start = new Date(today);
        start.setDate(start.getDate() - ((start.getDay() + 6) % 7) - (weeks - 1) * 7);
        node.replaceChildren();
        for (let i = 0; i < weeks * 7; i++) {
          const d = new Date(start);
          d.setDate(start.getDate() + i);
          const key = `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
          const n = counts.get(key) || 0;
          const cell = el("span", `pp-heat pp-heat-${d > today ? "future" : Math.ceil((n / max) * 4)}`);
          cell.title = `${key}: ${n}`;
          node.append(cell);
        }
      };
      this.lists.push(load);
      return node;
    },

    /**
     * Last N days as dots, e.g. habit check-ins: {days, end, label, action}.
     * `days` is one char per day, oldest first, the last being `end`:
     * d done, p partly, s skipped, m missed, o open, - not scheduled.
     * A tap runs `action` with `day`, `mark` and `next` ("done", or "cleared" from d).
     */
    "week-strip"(spec, scope) {
      const marks = String(resolve(spec.days, scope) ?? "");
      const end = new Date(`${resolve(spec.end, scope) || this.bridge.today()}T12:00:00`);
      const words = { d: "done", p: "partly done", s: "skipped", m: "missed", o: "not done yet", "-": "not scheduled" };
      const names = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
      const months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
      const node = el("div", "pp-weekstrip");
      node.setAttribute("role", "group");
      node.setAttribute("aria-label", `${resolve(spec.label, scope) || "Habit"}, last ${marks.length} days`);
      [...marks].forEach((ch, i) => {
        const d = new Date(end);
        d.setDate(end.getDate() - (marks.length - 1 - i));
        const day = `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
        let mark = words[ch] ? ch : "-";
        const cell = el("span", "pp-weekday");
        const dot = el(spec.action ? "button" : "span", `pp-wdot pp-wdot-${mark === "-" ? "off" : mark}${i === marks.length - 1 ? " pp-wdot-today" : ""}`);
        const name = `${names[d.getDay()]} ${d.getDate()} ${months[d.getMonth()]}`;
        const paint = () => {
          dot.className = dot.className.replace(/pp-wdot-(?!today)\S+/, `pp-wdot-${mark === "-" ? "off" : mark}`);
          dot.replaceChildren(...(mark === "d" ? [icon("check")] : []));
          dot.setAttribute("aria-label", `${name}: ${words[mark]}`);
          if (spec.action) dot.setAttribute("aria-pressed", String(mark === "d"));
        };
        paint();
        if (spec.action) {
          dot.type = "button";
          dot.addEventListener("click", async (e) => {
            e.stopPropagation();
            const before = mark;
            mark = before === "d" ? "o" : "d";
            paint();
            const ok = await this.act(spec.action, { ...scope, day, mark: before, next: before === "d" ? "cleared" : "done" });
            if (!ok) ((mark = before), paint());
          });
        } else dot.setAttribute("role", "img");
        cell.append(dot, el("span", "pp-weekday-name", names[d.getDay()][0]));
        node.append(cell);
      });
      return node;
    },

    /**
     * Bars from a query: {query, params, label, value, max?, value_label?,
     * title?, layout?, empty?}. `layout: "columns"` draws vertical columns
     * (e.g. tasks per day); `value_label` names a column shown instead of
     * the number, `title` one used as each bar's tooltip and spoken label.
     */
    bar(spec, scope) {
      const columns = spec.layout === "columns";
      const node = el("div", columns ? "pp-columns" : "pp-bars");
      if (columns) node.setAttribute("role", "list");
      const load = async () => {
        const params = resolve(spec.params || {}, this.scope(scope.row ? { row: scope.row } : {}));
        const rows = await this.bridge.query(spec.query, params).catch(() => []);
        if (this.disposed) return;
        if (rows.length === 0 && emptyText(spec, scope)) {
          node.classList.add("pp-chart-empty");
          return node.replaceChildren(el("p", "pp-empty", resolve(emptyText(spec, scope), scope)));
        }
        node.classList.remove("pp-chart-empty");
        const max = Number(spec.max) || Math.max(1, ...rows.map((r) => Number(r[spec.value]) || 0));
        const shown = (r, value) => (spec.value_label && r[spec.value_label] != null ? r[spec.value_label] : value);
        node.replaceChildren(
          ...rows.map((r) => {
            const value = Number(r[spec.value]) || 0;
            const fill = el("span", "pp-bar-fill");
            if (columns) {
              const col = el("div", `pp-col${value === 0 ? " pp-col-zero" : ""}`);
              col.setAttribute("role", "listitem");
              const name = (spec.title && r[spec.title]) || `${r[spec.label]}: ${shown(r, value)}`;
              col.title = name;
              col.setAttribute("aria-label", name);
              fill.style.height = `${(value / max) * 100}%`;
              const track = el("span", "pp-col-track");
              track.append(fill);
              col.append(el("span", "pp-col-value", value ? shown(r, value) : ""), track, el("span", "pp-col-label", r[spec.label] ?? ""));
              return col;
            }
            const row = el("div", "pp-bar-row");
            fill.style.width = `${(value / max) * 100}%`;
            row.append(el("span", "pp-bar-label", r[spec.label]), el("span", "pp-bar-track"), el("span", "pp-bar-value", shown(r, value)));
            row.children[1].append(fill);
            return row;
          }),
        );
      };
      this.lists.push(load);
      return node;
    },

    /**
     * Records from the microphone, then runs `action` with `value` =
     * {audio (base64), mime, duration_ms}. A round mic button with the
     * elapsed time and a live level meter; Cancel drops the take. Stops by
     * itself at max_seconds. The microphone is released as soon as
     * recording stops.
     */
    "audio-recorder"(spec, scope) {
      const node = el("div", "pp-recorder");
      node.dataset.state = "idle";
      const idleTitle = resolve(spec.label || "Record", scope);
      const idleHint = resolve(spec.hint || "", scope);
      const button = el("button", "pp-rec-button");
      button.type = "button";
      button.setAttribute("aria-label", idleTitle);
      button.append(icon("mic"));
      const info = el("div", "pp-rec-info");
      const title = el("strong", "pp-rec-title", idleTitle);
      const status = el("span", "pp-muted", idleHint);
      status.setAttribute("aria-live", "polite");
      info.append(title, status);
      const level = el("div", "pp-rec-level");
      level.setAttribute("aria-hidden", "true");
      const bars = [0, 1, 2, 3, 4].map(() => el("span"));
      level.append(...bars);
      const cancel = el("button", "pp-button pp-ghost", "Cancel");
      cancel.type = "button";
      node.append(button, info, level, cancel);
      const max = Math.min(Number(spec.max_seconds) || 180, 180);
      let recorder = null;
      let started = 0;
      let timer = null;
      let discard = false;
      let audio = null;
      let stream = null;
      let dead = false;
      this.cleanups.push(() => {
        dead = true; discard = true;
        this.cancel(timer);
        if (recorder?.state === "recording") recorder.stop();
        stream?.getTracks().forEach(track => track.stop());
        audio?.close().catch(() => {});
      });
      const clock = (ms) => {
        const s = Math.floor(ms / 1000);
        return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
      };
      const idle = (message) => {
        node.dataset.state = "idle";
        button.setAttribute("aria-label", idleTitle);
        button.replaceChildren(icon("mic"));
        title.textContent = idleTitle;
        status.textContent = message ?? idleHint;
      };
      const stop = () => recorder && recorder.state === "recording" && recorder.stop();
      cancel.addEventListener("click", () => {
        discard = true;
        stop();
      });
      button.addEventListener("click", async () => {
        if (recorder && recorder.state === "recording") return stop();
        if (node.dataset.state !== "idle") return;
        if (!navigator.mediaDevices?.getUserMedia || typeof MediaRecorder === "undefined") {
          status.textContent = "Recording is not supported here. Type instead.";
          return;
        }
        try {
          stream = await navigator.mediaDevices.getUserMedia({ audio: true });
          if (dead || this.disposed) { stream.getTracks().forEach(track => track.stop()); return; }
        } catch (e) {
          status.textContent = "Microphone access was not allowed.";
          return;
        }
        const type = ["audio/webm;codecs=opus", "audio/ogg;codecs=opus", "audio/mp4"].find((t) => MediaRecorder.isTypeSupported(t));
        recorder = new MediaRecorder(stream, type ? { mimeType: type } : undefined);
        discard = false;
        const chunks = [];
        // Level meter: optional, recording works without it.
        let analyser = null;
        try {
          audio = new (window.AudioContext || window.webkitAudioContext)();
          analyser = audio.createAnalyser();
          analyser.fftSize = 256;
          audio.createMediaStreamSource(stream).connect(analyser);
        } catch (e) {
          analyser = null;
        }
        const samples = analyser ? new Uint8Array(analyser.fftSize) : null;
        recorder.addEventListener("dataavailable", (e) => e.data.size && chunks.push(e.data));
        recorder.addEventListener("stop", async () => {
          this.cancel(timer);
          stream.getTracks().forEach((t) => t.stop());
          audio?.close().catch(() => {});
          audio = null;
          bars.forEach((b) => (b.style.transform = ""));
          if (dead || this.disposed) return;
          if (discard) return idle("Recording dropped.");
          const duration_ms = Date.now() - started;
          node.dataset.state = "saving";
          title.textContent = "Saving…";
          status.textContent = clock(duration_ms);
          const blob = new Blob(chunks, { type: recorder.mimeType || "audio/webm" });
          const bytes = new Uint8Array(await blob.arrayBuffer());
          let binary = "";
          for (let i = 0; i < bytes.length; i += 0x8000) binary += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
          const ok = await this.act(spec.action, { ...scope, value: { audio: btoa(binary), mime: blob.type, duration_ms } });
          idle(ok ? idleHint : "Could not save the recording.");
        });
        started = Date.now();
        recorder.start();
        node.dataset.state = "recording";
        button.setAttribute("aria-label", "Stop and save");
        button.replaceChildren(icon("stop"));
        title.textContent = "Recording 0:00";
        status.textContent = `Tap to stop and save · up to ${clock(max * 1000)}`;
        timer = this.schedule(() => {
          const ms = Date.now() - started;
          title.textContent = `Recording ${clock(ms)}`;
          if (samples) {
            analyser.getByteTimeDomainData(samples);
            let peak = 0;
            for (const v of samples) peak = Math.max(peak, Math.abs(v - 128));
            const loud = Math.min(1, peak / 64);
            bars.forEach((b, i) => (b.style.transform = `scaleY(${0.2 + loud * (0.5 + 0.5 * Math.abs(Math.sin(ms / 140 + i)))})`));
          }
          if (ms >= max * 1000) stop();
        }, 100, true);
      });
      return node;
    },

    list(spec, scope) {
      const board = ["board", "matrix"].includes(spec.layout);
      const node = el("ul", `pp-list${board ? ` pp-board pp-board-${spec.layout}` : ""}${spec.layout === "chips" ? " pp-list-chips" : spec.layout === "plain" ? " pp-list-plain" : ""}`);
      node.setAttribute("aria-busy", "true");
      if (board) node.setAttribute("aria-label", resolve(spec.label || "Board", scope));
      node.append(el("li", "pp-empty", "Loading…"));
      let region = { lists: [], watchers: [] };
      let rendered = null;
      let sequence = 0;
      this.cleanups.push(() => { sequence++; this.release(region); });
      // The list owns its current rows. Replacing them must release their
      // watchers instead of adding detached controls to the view forever.
      this.watchers.push(() => { for (const watch of region.watchers) watch(); });
      const load = async () => {
        if (this.disposed) return;
        const seq = ++sequence;
        // Params see the enclosing row too: a list inside a list item or sheet.
        const params = resolve(spec.params || {}, this.scope(scope.row ? { row: scope.row } : {}));
        let rows;
        try {
          rows = await this.bridge.query(spec.query, params);
        } catch (e) {
          if (seq !== sequence) return;
          rendered = null;
          this.release(region);
          region = { lists: [], watchers: [] };
          const retry = el("button", "pp-button pp-ghost", "Retry loading");
          retry.type = "button";
          retry.addEventListener("click", () => { node.setAttribute("aria-busy", "true"); retry.disabled = true; load(); });
          const error = el("li", "pp-list-error");
          error.append(el("p", "pp-error", `Couldn’t load items: ${String(e)}`), retry);
          node.replaceChildren(error);
          node.setAttribute("aria-busy", "false");
          return;
        }
        if (seq !== sequence) return;
        if (spec.limit) rows = rows.slice(0, Number(spec.limit));
        // Always query fresh data, but keep unchanged controls (and drafts)
        // in place. Include state because row templates may reference it.
        const signature = JSON.stringify([rows, this.state]);
        if (signature === rendered) {
          await Promise.all(region.lists.map((load) => load()));
          if (seq === sequence) node.setAttribute("aria-busy", "false");
          return;
        }
        rendered = null;
        node.setAttribute("aria-busy", "true");
        this.release(region);
        node.replaceChildren();
        region = { lists: [], watchers: [] };
        if (rows.length === 0 && emptyText(spec, scope)) node.append(el("li", "pp-empty", resolve(emptyText(spec, scope), scope)));
        // All lanes share one result set, so filters and row limits are
        // identical to the list. Empty lanes remain visible for orientation.
        const lanes = new Map();
        if (board) {
          for (const column of spec.columns || []) {
            const value = String(column.value);
            const matching = rows.filter(row => String(row[spec.column]) === value);
            const lane = el("li", "pp-board-lane");
            const title = resolve(column.label, scope);
            const heading = el("h3", "pp-board-heading");
            const headingId = nextId("pp-lane");
            heading.id = headingId;
            heading.append(el("span", "", title), el("span", "pp-board-count", matching.length));
            lane.append(heading);
            if (column.description) lane.append(el("p", "pp-board-description", resolve(column.description, scope)));
            const items = el("ul", "pp-list pp-board-items");
            items.setAttribute("aria-labelledby", headingId);
            if (!matching.length) items.append(el("li", "pp-empty", "No tasks"));
            lane.append(items);
            node.append(lane);
            lanes.set(value, items);
          }
        }
        let group;
        // Bound each synchronous task instead of monopolizing the main thread
        // for all 300–500 rows. Never leave an interrupted partial render cached.
        for (let index = 0; index < rows.length;) {
          if (seq !== sequence) return;
          const chunk = profile.measureSync("render/list-chunk", () => this.region(() => {
            const fragment = document.createDocumentFragment();
            const started = Date.now();
            let count = 0;
            do {
              const row = rows[index++];
              if (!board && spec.group && row[spec.group] !== group) {
                group = row[spec.group];
                const tone = spec.group_tone ? row[spec.group_tone] : null;
                if (group != null) fragment.append(el("li", `pp-group${tone ? ` pp-tone-${tone}` : ""}`, group));
              }
              const item = el("li", "pp-list-item");
              const child = this.node(spec.item, { ...scope, row });
              if (child) item.append(child);
              if (board) lanes.get(String(row[spec.column]))?.append(item);
              else fragment.append(item);
              count++;
            } while (index < rows.length && count < 32 && Date.now() - started < 8);
            node.append(fragment);
          }));
          region.lists.push(...chunk.lists);
          region.watchers.push(...chunk.watchers);
          (region.cleanups ||= []).push(...chunk.cleanups);
          if (index < rows.length) await new Promise(resolve => this.schedule(resolve, 0, false, resolve));
        }
        if (seq !== sequence) return;
        rendered = signature;
        await Promise.all(region.lists.map((load) => load()));
        if (seq === sequence) node.setAttribute("aria-busy", "false");
      };
      this.lists.push(load);
      return node;
    },
  };

  // Small stroke icons the renderer draws itself (24px grid paths).
  const ICONS = {
    calendar: "M4 6h16v14H4zM4 10h16M8 3v4M16 3v4",
    repeat: "M17 2l3 3-3 3M4 11V9a4 4 0 0 1 4-4h12M7 22l-3-3 3-3M20 13v2a4 4 0 0 1-4 4H4",
    steps: "M9 6h11M9 12h11M9 18h11M4 6l1 1 2-2M4 12l1 1 2-2M4 18l1 1 2-2",
    bell: "M6 16V11a6 6 0 0 1 12 0v5l2 2H4zM10 21h4",
    tag: "M3 12V4h8l10 10-8 8zM7.5 7.5h.01",
    close: "M6 6l12 12M18 6 6 18",
    trash: "M4 7h16M10 11v6M14 11v6M6 7l1 13h10l1-13M9 7V4h6v3",
    plus: "M12 5v14M5 12h14",
    check: "m5 12 5 5 9-10",
    note: "M6 3h9l4 4v14H6zM9 13h7M9 17h5",
    flame: "M12 3c1 4 5 6 5 11a5 5 0 0 1-10 0c0-2 1-3 2-4 0 2 1 3 2 3 0-3-1-6 1-10z",
    moon: "M20 14.5A8 8 0 0 1 9.5 4a8 8 0 1 0 10.5 10.5z",
    pause: "M8 5v14M16 5v14",
    play: "M7 4l13 8-13 8z",
    minus: "M5 12h14",
    chart: "M4 20V4M4 20h16M8 16v-5M12 16V7M16 16v-8",
    mic: "M12 3a3 3 0 0 0-3 3v6a3 3 0 0 0 6 0V6a3 3 0 0 0-3-3zM5 11a7 7 0 0 0 14 0M12 18v3",
    stop: "M7 7h10v10H7z",
    send: "M21 3 10 14M21 3l-7 18-4-7-7-4z",
    undo: "M9 14 4 9l5-5M4 9h11a5 5 0 0 1 0 10h-3",
    archive: "M3 4h18v4H3zM5 8v12h14V8M10 12h4",
    clock: "M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18zM12 7v5l3 2",
    pin: "M9 3h6l-1 6 4 4v2H6v-2l4-4zM12 15v6",
    edit: "M4 20h4L19 9l-4-4L4 16zM14 6l4 4",
    external: "M14 4h6v6M20 4l-9 9M18 14v6H4V6h6",
    folder: "M3 6h6l2 2h10v11H3z",
    eye: "M2 12s4-7 10-7 10 7 10 7-4 7-10 7S2 12 2 12zM12 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6z",
    link: "M10 14a4 4 0 0 0 6 0l3-3a4 4 0 0 0-6-6l-1 1M14 10a4 4 0 0 0-6 0l-3 3a4 4 0 0 0 6 6l1-1",
    sparkle: "M12 3l1.9 5.1L19 10l-5.1 1.9L12 17l-1.9-5.1L5 10l5.1-1.9zM19 15l.8 2.2L22 18l-2.2.8L19 21l-.8-2.2L16 18l2.2-.8z",
  };

  function icon(name) {
    const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    svg.setAttribute("viewBox", "0 0 24 24");
    svg.setAttribute("aria-hidden", "true");
    svg.setAttribute("class", "pp-icon");
    const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
    path.setAttribute("d", ICONS[name]);
    svg.append(path);
    return svg;
  }

  function field(spec, scope, tag, type) {
    const id = nextId("pp-field");
    const node = el("div", "pp-field");
    const label = el("label", `pp-label${spec.label_hidden ? " pp-sr" : ""}`, resolve(spec.label || spec.name, scope));
    label.htmlFor = id;
    if (spec.required) label.append(el("span", "pp-required", " (required)"));
    const input = el(tag, "pp-input");
    if (type) input.type = type;
    input.id = id;
    input.dataset.ppName = spec.name;
    input.dataset.ppKind = spec.type;
    if (spec.placeholder) input.placeholder = resolve(spec.placeholder, scope);
    if (spec.required) input.required = true;
    if ("value" in spec) {
      const value = resolve(spec.value, scope);
      if (value != null) input.value = String(value);
    }
    if (tag === "textarea" && spec.rows) input.rows = Number(spec.rows);
    node.append(label, input);
    return node;
  }

  // A deliberately small Markdown subset rendered as DOM: headings,
  // bulleted, numbered and check lists, quotes, code blocks, rules, links,
  // bold, italics and inline code. No HTML passes. `hooks.toggle(line)`
  // makes checkboxes tickable; `hooks.open(url)` opens links.
  function markdown(text, hooks = {}) {
    const root = el("div", "pp-markdown");
    let list = null;
    let listKind = null;
    let quote = null;
    let code = null;
    const inline = (line) => {
      const out = document.createDocumentFragment();
      const re = /(\*\*[^*]+\*\*|\*[^*]+\*|`[^`]+`|\[[^\]]+\]\(https?:\/\/[^)\s]+\)|https?:\/\/[^\s)]+)/g;
      let last = 0;
      for (const m of line.matchAll(re)) {
        if (m.index > last) out.append(line.slice(last, m.index));
        const token = m[0];
        if (token.startsWith("**")) out.append(el("strong", "", token.slice(2, -2)));
        else if (token.startsWith("`")) out.append(el("code", "", token.slice(1, -1)));
        else if (token.startsWith("[") || token.startsWith("http")) {
          const link = token.match(/^\[([^\]]+)\]\((.*)\)$/);
          const href = link ? link[2] : token;
          const a = el("a", "pp-link", link ? link[1] : token);
          a.href = href;
          a.addEventListener("click", (e) => {
            e.preventDefault();
            hooks.open?.(href);
          });
          out.append(a);
        } else out.append(el("em", "", token.slice(1, -1)));
        last = m.index + token.length;
      }
      if (last < line.length) out.append(line.slice(last));
      return out;
    };
    const lines = String(text || "").split("\n");
    lines.forEach((raw, index) => {
      const line = raw.trimEnd();
      if (line.trim().startsWith("```")) {
        if (code) code = null;
        else {
          const pre = el("pre", "pp-md-code");
          code = el("code");
          pre.append(code);
          root.append(pre);
        }
        list = quote = null;
        return;
      }
      if (code) {
        code.append(`${code.textContent ? "\n" : ""}${raw}`);
        return;
      }
      const heading = line.match(/^(#{1,3})\s+(.*)$/);
      const bullet = line.match(/^\s*[-*]\s+(\[( |x|X)\]\s+)?(.*)$/);
      const numbered = line.match(/^\s*(\d+)\.\s+(.*)$/);
      if (bullet || numbered) {
        const kind = numbered ? "ol" : "ul";
        if (!list || listKind !== kind) {
          list = el(kind);
          listKind = kind;
          if (numbered) list.start = Number(numbered[1]);
          root.append(list);
        }
        quote = null;
        const li = el("li");
        if (bullet?.[1]) {
          li.className = "pp-md-task";
          const box = el("input");
          box.type = "checkbox";
          box.checked = bullet[2] !== " ";
          box.disabled = !hooks.toggle;
          box.setAttribute("aria-label", bullet[3]);
          if (hooks.toggle) box.addEventListener("change", () => hooks.toggle(index));
          const span = el("span", box.checked ? "pp-done" : "");
          span.append(inline(bullet[3]));
          li.append(box, span);
        } else li.append(inline(bullet ? bullet[3] : numbered[2]));
        list.append(li);
        return;
      }
      list = null;
      if (line.startsWith(">")) {
        if (!quote) root.append((quote = el("blockquote")));
        const p = el("p");
        p.append(inline(line.replace(/^>\s?/, "")));
        quote.append(p);
        return;
      }
      quote = null;
      if (/^(-{3,}|\*{3,})$/.test(line.trim())) root.append(el("hr", "pp-divider"));
      else if (heading) root.append(el(`h${heading[1].length + 2}`, "", heading[2]));
      else if (line) {
        const p = el("p");
        p.append(inline(line));
        root.append(p);
      }
    });
    return root;
  }

  window.TerrariumRenderer = {
    /** Mounts `template` into `root`; returns the view (call .refresh() after data changes). */
    mount(root, template, bridge, options) {
      const view = new View(root, template, bridge, options);
      view.ready = view.mount();
      return view;
    },
    resolve,
    markdown,
  };
})();
