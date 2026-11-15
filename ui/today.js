// Today: the host view that merges rows plugins contribute through
// `today-v1`. Plugins own the queries and operations; the host resolves
// each source's row templates, merges rows by section and draws one shared
// row. No SQL crosses plugins and nothing is copied.
(function (root) {
  "use strict";

  const SECTIONS = ["my_day", "earlier", "due", "habits", "sort", "catch_up", "wrap_up", "done"];
  // `earlier` is drawn inside My Day, so it has no place of its own.
  const ORDERABLE = SECTIONS.filter((s) => s !== "earlier");
  const LABELS = {
    my_day: "My Day", earlier: "Still planned from earlier", due: "Due & reminders", habits: "Habits",
    sort: "To sort", catch_up: "Catch up", wrap_up: "Wrap up the day", done: "Done today",
  };
  const KIND_ICONS = { task: "check", reminder: "clock", habit: "target", capture: "mic", link: "link", article: "rss", sleep: "moon", note: "note" };
  const MAX_SOURCES = 16;
  const MAX_LIMIT = 50;
  const WRAP_UP_HOUR = 17;
  const PREFS_KEY = "pp-today:";
  const WHOLE = /^\{\{\s*([\w$.]+)\s*\}\}$/;
  const PART = /\{\{\s*([\w$.]+)\s*\}\}/g;

  // ---- Templates ----------------------------------------------------------

  function lookup(scope, path) {
    let value = scope;
    for (const key of path.split(".")) {
      if (value == null) return undefined;
      value = value[key];
    }
    return value;
  }

  /** `{{path}}` alone yields the raw value; otherwise lookups join as text. */
  function resolve(input, scope) {
    if (typeof input === "string") {
      const whole = input.match(WHOLE);
      if (whole) return lookup(scope, whole[1]);
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
    return !(v == null || v === false || v === 0 || v === "" || v === "0" || v === "false");
  }

  const pad = (n) => String(n).padStart(2, "0");
  const dayKey = (d) => `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;

  /** Clock values a source may use: `{{$today}}`, `{{$tomorrow}}`, `{{$now}}`, … */
  function context(now) {
    const d = new Date(now);
    const next = new Date(d.getFullYear(), d.getMonth(), d.getDate() + 1);
    return {
      $today: dayKey(d), $tomorrow: dayKey(next), $now: d.getTime(),
      $weekday: (d.getDay() + 6) % 7, $utc_offset_ms: -d.getTimezoneOffset() * 60000,
    };
  }

  // ---- Contract -----------------------------------------------------------

  const ident = (v, max) => typeof v === "string" && v.length >= 1 && v.length <= max && /^[a-z][a-z0-9_-]*$/.test(v);
  const label = (v, max) => typeof v === "string" && v.trim().length > 0 && v.length <= max;

  function declares(plugin, op) {
    if (Array.isArray(plugin?.operations)) return plugin.operations.includes(op);
    return (plugin?.quick_actions || []).some((a) => a.op === op);
  }

  function checkAction(plugin, action) {
    if (!action || typeof action !== "object") return "needs an operation, navigate or url";
    const { op, navigate, url } = action;
    if (op == null && navigate == null && url == null) return "needs an operation, navigate or url";
    if (navigate != null && (op != null || url != null)) return "cannot combine navigate with an operation or url";
    if (op != null && (typeof op !== "string" || !declares(plugin, op))) return `\`${op}\` is not a declared operation`;
    if (navigate != null && (typeof navigate !== "object" || Array.isArray(navigate) || !Object.keys(navigate).length
      || Object.keys(navigate).some((k) => k !== "item" && k !== "state"))) return "navigate takes item and state";
    if (url != null && typeof url !== "string") return "url must be text";
    return null;
  }

  /**
   * Re-checks a source the backend already validated, so an older backend
   * or a hand-edited manifest cannot make Today run something malformed.
   * Returns an error message, or null when the source is usable.
   */
  function validateSource(plugin, source) {
    if (!source || typeof source !== "object") return "not an object";
    if (!ident(source.id, 40)) return "id must use a-z, 0-9, _ and - and start with a letter";
    if (!SECTIONS.includes(source.section)) return `unknown section \`${source.section}\``;
    if (!label(source.query, 100)) return "query is required";
    const limit = source.limit ?? 20;
    if (!Number.isInteger(limit) || limit < 1 || limit > MAX_LIMIT) return `limit must be 1..=${MAX_LIMIT}`;
    const row = source.row;
    if (!row || typeof row !== "object") return "row is required";
    if (!label(row.key, 200)) return "row.key is required";
    if (!label(row.title, 400)) return "row.title is required";
    if (row.check) {
      if (!row.check.action?.op) return "row.check needs an operation";
      const bad = checkAction(plugin, row.check.action);
      if (bad) return `row.check ${bad}`;
    }
    const actions = row.actions || [];
    if (!Array.isArray(actions) || actions.length > 4) return "row: at most 4 actions";
    for (const action of actions) {
      if (!label(action?.label, 24)) return "row action label must be 1-24 characters";
      const bad = checkAction(plugin, action.action);
      if (bad) return `row action \`${action.label}\` ${bad}`;
    }
    if (row.open) {
      const bad = checkAction(plugin, row.open);
      if (bad) return `row.open ${bad}`;
    }
    if (source.link) {
      if (!label(source.link.label, 40)) return "link label must be 1-40 characters";
      const bad = checkAction(plugin, source.link.action);
      if (bad) return `link ${bad}`;
    }
    return null;
  }

  /** Plugins in display order: Task manager first (the planner), then load order. */
  function pluginOrder(plugins) {
    return (plugins || []).slice().sort((a, b) => Number(b.id === "tasks") - Number(a.id === "tasks"));
  }

  /** Usable sources of the shown plugins, and one message per rejected entry. */
  function sourcesOf(plugins, prefs = normalizePrefs(null)) {
    const sources = [], errors = [];
    for (const plugin of pluginOrder(plugins)) {
      for (const message of plugin.today_errors || []) errors.push(String(message));
      if (prefs.plugins.includes(plugin.id)) continue;
      const ids = new Set();
      for (const source of (plugin.today || []).slice(0, MAX_SOURCES)) {
        const bad = validateSource(plugin, source) || (ids.has(source.id) ? "id repeats" : null);
        if (bad) { errors.push(`${plugin.id}: today \`${source?.id ?? "?"}\`: ${bad}`); continue; }
        ids.add(source.id);
        sources.push({ plugin, source });
      }
    }
    return { sources, errors };
  }

  // ---- Rows and sections --------------------------------------------------

  /** Maps one query row through a source's templates to the shared row. */
  function toRow(plugin, source, raw, ctx, index = 0) {
    const scope = { ...ctx, row: raw };
    const text = (template) => {
      if (template == null) return "";
      const value = resolve(template, scope);
      return value == null ? "" : String(value).trim();
    };
    const spec = source.row;
    const sortRaw = spec.sort == null ? null : resolve(spec.sort, scope);
    const sort = sortRaw == null || sortRaw === "" || !Number.isFinite(Number(sortRaw)) ? null : Number(sortRaw);
    const key = text(spec.key);
    return {
      id: `${plugin.id}\u0000${key}`, plugin: plugin.id, pluginName: plugin.name || plugin.id,
      source: source.id, section: source.section, key,
      kind: text(spec.kind) || "item", title: text(spec.title), note: text(spec.note), time: text(spec.time),
      sort, tone: text(spec.tone), done: truthy(spec.done == null ? false : resolve(spec.done, scope)),
      chips: (spec.chips || []).map((c) => ({ text: text(c.text), tone: c.tone || "" })).filter((c) => c.text),
      check: spec.check?.action || null, actions: spec.actions || [], open: spec.open || null,
      extra: index >= (source.limit ?? 20), scope,
    };
  }

  // Rows from one source keep its order. When several sources share a
  // section, untimed rows stay first in source order and timed rows follow
  // by their `sort` value, so reminders from two plugins interleave.
  function orderRows(rows, mixed) {
    if (!mixed) return rows;
    const untimed = rows.filter((r) => r.sort == null);
    const timed = rows.filter((r) => r.sort != null).map((row, i) => ({ row, i }))
      .sort((a, b) => a.row.sort - b.row.sort || a.i - b.i).map((e) => e.row);
    return [...untimed, ...timed];
  }

  /**
   * Merges each source's rows into sections. A `(plugin, key)` seen in an
   * earlier section (in the fixed order) is left out of later ones.
   * `results`: `[{plugin, source, rows: [built rows], total}]` in source order.
   */
  function merge(results) {
    const seen = new Set();
    const out = {};
    for (const section of SECTIONS) {
      const parts = results.filter((r) => r.source.section === section);
      const rows = [];
      let total = 0;
      for (const part of parts) {
        let kept = 0;
        for (const row of part.rows) {
          if (seen.has(row.id)) continue;
          seen.add(row.id);
          rows.push(row);
          kept++;
        }
        const reported = Number.isFinite(part.total) ? part.total : part.rows.length;
        total += Math.max(kept, reported - (part.rows.length - kept));
      }
      out[section] = { section, parts, total, rows: orderRows(rows, parts.filter((p) => p.rows.length).length > 1) };
    }
    return out;
  }

  /** Section ids to draw, in the chosen order. Empty ones hide, except My Day. */
  function visibleSections(sections, prefs) {
    return prefs.order.filter((id) => {
      if (prefs.hidden.includes(id)) return false;
      const s = sections[id];
      if (!s || !s.parts.length) return false;
      return id === "my_day" || s.rows.length > 0;
    });
  }

  /** Wrap up opens at 17:00 local time; Done today stays folded. */
  function folded(section, now, overrides = {}) {
    if (Object.hasOwn(overrides, section)) return overrides[section];
    if (section === "done") return true;
    if (section === "wrap_up") return new Date(now).getHours() < WRAP_UP_HOUR;
    return false;
  }

  /** Where the "now" line goes among rows: before the first timed row at or after now. -1 when no row is timed. */
  function nowIndex(rows, now) {
    let last = -1;
    for (let i = 0; i < rows.length; i++) {
      if (rows[i].sort == null) continue;
      if (rows[i].sort >= now) return i;
      last = i;
    }
    return last < 0 ? -1 : last + 1;
  }

  function countText(section) {
    if (section.section === "habits") {
      const done = section.rows.filter((r) => r.done).length;
      return `${done} of ${section.total}`;
    }
    if (section.section === "catch_up") return `${section.total} unread`;
    return String(section.total);
  }

  /** "3 planned · 2 due · 2 habits · 4 to sort · 1 overdue" from the shown sections. */
  function summary(sections, prefs) {
    const shown = (id) => sections[id] && !prefs.hidden.includes(id) ? sections[id].total : 0;
    const overdue = SECTIONS.filter((id) => !prefs.hidden.includes(id))
      .flatMap((id) => sections[id]?.rows || []).filter((r) => r.tone === "overdue" && !r.done).length;
    const parts = [
      [shown("my_day"), "planned"], [shown("due"), "due"], [shown("habits"), shown("habits") === 1 ? "habit" : "habits"],
      [shown("sort"), "to sort"], [overdue, "overdue"],
    ].filter(([n]) => n > 0).map(([n, word]) => `${n} ${word}`);
    return parts.length ? parts.join(" · ") : "Nothing planned yet";
  }

  // ---- Capture ------------------------------------------------------------

  const SCHEME = /^https?:\/\/\S+$/i;
  const BARE = /^(?:www\.)?[a-z0-9-]+(?:\.[a-z0-9-]+)*\.[a-z]{2,}(?:\/\S*)?$/i;

  /** A token is a link when it has a scheme, starts with www., or is a domain with a path. */
  function isLink(token) {
    if (SCHEME.test(token)) return true;
    if (!BARE.test(token)) return false;
    return /^www\./i.test(token) || token.includes("/");
  }

  function looksLikeUrl(text) {
    const tokens = text.split(/\s+/).filter(Boolean);
    if (tokens.length === 1 && BARE.test(tokens[0])) return true;
    return tokens.some(isLink);
  }

  /**
   * Decides where capture text goes. Links go to Bookmarks, `note:` to
   * Notes, anything else to To-do's quick add in the chosen destination.
   * Each route needs its plugin installed with the operation declared;
   * otherwise the text falls through to the next one.
   * `dest`: `{id: "inbox" | "my_day" | <list id>}`.
   */
  function routeCapture(input, plugins, dest = { id: "inbox" }) {
    const text = String(input || "").trim();
    if (!text) return null;
    const find = (id, op) => (plugins || []).find((p) => p.id === id && declares(p, op));
    const note = text.match(/^note:\s*([\s\S]*)$/i);
    if (note) {
      const notes = find("notes", "create");
      if (notes && note[1].trim()) return { kind: "note", plugin: "notes", op: "create", args: { body: note[1].trim() } };
      if (notes) return { error: "Write the note after “note:”." };
    } else if (looksLikeUrl(text) && find("bookmarks", "create")) {
      return { kind: "link", plugin: "bookmarks", op: "create", args: { text } };
    }
    const todo = find("tasks", "create");
    if (!todo) return { error: "No app here can take this capture. Install Task manager, Notes or Bookmarks." };
    const args = { title: text };
    const id = dest?.id || "inbox";
    if (id !== "inbox" && id !== "my_day") args.list_id = id;
    return { kind: "task", plugin: "tasks", op: "create", args, plan: id === "my_day" && declares(todo, "update") };
  }

  // ---- Keyboard -----------------------------------------------------------

  /** Maps a keydown on Today to a command name, or null to leave it alone. */
  function keyCommand(event, { typing = false } = {}) {
    if (event.metaKey || event.ctrlKey || event.altKey) return null;
    if (event.key === "Escape") return "escape";
    if (typing) return null;
    switch (event.key) {
      case "j": case "J": case "ArrowDown": return "next";
      case "k": case "K": case "ArrowUp": return "prev";
      case "x": case "X": return "toggle";
      case "t": case "T": return "action:t";
      case "s": case "S": return "action:s";
      case "Enter": return "open";
      case "c": case "C": return "capture";
      default: return null;
    }
  }

  // ---- Preferences (per space, this device) --------------------------------

  function normalizePrefs(value) {
    const order = [];
    for (const id of Array.isArray(value?.order) ? value.order : []) if (ORDERABLE.includes(id) && !order.includes(id)) order.push(id);
    for (const id of ORDERABLE) if (!order.includes(id)) order.push(id);
    const hidden = [...new Set((Array.isArray(value?.hidden) ? value.hidden : []).filter((id) => SECTIONS.includes(id)))];
    const plugins = [...new Set((Array.isArray(value?.plugins) ? value.plugins : [])
      .filter((id) => typeof id === "string" && /^[A-Za-z0-9_.-]{1,64}$/.test(id)))].slice(0, 100);
    return { version: 1, order, hidden, plugins, density: value?.density === "comfortable" ? "comfortable" : "compact" };
  }

  function readPrefs(storage, space) {
    try { return normalizePrefs(JSON.parse(storage.getItem(PREFS_KEY + space) || "null")); }
    catch (_) { return normalizePrefs(null); }
  }

  function writePrefs(storage, space, prefs) {
    try { storage.setItem(PREFS_KEY + space, JSON.stringify(normalizePrefs(prefs))); return true; }
    catch (_) { return false; }
  }

  function moveSection(prefs, id, delta) {
    const next = normalizePrefs(prefs);
    const i = next.order.indexOf(id), to = i + delta;
    if (i >= 0 && to >= 0 && to < next.order.length) [next.order[i], next.order[to]] = [next.order[to], next.order[i]];
    return next;
  }

  function toggleIn(list, id) { return list.includes(id) ? list.filter((x) => x !== id) : [...list, id]; }
  function toggleSection(prefs, id) { return normalizePrefs({ ...prefs, hidden: toggleIn(prefs.hidden, id) }); }
  function togglePlugin(prefs, id) { return normalizePrefs({ ...prefs, plugins: toggleIn(prefs.plugins, id) }); }

  // ---- View ---------------------------------------------------------------

  /**
   * Mounts Today into `root`. `deps` supplies the shell: `el`, `icon`,
   * `toast`, `plugins()`, `space()`, `storage`, `query(plugin, name, params)`,
   * `action(plugin, op, args)`, `open(plugin, {item?, state?})`,
   * `openExternal(url)`, `changed()`, `visible()`, `now()`, optional
   * `recorder(node)` (mounts Voice's recorder, returns a view with flush/dispose),
   * `menu` (`openDashboard()`, `pinned()`, `togglePin()`, `weeklyReview()`, `editHome()`), and
   * optional `detail()` (the record shown docked beside Today: {plugin, item}
   * or null) with `closeDetail()`.
   */
  function mount(rootNode, deps) {
    const { el, icon, toast } = deps;
    const now = () => (deps.now ? deps.now() : Date.now());
    const state = {
      seq: 0, sections: null, errors: [], loading: true, prefs: readPrefs(deps.storage, deps.space()),
      space: deps.space(), folds: {}, expanded: new Set(), dest: { id: "inbox", label: "Inbox" },
      focus: null, recorder: null, busy: new Set(), disposed: false,
    };
    const items = new Map(); // DOM id -> {row, node}, in keyboard order

    const title = el("h1", { class: "today-title", id: "today-title" });
    const summaryLine = el("p", { class: "today-summary", "aria-live": "polite", id: "today-summary" });
    const customizeButton = el("button", { type: "button", class: "icon-button today-head-button", "aria-label": "Customize Home", title: "Customize Home", "aria-expanded": "false", "aria-controls": "today-customize", onclick: () => toggleCustomize() }, icon("sliders"));
    const menuButton = el("button", { type: "button", class: "icon-button today-head-button", "aria-label": "More Home actions", title: "More", "aria-haspopup": "menu", "aria-expanded": "false" }, icon("more"));
    const menu = el("div", { class: "today-menu", role: "menu", hidden: true });
    const customize = el("div", { class: "today-customize", id: "today-customize", role: "region", "aria-label": "Customize Home", hidden: true });
    const input = el("input", { class: "today-capture-input", id: "today-capture-input", type: "text", autocomplete: "off", "aria-label": "Capture a task, link or note", placeholder: "Capture anything: “call Sam 6pm #work”, a link, “note: …”", "aria-keyshortcuts": "C" });
    const destButton = el("button", { type: "button", class: "today-dest", "aria-haspopup": "menu", "aria-expanded": "false", title: "Where new tasks go" });
    const destMenu = el("div", { class: "today-menu today-dest-menu", role: "menu", hidden: true });
    const micButton = el("button", { type: "button", class: "icon-button today-mic", "aria-label": "Record a voice capture", title: "Record a voice capture", "aria-expanded": "false", hidden: true }, icon("mic"));
    const recorderBox = el("div", { class: "today-recorder", hidden: true });
    const form = el("form", { class: "today-capture", role: "search", "aria-label": "Capture" },
      el("span", { class: "today-capture-plus", "aria-hidden": "true" }, icon("plus")), input,
      el("span", { class: "today-dest-wrap" }, destButton, destMenu),
      el("kbd", { class: "today-kbd", "aria-hidden": "true" }, "C"), micButton);
    const body = el("div", { class: "today-sections", "aria-busy": "true" });
    rootNode.replaceChildren(el("div", { class: "today-col" },
      el("header", { class: "today-head" },
        el("div", { class: "today-heading" }, title, summaryLine),
        el("span", { class: "today-spacer" }),
        customizeButton, el("span", { class: "today-menu-wrap" }, menuButton, menu)),
      customize, form, recorderBox, body));

    // -- small menus
    function closeMenus(except) {
      for (const [button, box] of [[menuButton, menu], [destButton, destMenu]]) {
        if (box === except) continue;
        box.hidden = true;
        button.setAttribute("aria-expanded", "false");
      }
    }
    function openMenu(button, box, entries) {
      closeMenus(box);
      if (!box.hidden) { box.hidden = true; button.setAttribute("aria-expanded", "false"); return; }
      box.replaceChildren(...entries.map((e) => el("button", { type: "button", role: "menuitem", class: e.current ? "is-current" : "", onclick: () => { closeMenus(); e.run(); } }, e.label)));
      box.hidden = false;
      button.setAttribute("aria-expanded", "true");
      box.querySelector("button")?.focus();
    }
    menuButton.addEventListener("click", () => {
      const m = deps.menu || {};
      openMenu(menuButton, menu, [
        m.openDashboard && { label: "Open Dashboard", run: () => m.openDashboard() },
        m.togglePin && { label: m.pinned?.() ? "Unpin Dashboard" : "Pin Dashboard", run: () => m.togglePin() },
        m.weeklyReview && { label: "New weekly review", run: () => m.weeklyReview() },
        m.editHome && { label: "Edit Dashboard", run: () => m.editHome() },
      ].filter(Boolean));
    });
    const outside = (event) => {
      if (!event.target?.closest?.(".today-menu-wrap, .today-dest-wrap")) closeMenus();
    };
    root.document?.addEventListener?.("pointerdown", outside);

    // -- capture
    function destLabel() { return state.dest.label || "Inbox"; }
    function drawDest() {
      const todo = (deps.plugins() || []).find((p) => p.id === "tasks" && declares(p, "create"));
      const route = routeCapture(input.value, deps.plugins(), state.dest);
      const elsewhere = route && !route.error && route.kind !== "task";
      destButton.hidden = !todo && !elsewhere;
      destButton.disabled = elsewhere;
      destButton.textContent = elsewhere ? `→ ${route.kind === "link" ? "Bookmarks" : "Notes"}` : `→ ${destLabel()} ▾`;
      destButton.setAttribute("aria-label", elsewhere ? `Saves to ${route.kind === "link" ? "Bookmarks" : "Notes"}` : `New tasks go to ${destLabel()}. Change destination`);
      micButton.hidden = !deps.recorder || !(deps.plugins() || []).some((p) => p.id === "voice" && declares(p, "capture_audio"));
    }
    input.addEventListener("input", drawDest);
    destButton.addEventListener("click", async () => {
      let lists = [];
      try { lists = await deps.query("tasks", "list_choices", {}); } catch (_) {}
      if (state.disposed) return;
      const choose = (dest) => { state.dest = dest; drawDest(); input.focus(); };
      openMenu(destButton, destMenu, [
        { label: "Inbox", current: state.dest.id === "inbox", run: () => choose({ id: "inbox", label: "Inbox" }) },
        { label: "My Day", current: state.dest.id === "my_day", run: () => choose({ id: "my_day", label: "My Day" }) },
        ...(Array.isArray(lists) ? lists : []).filter((l) => l && l.value != null).map((l) => ({
          label: String(l.label ?? l.value), current: state.dest.id === String(l.value), run: () => choose({ id: String(l.value), label: String(l.label ?? l.value) }),
        })),
      ]);
    });
    form.addEventListener("submit", async (event) => {
      event.preventDefault();
      const route = routeCapture(input.value, deps.plugins(), state.dest);
      if (!route) return;
      if (route.error) { toast(route.error, "error"); return; }
      input.disabled = true;
      try {
        const result = await deps.action(route.plugin, route.op, route.args);
        if (route.plan && result?.id != null) await deps.action(route.plugin, "update", { id: String(result.id), planned_for: context(now()).$today });
        input.value = "";
        const where = route.kind === "task" ? destLabel() : route.kind === "link" ? "Bookmarks" : "Notes";
        const message = typeof result?.message === "string" ? result.message : `Saved to ${where}`;
        toast(message, "ok", result?.id != null ? { label: "Open", run: () => deps.open(route.plugin, { item: String(result.id) }) } : null);
        deps.changed();
      } catch (e) {
        toast(String(e), "error");
      } finally {
        input.disabled = false;
        drawDest();
        if (deps.visible()) input.focus();
      }
    });
    micButton.addEventListener("click", async () => {
      if (!recorderBox.hidden) {
        if (state.recorder && await state.recorder.flush?.({ unmount: true }) === false) {
          toast("Stop and save your recording first.", "error"); return;
        }
        state.recorder?.dispose?.(); state.recorder = null;
        recorderBox.hidden = true; recorderBox.replaceChildren();
        micButton.setAttribute("aria-expanded", "false");
        return;
      }
      recorderBox.hidden = false;
      micButton.setAttribute("aria-expanded", "true");
      state.recorder = deps.recorder(recorderBox);
    });

    // -- customize
    function savePrefs(next) {
      state.prefs = next;
      if (!writePrefs(deps.storage, state.space, next)) toast("Today changed for this session; device storage is unavailable.", "error");
      drawCustomize();
      refresh();
    }
    function toggleCustomize(open = customize.hidden) {
      customize.hidden = !open;
      customizeButton.setAttribute("aria-expanded", String(open));
      if (open) { drawCustomize(); customize.querySelector("input, button")?.focus(); }
    }
    function drawCustomize() {
      if (customize.hidden) return;
      const p = state.prefs;
      const sectionRow = (id, index, orderable) => el("li", { class: "today-custom-row" },
        el("label", { class: "pp-checkbox" }, el("input", { type: "checkbox", checked: !p.hidden.includes(id), onchange: () => savePrefs(toggleSection(state.prefs, id)) }), ` ${LABELS[id]}`),
        orderable ? el("span", { class: "today-custom-move" },
          el("button", { type: "button", disabled: index === 0, "aria-label": `Move ${LABELS[id]} up`, onclick: () => savePrefs(moveSection(state.prefs, id, -1)) }, "Up"),
          el("button", { type: "button", disabled: index === p.order.length - 1, "aria-label": `Move ${LABELS[id]} down`, onclick: () => savePrefs(moveSection(state.prefs, id, 1)) }, "Down")) : null);
      const providers = pluginOrder(deps.plugins()).filter((plugin) => (plugin.today || []).length);
      const density = el("select", { class: "pp-input", "aria-label": "Row density", onchange: (e) => savePrefs(normalizePrefs({ ...state.prefs, density: e.target.value })) },
        el("option", { value: "compact" }, "Compact"), el("option", { value: "comfortable" }, "Comfortable"));
      density.value = p.density;
      customize.replaceChildren(
        el("div", { class: "today-custom-head" }, el("h2", {}, "Customize Home"), el("button", { type: "button", onclick: () => { toggleCustomize(false); customizeButton.focus(); } }, "Done")),
        el("p", { class: "muted" }, "Saved for this space on this device."),
        el("h3", {}, "Sections"),
        el("ul", { class: "today-custom-list" }, ...p.order.map((id, i) => sectionRow(id, i, true)), sectionRow("earlier", 0, false)),
        el("h3", {}, "Apps"),
        providers.length ? el("ul", { class: "today-custom-list" }, ...providers.map((plugin) => el("li", { class: "today-custom-row" },
          el("label", { class: "pp-checkbox" }, el("input", { type: "checkbox", checked: !p.plugins.includes(plugin.id), onchange: () => savePrefs(togglePlugin(state.prefs, plugin.id)) }), ` Show rows from ${plugin.name || plugin.id}`))))
          : el("p", { class: "muted" }, "No installed app adds rows to Today yet."),
        el("label", { class: "today-custom-density" }, "Rows ", density));
    }

    // -- actions
    async function run(row, action, extra = {}) {
      if (!action || state.disposed) return false;
      const scope = { ...row.scope, ...extra };
      if (action.navigate) {
        await deps.open(row.plugin, resolve(action.navigate, scope) || {});
        return true;
      }
      if (action.url) {
        const url = String(resolve(action.url, scope) || "");
        if (/^https?:\/\//i.test(url)) deps.openExternal(url);
        if (!action.op) return true;
      }
      if (!action.op) return false;
      try {
        const result = await deps.action(row.plugin, action.op, resolve(action.args || {}, scope));
        const after = { ...scope, result };
        const f = action.followup;
        const offer = f && (f.if === undefined || truthy(resolve(f.if, after)));
        const undo = action.undo ? { label: "Undo", run: () => run(row, action.undo, { result }) } : null;
        const follow = offer ? { label: String(resolve(f.label, after)), run: () => run(row, f.action, { result }) } : null;
        if (action.success) toast(String(resolve(action.success, after)), "ok", undo || follow);
        else if (follow) toast(typeof result?.message === "string" ? result.message : "Done", "ok", follow);
        deps.changed();
        return true;
      } catch (e) {
        if (!state.disposed) toast(String(e), "error");
        return false;
      }
    }
    async function toggle(row) {
      if (!row?.check || state.busy.has(row.id)) return;
      state.busy.add(row.id);
      row.done = !row.done;
      draw();
      const ok = await run(row, row.check, { value: row.done });
      state.busy.delete(row.id);
      if (!ok) { row.done = !row.done; draw(); }
    }
    async function runLink(section, part) {
      const link = part.source.link;
      if (!link.each) return run({ plugin: part.plugin.id, scope: context(now()) }, link.action);
      const rows = section.rows.filter((r) => r.source === part.source.id && r.plugin === part.plugin.id && (link.each === "all" || r.tone === link.each));
      let done = 0;
      for (const row of rows) {
        try { await deps.action(row.plugin, link.action.op, resolve(link.action.args || {}, row.scope)); done++; }
        catch (e) { toast(String(e), "error"); break; }
      }
      if (done) toast(`${link.label}: ${done} ${done === 1 ? "item" : "items"}`);
      deps.changed();
    }
    function linkFor(section) {
      for (const part of section.parts) {
        const link = part.source.link;
        if (!link) continue;
        if (link.each && !section.rows.some((r) => r.source === part.source.id && (link.each === "all" || r.tone === link.each))) continue;
        return el("button", { type: "button", class: "link-button today-link", onclick: () => runLink(section, part) }, link.label);
      }
      return null;
    }

    // -- drawing
    function lead(row) {
      if (row.check) {
        const habit = row.kind === "habit";
        return el("button", {
          type: "button", role: "checkbox", class: `today-check${habit ? " today-ring" : ""}`, "aria-checked": String(row.done),
          "aria-label": `${row.done ? "Mark not done" : "Mark done"}: ${row.title}`, tabindex: "-1",
          onclick: (e) => { e.stopPropagation(); toggle(row); },
        });
      }
      return el("span", { class: "today-kind", "aria-hidden": "true" }, icon(KIND_ICONS[row.kind] || "list"));
    }
    function item(row, node) {
      const domId = `today-item-${items.size}`;
      items.set(domId, { row, node });
      node.id = domId;
      node.dataset.todayItem = "";
      node.tabIndex = state.focus === row.id ? 0 : -1;
      node.addEventListener("focusin", () => { state.focus = row.id; });
      node.addEventListener("click", (e) => { if (e.target === node) node.focus(); });
      return node;
    }
    function rowNode(row) {
      const text = el("button", { type: "button", class: "today-open", tabindex: "-1", disabled: !row.open, onclick: () => row.open && run(row, row.open) },
        el("span", { class: "today-row-title" }, row.title), row.note ? el("small", { class: "today-note" }, row.note) : null);
      const meta = el("span", { class: "today-meta" },
        row.time ? el("span", { class: `today-time${row.tone === "overdue" ? " is-overdue" : ""}` }, row.time) : null,
        ...row.chips.map((c) => el("span", { class: c.tone === "accent" ? "today-tag" : "today-chip" }, c.text)));
      const actions = el("span", { class: "today-actions" }, ...row.actions.map((a) => el("button", {
        type: "button", tabindex: "-1", title: a.key ? `${a.label} (${a.key.toUpperCase()})` : a.label,
        "aria-keyshortcuts": a.key ? a.key.toUpperCase() : false, onclick: (e) => { e.stopPropagation(); run(row, a.action); },
      }, a.label)));
      const description = [row.time, ...row.chips.map((c) => c.text), row.pluginName].filter(Boolean).join(", ");
      return item(row, el("div", {
        class: `today-row${row.done ? " is-done" : ""}${row.tone === "overdue" ? " is-overdue" : ""}`, role: "listitem",
        "aria-label": `${row.title}${row.note ? `, ${row.note}` : ""}${description ? `, ${description}` : ""}`,
      }, lead(row), text, meta, row.actions.length ? actions : null));
    }
    function habitNode(row) {
      return item(row, el("div", { class: `today-habit${row.done ? " is-done" : ""}`, role: "listitem", "aria-label": `${row.title}${row.note ? `, ${row.note}` : ""}` },
        lead(row),
        el("button", { type: "button", class: "today-open", tabindex: "-1", disabled: !row.open, onclick: () => row.open && run(row, row.open) },
          el("span", { class: "today-row-title" }, row.title), row.note ? el("small", { class: "today-note" }, row.note) : null)));
    }
    function rowsOf(section) {
      const all = section.rows;
      const shown = state.expanded.has(section.section) ? all : all.filter((r) => !r.extra);
      const nodes = [];
      const at = section.section === "due" ? nowIndex(shown, now()) : -1;
      shown.forEach((row, i) => {
        if (i === at) nodes.push(nowLine());
        nodes.push(rowNode(row));
      });
      if (at === shown.length && at > 0) nodes.push(nowLine());
      const list = el("div", { class: "today-rows", role: "list", "aria-label": LABELS[section.section] }, ...nodes);
      const more = section.total - shown.length;
      const out = [list];
      if (more > 0 && !state.expanded.has(section.section) && all.length > shown.length) {
        out.push(el("button", { type: "button", class: "today-fold", "aria-expanded": "false", onclick: () => { state.expanded.add(section.section); draw(); } }, `▸ ${more} more`));
      } else if (more > 0) {
        const part = section.parts.find((p) => p.source.link && !p.source.link.each);
        out.push(part ? el("button", { type: "button", class: "today-fold", onclick: () => runLink(section, part) }, `${more} more in ${part.plugin.name || part.plugin.id}`)
          : el("p", { class: "today-fold" }, `${more} more`));
      }
      return out;
    }
    function nowLine() {
      const d = new Date(now());
      return el("div", { class: "today-now", role: "separator", "aria-label": `Now, ${pad(d.getHours())}:${pad(d.getMinutes())}` }, `now ${pad(d.getHours())}:${pad(d.getMinutes())}`);
    }
    function head(section, extra = null) {
      return el("h2", { class: `today-section-head${section.section === "due" && section.rows.some((r) => r.tone === "overdue") ? " has-overdue" : ""}` },
        el("span", {}, LABELS[section.section]), el("span", { class: "today-count" }, countText(section)), extra || linkFor(section));
    }
    function sectionNode(id) {
      const section = state.sections[id];
      const node = el("section", { class: "today-section", "data-section": id, "aria-label": LABELS[id] });
      if (id === "wrap_up" || id === "done") {
        const isFolded = folded(id, now(), state.folds);
        const toggleFold = el("button", { type: "button", class: "today-fold today-fold-head", "aria-expanded": String(!isFolded), onclick: () => { state.folds[id] = !isFolded; draw(); } },
          `${isFolded ? "▸" : "▾"} ${LABELS[id]} · ${countText(section)}`);
        node.append(toggleFold);
        if (!isFolded) node.append(...rowsOf(section));
        return node;
      }
      if (id === "habits") {
        node.append(head(section), el("div", { class: "today-habits", role: "list", "aria-label": "Habits" }, ...section.rows.map(habitNode)));
        return node;
      }
      node.append(head(section));
      if (id === "my_day" && !section.rows.length) {
        node.append(el("p", { class: "today-empty" }, "Nothing planned for today."));
      } else {
        node.append(...rowsOf(section));
      }
      const earlier = state.sections.earlier;
      if (id === "my_day" && earlier && !state.prefs.hidden.includes("earlier") && earlier.rows.length) {
        node.append(el("div", { class: "today-subhead" }, el("span", {}, `${LABELS.earlier} · ${earlier.total}`), linkFor(earlier)), ...rowsOf(earlier));
      }
      return node;
    }
    function draw() {
      if (state.disposed) return;
      const focused = !!body.contains?.(root.document?.activeElement);
      title.textContent = new Date(now()).toLocaleDateString(undefined, { weekday: "long", day: "numeric", month: "long" });
      rootNode.dataset.density = state.prefs.density;
      drawDest();
      if (!state.sections) {
        body.replaceChildren(el("p", { class: "muted", role: "status" }, state.loading ? "Loading Today…" : ""));
        return;
      }
      items.clear();
      const ids = visibleSections(state.sections, state.prefs);
      summaryLine.textContent = summary(state.sections, state.prefs);
      const nodes = ids.map(sectionNode);
      if (!(deps.plugins() || []).some((p) => (p.today || []).length)) {
        nodes.push(el("p", { class: "today-empty" }, "Install To-do, Habits, Voice, Bookmarks or Feeds to fill Today."));
      }
      if (state.errors.length) nodes.push(el("details", { class: "today-errors" }, el("summary", {}, `Some rows couldn’t load (${state.errors.length})`), el("ul", {}, ...state.errors.map((e) => el("li", {}, e)))));
      body.replaceChildren(...nodes);
      body.setAttribute("aria-busy", "false");
      const list = [...items.values()];
      if (list.length && !list.some((entry) => entry.row.id === state.focus)) list[0].node.tabIndex = 0;
      if (focused) focusRow(state.focus);
      selected();
    }

    // -- data
    async function load(entry, ctx) {
      const { plugin, source } = entry;
      const params = resolve(source.params || {}, ctx);
      const rows = await deps.query(plugin.id, source.query, params);
      let total = Array.isArray(rows) ? rows.length : 0;
      if (source.count_query) {
        try {
          const counted = await deps.query(plugin.id, source.count_query, params);
          const n = Number(counted?.[0]?.count);
          if (Number.isFinite(n)) total = n;
        } catch (_) { /* the rows still show */ }
      }
      return { plugin, source, total, rows: (Array.isArray(rows) ? rows : []).map((raw, i) => toRow(plugin, source, raw, ctx, i)).filter((r) => r.key && r.title) };
    }
    async function refresh() {
      if (state.disposed) return;
      if (state.space !== deps.space()) {
        state.space = deps.space();
        state.prefs = readPrefs(deps.storage, state.space);
        state.sections = null; state.focus = null; state.expanded.clear(); state.folds = {};
        state.dest = { id: "inbox", label: "Inbox" };
      }
      const seq = ++state.seq;
      const ctx = context(now());
      const { sources, errors } = sourcesOf(deps.plugins(), state.prefs);
      const results = await Promise.all(sources.map((entry) => load(entry, ctx).catch((e) => {
        errors.push(`${entry.plugin.name || entry.plugin.id}: ${String(e?.message || e)}`);
        return { ...entry, rows: [], total: 0 };
      })));
      if (seq !== state.seq || state.disposed) return;
      state.loading = false;
      state.errors = errors;
      state.sections = merge(results);
      draw();
    }

    // -- keyboard
    function focusRow(rowId) {
      const entry = [...items.values()].find((e) => e.row.id === rowId);
      if (!entry) return;
      for (const other of items.values()) other.node.tabIndex = -1;
      entry.node.tabIndex = 0;
      entry.node.focus();
    }
    function currentRow() {
      const holder = root.document?.activeElement?.closest?.("[data-today-item]");
      return (holder && items.get(holder.id)?.row) || [...items.values()].find((e) => e.row.id === state.focus)?.row || null;
    }
    function move(delta) {
      const list = [...items.values()].map((e) => e.row);
      if (!list.length) return;
      const i = list.findIndex((r) => r.id === currentRow()?.id);
      const next = list[i < 0 ? (delta > 0 ? 0 : list.length - 1) : Math.max(0, Math.min(list.length - 1, i + delta))];
      state.focus = next.id;
      focusRow(next.id);
      // With a record docked beside Today, J/K show the next row's record.
      if (deps.detail?.() && openTarget(next)?.item) run(next, next.open);
    }
    // The record a row opens, when its `open` navigates to an item.
    function openTarget(row) {
      if (!row?.open?.navigate) return null;
      const target = resolve(row.open.navigate, row.scope) || {};
      return target.item != null && target.item !== "" ? { plugin: row.plugin, item: String(target.item) } : null;
    }
    // Highlights the row whose record is docked.
    function selected() {
      const shown = deps.detail?.() || null;
      for (const { row, node } of items.values()) {
        const target = shown && openTarget(row);
        const on = Boolean(target && target.plugin === shown.plugin && target.item === shown.item);
        node.classList?.toggle("is-selected", on);
        if (on) node.setAttribute("aria-current", "true");
        else node.removeAttribute?.("aria-current");
      }
    }
    function handle(command, event) {
      if (command === "escape") {
        if (!customize.hidden) { toggleCustomize(false); customizeButton.focus(); return true; }
        if (!menu.hidden || !destMenu.hidden) { closeMenus(); return true; }
        if (root.document?.activeElement === input) { input.blur(); return true; }
        if (deps.detail?.()) { deps.closeDetail?.(); return true; }
        return false;
      }
      if (command === "capture") { input.focus(); return true; }
      if (command === "next") { move(1); return true; }
      if (command === "prev") { move(-1); return true; }
      const row = currentRow();
      if (!row) return false;
      if (command === "toggle") { if (row.check) toggle(row); return !!row.check; }
      if (command === "open") {
        if (event?.target?.closest?.("button") && !event.target.dataset?.todayItem) return false;
        if (row.open) run(row, row.open);
        return !!row.open;
      }
      const key = command.startsWith("action:") ? command.slice(7) : null;
      const action = key && row.actions.find((a) => a.key === key);
      if (action) { run(row, action.action); return true; }
      return false;
    }
    const onKey = (event) => {
      if (state.disposed || event.defaultPrevented || !deps.visible()) return;
      // A docked record sheet does not block Today's keys; a modal one does.
      if (root.document?.querySelector?.("dialog[open]:not(.pp-sheet-docked)") || root.document?.querySelector?.(".overlay:not([hidden])")) return;
      const target = event.target;
      const typing = !!target && (/^(INPUT|TEXTAREA|SELECT)$/.test(target.tagName || "") || target.isContentEditable);
      const command = keyCommand(event, { typing });
      if (!command) return;
      if (command !== "escape" && command !== "capture" && target && !rootNode.contains?.(target) && target !== root.document?.body) return;
      if (handle(command, event)) event.preventDefault();
    };
    root.document?.addEventListener?.("keydown", onKey);

    draw();
    return {
      refresh,
      handle,
      focusCapture: () => input.focus(),
      selected,
      flush: async (options) => (state.recorder ? await state.recorder.flush?.(options) !== false : true),
      get state() { return state; },
      dispose() {
        state.disposed = true;
        state.recorder?.dispose?.();
        root.document?.removeEventListener?.("keydown", onKey);
        root.document?.removeEventListener?.("pointerdown", outside);
      },
    };
  }

  const api = {
    SECTIONS, ORDERABLE, LABELS, resolve, context, declares, validateSource, sourcesOf, toRow, merge,
    visibleSections, folded, nowIndex, countText, summary, looksLikeUrl, routeCapture, keyCommand,
    normalizePrefs, readPrefs, writePrefs, moveSection, toggleSection, togglePlugin, mount,
  };
  if (typeof module !== "undefined" && module.exports) module.exports = api;
  else root.TerrariumToday = api;
})(typeof window === "undefined" ? globalThis : window);
