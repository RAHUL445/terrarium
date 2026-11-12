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
  const MAX_BLOB_BYTES = 25 * 1024 * 1024;
  const IMAGE_MIME = new Set(["image/png", "image/jpeg", "image/gif", "image/webp"]);

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

  // Adds `$ref` ("plugin/table/id") to a row when the node names its table.
  function withRef(row, table, plugin) {
    if (!row || !table || !plugin || row.id == null || !/^[a-z][a-z0-9_]*$/.test(table)) return row;
    return { ...row, $ref: `${plugin}/${table}/${row.id}` };
  }

  function truthy(v) {
    return !(v == null || v === false || v === 0 || v === "" || (Array.isArray(v) && v.length === 0));
  }

  // Orders two cell values of a column type. Empty values (null, "") sort
  // last in both directions.
  function compareValues(a, b, type) {
    const kind = String(type || "string");
    const numberLike = kind === "integer" || kind === "number";
    const av = numberLike ? Number(a) : a;
    const bv = numberLike ? Number(b) : b;
    const aEmpty = a == null || a === "" || (numberLike && Number.isNaN(av));
    const bEmpty = b == null || b === "" || (numberLike && Number.isNaN(bv));
    if (aEmpty || bEmpty) return aEmpty === bEmpty ? 0 : (aEmpty ? 1 : -1);
    if (numberLike) return av - bv;
    if (kind === "boolean") return Number(truthy(av)) - Number(truthy(bv));
    return String(av).localeCompare(String(bv), undefined, { numeric: true, sensitivity: "base" });
  }

  function sortRows(rows, key, type, dir) {
    const order = dir === "desc" ? "desc" : "asc";
    const isEmpty = (value) => {
      if (value == null || value === "") return true;
      if (type === "integer" || type === "number") return Number.isNaN(Number(value));
      return false;
    };
    return rows.map((row, index) => ({ row, index })).sort((left, right) => {
      const leftEmpty = isEmpty(left.row?.[key]);
      const rightEmpty = isEmpty(right.row?.[key]);
      if (leftEmpty || rightEmpty) return leftEmpty === rightEmpty ? left.index - right.index : (leftEmpty ? 1 : -1);
      const cmp = compareValues(left.row?.[key], right.row?.[key], type);
      if (cmp === 0) return left.index - right.index;
      if (order === "asc") return cmp;
      return -cmp;
    }).map((entry) => entry.row);
  }

  function pad2(value) {
    return String(value).padStart(2, "0");
  }

  function formatDay(date) {
    return `${date.getFullYear()}-${pad2(date.getMonth() + 1)}-${pad2(date.getDate())}`;
  }

  function parseMonth(value) {
    const match = String(value ?? "").match(/^(\d{4})-(\d{2})$/);
    if (!match) return null;
    const year = Number(match[1]);
    const month = Number(match[2]);
    if (!Number.isInteger(year) || !Number.isInteger(month) || month < 1 || month > 12) return null;
    return { year, month, key: `${match[1]}-${match[2]}` };
  }

  function monthGrid(year, month) {
    const startOfMonth = new Date(`${year}-${pad2(month)}-01T12:00:00`);
    const gridStart = new Date(startOfMonth);
    gridStart.setDate(startOfMonth.getDate() - ((startOfMonth.getDay() + 6) % 7));
    const days = [];
    for (let i = 0; i < 42; i++) {
      const date = new Date(gridStart);
      date.setDate(gridStart.getDate() + i);
      days.push({ day: formatDay(date), inMonth: date.getFullYear() === year && date.getMonth() + 1 === month });
    }
    return days;
  }

  function dayKey(value) {
    const text = String(value ?? "");
    return /^\d{4}-\d{2}-\d{2}/.test(text) ? text.slice(0, 10) : null;
  }

  function formatBytes(bytes) {
    const n = Number(bytes) || 0;
    if (n >= 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MB`;
    if (n >= 1024) return `${Math.max(1, Math.round(n / 1024))} KB`;
    return `${Math.max(0, Math.round(n))} B`;
  }

  function blobLinkRef(url) {
    const text = String(url || "");
    const m = text.match(/^terrarium:blob\/(b1-[0-9a-f]{8}-[0-9a-f]{64})(?:\?.*)?$/);
    if (!m) return null;
    return { blob: m[1] };
  }

  function parseFileRef(value) {
    if (!value) return null;
    let raw = value;
    if (typeof raw === "string") {
      try { raw = JSON.parse(raw); } catch (_) { return null; }
    }
    if (!raw || typeof raw !== "object") return null;
    if (typeof raw.blob !== "string" || typeof raw.name !== "string") return null;
    if (typeof raw.size !== "number") return null;
    return {
      blob: raw.blob,
      name: raw.name,
      mime: String(raw.mime || "application/octet-stream"),
      size: Number(raw.size) || 0,
    };
  }

  function escapeMd(text) {
    return String(text || "").replace(/[\[\]()!]/g, "\\$&");
  }

  function fileToBase64(file) {
    return new Promise((resolve, reject) => {
      const reader = new FileReader();
      reader.onload = () => resolve(String(reader.result || "").split(",", 2)[1] || "");
      reader.onerror = () => reject(reader.error || new Error("Couldn’t read file"));
      reader.readAsDataURL(file);
    });
  }

  function sniffImageMime(dataBase64) {
    const text = String(dataBase64 || "");
    if (text.startsWith("iVBORw0KGgo")) return "image/png";
    if (text.startsWith("/9j/")) return "image/jpeg";
    if (text.startsWith("R0lGOD")) return "image/gif";
    try {
      const head = atob(text.slice(0, 32));
      if (head.startsWith("RIFF") && head.slice(8, 12) === "WEBP") return "image/webp";
    } catch (_) {}
    return null;
  }

  function renderBlobImage(file, { alt = "", maxHeight = null, blobGet = null } = {}) {
    const wrap = el("div", "pp-blob-image");
    const status = el("p", "pp-muted", "Loading image…");
    wrap.append(status);
    if (!file) {
      status.textContent = "Image preview not available.";
      return wrap;
    }
    if (file.mime && !IMAGE_MIME.has(file.mime)) {
      status.textContent = "Image preview not available.";
      return wrap;
    }
    if (!blobGet) {
      status.textContent = "Not on this device yet";
      return wrap;
    }
    blobGet(file.blob).then((result) => {
      if (!result || result.missing) {
        status.textContent = "Not on this device yet";
        return;
      }
      if (!result.data_base64) {
        status.textContent = "Couldn’t load image.";
        return;
      }
      const mime = IMAGE_MIME.has(file.mime) ? file.mime : sniffImageMime(result.data_base64);
      if (!mime) {
        status.textContent = "Image preview not available.";
        return;
      }
      const img = el("img", "pp-blob-image-img");
      img.alt = String(alt || file.name || "");
      if (maxHeight != null) img.style.maxHeight = `${Math.max(32, Number(maxHeight) || 0)}px`;
      img.src = `data:${mime};base64,${result.data_base64}`;
      wrap.replaceChildren(img);
    }).catch(() => {
      status.textContent = "Couldn’t load image.";
    });
    return wrap;
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

  // Host-owned labels stay on DOM nodes, never in plugin template scope.
  class RefChips {
    constructor(view) {
      this.view = view;
      this.cache = new Map();
      this.waiting = new Map();
      this.inflight = new Map();
      this.nodes = new Set();
      this.timer = null;
      this.generation = 0;
    }

    chip(ref, label) {
      const node = el("button", "pp-ref");
      node.type = "button";
      const entry = { node, ref, label: String(label || ref) };
      node.addEventListener("click", () => {
        if (!this.view.disposed && entry.hit?.state === "ok") this.view.bridge.openRef?.(entry.hit);
      });
      this.nodes.add(entry);
      this.draw(entry);
      this.enqueue(entry);
      return node;
    }

    enqueue(entry) {
      if (this.cache.has(entry.ref)) return this.draw(entry, this.cache.get(entry.ref));
      const pending = this.inflight.get(entry.ref);
      if (pending) { pending.push(entry); return; }
      if (!this.waiting.has(entry.ref)) this.waiting.set(entry.ref, []);
      this.waiting.get(entry.ref).push(entry);
      if (this.timer) return;
      this.timer = true;
      queueMicrotask(() => { this.timer = null; this.flush(); });
    }

    flush() {
      if (this.view.disposed) return;
      const waiting = this.waiting;
      this.waiting = new Map();
      const generation = this.generation;
      const refs = [...waiting.keys()];
      for (let i = 0; i < refs.length; i += 200) {
        const batch = refs.slice(i, i + 200);
        for (const ref of batch) this.inflight.set(ref, waiting.get(ref));
        // Independent chunks may finish in any order.
        (async () => {
          let results = [];
          try { results = await this.view.bridge.command?.("query", "ref.resolve", { refs: batch }) || []; } catch (_) {}
          if (this.view.disposed || generation !== this.generation) return;
          const hits = new Map(results.map(hit => [hit.ref, hit]));
          for (const ref of batch) {
            const hit = hits.get(ref) || { ref, state: "unknown" };
            this.cache.set(ref, hit);
            for (const entry of this.inflight.get(ref) || []) this.draw(entry, hit);
            this.inflight.delete(ref);
          }
        })();
      }
    }

    draw(entry, hit) {
      const { node, label } = entry;
      entry.hit = hit;
      const ok = hit?.state === "ok";
      const gone = hit?.state === "trash" || hit?.state === "missing";
      node.className = `pp-ref${gone ? " pp-ref-gone" : ""}`;
      node.disabled = !ok;
      node.title = ok ? `${hit.kind}: ${hit.title}` : label;
      node.setAttribute("aria-label", ok ? `Open ${hit.kind || "item"}: ${hit.title}` : label);
      const title = el("span", `pp-ref-title${ok && hit.done ? " pp-done" : ""}`,
        ok ? hit.title : hit?.state === "trash" ? "In trash" : hit?.state === "missing" ? "Missing" : label);
      node.replaceChildren(icon(ok && Object.hasOwn(ICONS, hit.icon) ? hit.icon : "link"), title);
    }

    clear() {
      this.cache.clear();
      this.generation++;
      this.inflight.clear();
    }

    refresh() {
      for (const entry of this.nodes) {
        if (this.view.root.contains(entry.node)) this.enqueue(entry);
        else this.nodes.delete(entry);
      }
    }
  }

  // Shared host-owned search results. Only onChoose may cross into action scope.
  function mentionList(view, { onChoose, isCurrent = () => true }) {
    const node = el("div", "pp-mention");
    node.hidden = true;
    node.id = nextId("pp-mention");
    node.setAttribute("role", "listbox");
    node.setAttribute("aria-label", "Link to an item");
    let hits = [], index = 0, sequence = 0, timer = null, blurTimer = null;
    let control = null, dead = false, choosing = false;
    const sync = () => {
      if (!control) return;
      control.setAttribute("aria-expanded", String(!node.hidden));
      const selected = !node.hidden && hits.length ? `${node.id}-${index}` : null;
      if (selected) control.setAttribute("aria-activedescendant", selected);
      else control.removeAttribute("aria-activedescendant");
    };
    const close = () => {
      sequence++;
      view.cancel(timer); view.cancel(blurTimer);
      timer = blurTimer = null;
      hits = []; index = 0;
      node.hidden = true; node.replaceChildren(); sync();
    };
    const chooseHit = async hit => {
      if (dead || view.disposed || choosing || node.hidden || !hit) return;
      if (!isCurrent()) return close();
      const seq = sequence;
      choosing = true;
      view.cancel(blurTimer); blurTimer = null;
      for (const item of node.children) if (item.type === "button") item.disabled = true;
      control?.focus();
      try {
        const ok = await onChoose(hit);
        if (!dead && !view.disposed && seq === sequence && ok !== false) close();
      } finally {
        choosing = false;
        if (!dead && !view.disposed) for (const item of node.children) if (item.type === "button") item.disabled = false;
      }
    };
    const draw = () => {
      node.replaceChildren();
      hits.forEach((hit, i) => {
        const item = el("button", "pp-mention-item", hit.title || "Untitled");
        item.type = "button"; item.id = `${node.id}-${i}`;
        item.setAttribute("role", "option");
        item.setAttribute("aria-selected", String(i === index));
        item.append(el("span", "pp-muted", hit.kind || ""));
        let mouseChosen = false;
        item.addEventListener("mousedown", e => { e.preventDefault(); mouseChosen = true; return chooseHit(hit); });
        item.addEventListener("click", () => {
          if (mouseChosen) { mouseChosen = false; return; }
          return chooseHit(hit);
        });
        node.append(item);
      });
      if (!hits.length) node.append(el("p", "pp-muted", "No matching items"));
      node.hidden = false; sync();
    };
    const search = q => {
      close();
      if (!q || dead || view.disposed) return;
      const seq = sequence;
      timer = view.schedule(async () => {
        timer = null;
        let result;
        try { result = await view.bridge.command?.("query", "search.query", { q, limit: 8 }); } catch (_) {}
        if (dead || view.disposed || seq !== sequence) return;
        if (!isCurrent()) return close();
        hits = (result?.results || []).filter(hit => typeof hit.ref === "string" && /^[a-z][a-z0-9-]*\/[a-z][a-z0-9_]*\/[A-Za-z0-9_-]{1,64}$/.test(hit.ref));
        index = 0; draw();
      }, 150);
    };
    const move = delta => {
      if (node.hidden || !hits.length || choosing) return;
      index = (index + delta + hits.length) % hits.length;
      draw(); node.children[index]?.scrollIntoView?.({ block: "nearest" });
    };
    const choose = () => chooseHit(hits[index]);
    // Both callers retain focus in their own input, including on touch keyboards.
    const bind = input => {
      control = input;
      input.setAttribute("aria-controls", node.id); sync();
      input.addEventListener("blur", () => {
        if (choosing) return;
        view.cancel(blurTimer); blurTimer = view.schedule(close, 150);
      });
      input.addEventListener("keydown", e => {
        if (node.hidden || e.isComposing || !["ArrowDown", "ArrowUp", "Enter", "Tab", "Escape"].includes(e.key)) return;
        e.preventDefault(); e.stopImmediatePropagation();
        if (e.key === "Escape") return close();
        if (e.key === "Enter" || e.key === "Tab") return choose();
        move(e.key === "ArrowDown" ? 1 : -1);
      });
    };
    view.cleanups.push(() => { dead = true; close(); });
    return { node, search, move, choose, close, isOpen: () => !node.hidden, bind };
  }

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
      this.refs = new RefChips(this);
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
      this.refs.clear();
      this.refs.waiting.clear();
      this.refs.nodes.clear();
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
      this.refs.clear();
      return Promise.all(this.lists.map((list) => list())).then(() => {
        if (!this.disposed) this.refs.refresh();
      });
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
      // Opens the host's History dialog for an item in this plugin.
      if (action.history) {
        const spec = resolve(action.history, scope);
        await this.bridge.history?.({ table: String(spec.table || ""), id: String(spec.id || ""), title: String(spec.title || "") });
        return true;
      }
      // Local state changes, e.g. {"set": {"state.selected": "{{row.id}}"}}.
      if (action.set) {
        for (const [path, value] of Object.entries(action.set)) this.setState(path, resolve(value, scope));
        if (!action.op && !action.open && !action.link) return true;
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
      // Relates two items through the host, before any plugin operation.
      if (action.link) {
        const { from, to, kind } = resolve(action.link, scope);
        try {
          await this.bridge.command("action", "link.add", { source: String(from || ""), target: String(to || ""), kind: kind || "related" });
        } catch (e) {
          if (!this.disposed) this.bridge.toast(String(e), "error");
          return false;
        }
        if (this.disposed) { this.bridge.changed(); return true; }
        this.bridge.toast(resolve(action.success || "Linked", scope), "ok");
        this.bridge.changed();
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

    async undoSelected(undo, ids, scope) {
      const s = { ...scope, count: ids.length };
      try {
        const report = await this.bridge.bulk(undo.op, ids, resolve(undo.args || {}, s));
        if (!this.disposed) {
          const message = report.failed
            ? report.message
            : (resolve(undo.success, { ...s, result: report }) || report.message);
          this.bridge.toast(message, report.failed ? "error" : "ok");
        }
        this.bridge.changed();
        return report.failed === 0;
      } catch (e) {
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
    /** An item from any plugin; incomplete refs draw nothing. */
    ref(spec, scope) {
      const ref = String(resolve(spec.ref, scope) ?? "");
      if (!/^[a-z][a-z0-9-]*\/[a-z][a-z0-9_]*\/[A-Za-z0-9_-]{1,64}$/.test(ref)) return null;
      return this.refs.chip(ref, resolve(spec.label, scope));
    },
    /** Search items and run an action with only the selected ref in value. */
    "ref-picker"(spec, scope) {
      const node = el("div", "pp-field pp-ref-picker");
      const input = el("input", "pp-input");
      input.id = nextId("pp-ref-picker"); input.type = "search";
      input.placeholder = resolve(spec.placeholder || "Search items", scope);
      input.setAttribute("role", "combobox");
      input.setAttribute("aria-autocomplete", "list");
      const label = el("label", "pp-label", resolve(spec.label || "Link to…", scope));
      label.htmlFor = input.id;
      let dead = false;
      this.cleanups.push(() => { dead = true; });
      const list = mentionList(this, { onChoose: async hit => {
        input.disabled = true;
        try {
          const ok = await this.act(spec.action, { ...scope, value: { ref: hit.ref } });
          if (dead || this.disposed) return false;
          if (ok) { input.value = ""; list.close(); }
          return ok;
        } finally {
          if (!dead && !this.disposed) { input.disabled = false; input.focus(); }
        }
      } });
      list.bind(input);
      input.addEventListener("input", () => {
        const query = input.value.trim();
        if (query.length >= 1) list.search(query);
        else list.close();
      });
      node.append(label, input, list.node);
      return node;
    },

    /** Mentions and relations for an item, drawn only from host-owned data. */
    backlinks(spec, scope) {
      const valid = ref => /^[a-z][a-z0-9-]*\/[a-z][a-z0-9_]*\/[A-Za-z0-9_-]{1,64}$/.test(ref);
      const currentRef = () => String(resolve(spec.ref, { ...scope, state: this.state }) ?? "");
      if (!valid(currentRef())) return null;
      const node = el("section", "pp-backlinks");
      const heading = el("h3", "pp-section-title", resolve(spec.title || "Linked from", scope));
      const body = el("div", "pp-backlinks-body");
      node.append(heading, body);
      let sequence = 0, dead = false;
      this.cleanups.push(() => { dead = true; sequence++; });
      const command = (kind, id, args) => Promise.resolve().then(() => this.bridge.command?.(kind, id, args));
      const load = async () => {
        if (this.disposed || dead) return;
        const seq = ++sequence;
        const current = () => !this.disposed && !dead && seq === sequence;
        const ref = currentRef();
        node.hidden = !valid(ref);
        if (node.hidden) { body.replaceChildren(); node.setAttribute("aria-busy", "false"); return; }
        node.setAttribute("aria-busy", "true");
        const [mentions, relations] = await Promise.all([
          command("query", "ref.backlinks", { ref }).catch(() => null),
          command("query", "link.list", { ref }).catch(() => null),
        ]);
        if (!current()) return;
        // Relations take precedence and get the display budget before mentions.
        const entries = new Map();
        for (const link of relations || []) {
          if (valid(link.other) && !entries.has(link.other)) entries.set(link.other, { ref: link.other, relation: link });
        }
        for (const item of mentions?.mentions || []) {
          if (valid(item) && !entries.has(item)) entries.set(item, { ref: item, relation: null });
        }
        const shown = [...entries.values()].slice(0, 500);
        const more = entries.size - shown.length;
        const hits = new Map();
        for (let i = 0; i < shown.length; i += 200) {
          const refs = shown.slice(i, i + 200).map(entry => entry.ref);
          const results = await command("query", "ref.resolve", { refs }).catch(() => []);
          if (!current()) return;
          for (const hit of results || []) hits.set(hit.ref, hit);
        }
        const groups = new Map();
        for (const entry of shown) {
          const hit = hits.get(entry.ref);
          if (!hit || hit.state === "unknown") continue;
          const kind = hit.state === "ok" ? hit.kind || "Other" : "Other";
          if (!groups.has(kind)) groups.set(kind, []);
          groups.get(kind).push(entry);
          // Reuse the grouping response for chips instead of resolving twice.
          this.refs.cache.set(entry.ref, hit);
        }
        body.replaceChildren();
        for (const kind of [...groups.keys()].sort()) {
          body.append(el("p", "pp-group", kind));
          const rows = el("div", "pp-backlinks-rows");
          for (const entry of groups.get(kind)) {
            const row = el("div", "pp-backlinks-row");
            row.append(this.refs.chip(entry.ref));
            if (entry.relation) {
              const link = entry.relation;
              const remove = el("button", "pp-button pp-ghost pp-icon-button");
              remove.type = "button";
              remove.setAttribute("aria-label", "Remove link");
              remove.append(icon("close"));
              remove.addEventListener("click", async () => {
                if (this.disposed || dead || remove.disabled) return;
                remove.disabled = true;
                try {
                  await command("action", "link.remove", { source: link.source, target: link.target, kind: link.kind });
                  if (this.disposed || dead) return;
                  this.bridge.changed();
                  await load();
                } catch (e) {
                  if (!this.disposed && !dead) this.bridge.toast(String(e), "error");
                } finally {
                  if (!this.disposed && !dead) remove.disabled = false;
                }
              });
              row.append(remove);
            }
            rows.append(row);
          }
          body.append(rows);
        }
        if (!groups.size) body.append(el("p", "pp-muted", resolve(spec.empty || "Nothing links here yet.", scope)));
        if (more) body.append(el("p", "pp-muted", `${more} more not shown`));
        node.setAttribute("aria-busy", "false");
      };
      this.lists.push(load);
      return node;
    },

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
      const fileStatus = el("span", "pp-muted", "");
      foot.append(count, fileStatus);
      const changed = () => area.dispatchEvent(new Event("input", { bubbles: true }));
      const words = () => {
        const text = area.value.replace(/^\s*(#+|[-*>]|\d+\.)\s+(\[[ xX]\]\s+)?/gm, "");
        const n = (text.match(/[^\s*_`#>-]\S*/g) || []).length;
        count.textContent = `${n} ${n === 1 ? "word" : "words"}`;
      };
      area.addEventListener("input", words);
      words();
      const setFileStatus = (text, error = false) => {
        fileStatus.textContent = text || "";
        fileStatus.classList.toggle("pp-tone-danger", !!error);
      };
      const insertAtCursor = (text) => {
        const start = area.selectionStart;
        const end = area.selectionEnd;
        replace(start, end, text);
      };
      const storeFiles = async (files) => {
        if (!files.length || !this.bridge.blobPut) return false;
        setFileStatus("Adding file…");
        try {
          const refs = [];
          for (const file of files) {
            if (file.size > MAX_BLOB_BYTES) throw new Error(`"${file.name}" exceeds 25 MB`);
            const data_base64 = await fileToBase64(file);
            const mime = file.type || "application/octet-stream";
            const stored = await this.bridge.blobPut({ name: file.name, mime, data_base64 });
            const ref = parseFileRef(stored);
            if (!ref) throw new Error(`Couldn’t add "${file.name}"`);
            const safeName = escapeMd(ref.name);
            refs.push(IMAGE_MIME.has(ref.mime)
              ? `![${safeName}](terrarium:blob/${ref.blob})`
              : `[${safeName}](terrarium:blob/${ref.blob})`);
          }
          insertAtCursor(refs.join("\n"));
          setFileStatus("");
          return true;
        } catch (e) {
          setFileStatus(String(e), true);
          return true;
        }
      };

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
      let mention = null;
      const findMention = () => {
        const caret = area.selectionStart;
        const match = area.value.slice(0, caret).match(/(^|\s)@([^\s@\[\]()]{1,40})$/);
        return match ? { start: caret - match[2].length - 1, query: match[2] } : null;
      };
      const list = mentionList(this, {
        isCurrent: () => {
          const current = findMention();
          return !!mention && !!current && current.start === mention.start && current.query === mention.query;
        },
        onChoose: hit => {
          const label = String(hit.title || "Untitled").replace(/[\[\]\n\r]/g, " ").trim();
          replace(mention.start, area.selectionStart, `[${label}](terrarium:${hit.ref}) `);
          closeMention();
        },
      });
      const picker = list.node;
      const closeMention = () => { mention = null; list.close(); };
      list.bind(area);
      area.addEventListener("input", () => {
        closeMention();
        mention = findMention();
        if (mention && !area.hidden) list.search(mention.query);
      });
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
        if (on) closeMention();
        preview.hidden = !on;
        area.hidden = on;
        toggle.textContent = on ? "Edit" : "Preview";
        toggle.setAttribute("aria-pressed", String(on));
        for (const b of bar.querySelectorAll(".pp-md-tool:not(.pp-md-mode)")) b.disabled = on;
        if (on) {
          preview.replaceChildren(
            markdown(area.value, {
              open: (url) => this.bridge.open(url),
              ref: (r, label) => this.refs.chip(r, label),
              blobGet: this.bridge.blobGet ? (id) => this.bridge.blobGet(id) : null,
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
      area.addEventListener("paste", (event) => {
        const files = [...(event.clipboardData?.files || [])];
        if (!files.length) return;
        event.preventDefault();
        storeFiles(files);
      });
      area.addEventListener("dragover", (event) => {
        if ((event.dataTransfer?.files || []).length) event.preventDefault();
      });
      area.addEventListener("drop", (event) => {
        const files = [...(event.dataTransfer?.files || [])];
        if (!files.length) return;
        event.preventDefault();
        storeFiles(files);
      });
      node.append(label, bar, area, picker, preview, foot);
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
          row = withRef(rows[0], spec.table, this.bridge.plugin);
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
          const row = withRef(rows[0], spec.table, this.bridge.plugin);
          if (row && JSON.stringify(row) !== JSON.stringify(closeScope?.row) && !typing) {
            const inner = this.scope({ row });
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
      return markdown(resolve(spec.text, scope), {
        open: (url) => this.bridge.open(url),
        ref: (r, label) => this.refs.chip(r, label),
        blobGet: this.bridge.blobGet ? (id) => this.bridge.blobGet(id) : null,
      });
    },

    image(spec, scope) {
      const file = parseFileRef(resolve(spec.file, scope));
      return renderBlobImage(file, {
        alt: resolve(spec.alt, scope) || file?.name || "",
        maxHeight: resolve(spec.max_height, scope),
        blobGet: this.bridge.blobGet ? (id) => this.bridge.blobGet(id) : null,
      });
    },

    file(spec, scope) {
      const file = parseFileRef(resolve(spec.file, scope));
      if (!file) return el("p", "pp-muted", "Not on this device yet");
      const card = el("div", "pp-file");
      const meta = el("div", "pp-file-meta");
      meta.append(
        el("strong", "pp-file-name", file.name),
        el("span", "pp-muted", formatBytes(file.size)),
      );
      card.append(meta);
      if (this.bridge.blobSave && this.bridge.canSaveBlob !== false) {
        const button = el("button", "pp-button pp-ghost", "Save");
        button.type = "button";
        button.addEventListener("click", async () => {
          button.disabled = true;
          try {
            await this.bridge.blobSave(file.blob, file.name);
          } catch (e) {
            this.bridge.toast(String(e), "error");
          } finally {
            button.disabled = false;
          }
        });
        card.append(button);
      }
      return card;
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
     * Month grid by a date column:
     * {type: "calendar", query, params?, date: "due", title: "title", label,
     * month?: "YYYY-MM", empty?, action?, max_per_day?: 3}
     * Params may use {{$month_start}} and {{$month_end}}.
     */
    calendar(spec, scope) {
      const wrap = el("div", "pp-calendar-wrap");
      const toolbar = el("div", "pp-cal-toolbar");
      const previous = el("button", "pp-button pp-ghost", "Previous month");
      previous.type = "button";
      const heading = el("h3", "pp-cal-heading");
      heading.setAttribute("aria-live", "polite");
      const next = el("button", "pp-button pp-ghost", "Next month");
      next.type = "button";
      const todayButton = el("button", "pp-button pp-ghost", "Today");
      todayButton.type = "button";
      toolbar.append(previous, heading, next, todayButton);
      const empty = el("p", "pp-empty");
      const body = el("div", "pp-cal-body");
      const list = el("section", "pp-cal-list");
      list.setAttribute("aria-live", "polite");
      wrap.append(toolbar, empty, body, list);

      const today = this.bridge.today();
      const todayMonth = parseMonth(today.slice(0, 7));
      const chosen = parseMonth(resolve(spec.month, scope));
      let month = chosen?.key || todayMonth?.key || "1970-01";
      let selected = (today.startsWith(`${month}-`) ? today : `${month}-01`);
      let sequence = 0;
      let rows = [];

      this.cleanups.push(() => { sequence++; });

      const monthState = () => parseMonth(month) || { year: 1970, month: 1, key: "1970-01" };
      const monthRange = () => {
        const { year, month: number } = monthState();
        const start = `${year}-${pad2(number)}-01`;
        const endDate = new Date(`${start}T12:00:00`);
        endDate.setMonth(endDate.getMonth() + 1);
        endDate.setDate(0);
        return { start, end: formatDay(endDate) };
      };
      const shiftMonth = (delta) => {
        const { year, month: number } = monthState();
        const date = new Date(`${year}-${pad2(number)}-01T12:00:00`);
        date.setMonth(date.getMonth() + delta);
        month = `${date.getFullYear()}-${pad2(date.getMonth() + 1)}`;
        selected = `${month}-01`;
      };
      const longDay = (day) => new Date(`${day}T12:00:00`).toLocaleDateString(undefined, {
        weekday: "long", day: "numeric", month: "long",
      });
      const countLabel = (count) => (count === 0 ? "no items" : count === 1 ? "1 item" : `${count} items`);
      const entryLabel = (row) => String(row?.[spec.title] ?? "");
      const runAction = (row) => spec.action && this.act(spec.action, { ...scope, row });

      const render = () => {
        const grid = monthGrid(monthState().year, monthState().month);
        const visible = new Set(grid.map((entry) => entry.day));
        if (!visible.has(selected)) selected = `${month}-01`;
        const byDay = new Map(grid.map((entry) => [entry.day, []]));
        for (const row of rows) {
          const day = dayKey(row?.[spec.date]);
          if (day && byDay.has(day)) byDay.get(day).push(row);
        }
        const monthEntries = grid.reduce((sum, entry) => sum + (entry.inMonth ? byDay.get(entry.day).length : 0), 0);
        const emptyTextValue = resolve(spec.empty, scope);
        empty.hidden = !(emptyTextValue && monthEntries === 0);
        empty.textContent = emptyTextValue || "";
        const shownDate = new Date(`${month}-01T12:00:00`);
        heading.textContent = shownDate.toLocaleDateString(undefined, { month: "long", year: "numeric" });

        const table = el("table", "pp-calendar");
        table.setAttribute("aria-label", resolve(spec.label || "Calendar", scope));
        table.setAttribute("aria-busy", "false");
        const thead = el("thead");
        const labels = el("tr");
        const monday = new Date("2026-11-02T12:00:00");
        for (let i = 0; i < 7; i++) {
          const date = new Date(monday);
          date.setDate(monday.getDate() + i);
          const th = el("th", "", date.toLocaleDateString(undefined, { weekday: "short" }));
          th.setAttribute("scope", "col");
          labels.append(th);
        }
        thead.append(labels);
        const tbody = el("tbody");
        const maxEntries = Math.max(1, Number(spec.max_per_day) || 3);
        for (let row = 0; row < 6; row++) {
          const tr = el("tr");
          for (let col = 0; col < 7; col++) {
            const cell = grid[row * 7 + col];
            const items = byDay.get(cell.day) || [];
            const day = Number(cell.day.slice(8, 10));
            const td = el("td", `pp-cal-day${cell.inMonth ? "" : " pp-cal-out"}${cell.day === today ? " pp-cal-today" : ""}${cell.day === selected ? " pp-cal-selected" : ""}`);
            if (cell.day === today) td.setAttribute("aria-current", "date");
            const dayButton = el("button", "pp-cal-daybtn", day);
            dayButton.type = "button";
            dayButton.dataset.day = cell.day;
            dayButton.setAttribute("aria-pressed", String(cell.day === selected));
            dayButton.setAttribute("aria-label", `${longDay(cell.day)}, ${countLabel(items.length)}`);
            dayButton.addEventListener("click", () => {
              selected = cell.day;
              render();
            });
            td.append(dayButton);
            for (const item of items.slice(0, maxEntries)) {
              const button = el("button", "pp-cal-entry", entryLabel(item));
              button.type = "button";
              button.addEventListener("click", () => runAction(item));
              td.append(button);
            }
            if (items.length > maxEntries) {
              const more = el("button", "pp-cal-more", `+${items.length - maxEntries} more`);
              more.type = "button";
              more.setAttribute("aria-label", `Show all ${items.length} items on ${longDay(cell.day)}`);
              more.addEventListener("click", () => {
                selected = cell.day;
                render();
              });
              td.append(more);
            }
            const badge = el("span", "pp-cal-count", items.length);
            badge.setAttribute("aria-hidden", "true");
            td.append(badge);
            tr.append(td);
          }
          tbody.append(tr);
        }
        table.append(thead, tbody);
        body.replaceChildren(table);

        const selectedItems = byDay.get(selected) || [];
        const listHeading = el("h4", "pp-cal-list-heading", longDay(selected));
        list.replaceChildren(listHeading);
        if (selectedItems.length === 0) {
          list.append(el("p", "pp-empty", "Nothing on this day."));
          return;
        }
        const listRows = el("ul", "pp-list");
        for (const row of selectedItems) {
          const item = el("li", "pp-list-item");
          const content = el("button", "pp-item-main");
          content.type = "button";
          if (spec.action) content.addEventListener("click", () => runAction(row));
          content.append(el("span", "pp-item-title", entryLabel(row)));
          item.append(content);
          listRows.append(item);
        }
        list.append(listRows);
      };

      const load = async () => {
        if (this.disposed) return;
        const seq = ++sequence;
        body.replaceChildren(el("p", "pp-empty", "Loading…"));
        try {
          const range = monthRange();
          rows = await this.bridge.query(spec.query, resolve(spec.params || {}, this.scope({
            ...(scope.row ? { row: scope.row } : {}),
            $month_start: range.start,
            $month_end: range.end,
          })));
        } catch (e) {
          if (seq !== sequence) return;
          const retry = el("button", "pp-button pp-ghost", "Retry loading");
          retry.type = "button";
          retry.addEventListener("click", () => { retry.disabled = true; load(); });
          const error = el("div", "pp-list-error");
          error.append(el("p", "pp-error", `Couldn’t load items: ${String(e)}`), retry);
          body.replaceChildren(error);
          list.replaceChildren();
          empty.hidden = true;
          return;
        }
        if (seq !== sequence || this.disposed) return;
        render();
      };

      previous.addEventListener("click", () => {
        shiftMonth(-1);
        load();
      });
      next.addEventListener("click", () => {
        shiftMonth(1);
        load();
      });
      todayButton.addEventListener("click", () => {
        const now = this.bridge.today();
        const nowMonth = parseMonth(now.slice(0, 7));
        if (!nowMonth) return;
        month = nowMonth.key;
        selected = now;
        load();
      });
      this.lists.push(load);
      return wrap;
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
      if (spec.layout === "board") node.style.setProperty("--pp-lanes", String((spec.columns || []).length));
      const selectable = !!(spec.select && Array.isArray(spec.select.actions) && this.bridge.bulk);
      const wrap = selectable ? el("div", "pp-select-list") : null;
      const bar = selectable ? el("div", "pp-select-bar") : null;
      let selecting = false;
      let running = false;
      const selected = new Set();
      let currentRows = [];
      let rowNodes = new Map();
      let anchor = null;
      let suppressClickId = null;
      const runSelected = async (action, ids, args, s) => {
        running = true;
        drawBar();
        let report;
        try {
          report = await this.bridge.bulk(action.op, ids, args);
        } catch (e) {
          running = false;
          drawBar();
          this.bridge.toast(String(e), "error");
          return;
        }
        if (this.disposed) {
          this.bridge.changed();
          return;
        }
        running = false;
        const message = report.failed
          ? report.message
          : (resolve(action.success, { ...s, result: report }) || report.message);
        const canUndo = !!action.undo?.op && !report.dry_run && report.done > 0;
        const okIds = canUndo
          ? (report.items || []).filter((item) => item.ok).map((item) => item.id)
          : [];
        if (report.failed === 0) {
          setSelecting(false);
        } else {
          setSelecting(true);
          selected.clear();
          for (const item of report.items || []) if (!item.ok && (typeof item.id === "string" || typeof item.id === "number")) selected.add(String(item.id));
          syncRows();
          drawBar();
        }
        this.bridge.toast(message, report.failed ? "error" : "ok", canUndo ? {
          label: "Undo",
          run: () => this.undoSelected(action.undo, okIds, s),
        } : null);
        this.bridge.changed();
      };
      if (selectable) {
        wrap.dataset.selecting = "false";
        wrap.append(bar, node);
      }
      const validId = (row) => row && (typeof row.id === "string" || typeof row.id === "number");
      const toggle = (id, on) => {
        if (on == null) {
          if (selected.has(id)) selected.delete(id);
          else selected.add(id);
          return;
        }
        if (on) selected.add(id);
        else selected.delete(id);
      };
      const setSelecting = (on) => {
        selecting = !!on;
        wrap.dataset.selecting = String(selecting);
        if (!selecting) {
          selected.clear();
          anchor = null;
        }
        syncRows();
        drawBar();
      };
      const syncRows = () => {
        for (const [id, refs] of rowNodes) {
          const on = selected.has(id);
          refs.check.checked = on;
          refs.check.hidden = !selecting;
          refs.item.dataset.ppSelected = String(on);
        }
      };
      const drawBar = () => {
        if (!selectable) return;
        bar.replaceChildren();
        bar.setAttribute("role", "toolbar");
        bar.setAttribute("aria-label", "Selection");
        if (!selecting) {
          const select = el("button", "pp-button pp-ghost", "Select");
          select.type = "button";
          select.setAttribute("aria-label", resolve(spec.select.label || "Select items", scope));
          select.addEventListener("click", () => {
            setSelecting(true);
          });
          bar.append(select);
          return;
        }
        bar.append(el("span", "pp-select-count", `${selected.size} selected`));
        const selectAll = el("button", "pp-button pp-ghost", "Select all");
        selectAll.type = "button";
        selectAll.disabled = running;
        selectAll.addEventListener("click", () => {
          selected.clear();
          for (const row of currentRows) if (validId(row)) selected.add(String(row.id));
          syncRows();
          drawBar();
        });
        bar.append(selectAll);
        for (const action of spec.select.actions) {
          const button = el("button", `pp-button${action.variant === "danger" ? " pp-danger" : ""}`, resolve(action.label, scope));
          button.type = "button";
          if (action.icon && ICONS[action.icon]) button.prepend(icon(action.icon));
          button.disabled = running || selected.size === 0 || selected.size > 500;
          button.addEventListener("click", async () => {
            const ids = [...selected];
            const count = ids.length;
            let s = { ...scope, count };
            let askedValue = null;
            const ask = action.ask && typeof action.ask === "object" ? action.ask : null;
            if (ask) {
              if (!window.terrariumAsk) {
                this.bridge.toast("This action needs a newer app.", "error");
                return;
              }
              askedValue = await (window.terrariumAsk?.(resolve(ask.title, s), {
                label: resolve(ask.label, s),
                confirm: resolve(action.label, s),
              }) ?? null);
              if (askedValue == null) return;
              s = { ...s, value: askedValue };
            }
            if (action.confirm) {
              const question = resolve(action.confirm, s);
              const ok = window.terrariumConfirm
                ? await window.terrariumConfirm(question, { confirm: resolve(action.label, s), danger: action.variant === "danger" })
                : window.confirm(question);
              if (!ok) return;
            }
            const args = resolve(action.args || {}, s);
            if (ask && typeof ask.name === "string" && ask.name) args[ask.name] = askedValue;
            await runSelected(action, ids, args, s);
          });
          bar.append(button);
        }
        const cancel = el("button", "pp-button pp-ghost", "Cancel");
        cancel.type = "button";
        cancel.disabled = running;
        cancel.addEventListener("click", () => {
          setSelecting(false);
        });
        bar.append(cancel);
        if (selected.size > 500) bar.append(el("p", "pp-select-note", "Select up to 500 items at a time."));
      };
      if (selectable) {
        wrap.addEventListener("keydown", (event) => {
          const key = String(event.key || "");
          if (key === "Escape" && selecting) {
            event.preventDefault?.();
            setSelecting(false);
            return;
          }
          if (!(event.metaKey || event.ctrlKey) || key.toLowerCase() !== "a" || !selecting) return;
          event.preventDefault?.();
          selected.clear();
          for (const row of currentRows) if (validId(row)) selected.add(String(row.id));
          syncRows();
          drawBar();
        });
      }
      node.setAttribute("aria-busy", "true");
      if (board) node.setAttribute("aria-label", resolve(spec.label || "Board", scope));
      node.append(el("li", "pp-empty", "Loading…"));
      let region = { lists: [], watchers: [] };
      let rendered = null;
      let sequence = 0;
      let moveRunning = false;
      let status = null;
      const hasClass = (target, className) => String(target?.className || "").split(/\s+/).includes(className);
      const addClass = (target, className) => {
        if (!target || hasClass(target, className)) return;
        target.className = `${String(target.className || "").trim()} ${className}`.trim();
      };
      const removeClass = (target, className) => {
        if (!target) return;
        target.className = String(target.className || "").split(/\s+/).filter((name) => name && name !== className).join(" ");
      };
      const findLane = (target) => {
        if (!target) return null;
        if (typeof target.closest === "function") {
          const lane = target.closest(".pp-board-lane");
          if (lane) return lane;
        }
        for (let current = target; current; current = current.parentNode || current.parent) {
          if (hasClass(current, "pp-board-lane")) return current;
        }
        return null;
      };
      const laneAt = (x, y) => findLane(document.elementFromPoint?.(x, y));
      const statusNode = () => {
        if (!status) {
          status = el("li", "pp-sr pp-board-status");
          status.setAttribute("role", "status");
          status.setAttribute("aria-live", "polite");
        }
        return status;
      };
      const rowMarker = (row) => {
        try {
          return JSON.stringify(row ?? null);
        } catch (_) {
          return "";
        }
      };
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
        if (selectable) {
          currentRows = rows;
          const visible = new Set(rows.filter(validId).map((row) => String(row.id)));
          for (const id of [...selected]) if (!visible.has(id)) selected.delete(id);
        }
        // Always query fresh data, but keep unchanged controls (and drafts)
        // in place. Include state because row templates may reference it.
        const signature = JSON.stringify([rows, this.state]);
        if (signature === rendered) {
          if (selectable) {
            syncRows();
            drawBar();
          }
          await Promise.all(region.lists.map((load) => load()));
          if (seq === sequence) node.setAttribute("aria-busy", "false");
          return;
        }
        rendered = null;
        node.setAttribute("aria-busy", "true");
        this.release(region);
        node.replaceChildren();
        region = { lists: [], watchers: [] };
        rowNodes = new Map();
        if (rows.length === 0 && emptyText(spec, scope)) node.append(el("li", "pp-empty", resolve(emptyText(spec, scope), scope)));
        // All lanes share one result set, so filters and row limits are
        // identical to the list. Empty lanes remain visible for orientation.
        const lanes = new Map();
        const laneOrder = [];
        if (board) {
          for (const column of spec.columns || []) {
            const value = String(column.value);
            const matching = rows.filter(row => String(row[spec.column]) === value);
            const lane = el("li", "pp-board-lane");
            lane.dataset.ppLane = value;
            const title = resolve(column.label, scope);
            const heading = el("h3", "pp-board-heading");
            const headingId = nextId("pp-lane");
            heading.id = headingId;
            const count = el("span", "pp-board-count", matching.length);
            heading.append(el("span", "", title), count);
            lane.append(heading);
            if (column.description) lane.append(el("p", "pp-board-description", resolve(column.description, scope)));
            const items = el("ul", "pp-list pp-board-items");
            items.setAttribute("aria-labelledby", headingId);
            if (!matching.length) items.append(el("li", "pp-empty", "No tasks"));
            lane.append(items);
            node.append(lane);
            laneOrder.push(value);
            lanes.set(value, { lane, items, count, cards: matching.length, label: String(title || value) });
          }
        }
        if (board && spec.move) node.append(statusNode());
        const isEmptyNode = (child) => hasClass(child, "pp-empty");
        const keepCards = (children) => children.filter((child) => !isEmptyNode(child));
        const syncLaneCount = (lane) => { lane.count.textContent = String(lane.cards); };
        const ensurePlaceholder = (lane) => {
          if (lane.cards > 0) return;
          if ([...lane.items.children].some(isEmptyNode)) return;
          lane.items.append(el("li", "pp-empty", "No tasks"));
        };
        const removePlaceholder = (lane) => {
          const kept = keepCards([...lane.items.children]);
          if (kept.length === lane.items.children.length) return false;
          lane.items.replaceChildren(...kept);
          return true;
        };
        // Moves a row's card to lane `to` and runs the declared operation.
        const moveTo = async (row, from, to, item, handle) => {
          if (!spec.move || to === from || moveRunning) return false;
          const source = lanes.get(String(from));
          const target = lanes.get(String(to));
          if (!source || !target) return false;
          moveRunning = true;
          const marker = rowMarker(row);
          const titleResolved = String(resolve(spec.move.label || "{{row.title}}", { ...scope, row }) || "").trim();
          const title = titleResolved || "card";
          source.items.replaceChildren(...[...source.items.children].filter((child) => child !== item));
          source.cards = Math.max(0, source.cards - 1);
          syncLaneCount(source);
          ensurePlaceholder(source);
          removePlaceholder(target);
          target.items.replaceChildren(...keepCards([...target.items.children]).filter((child) => child !== item), item);
          target.cards += 1;
          syncLaneCount(target);
          handle.disabled = true;
          try {
            const ok = await this.act(spec.move, { ...scope, row, value: to });
            if (ok) {
              statusNode().textContent = `Moved ${title} to ${target.label}`;
              handle.focus?.();
              return true;
            }
            statusNode().textContent = `Couldn't move ${title}`;
            rendered = null;
            await load();
            const all = node.querySelectorAll ? node.querySelectorAll(".pp-board-handle") : [];
            for (const entry of all) {
              if (entry?.dataset?.ppMoveRow === marker) {
                entry.focus?.();
                break;
              }
            }
            return false;
          } finally {
            moveRunning = false;
            handle.disabled = false;
          }
        };
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
              const row = withRef(rows[index++], spec.table, this.bridge.plugin);
              if (!board && spec.group && row[spec.group] !== group) {
                group = row[spec.group];
                const tone = spec.group_tone ? row[spec.group_tone] : null;
                if (group != null) fragment.append(el("li", `pp-group${tone ? ` pp-tone-${tone}` : ""}`, group));
              }
              const item = el("li", "pp-list-item");
              const rowScope = { ...scope, row };
              if (board && spec.move) {
                addClass(item, "pp-board-movable");
                const cardTitle = String(resolve(spec.move.label || "{{row.title}}", rowScope) || "").trim() || "card";
                const handle = el("button", "pp-board-handle");
                handle.type = "button";
                handle.title = "Drag to move";
                handle.setAttribute("aria-label", `Move ${cardTitle}. Drag, or use arrow keys.`);
                handle.dataset.ppMoveRow = rowMarker(row);
                handle.append(icon("grip"));
                const moveBy = async (delta, event) => {
                  const from = String(row[spec.column]);
                  const at = laneOrder.indexOf(from);
                  const target = laneOrder[at + delta];
                  if (!target) return false;
                  event?.preventDefault?.();
                  return moveTo(row, from, target, item, handle);
                };
                handle.addEventListener("keydown", async (event) => {
                  if (event.key === "Escape") {
                    removeClass(item, "pp-dragging");
                    for (const lane of lanes.values()) removeClass(lane.lane, "pp-drop-target");
                    return;
                  }
                  if (spec.layout === "board") {
                    if (event.key === "ArrowLeft") await moveBy(-1, event);
                    if (event.key === "ArrowRight") await moveBy(1, event);
                    return;
                  }
                  if (spec.layout === "matrix") {
                    if (event.key === "ArrowLeft") await moveBy(-1, event);
                    if (event.key === "ArrowRight") await moveBy(1, event);
                    if (event.key === "ArrowUp") await moveBy(-2, event);
                    if (event.key === "ArrowDown") await moveBy(2, event);
                  }
                });
                let drag = null;
                const clearDrag = () => {
                  if (!drag) return;
                  removeClass(item, "pp-dragging");
                  for (const lane of lanes.values()) removeClass(lane.lane, "pp-drop-target");
                  drag = null;
                };
                handle.addEventListener("pointerdown", (event) => {
                  if (moveRunning) return;
                  const touchLike = event.pointerType === "touch" || event.pointerType === "pen";
                  const primaryMouse = event.pointerType === "mouse" ? event.button === 0 : true;
                  if (!touchLike && !primaryMouse) return;
                  drag = {
                    id: event.pointerId,
                    from: String(row[spec.column]),
                    x: Number(event.clientX || 0),
                    y: Number(event.clientY || 0),
                    active: false,
                    over: null,
                  };
                  handle.setPointerCapture?.(event.pointerId);
                });
                handle.addEventListener("pointermove", (event) => {
                  if (!drag || event.pointerId !== drag.id) return;
                  if (!drag.active) {
                    const dx = Number(event.clientX || 0) - drag.x;
                    const dy = Number(event.clientY || 0) - drag.y;
                    if (Math.hypot(dx, dy) < 4) return;
                    drag.active = true;
                    addClass(item, "pp-dragging");
                  }
                  const lane = laneAt(event.clientX, event.clientY);
                  if (lane === drag.over) return;
                  if (drag.over) removeClass(drag.over, "pp-drop-target");
                  drag.over = lane;
                  if (drag.over) addClass(drag.over, "pp-drop-target");
                });
                handle.addEventListener("pointercancel", clearDrag);
                handle.addEventListener("pointerup", async (event) => {
                  if (!drag || event.pointerId !== drag.id) return;
                  const drop = drag.over || laneAt(event.clientX, event.clientY);
                  const from = drag.from;
                  clearDrag();
                  const to = drop?.dataset?.ppLane;
                  if (to && to !== from) await moveTo(row, from, to, item, handle);
                });
                item.append(handle);
              }
              if (selectable && validId(row)) {
                const id = String(row.id);
                item.dataset.ppId = id;
                item.dataset.ppSelected = String(selected.has(id));
                const check = el("input", "pp-select-check");
                check.type = "checkbox";
                check.checked = selected.has(id);
                check.hidden = !selecting;
                check.setAttribute("aria-label", `Select ${row.title ?? id}`);
                check.addEventListener("change", () => {
                  if (check.checked) selected.add(id);
                  else selected.delete(id);
                  anchor = id;
                  syncRows();
                  drawBar();
                });
                let longPressTimer = null;
                let longPressX = 0;
                let longPressY = 0;
                let longPressed = false;
                const clearLongPress = () => {
                  if (longPressTimer == null) return;
                  this.cancel(longPressTimer);
                  longPressTimer = null;
                };
                const movedTooFar = (event) => {
                  const dx = (event.clientX ?? 0) - longPressX;
                  const dy = (event.clientY ?? 0) - longPressY;
                  return Math.sqrt(dx * dx + dy * dy) > 10;
                };
                item.addEventListener("pointerdown", (event) => {
                  if (event.pointerType !== "touch") return;
                  longPressX = event.clientX ?? 0;
                  longPressY = event.clientY ?? 0;
                  longPressed = false;
                  clearLongPress();
                  longPressTimer = this.schedule(() => {
                    longPressTimer = null;
                    longPressed = true;
                    suppressClickId = id;
                    setSelecting(true);
                    toggle(id, true);
                    anchor = id;
                    syncRows();
                    drawBar();
                    if (typeof navigator !== "undefined" && typeof navigator.vibrate === "function") navigator.vibrate(10);
                  }, 500);
                });
                item.addEventListener("pointermove", (event) => {
                  if (longPressTimer == null) return;
                  if (movedTooFar(event)) clearLongPress();
                });
                item.addEventListener("pointerup", () => clearLongPress());
                item.addEventListener("pointercancel", () => clearLongPress());
                item.addEventListener("contextmenu", (event) => {
                  if (selecting || longPressed) event.preventDefault?.();
                });
                item.addEventListener("click", (event) => {
                  if (suppressClickId === id) {
                    suppressClickId = null;
                    longPressed = false;
                    event.preventDefault?.();
                    event.stopPropagation?.();
                    return;
                  }
                  longPressed = false;
                  const mod = !!(event.metaKey || event.ctrlKey);
                  if (!selecting && !mod) return;
                  if (event.target === check) {
                    anchor = id;
                    return;
                  }
                  event.preventDefault?.();
                  event.stopPropagation?.();
                  if (!selecting && mod) {
                    setSelecting(true);
                    toggle(id, true);
                    anchor = id;
                    syncRows();
                    drawBar();
                    return;
                  }
                  if (event.shiftKey && anchor != null) {
                    const ids = currentRows.filter(validId).map((row) => String(row.id));
                    const start = ids.indexOf(String(anchor));
                    const end = ids.indexOf(id);
                    if (start >= 0 && end >= 0) {
                      const [from, to] = start <= end ? [start, end] : [end, start];
                      for (let i = from; i <= to; i++) selected.add(ids[i]);
                    } else {
                      toggle(id);
                      anchor = id;
                    }
                  } else {
                    toggle(id);
                    anchor = id;
                  }
                  syncRows();
                  drawBar();
                }, true);
                item.prepend(check);
                rowNodes.set(id, { item, check });
              }
              const child = this.node(spec.item, { ...scope, row });
              if (child) item.append(child);
              if (board) lanes.get(String(row[spec.column]))?.items.append(item);
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
        if (selectable) drawBar();
        if (seq === sequence) node.setAttribute("aria-busy", "false");
      };
      this.lists.push(load);
      drawBar();
      return selectable ? wrap : node;
    },

    /**
     * Sortable query table:
     * {type: "table", query, params?, label, columns: [{key, label, type?}],
     * sort?: key, dir?: "asc"|"desc", empty?, empty_focus?, limit?, action?}
     */
    table(spec, scope) {
      const wrap = el("div", "pp-table-wrap");
      const sortbar = el("label", "pp-table-sortbar");
      sortbar.append(el("span", "", "Sort by"));
      const sortSelect = el("select", "pp-input");
      const orderLabel = el("label", "pp-table-order");
      orderLabel.append(el("span", "", "Order"));
      const orderSelect = el("select", "pp-input");
      const asc = el("option", "", "Ascending");
      asc.value = "asc";
      const desc = el("option", "", "Descending");
      desc.value = "desc";
      orderSelect.append(asc, desc);
      orderLabel.append(orderSelect);
      sortbar.append(sortSelect, orderLabel);
      wrap.append(sortbar);

      const table = el("table", "pp-table");
      table.setAttribute("aria-label", resolve(spec.label || "Table", scope));
      const head = el("thead");
      const headRow = el("tr");
      const body = el("tbody");
      body.setAttribute("aria-busy", "true");
      table.append(head, body);
      head.append(headRow);
      wrap.append(table);

      const columns = (spec.columns || []).map((column) => ({ ...column, type: column.type || "string" }));
      let sort = columns.some((column) => column.key === spec.sort) ? spec.sort : (columns[0]?.key || "");
      let dir = spec.dir === "desc" ? "desc" : "asc";
      let rows = [];
      let region = { lists: [], watchers: [], cleanups: [] };
      let sequence = 0;
      this.cleanups.push(() => { sequence++; this.release(region); });
      this.watchers.push(() => { for (const watch of region.watchers) watch(); });

      const formatCell = (value, type) => {
        if (value == null) return "";
        if (type === "boolean") return truthy(value) ? "Yes" : "";
        if (type === "json") {
          try {
            return JSON.stringify(value);
          } catch (_) {
            return "";
          }
        }
        return String(value);
      };

      const syncControls = () => {
        sortSelect.value = sort;
        orderSelect.value = dir;
      };

      const drawHead = () => {
        headRow.replaceChildren();
        sortSelect.replaceChildren();
        for (const column of columns) {
          const opt = el("option", "", String(column.label || column.key || ""));
          opt.value = String(column.key || "");
          sortSelect.append(opt);

          const th = el("th");
          th.setAttribute("scope", "col");
          const current = column.key === sort;
          th.setAttribute("aria-sort", current ? (dir === "desc" ? "descending" : "ascending") : "none");
          const button = el("button", "pp-table-sort", String(column.label || column.key || ""));
          button.type = "button";
          button.title = `Sort by ${String(column.label || column.key || "")}`;
          button.addEventListener("click", () => {
            if (column.key === sort) dir = dir === "asc" ? "desc" : "asc";
            else {
              sort = String(column.key || "");
              dir = "asc";
            }
            syncControls();
            drawHead();
            drawBody();
          });
          th.append(button);
          headRow.append(th);
        }
        syncControls();
      };

      const drawBody = () => {
        this.release(region);
        region = { lists: [], watchers: [], cleanups: [] };
        body.replaceChildren();
        const sorted = sort ? sortRows(rows, sort, columns.find((c) => c.key === sort)?.type || "string", dir) : [...rows];
        if (sorted.length === 0 && emptyText(spec, scope)) {
          const tr = el("tr");
          const td = el("td", "pp-empty", resolve(emptyText(spec, scope), scope));
          td.setAttribute("colspan", String(Math.max(1, columns.length)));
          tr.append(td);
          body.append(tr);
          return;
        }
        for (const row of sorted) {
          const tr = el("tr");
          for (const [index, column] of columns.entries()) {
            const td = el("td");
            td.setAttribute("data-label", String(column.label || column.key || ""));
            const value = formatCell(row?.[column.key], column.type);
            if (index === 0 && spec.action) {
              const open = el("button", "pp-table-open", value);
              open.type = "button";
              open.addEventListener("click", () => this.act(spec.action, { ...scope, row }));
              td.append(open);
            } else td.textContent = value;
            tr.append(td);
          }
          body.append(tr);
        }
      };

      sortSelect.addEventListener("change", () => {
        sort = String(sortSelect.value || "");
        drawHead();
        drawBody();
      });
      orderSelect.addEventListener("change", () => {
        dir = orderSelect.value === "desc" ? "desc" : "asc";
        drawHead();
        drawBody();
      });

      drawHead();
      const loadingRow = el("tr");
      const loadingCell = el("td", "pp-empty", "Loading…");
      loadingCell.setAttribute("colspan", String(Math.max(1, columns.length)));
      loadingRow.append(loadingCell);
      body.append(loadingRow);
      const load = async () => {
        if (this.disposed) return;
        const seq = ++sequence;
        const params = resolve(spec.params || {}, this.scope(scope.row ? { row: scope.row } : {}));
        body.setAttribute("aria-busy", "true");
        let loaded;
        try {
          loaded = await this.bridge.query(spec.query, params);
        } catch (e) {
          if (seq !== sequence) return;
          this.release(region);
          region = { lists: [], watchers: [], cleanups: [] };
          body.replaceChildren();
          const tr = el("tr");
          const td = el("td");
          td.setAttribute("colspan", String(Math.max(1, columns.length)));
          const retry = el("button", "pp-button pp-ghost", "Retry loading");
          retry.type = "button";
          retry.addEventListener("click", () => { retry.disabled = true; load(); });
          td.append(el("p", "pp-error", `Couldn’t load items: ${String(e)}`), retry);
          tr.append(td);
          body.append(tr);
          body.setAttribute("aria-busy", "false");
          return;
        }
        if (seq !== sequence) return;
        rows = loaded || [];
        if (spec.limit) rows = rows.slice(0, Number(spec.limit));
        drawBody();
        body.setAttribute("aria-busy", "false");
      };
      this.lists.push(load);
      return wrap;
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
    grip: "M9 6h.01M15 6h.01M9 12h.01M15 12h.01M9 18h.01M15 18h.01",
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
      const re = /(\!\[[^\]]*\]\([^)]+\)|\*\*[^*]+\*\*|\*[^*]+\*|`[^`]+`|\[[^\]]+\]\(terrarium:[a-z][a-z0-9-]*\/[a-z][a-z0-9_]*\/[A-Za-z0-9_-]{1,64}\)|\[[^\]]+\]\(https?:\/\/[^)\s]+\)|https?:\/\/[^\s)]+)/g;
      let last = 0;
      for (const m of line.matchAll(re)) {
        if (m.index > last) out.append(line.slice(last, m.index));
        const token = m[0];
        if (token.startsWith("![")) {
          const image = token.match(/^!\[([^\]]*)\]\(([^)\s]+)\)$/);
          const ref = image ? blobLinkRef(image[2]) : null;
          if (ref) out.append(renderBlobImage(
            { blob: ref.blob, name: image[1] || "Attachment", mime: "", size: 0 },
            { alt: image[1], blobGet: hooks.blobGet },
          ));
          else out.append(token);
        } else if (token.startsWith("**")) out.append(el("strong", "", token.slice(2, -2)));
        else if (token.startsWith("`")) out.append(el("code", "", token.slice(1, -1)));
        else if (token.startsWith("[") || token.startsWith("http")) {
          const link = token.match(/^\[([^\]]+)\]\((.*)\)$/);
          const href = link ? link[2] : token;
          if (href.startsWith("terrarium:")) {
            out.append(typeof hooks.ref === "function" ? hooks.ref(href.slice(10), link[1]) : link[1]);
            last = m.index + token.length;
            continue;
          }
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
    compareValues,
    sortRows,
    monthGrid,
    dayKey,
  };
})();
