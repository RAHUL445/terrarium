// App shell. Renders host data with DOM APIs only (no innerHTML), so
// stored values can never inject markup.
"use strict";

const invoke = window.__TAURI__ ? window.__TAURI__.core.invoke : null;
const profile = window.TerrariumPerformance;
const templateCache = new window.TerrariumTemplateCache();

const $ = (id) => document.getElementById(id);

function el(tag, attrs = {}, ...children) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs)) {
    if (key === "class") node.className = value;
    else if (key.startsWith("on")) node.addEventListener(key.slice(2), value);
    else if (value === true) node.setAttribute(key, "");
    else if (value !== false && value != null) node.setAttribute(key, value);
  }
  for (const child of children) {
    if (child == null || child === false) continue;
    node.append(child instanceof Node ? child : document.createTextNode(String(child)));
  }
  return node;
}

function showError(message) {
  const box = $("error");
  box.textContent = message || "";
  box.hidden = !message;
}

async function call(command, args) {
  if (!invoke) throw new Error("Not running inside the app");
  const run = () => profile.measure(`ipc/${command}`, () => invoke(command, args));
  const result = await (command === "view_template" ? templateCache.get(args, run) : run());
  if (command === "run_action") {
    const labels = { create: "Added an item", update: "Saved changes", delete: "Moved to trash", restore: "Restored from trash", toggle: "Task status changed", checkin: "Habit updated", capture_text: "Saved a capture", capture_audio: "Saved a recording", save_note: "Outcome saved" };
    const plugin = pluginList.plugins.find((p) => p.id === args?.plugin);
    rails?.record({ space: args?.space || currentSpace, title: `${plugin?.name || "App"} · ${labels[args?.op] || "Updated"}`, at: Date.now() });
  }
  return result;
}

async function act(command, args) {
  try {
    showError("");
    await call(command, args);
    await refresh();
  } catch (e) {
    showError(String(e));
  }
}

function shortHlc(hlc) {
  // wall(16 hex)-counter(8 hex)-device
  const [wall, counter] = hlc.split("-");
  return `${parseInt(wall, 16)}.${parseInt(counter, 16)}`;
}

function shortId(id) {
  return id.length > 12 ? `…${id.slice(-8)}` : id;
}

function describeOp(op) {
  switch (op.op) {
    case "create": return "create";
    case "set": return `set ${op.column} = ${JSON.stringify(op.value)}`;
    case "increment": return `increment ${op.column} by ${op.delta}`;
    case "delete": return "delete";
    case "restore": return "restore";
    default: return op.op;
  }
}

function renderItems(items) {
  const body = $("items");
  body.replaceChildren();
  if (items.length === 0) {
    body.append(el("tr", {}, el("td", { colspan: "5", class: "muted" }, "No items yet. Add one above.")));
    return;
  }
  for (const item of items) {
    const trashed = item._deleted != null;
    const checkbox = el("input", {
      type: "checkbox",
      "aria-label": `Done: ${item.title ?? ""}`,
      checked: item.done === 1,
      disabled: trashed,
      onchange: (e) => act("toggle_item", { id: item.id, done: e.target.checked }),
    });
    const actions = trashed
      ? [el("button", { type: "button", onclick: () => act("restore_item", { id: item.id }) }, "Restore")]
      : [
          el("button", { type: "button", onclick: () => act("bump_item", { id: item.id }) }, "+1 point"),
          el("button", { type: "button", onclick: () => act("delete_item", { id: item.id }) }, "Delete"),
        ];
    body.append(
      el("tr", { class: trashed ? "trashed" : "" },
        el("td", {}, checkbox),
        el("td", { class: "title" }, item.title ?? ""),
        el("td", { class: "mono" }, item.points ?? 0),
        el("td", {}, el("span", { class: trashed ? "state-trashed" : "state-live" }, trashed ? "In trash" : "Live")),
        el("td", { class: "actions" }, ...actions),
      ),
    );
  }
}

function renderList(id, entries, empty, format) {
  const list = $(id);
  list.replaceChildren();
  if (entries.length === 0) {
    list.append(el("li", {}, empty));
    return;
  }
  for (const entry of entries) list.append(format(entry));
}

function renderChanges(changes, device) {
  const body = $("changes");
  body.replaceChildren();
  $("log-count").textContent = `latest ${changes.length}`;
  for (const { lseq, change } of changes) {
    const origin = change.hlc.split("-").slice(2).join("-");
    body.append(
      el("tr", {},
        el("td", {}, lseq),
        el("td", {}, shortHlc(change.hlc)),
        el("td", {}, origin === device ? "this device" : origin),
        el("td", {}, `${change.table}/${shortId(change.row)}`),
        el("td", {}, describeOp(change.op)),
      ),
    );
  }
}

async function refresh() {
  const snap = await call("snapshot");
  if (!syncStatus) $("space-status").textContent = `Space: ${snap.space} · this device only`;
  renderItems(snap.items);
  renderList("conflicts", snap.conflicts, "No conflicts yet. Try “Simulate offline conflict”.", (c) =>
    el("li", {},
      el("strong", {}, `${c.table}/${shortId(c.row)} · ${c.column}`), " kept ",
      el("code", {}, JSON.stringify(c.kept_value)), ", overwritten ",
      el("code", {}, JSON.stringify(c.lost_value)), " (kept 30 days)"),
  );
  renderList("notices", snap.notices, "No trash notices.", (n) =>
    el("li", {}, el("strong", {}, `${n.table}/${shortId(n.row)}`), ` ${n.kind.replaceAll("_", " ")}`),
  );
  renderChanges(snap.changes, snap.device);
}

// ---- Plugins -------------------------------------------------------------

const views = { page: null, get widgets() { return pages ? pages.mounted() : []; } };
let pages = null;
let rails = null;
let pluginList = { plugins: [], removed: [], errors: [], dev: false };
let currentView = "home";
let settingsSection = "spaces";
let spaces = [];
let currentSpace = "personal";
try {
  currentSpace = localStorage.getItem("space") || "personal";
} catch (_) {}
let currentPage = null; // { plugin, page }
let sessionStorageSafe;
try { sessionStorageSafe = localStorage; } catch (_) { sessionStorageSafe = { getItem() { return null; }, setItem() {} }; }
const workspaceSession = new window.TerrariumSession(sessionStorageSafe);
const locationKey = (plugin, page) => `${currentSpace}/page/${plugin}/${page}`;
function rememberWorkspace() {
  if (currentPage && views.page) workspaceSession.save(locationKey(currentPage.plugin, currentPage.page), views.page);
  for (const view of views.widgets) if (view.sessionKey) workspaceSession.save(view.sessionKey, view);
  const bind = views.page?.template.entry?.bind?.replace(/^state\./, "");
  workspaceSession.write(currentSpace, { view: currentView, plugin: currentPage?.plugin, page: currentPage?.page,
    item: currentView === "plugin" && bind ? String(views.page.state[bind] || "") : null,
    settings: settingsSection, scroll: window.scrollY || $("content").scrollTop });
}
async function restoreWorkspace(location) {
  if (!location) return;
  if (location.settings && ["spaces", "devices", "backups", "appearance", "ai", "scheduled", "plugins", "developer"].includes(location.settings)) settingsSection = location.settings;
  const plugin = pluginList.plugins.find(p => p.id === location.plugin);
  if (location.view === "plugin" && plugin?.pages.some(p => p.id === location.page)) await openPage(location.plugin, location.page, { item: location.item || null });
  else await showView(location.view === "plugin" ? "home" : location.view);
  requestAnimationFrame(() => { window.scrollTo(0, location.scroll); $("content").scrollTop = location.scroll; });
}
window.addEventListener("pagehide", rememberWorkspace);

let searchReturn = null; // Search text and scope when opening a result.
let pageSeq = 0;
let navigated = false; // the user has picked a view themselves
let badges = {}; // plugin id -> count

if (/Android/.test(navigator.userAgent)) document.documentElement.classList.add("platform-android");

const phoneLayout = () => window.matchMedia("(max-width: 760px)").matches;

// Icons: stroke paths on a 24 px grid. Plugins name one in their manifest.
const ICONS = {
  check: ["m5 12 5 5 9-10"],
  note: ["M6 3h9l4 4v14H6z", "M9 13h7M9 17h5"],
  target: ["M12 4a8 8 0 1 0 0 16a8 8 0 1 0 0-16", "M12 8.5a3.5 3.5 0 1 0 0 7a3.5 3.5 0 1 0 0-7"],
  bookmark: ["M6 3h12v18l-6-4-6 4z"],
  chart: ["M6 20V11M12 20V5M18 20v-6"],
  mic: ["M12 3a3 3 0 0 0-3 3v5a3 3 0 0 0 6 0V6a3 3 0 0 0-3-3z", "M5 11a7 7 0 0 0 14 0M12 18v3"],
  calendar: ["M4 6h16v15H4z", "M4 10h16M8 3v4M16 3v4"],
  book: ["M4 4h6a2 2 0 0 1 2 2v14a2 2 0 0 0-2-2H4z", "M20 4h-6a2 2 0 0 0-2 2v14a2 2 0 0 1 2-2h6z"],
  star: ["m12 3 2.7 5.6 6.1.9-4.4 4.3 1 6.1L12 17l-5.4 2.9 1-6.1L3.2 9.5l6.1-.9z"],
  heart: ["M12 20s-7-4.4-7-10a4 4 0 0 1 7-2.6A4 4 0 0 1 19 10c0 5.6-7 10-7 10z"],
  folder: ["M3 6h6l2 2h10v11H3z"],
  inbox: ["M3 13h5l2 3h4l2-3h5", "M5 5h14l2 8v6H3v-6z"],
  link: ["M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1", "M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1"],
  clock: ["M12 3a9 9 0 1 0 0 18a9 9 0 1 0 0-18", "M12 7v5l3 2"],
  list: ["M9 6h11M9 12h11M9 18h11M4 6h.01M4 12h.01M4 18h.01"],
  home: ["M3 10.5 12 3l9 7.5V21h-6v-6H9v6H3z"],
  grid: ["M3 3h7v7H3z", "M14 3h7v7h-7z", "M3 14h7v7H3z", "M14 14h7v7h-7z"],
  "apps-plus": ["M3 3h7v7H3z", "M14 3h7v7h-7z", "M3 14h7v7H3z", "M17.5 14v7M14 17.5h7"],
  search: ["M11 4a7 7 0 1 0 0 14a7 7 0 1 0 0-14", "m20 20-3.5-3.5"],
  sliders: ["M4 7h10M18 7h2M4 17h4M12 17h8", "M16 5a2 2 0 1 0 0 4a2 2 0 1 0 0-4", "M10 15a2 2 0 1 0 0 4a2 2 0 1 0 0-4"],
  plus: ["M12 5v14M5 12h14"],
  "chevron-down": ["m6 9 6 6 6-6"],
  "chevron-up": ["m6 15 6-6 6 6"],
  "chevron-right": ["m9 6 6 6-6 6"],
  "panel-right": ["M3 4h18v16H3z", "M15 4v16"],
  "panel-left": ["M3 4h18v16H3z", "M9 4v16"],
  "chevron-left": ["m15 6-6 6 6 6"],
  layers: ["m12 3 9 5-9 5-9-5z", "m3 13 9 5 9-5"],
  copy: ["M9 9h11v11H9z", "M5 15H4V4h11v1"],
  trash: ["M4 7h16", "M10 11v6M14 11v6", "M6 7l1 13h10l1-13", "M9 7V4h6v3"],
  grip: ["M9 6h.01M15 6h.01M9 12h.01M15 12h.01M9 18h.01M15 18h.01"],
  sync: ["M20 11a8 8 0 0 0-14.9-3M4 13a8 8 0 0 0 14.9 3", "M5 3v5h5M19 21v-5h-5"],
  archive: ["M3 4h18v5H3z", "M5 9v10h14V9M10 13h4"],
  code: ["m8 8-4 4 4 4M16 8l4 4-4 4"],
  arrow: ["M5 12h14M13 6l6 6-6 6"],
  bolt: ["M13 3 5 14h6l-1 7 8-11h-6z"],
  sparkle: ["M12 3l1.9 5.1L19 10l-5.1 1.9L12 17l-1.9-5.1L5 10l5.1-1.9z", "M19 15l.8 2.2L22 18l-2.2.8L19 21l-.8-2.2L16 18l2.2-.8z"],
  palette: ["M12 3a9 9 0 1 0 0 18c1.1 0 2-.9 2-2 0-.5-.2-1-.5-1.3-.3-.4-.5-.8-.5-1.3 0-1.1.9-2 2-2h2.3A4.7 4.7 0 0 0 21 9.7C21 6 17 3 12 3z", "M7.5 11.5h.01M10 7.5h.01M15 7.5h.01"],
};

const SVG = "http://www.w3.org/2000/svg";

function drawIcon(svg, name) {
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("fill", "none");
  svg.setAttribute("stroke", "currentColor");
  svg.setAttribute("stroke-width", "2");
  svg.setAttribute("stroke-linecap", "round");
  svg.setAttribute("stroke-linejoin", "round");
  svg.setAttribute("aria-hidden", "true");
  svg.replaceChildren(...(ICONS[name] || []).map((d) => {
    const path = document.createElementNS(SVG, "path");
    path.setAttribute("d", d);
    return path;
  }));
  return svg;
}

function icon(name) {
  const svg = document.createElementNS(SVG, "svg");
  svg.classList.add("icon");
  return drawIcon(svg, name);
}

for (const svg of document.querySelectorAll("svg[data-icon]")) drawIcon(svg, svg.dataset.icon);

const FALLBACK_COLORS = ["#4338CA", "#0F766E", "#BE185D", "#B45309", "#6D28D9", "#2563EB", "#15803D"];

function colorFor(key) {
  let h = 0;
  for (const c of String(key)) h = (h * 31 + c.charCodeAt(0)) >>> 0;
  return FALLBACK_COLORS[h % FALLBACK_COLORS.length];
}

/** Shared app identity; appearance is controlled by the device preference. */
function appTile(plugin, size = "md") {
  const tile = el("span", { class: `app-tile app-tile-${size}`, "aria-hidden": "true" });
  tile.style.setProperty("--app-color", plugin.color || colorFor(plugin.id));
  if (plugin.icon && ICONS[plugin.icon]) tile.append(icon(plugin.icon));
  else tile.append((plugin.name || "?").slice(0, 1).toUpperCase());
  return tile;
}

function spaceTile(space, className = "space-tile") {
  const words = (space.name || "?").trim().split(/\s+/u);
  const initials = (words.length > 1 ? [words[0], words[words.length - 1]] : words)
    .map((word) => Array.from(word)[0] || "?").join("").toLocaleUpperCase();
  return el("span", { class: className, "aria-hidden": "true" }, el("span", { class: "space-initials" }, initials));
}

function localToday() {
  const d = new Date();
  const pad = (n) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

function localWeekday() {
  return (new Date().getDay() + 6) % 7;
}

function toast(message, kind = "ok", action = null) {
  const box = el("div", { class: `toast toast-${kind}` }, el("span", {}, message));
  if (action) box.append(el("button", {
    type: "button", "aria-label": `${action.label}: ${message}`,
    onclick: async (event) => {
      event.currentTarget.disabled = true;
      const ok = await action.run();
      if (ok !== false) box.remove();
      else event.target.disabled = false;
    },
  }, action.label));
  const area = $("toasts");
  area.append(box);
  // A popover sits in the top layer, so toasts show above open sheets too;
  // re-showing it puts it above the newest one.
  if (area.showPopover) {
    if (area.matches(":popover-open")) area.hidePopover();
    area.showPopover();
  }
  setTimeout(() => {
    box.remove();
    if (!area.children.length && area.matches?.(":popover-open")) area.hidePopover();
  }, action ? 10000 : kind === "error" ? 6000 : 2500);
}

function bridge(plugin) {
  const space = currentSpace;
  return {
    query: (name, params) => call("run_query", { space, plugin, name, params }),
    action: (op, args) => call("run_action", { space, plugin, op, args }),
    toast,
    aiStatus: async () => {
      const state = await call("service", { name: "ai_settings" });
      const provider = state.presets.find((p) => p.id === state.config.preset)?.name || "Custom";
      let host = "";
      try { host = new URL(state.config.base_url).host; } catch (_) {}
      return { ready: state.status.ready, reason: state.status.reason, provider, host, model: state.config.model };
    },
    today: localToday,
    open: (url) => call("open_external", { url }).catch((e) => toast(String(e), "error")),
    openItem: (item) => {
      if (space !== currentSpace) return;
      rails?.closeDrawer();
      const page = pluginList.plugins.find((p) => p.id === plugin)?.pages[0];
      if (page) return openPage(plugin, page.id, { item });
    },
    // Any successful action may change data shown elsewhere.
    changed: () => refreshPluginViews(),
  };
}

function refreshPluginViews() {
  if (views.page) views.page.refresh();
  for (const view of views.widgets) view.refresh();
  rails?.refresh();
  loadBadges();
}

let badgeSeq = 0;
async function loadBadges() {
  const seq = ++badgeSeq, space = currentSpace;
  const next = {};
  await Promise.all(pluginList.plugins.filter((p) => p.badge).map(async (p) => {
    try {
      const rows = await call("run_query", {
        space, plugin: p.id, name: p.badge,
        params: { today: localToday(), weekday: localWeekday() },
      });
      const count = Number(rows[0]?.count) || 0;
      if (count > 0) next[p.id] = count;
    } catch (_) {}
  }));
  if (seq !== badgeSeq || space !== currentSpace) return;
  badges = next;
  renderAppNav();
  renderApps();
}

// Desktop chrome reuses manifest quick actions and the host command palette.
function renderWorkspace() {
  const plugin = currentView === "plugin" ? pluginList.plugins.find(p => p.id === currentPage?.plugin) : null;
  const page = plugin?.pages.find(p => p.id === currentPage.page);
  const desktopTitle = !!plugin || currentView === "apps";
  if (desktopTitle) {
    $("workspace-context").setAttribute("role", "heading");
    $("workspace-context").setAttribute("aria-level", "1");
  } else {
    $("workspace-context").removeAttribute("role");
    $("workspace-context").removeAttribute("aria-level");
  }
  $("workspace-context").textContent = plugin ? `${plugin.name}${plugin.pages.length > 1 && page ? ` / ${page.title}` : ""}` : currentView === "home" ? pages?.title() || "Home" : currentView === "settings" ? `Settings / ${settingsSection[0].toUpperCase() + settingsSection.slice(1)}` : "Apps";
  const actions = plugin?.quick_actions || [];
  const make = (action, primary) => el("button", { type: "button", class: primary ? "workspace-primary" : "", onclick: () => {
    $("workspace-actions").querySelector("details")?.removeAttribute("open");
    openPalette({ action: { plugin, action } });
  } }, action.label);
  const nodes = actions.length ? [make(actions[0], true)] : currentView === "home" ? [el("button", { type: "button", class: "workspace-primary", onclick: () => openPalette({ mode: "add" }) }, icon("plus"), "Quick add")] : [];
  if (actions.length > 1) nodes.push(el("details", { class: "workspace-overflow" }, el("summary", { "aria-label": "More page actions" }, "More"), el("div", {}, ...actions.slice(1).map(a => make(a, false)))));
  $("workspace-actions").replaceChildren(...nodes);
  updateWorkspaceStatus();
}
let backgroundStatus = { space: null, active: 0, failed: 0, unavailable: false };
let backgroundLoading = false;
async function refreshBackgroundStatus() {
  if (backgroundLoading || document.hidden) return;
  const space = currentSpace;
  backgroundLoading = true;
  try {
    const data = await call("service", { name: "schedules" });
    if (space === currentSpace) backgroundStatus = { space, ...window.TerrariumStatus.summarize(data.runs, space), unavailable: false };
  } catch (_) {
    if (space === currentSpace) backgroundStatus = { space, active: 0, failed: 0, unavailable: true };
  } finally { backgroundLoading = false; updateWorkspaceStatus(); }
}
function updateWorkspaceStatus() {
  const put = (id, text) => { if ($(id).textContent !== text) $(id).textContent = text; };
  const status = backgroundStatus.space === currentSpace ? backgroundStatus : { active: 0, failed: 0 };
  const unsaved = [...document.querySelectorAll('form[data-pp-unsaved="true"]')].some(form => form.getClientRects().length);
  put("workspace-work", document.querySelector('.pp-recorder[data-state="recording"]') ? "Recording…" : document.querySelector('form[aria-busy="true"]') ? "Saving…" : unsaved ? "Unsaved changes" : "");
  const ai = rails?.conversations.get(currentSpace)?.pending ? 1 : 0;
  const jobs = [status.active ? `${status.active} ${status.active === 1 ? "job" : "jobs"} in progress` : "", ai ? "AI reply pending" : ""].filter(Boolean).join(" · ");
  put("workspace-jobs", jobs);
  $("workspace-jobs").title = jobs;
  $("workspace-jobs").hidden = !jobs;
  const issues = pluginList.errors.length;
  const health = [issues ? `${issues} plugin ${issues === 1 ? "issue" : "issues"}` : "", status.failed ? `${status.failed} failed ${status.failed === 1 ? "job" : "jobs"}` : "", status.unavailable ? "Activity unavailable" : ""].filter(Boolean).join(" · ");
  put("workspace-health", health);
  $("workspace-health").title = health;
  $("workspace-health").hidden = !health;
  $("workspace-health").dataset.state = health ? "error" : "ok";
  const foot = currentView === "plugin" && $("app-body").querySelector(".pp-md-foot");
  put("workspace-page-info", foot && foot.getClientRects().length ? foot.querySelector(".pp-muted")?.textContent || "" : "");
}
$("workspace-jobs").addEventListener("click", () => rails.showTab("activity"));
$("workspace-health").addEventListener("click", () => pluginList.errors.length ? showSettings("plugins") : rails.showTab("activity"));
setInterval(updateWorkspaceStatus, 1000);
setInterval(refreshBackgroundStatus, 15000);
document.addEventListener("visibilitychange", () => { if (!document.hidden) refreshBackgroundStatus(); });
document.addEventListener("keydown", event => {
  if (event.key === "Escape") $("workspace-actions").querySelector("details")?.removeAttribute("open");
});

function openApp(plugin) {
  navigated = true;
  const page = plugin.pages[0];
  if (page) openPage(plugin.id, page.id);
  else toast(`${plugin.name} has no page`, "error");
}

function renderAppNav() {
  $("app-nav").replaceChildren(...(rails ? rails.navigation(pluginList.plugins) : pluginList.plugins.filter((p) => p.pages.length)).map((p) => {
    const count = badges[p.id];
    return el("button", {
      type: "button", class: "nav-item", "data-view": "plugin", "data-plugin": p.id,
      "aria-label": count ? `${p.name}, ${count} due` : p.name, title: count ? `${p.name} · ${count} due` : p.name,
      onclick: () => openApp(p),
    }, appTile(p, "sm"), el("span", {}, p.name), count ? el("span", { class: "nav-count", "aria-label": `${count} due` }, count) : "");
  }));
  markNav();
}

function renderApps() {
  $("app-grid").replaceChildren(
    ...pluginList.plugins.filter((p) => p.pages.length).map((p) => {
      const count = badges[p.id];
      return el("button", { type: "button", class: "app-card", onclick: () => openApp(p) },
        appTile(p, "lg"), el("span", { class: "app-card-name" }, p.name),
        count ? el("span", { class: "app-card-badge", "aria-label": `${count} due` }, count) : "");
    }),
    el("button", { type: "button", class: "app-card app-card-add", onclick: () => showSettings("plugins") },
      el("span", { class: "app-tile app-tile-lg app-tile-ghost", "aria-hidden": "true" }, icon("plus")),
      el("span", { class: "app-card-name" }, "Add app")),
  );

}

async function loadPlugins() {
  templateCache.clear();
  pluginList = await call("plugins");
  const errors = $("load-errors");
  errors.replaceChildren(
    ...pluginList.errors.map((e) =>
      el("div", { class: "card error-card", role: "alert" },
        el("strong", {}, "A plugin failed to load"),
        el("code", {}, e.dir),
        el("pre", {}, e.error)),
    ),
  );
  renderAppNav();
  renderApps();
  loadBadges();
  await renderHome();
  await rails?.update(pluginList.plugins);
  if (currentView === "settings" && settingsSection === "plugins") renderInstalled();
  if (currentPage && pluginList.plugins.some((p) => p.id === currentPage.plugin)) {
    await openPage(currentPage.plugin, currentPage.page, { keepView: currentView !== "plugin" });
  } else {
    currentPage = null;
    if (currentView === "plugin") showView("home");
  }
  markNav();
  renderWorkspace();
  refreshBackgroundStatus();
}

function renderGreeting() {
  if (pages && !pages.isHome()) {
    $("home-greeting").textContent = pages.title();
    $("home-date").textContent = "";
    return;
  }
  const now = new Date();
  const hour = now.getHours();
  $("home-greeting").textContent = hour < 12 ? "Good morning" : hour < 17 ? "Good afternoon" : "Good evening";
  $("home-date").textContent = now.toLocaleDateString(undefined, { weekday: "long", day: "numeric", month: "long" });
}

let homeSeq = 0;

async function renderHome() {
  if (!await pages.flush()) {
    toast("Save your edits and stop any recording before changing the page.", "error"); return;
  }
  renderGreeting();
  // Startup sends several reloads; only the latest render is kept.
  const seq = ++homeSeq;
  $("home-empty").hidden = pluginList.plugins.length > 0;
  try {
    await pages.load(currentSpace, pluginList.plugins);
  } catch (e) {
    if (seq === homeSeq) toast(String(e), "error");
  }
}

async function showUserPage(id) {
  navigated = true;
  if (!id) return;
  if (currentView !== "home") {
    await showView("home");
    if (currentView !== "home") return;
  }
  if (pages.currentId !== id && !await flushPage()) return;
  await pages.open(id);
  $("content").scrollTop = 0;
}

async function openPage(pluginId, pageId, { keepView = false, item = null, fromSearch = false } = {}) {
  return profile.measure(`navigation/${pluginId}/${pageId}`, () => renderPage(pluginId, pageId, { keepView, item, fromSearch }));
}

async function renderPage(pluginId, pageId, { keepView = false, item = null, fromSearch = false } = {}) {
  const seq = ++pageSeq;
  if (!await flushPage() || seq !== pageSeq) return;
  rememberWorkspace();
  if (!fromSearch) searchReturn = null;
  $("search-return").hidden = !searchReturn;
  const plugin = pluginList.plugins.find((p) => p.id === pluginId);
  const body = $("app-body");
  views.page?.dispose();
  views.page = null;
  if (plugin) {
    $("app-tile").replaceWith(Object.assign(appTile(plugin, "md"), { id: "app-tile" }));
    $("app-title").textContent = plugin.name;
    $("app-actions").replaceChildren(...plugin.quick_actions.map((a, index) => el("button", {
      type: "button", class: index === 0 ? "primary" : "", onclick: () => openPalette({ action: { plugin, action: a } }),
    }, icon("plus"), a.label)));
    const tabs = plugin.pages.length > 1 ? plugin.pages : [];
    $("app-tabs").hidden = !tabs.length;
    $("app-tabs").replaceChildren(...tabs.map((pg) => el("button", {
      type: "button", class: "pill", "aria-pressed": String(pg.id === pageId),
      onclick: () => openPage(pluginId, pg.id),
    }, pg.title)));
  }
  try {
    body.setAttribute("aria-busy", "true");
    body.replaceChildren(el("p", { class: "muted", role: "status" }, "Loading page…"));
    const template = await call("view_template", { plugin: pluginId, kind: "page", id: pageId });
    if (seq !== pageSeq) return;
    if (!keepView) await showView("plugin");
    if (seq !== pageSeq) return;
    currentPage = { plugin: pluginId, page: pageId };
    markNav(); // showView ran before currentPage changed.
    await profile.measure("navigation/mount", async () => {
      const mounted = window.TerrariumRenderer.mount(body, template, bridge(pluginId), { hideTitle: true, item, snapshot: workspaceSession.get(locationKey(pluginId, pageId)) });
      views.page = mounted;
      await mounted.ready;
      if (seq !== pageSeq) mounted.dispose();
    });
    if (seq !== pageSeq) return;
  } catch (e) {
    if (seq !== pageSeq) return;
    views.page?.dispose(); views.page = null;
    body.replaceChildren(el("p", { class: "error", role: "alert" }, `Couldn’t load this page: ${String(e)}`), el("button", { type: "button", onclick: () => openPage(pluginId, pageId, { item, fromSearch }) }, "Retry loading"));
  }
  body.setAttribute("aria-busy", "false");
  renderWorkspace();
  if (!keepView && currentView !== "plugin") showView("plugin");
  return !!views.page;
}

$("app-back").addEventListener("click", () => showView("apps"));
$("search-return").addEventListener("click", () => {
  if (searchReturn) openPalette({ query: searchReturn.query, allSpaces: searchReturn.allSpaces });
});

// ---- Devices and sync ----------------------------------------------------

let syncStatus = null;
let pairTimer = null;

function syncError(message) {
  const box = $("sync-error");
  box.textContent = message || "";
  box.hidden = !message;
}

function ago(ms) {
  const s = Math.max(0, Math.round((Date.now() - ms) / 1000));
  if (s < 5) return "just now";
  if (s < 60) return `${s} s ago`;
  if (s < 3600) return `${Math.round(s / 60)} min ago`;
  return new Date(ms).toLocaleString();
}

function describeLast(last) {
  if (!last) return "not yet";
  const down = Object.keys(last.stores || {});
  const via = down.length ? ` (unreachable: ${down.join(", ")})` : "";
  if (last.error) return `failed ${ago(last.at_ms)}: ${last.error}`;
  const r = last.report || {};
  const parts = [`ok ${ago(last.at_ms)}${via}`];
  if (r.batches) parts.push(`${r.batches} received`);
  if (r.pushed) parts.push(`sent #${r.pushed}`);
  if (r.rejected?.length) parts.push(`${r.rejected.length} rejected`);
  return parts.join(" · ");
}

function updateTitleStatus() {
  const s = syncStatus;
  if (!s) return;
  let text;
  let state = "ok";
  const peers = s.devices.filter((device) => !device.this_device).length;
  if (!s.role || peers === 0) {
    text = "Saved here · this device only";
    state = "off";
  } else if (!s.last) {
    text = "Saved here · waiting for first sync";
    state = "off";
  } else if (s.last.error || s.last.ok === false || s.last.report?.rejected?.length) {
    text = "Saved here · sync needs attention";
    state = "error";
  } else if (Object.keys(s.last.stores || {}).length) {
    text = "Saved here · some sync routes unavailable";
    state = "off";
  } else {
    text = `Last sync ${ago(s.last.at_ms)}`;
  }
  $("space-status").textContent = text;
  $("space-status").setAttribute("aria-label", `${text}. Open Devices and sync`);
  $("space-status").title = text;
  $("space-status").dataset.state = state;
  $("sync-dot").dataset.state = state;
  $("sync-dot").title = text;
  $("sync-mobile-text").textContent = state === "error" ? "Sync needs attention" : state === "ok" ? "Last sync" : "Saved here";
  $("sync-mobile").setAttribute("aria-label", `${text}. Open Devices and sync`);
  $("sync-mobile").title = text;
  rails?.setSync(text, state);
}

async function loadSyncStatus() {
  try {
    syncStatus = await call("sync_status");
  } catch (e) {
    syncError(String(e));
    return;
  }
  const s = syncStatus;
  const role = s.role === "host" ? "hosts this space" : s.role === "peer" ? `joined via ${s.host}` : "not paired";
  const rows = [
    ["Name", s.name],
    ["Device id", `…${s.device.slice(-8)}`],
    ["Role", role],
    ["Tailscale", s.tailnet_addresses.length ? s.tailnet_addresses.join(", ") : "not connected"],
    ["Keys stored in", s.key_storage],
    ["Last sync", describeLast(s.last)],
  ];
  $("sync-summary").replaceChildren(...rows.flatMap(([k, v]) => [el("dt", {}, k), el("dd", {}, v)]));
  $("sync-now").hidden = !s.role;
  $("devices-card").hidden = !s.role;
  $("device-list").replaceChildren(
    ...s.devices.map((d) => {
      const row = el("li", {}, el("strong", {}, d.name || "Device"), ` …${d.device.slice(-8)}`, d.this_device ? " (this device)" : "");
      if (s.role === "host" && !d.this_device) row.append(el("button", { class: "btn small", onclick: async () => {
        if (!await window.terrariumConfirm(`Block ${d.name || "this device"} from this host? Its downloaded data, space key and Google Drive access remain. Revoke Drive access separately. Key rotation is not yet available.`)) return;
        try { await call("remove_sync_device", { device: d.device }); await loadSyncStatus(); }
        catch (error) { toast(String(error)); }
      } }, "Block device"));
      return row;
    }),
  );
  $("pair-card").hidden = !(s.can_host && s.role !== "peer");
  $("join-card").hidden = !!s.role;
  $("recovery-card").hidden = !s.role;
  $("drive-card").hidden = !s.role;
  const host = s.role === "host";
  $("drive-state").textContent = s.drive ? "connected" : host ? "not connected" : "not set up on the host";
  $("drive-form").hidden = !(host && !s.drive);
  $("drive-disconnect").hidden = !(host && s.drive);
  if (!s.pairing) hidePairing();
  updateTitleStatus();
}

function hidePairing() {
  clearInterval(pairTimer);
  pairTimer = null;
  $("pair-code").hidden = true;
}

$("pair-start").addEventListener("click", async () => {
  syncError("");
  try {
    const p = await call("start_pairing");
    $("pair-code-value").textContent = p.code;
    $("pair-address").textContent = p.addresses.length ? p.addresses.join(" or ") : "Tailscale is not connected on this computer";
    let left = p.expires_in_s;
    const tick = () => {
      $("pair-expiry").textContent = `${Math.floor(left / 60)}:${String(left % 60).padStart(2, "0")}`;
      if (left-- <= 0) hidePairing();
    };
    clearInterval(pairTimer);
    tick();
    pairTimer = setInterval(tick, 1000);
    $("pair-code").hidden = false;
    await loadSyncStatus();
  } catch (e) {
    syncError(String(e));
  }
});

$("pair-cancel").addEventListener("click", async () => {
  await call("cancel_pairing");
  hidePairing();
});

$("join-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  syncError("");
  const button = e.target.querySelector("button[type=submit]");
  button.disabled = true;
  button.textContent = "Joining…";
  try {
    await call("join_space", { host: $("join-host").value, code: $("join-code").value });
    toast("Joined the space", "ok");
    await loadSyncStatus();
  } catch (err) {
    syncError(String(err));
  } finally {
    button.disabled = false;
    button.textContent = "Join";
  }
});

$("drive-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  syncError("");
  const button = e.target.querySelector("button[type=submit]");
  button.disabled = true;
  button.textContent = "Waiting for Google sign-in in your browser…";
  try {
    await call("connect_drive", { clientId: $("drive-id").value, clientSecret: $("drive-secret").value });
    $("drive-secret").value = "";
    toast("Google Drive connected", "ok");
  } catch (err) {
    syncError(String(err));
  } finally {
    button.disabled = false;
    button.textContent = "Connect Google Drive";
    await loadSyncStatus();
  }
});

$("drive-disconnect").addEventListener("click", async () => {
  if (!await window.terrariumConfirm("Stop using Google Drive on this device?")) return;
  try {
    await call("disconnect_drive");
  } catch (e) {
    syncError(String(e));
  }
  await loadSyncStatus();
});

$("sync-now").addEventListener("click", async () => {
  const button = $("sync-now");
  button.disabled = true;
  try {
    await call("sync_now");
  } catch (e) {
    syncError(String(e));
  }
  button.disabled = false;
  await loadSyncStatus();
});

$("recovery-show").addEventListener("click", async () => {
  const out = $("recovery-phrase");
  if (!out.hidden) {
    out.hidden = true;
    $("recovery-show").textContent = "Show";
    return;
  }
  try {
    out.textContent = await call("recovery_phrase");
    out.hidden = false;
    $("recovery-show").textContent = "Hide";
  } catch (e) {
    syncError(String(e));
  }
});

// ---- Plugins: install, update, roll back, remove -------------------------

const isPhone = /Android|iPhone|iPad/.test(navigator.userAgent);
let review = null;

function pluginsError(message) {
  const box = $("plugins-error");
  box.textContent = message || "";
  box.hidden = !message;
}

function readFileBase64(file) {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result).split(",", 2)[1] || "");
    reader.onerror = () => reject(reader.error);
    reader.readAsDataURL(file);
  });
}

const ORIGIN_LABEL = { bundled: "Built in", installed: "Installed", dev: "Development" };

/** Negative when version a is older than b ("0.1.10" > "0.1.9"). */
function compareVersions(a, b) {
  const pa = String(a).split(/[.+-]/).map((n) => parseInt(n, 10) || 0);
  const pb = String(b).split(/[.+-]/).map((n) => parseInt(n, 10) || 0);
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    if ((pa[i] || 0) !== (pb[i] || 0)) return (pa[i] || 0) - (pb[i] || 0);
  }
  return 0;
}

function renderInstalled() {
  const list = $("installed-list");
  if (!pluginList.plugins.length) {
    list.replaceChildren(el("li", { class: "pp-empty" }, "No plugins yet."));
  } else {
    list.replaceChildren(...pluginList.plugins.map((p) => {
      const actions = el("div", { class: "card-foot" });
      if (p.previous_version) {
        const verb = compareVersions(p.previous_version, p.version) < 0 ? "Roll back to" : "Return to";
        actions.append(el("button", {
          type: "button", class: "pp-button",
          onclick: () => pluginAction("rollback_plugin", { id: p.id }, `${p.name} switched to ${p.previous_version}`),
        }, `${verb} ${p.previous_version}`));
      }
      actions.append(el("button", {
        type: "button", class: "pp-button",
        onclick: async () => {
          if (!await window.terrariumConfirm(`Remove ${p.name} from all your devices? Its data is kept.`)) return;
          pluginAction("set_plugin_removed", { id: p.id, removed: true }, `${p.name} removed`);
        },
      }, "Remove"));
      return el("li", { class: "pp-list-item plugin-row" },
        el("div", { class: "widget-head" },
          el("strong", {}, `${p.name} `, el("span", { class: "muted" }, p.version)),
          el("span", { class: "pp-badge" }, ORIGIN_LABEL[p.origin] || "Loaded")),
        p.description ? el("p", { class: "muted" }, p.description) : "",
        p.capabilities.length
          ? el("p", { class: "muted" }, "Can: ", p.capabilities.map((c) => c.label).join("; "))
          : "",
        p.source && p.origin === "installed" ? el("p", { class: "muted" }, `From ${p.source}`) : "",
        actions);
    }));
  }
  $("pending-plugins-card").hidden = !pluginList.pending?.length;
  $("pending-plugins-list").replaceChildren(...(pluginList.pending || []).map((p) =>
    el("li", { class: "pp-list-item widget-head" },
      el("span", {}, `${p.id} ${p.version}`),
      el("button", { type: "button", class: "pp-button", onclick: async () => {
        try { pluginsError(""); showReview(await call("stage_install", { source: { kind: "synced", id: p.id } })); }
        catch (e) { pluginsError(String(e)); }
      } }, "Review"))));
  $("removed-card").hidden = !pluginList.removed?.length;
  $("removed-list").replaceChildren(...(pluginList.removed || []).map((r) =>
    el("li", { class: "pp-list-item widget-head" },
      el("span", {}, `${r.id} ${r.version}`),
      el("button", {
        type: "button", class: "pp-button",
        onclick: () => pluginAction("set_plugin_removed", { id: r.id, removed: false }, `${r.id} added back`),
      }, "Add back"))));
}

async function loadSigners() {
  try {
    const signers = await call("trusted_signers");
    $("signer-list").replaceChildren(...(signers.length
      ? signers.map((s) => el("li", {}, el("strong", {}, s.name), s.name.includes(s.fingerprint) ? "" : ` · ${s.fingerprint}`))
      : [el("li", {}, "None yet.")]));
  } catch (e) {
    pluginsError(String(e));
  }
}

async function pluginAction(command, args, done) {
  pluginsError("");
  try {
    await call(command, args);
    toast(done);
    await loadPlugins();
  } catch (e) {
    pluginsError(String(e));
  }
}

function updateInstallKind() {
  const kind = $("install-kind").value;
  $("install-file-field").hidden = kind !== "upload";
  $("install-location-field").hidden = kind === "upload";
  $("install-location-label").textContent = kind === "git" ? "Repository" : "Folder";
  $("install-location").placeholder = kind === "git"
    ? "https://github.com/you/plugin.git#path/inside"
    : "/Users/you/Code/my-plugin";
  $("install-location-hint").textContent = kind === "git"
    ? "The repository must contain a built package (dist/plugin.wasm) or a .terrapkg; the app does not build code from Git."
    : "A folder with manifest.json and dist/plugin.wasm, e.g. after terra build.";
}

function showReview(r) {
  review = r;
  $("review-card").hidden = false;
  $("review-title").textContent = `${r.name} ${r.version}`;
  $("review-kind").textContent = r.current_version
    ? (r.current_version === r.version ? "Reinstall" : `Update from ${r.current_version}`)
    : "New plugin";
  $("review-description").textContent = r.description;
  $("review-trust").textContent =
    r.trust === "trusted" ? `Signed by ${r.signer_name} (${r.signer}).`
      : r.trust === "unknown" ? `Signed by an unknown publisher, key ${r.signer}.`
        : "Not signed: nobody vouches that these files are unchanged.";
  $("review-dev").hidden = !r.shadowed_by_dev;
  const added = new Set(r.added.map((c) => c.id));
  $("review-caps").replaceChildren(...(r.capabilities.length
    ? r.capabilities.map((c) => el("li", {}, c.label, added.has(c.id) && r.current_version ? el("strong", {}, " (new)") : ""))
    : [el("li", {}, "Keep its own data only.")]));
  const needsTrust = r.trust !== "trusted";
  $("review-trust-confirm").hidden = !needsTrust;
  $("review-trust-text").textContent = r.trust === "unknown"
    ? "Trust this publisher on all my devices"
    : "Install it anyway";
  $("review-trust-box").checked = false;
  const needsCaps = r.added.length > 0;
  $("review-caps-confirm").hidden = !needsCaps;
  $("review-caps-box").checked = false;
  $("review-card").scrollIntoView({ behavior: "smooth", block: "nearest" });
}

function hideReview() {
  review = null;
  $("review-card").hidden = true;
}

$("install-kind").addEventListener("change", updateInstallKind);

$("install-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  pluginsError("");
  const kind = $("install-kind").value;
  const button = $("install-check");
  button.disabled = true;
  try {
    let source;
    if (kind === "upload") {
      const file = $("install-file").files[0];
      if (!file) throw new Error("Choose a .terrapkg file.");
      source = { kind, name: file.name, data: await readFileBase64(file) };
    } else {
      const location = $("install-location").value.trim();
      if (!location) throw new Error(kind === "git" ? "Enter a repository address." : "Enter a folder path.");
      source = kind === "git" ? { kind, url: location } : { kind, path: location };
    }
    showReview(await call("stage_install", { source }));
  } catch (err) {
    hideReview();
    pluginsError(String(err));
  } finally {
    button.disabled = false;
  }
});

$("review-install").addEventListener("click", async () => {
  if (!review) return;
  const needsTrust = review.trust !== "trusted";
  if (needsTrust && !$("review-trust-box").checked) {
    pluginsError(review.trust === "unknown" ? "Tick the box to trust this publisher." : "Tick the box to install an unsigned package.");
    return;
  }
  if (!$("review-caps-confirm").hidden && !$("review-caps-box").checked) {
    pluginsError("Tick the box to allow the new permissions.");
    return;
  }
  const r = review;
  hideReview();
  await pluginAction("confirm_install", { token: r.token, trust: needsTrust, approveCapabilities: $("review-caps-box").checked }, `${r.name} ${r.version} installed`);
  $("install-file").value = "";
  $("install-location").value = "";
  loadSigners();
});

$("review-cancel").addEventListener("click", () => {
  hideReview();
  call("cancel_install").catch(() => {});
});

if (isPhone) {
  for (const option of document.querySelectorAll("#install-kind option[data-desktop]")) option.remove();
}
updateInstallKind();

// ---- Appearance -------------------------------------------------------------

let themes = []; // installed: [{ theme, warnings }]
let gallery = null; // themes shipped in ui/themes, loaded once

// Colours of the built-in theme, for its preview card (app.css has the rest).
const DEFAULT_THEME = {
  id: "default", name: "Default", description: "The app's own indigo and grey.",
  light: { bg: "#f7f7f8", surface: "#ffffff", border: "#e4e4e7", ink: "#18181b", muted: "#52525b", accent: "#4338ca" },
  dark: { bg: "#0f0f11", surface: "#18181b", border: "#2e2e33", ink: "#f4f4f5", muted: "#a1a1aa", accent: "#818cf8" },
};
const FONT_LABEL = { serif: "Serif", rounded: "Rounded", mono: "Monospace" };

function themeError(message) {
  const box = $("theme-error");
  box.textContent = message || "";
  box.hidden = !message;
}

function describeTheme(t) {
  const modes = t.light && t.dark ? "Light and dark" : t.light ? "Light only" : "Dark only";
  return [modes, FONT_LABEL[t.font]].filter(Boolean).join(" · ");
}

// A small picture of the app in the theme's colours for the current mode.
function themePreview(t) {
  const dark = window.ppTheme.isDark();
  const own = (dark ? t.dark : t.light) || t.light || t.dark || {};
  const base = (dark && (t.dark || !t.light)) || (!t.light && t.dark) ? DEFAULT_THEME.dark : DEFAULT_THEME.light;
  const c = { ...base, ...own };
  const part = (cls, styles) => {
    const node = el("span", { class: cls });
    for (const [k, v] of Object.entries(styles)) node.style.setProperty(k, v);
    return node;
  };
  const preview = part("theme-preview", { background: c.bg, "border-color": c.border });
  if (t.font) preview.style.setProperty("font-family", window.ppTheme.FONTS[t.font]);
  preview.append(
    part("theme-preview-rail", { background: c.surface, "border-color": c.border }),
    (() => {
      const card = part("theme-preview-card", { background: c.surface, "border-color": c.border, color: c.ink });
      card.append(
        el("span", { class: "theme-preview-aa" }, "Aa"),
        part("theme-preview-line", { background: c.muted }),
        part("theme-preview-pill", { background: c.accent }));
      return card;
    })());
  return preview;
}

function activeThemeId() {
  return document.documentElement.dataset.theme || "default";
}

function renderMode() {
  const theme = window.ppTheme.current();
  const chosen = window.ppTheme.mode();
  const forced = window.ppTheme.modeFor(theme, chosen) !== chosen && theme && !(theme.light && theme.dark);
  for (const b of document.querySelectorAll("#mode-picker [data-mode]")) {
    const on = forced ? b.dataset.mode === document.documentElement.dataset.mode : b.dataset.mode === chosen;
    b.setAttribute("aria-checked", String(on));
    b.disabled = Boolean(forced);
  }
  $("mode-note").hidden = !forced;
  if (forced) $("mode-note").textContent = `${theme.name} has ${theme.light ? "light" : "dark"} colours only.`;
}

function useTheme(theme, announce = true) {
  window.ppTheme.use(theme);
  renderThemes();
  if (announce) toast(`Theme: ${theme ? theme.name : "Default"}`);
}

function renderThemes() {
  $("icon-style").value = window.ppTheme.iconStyle();
  const active = activeThemeId();
  const all = [{ theme: DEFAULT_THEME, warnings: [] }, ...themes];
  $("theme-list").replaceChildren(...all.map(({ theme: t, warnings }) => {
    const on = t.id === active;
    const card = el("button", {
      type: "button", role: "radio", class: "theme-card", "aria-checked": String(on),
      onclick: () => { if (!on) useTheme(t.id === "default" ? null : t); },
    },
    themePreview(t),
    el("span", { class: "theme-meta" },
      el("strong", {}, t.name, " ", t.id === "default" ? "" : el("span", { class: "muted" }, t.version)),
      el("span", { class: "muted" }, describeTheme(t)),
      warnings.length ? el("span", { class: "theme-warn", title: warnings.join("\n") }, "Some text may be hard to read") : ""));
    const item = el("div", { class: "theme-item" }, card);
    if (t.id !== "default") {
      item.append(el("button", {
        type: "button", class: "pp-button theme-remove", "aria-label": `Remove ${t.name}`,
        onclick: () => removeTheme(t),
      }, "Remove"));
    }
    return item;
  }));
  renderGallery();
  renderMode();
}

$("icon-style").addEventListener("change", (event) => {
  window.ppTheme.useIconStyle(event.target.value);
});

function renderGallery() {
  if (!gallery) return;
  const installed = new Map(themes.map(({ theme }) => [theme.id, theme.version]));
  const offered = gallery.filter((g) => !installed.has(g.theme.id) || compareVersions(installed.get(g.theme.id), g.theme.version) < 0);
  $("theme-gallery-card").hidden = !offered.length;
  $("theme-gallery").replaceChildren(...offered.map(({ theme: t, text }) => {
    const update = installed.has(t.id);
    return el("div", { class: "theme-item" },
      el("div", { class: "theme-card theme-card-static" },
        themePreview(t),
        el("span", { class: "theme-meta" },
          el("strong", {}, t.name),
          el("span", { class: "muted" }, t.description || describeTheme(t)),
          el("span", { class: "muted" }, describeTheme(t)))),
      el("button", {
        type: "button", class: "pp-button pp-primary", "aria-label": `${update ? "Update" : "Install"} ${t.name}`,
        onclick: () => installTheme(text),
      }, update ? `Update to ${t.version}` : "Install"));
  }));
}

async function loadGallery() {
  try {
    const ids = await (await fetch("themes/index.json")).json();
    gallery = (await Promise.all(ids.map(async (id) => {
      const text = await (await fetch(`themes/${id}.theme.json`)).text();
      return { theme: JSON.parse(text), text };
    }))).sort((a, b) => a.theme.name.localeCompare(b.theme.name));
  } catch (_) {
    gallery = [];
  }
  renderGallery();
}

async function loadThemes() {
  try {
    themes = await call("list_themes");
  } catch (e) {
    themeError(String(e));
    return;
  }
  // The theme in use may have been updated or removed on another device.
  const current = window.ppTheme.current();
  if (current) {
    const now = themes.find(({ theme }) => theme.id === current.id)?.theme || null;
    if (JSON.stringify(now) !== JSON.stringify(current)) window.ppTheme.use(now);
  }
  if (currentView === "settings" && settingsSection === "appearance") renderThemes();
}

async function installTheme(text) {
  themeError("");
  try {
    const r = await call("install_theme", { data: text });
    await loadThemes();
    useTheme(r.theme, false);
    toast(r.replaced ? `${r.theme.name} updated to ${r.theme.version}` : `${r.theme.name} installed`);
    if (r.warnings.length) themeError(`${r.theme.name} is installed, but some text may be hard to read: ${r.warnings.join("; ")}.`);
    return true;
  } catch (e) {
    themeError(String(e));
    return false;
  }
}

async function removeTheme(t) {
  if (!await window.terrariumConfirm(`Remove ${t.name} from all your devices?`)) return;
  themeError("");
  try {
    await call("remove_theme", { id: t.id });
    if (activeThemeId() === t.id) window.ppTheme.use(null);
    toast(`${t.name} removed`);
    await loadThemes();
  } catch (e) {
    themeError(String(e));
  }
}

for (const b of document.querySelectorAll("#mode-picker [data-mode]")) {
  b.addEventListener("click", () => {
    window.ppTheme.use(window.ppTheme.current(), b.dataset.mode);
    renderThemes();
  });
}

window.matchMedia("(prefers-color-scheme: dark)").addEventListener("change", () => {
  if (currentView === "settings" && settingsSection === "appearance") renderThemes();
});

$("theme-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const file = $("theme-file").files[0];
  if (!file) return themeError("Choose a .theme.json file.");
  if (file.size > 64 * 1024) return themeError("A theme file must be under 64 KB.");
  if (await installTheme(await file.text())) $("theme-file").value = "";
});

// ---- AI -----------------------------------------------------------------------

let aiState = null;

function aiError(message) {
  $("ai-error").textContent = message || "";
  $("ai-error").hidden = !message;
}

function aiForm() {
  const num = (id, fallback) => {
    const v = Number($(id).value);
    return Number.isFinite(v) && $(id).value !== "" ? v : fallback;
  };
  const d = aiState ? aiState.defaults : {};
  return {
    enabled: $("ai-enabled").checked,
    preset: $("ai-preset").value,
    base_url: $("ai-base-url").value.trim(),
    model: $("ai-model").value.trim(),
    temperature: num("ai-temperature", d.temperature),
    max_tokens: num("ai-max-tokens", d.max_tokens),
    timeout_s: num("ai-timeout", d.timeout_s),
    concurrency: num("ai-concurrency", d.concurrency),
    monthly_limit: num("ai-monthly", d.monthly_limit),
    max_prompt_kb: num("ai-prompt-kb", d.max_prompt_kb),
  };
}

function fillAiForm(c) {
  $("ai-enabled").checked = c.enabled;
  $("ai-preset").value = c.preset;
  $("ai-base-url").value = c.base_url;
  $("ai-model").value = c.model;
  $("ai-temperature").value = c.temperature;
  $("ai-max-tokens").value = c.max_tokens;
  $("ai-timeout").value = c.timeout_s;
  $("ai-concurrency").value = c.concurrency;
  $("ai-monthly").value = c.monthly_limit;
  $("ai-prompt-kb").value = c.max_prompt_kb;
  presetHint();
}

function presetHint() {
  const p = (aiState?.presets || []).find((x) => x.id === $("ai-preset").value);
  $("ai-preset-hint").textContent = p ? p.hint : "";
  $("ai-base-url").readOnly = !!(p && p.base_url);
}

function renderAi(state, refill = true) {
  aiState = state;
  rails?.setAiState(state);
  const select = $("ai-preset");
  if (!select.options.length) {
    select.replaceChildren(...state.presets.map((p) => el("option", { value: p.id }, p.name)));
  }
  if (refill) fillAiForm(state.config);
  const st = state.status;
  const status = $("ai-status");
  status.className = `ai-status ${st.ready ? "ok" : "warn"}`;
  status.textContent = st.ready
    ? `Ready · ${state.config.model}${st.queued || st.running ? ` · ${st.running} running, ${st.queued} waiting` : ""}`
    : st.reason;
  $("ai-key-set").hidden = !st.key_set;
  $("ai-key-hint").textContent = st.key_unreadable ? `${st.key_hint || "Key"} (not readable on this device yet)` : (st.key_hint || "");
  $("ai-key-save").textContent = st.key_set ? "Replace key" : "Save key";
  const u = state.usage;
  const stat = (label, value) => el("div", { class: "pp-stat" }, el("span", { class: "pp-stat-value" }, value), el("span", { class: "pp-stat-label" }, label));
  const limit = state.config.monthly_limit;
  $("ai-usage").replaceChildren(
    stat(limit ? `Requests (of ${limit})` : "Requests", String(u.requests)),
    stat("Failed", String(u.failures)),
    stat("Tokens in", u.input_tokens.toLocaleString()),
    stat("Tokens out", u.output_tokens.toLocaleString()),
  );
  $("ai-plugins").textContent = state.plugins.length
    ? `Plugins that can use AI: ${state.plugins.map((p) => p.name).join(", ")}.`
    : "No installed plugin asks to use AI.";
}

async function aiCall(name, args) {
  aiError("");
  try {
    const state = await call("service", { name, args });
    return state;
  } catch (e) {
    aiError(String(e));
    return null;
  }
}

async function loadAi() {
  const state = await aiCall("ai_settings");
  if (state) renderAi(state);
}

$("ai-preset").addEventListener("change", () => {
  const p = aiState.presets.find((x) => x.id === $("ai-preset").value);
  if (p && p.base_url) $("ai-base-url").value = p.base_url;
  if (p && !p.base_url) $("ai-base-url").value = "";
  presetHint();
});

$("ai-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const state = await aiCall("ai_save_settings", { config: aiForm() });
  if (state) {
    renderAi(state);
    toast("AI settings saved");
  }
});

$("ai-defaults").addEventListener("click", () => {
  const d = aiState.defaults;
  for (const [id, key] of [["ai-temperature", "temperature"], ["ai-max-tokens", "max_tokens"], ["ai-timeout", "timeout_s"],
    ["ai-concurrency", "concurrency"], ["ai-monthly", "monthly_limit"], ["ai-prompt-kb", "max_prompt_kb"]]) {
    $(id).value = d[key];
  }
});

$("ai-test").addEventListener("click", async () => {
  aiError("");
  const out = $("ai-test-result");
  out.hidden = false;
  out.textContent = "Testing…";
  try {
    const r = await call("service_net", { name: "ai_test", config: aiForm() });
    out.textContent = `Connected to ${r.model} in ${(r.ms / 1000).toFixed(1)} s: “${r.text}”`;
  } catch (e) {
    out.hidden = true;
    aiError(String(e));
  }
});

$("ai-load-models").addEventListener("click", async () => {
  aiError("");
  try {
    const models = await call("service_net", { name: "ai_models", config: aiForm() });
    $("ai-model-list").replaceChildren(...models.map((m) => el("option", { value: m })));
    toast(models.length ? `${models.length} models; start typing to pick one` : "The provider listed no models");
    $("ai-model").focus();
  } catch (e) {
    aiError(`Could not list models: ${e}`);
  }
});

$("ai-key-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const key = $("ai-key").value.trim();
  if (!key) return aiError("Paste the API key first.");
  const state = await aiCall("ai_set_key", { key });
  if (state) {
    $("ai-key").value = "";
    renderAi(state, false);
    toast("API key saved");
  }
});

$("ai-key-remove").addEventListener("click", async () => {
  if (!await window.terrariumConfirm("Remove the API key from all your devices?")) return;
  const state = await aiCall("ai_remove_key");
  if (state) {
    renderAi(state, false);
    toast("API key removed");
  }
});

// ---- Scheduled ----------------------------------------------------------------

const WEEKDAYS = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];
const STATUS_LABELS = { done: "Done", failed: "Failed", skipped: "Skipped", retry: "Will retry", waiting: "Waiting for AI", running: "Running" };
let schedEditing = null;

function schedError(message) {
  $("sched-error").textContent = message || "";
  $("sched-error").hidden = !message;
}

function when(ms) {
  if (!ms) return "";
  const d = new Date(ms);
  const today = new Date();
  const sameDay = d.toDateString() === today.toDateString();
  const time = d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  if (sameDay) return `today ${time}`;
  return `${d.toLocaleDateString([], { weekday: "short", day: "numeric", month: "short" })} ${time}`;
}

function statusChip(status) {
  return el("span", { class: `status status-${status}` }, STATUS_LABELS[status] || status);
}

async function schedCall(name, args) {
  schedError("");
  try {
    return await call("service", { name, args });
  } catch (e) {
    schedError(String(e));
    return null;
  }
}

function repeatEditor(s) {
  const r = s.repeat;
  const kind = el("select", { class: "pp-input", "aria-label": "Repeat" },
    ...[["day", "Every day"], ["week", "Weekly"], ["month", "Monthly"], ["minutes", "Every few hours"]]
      .map(([v, t]) => el("option", { value: v }, t)));
  kind.value = r.every;
  const at = el("input", { class: "pp-input", type: "time", value: r.at || "09:00", "aria-label": "Time" });
  const days = el("div", { class: "sched-days" }, ...WEEKDAYS.map((d) => el("label", {},
    el("input", { type: "checkbox", value: d, checked: (r.days || ["mon"]).includes(d) }), d[0].toUpperCase() + d.slice(1))));
  const monthDay = el("select", { class: "pp-input", "aria-label": "Day of month" },
    ...Array.from({ length: 31 }, (_, i) => el("option", { value: String(i + 1) }, `Day ${i + 1}`)),
    el("option", { value: "last" }, "Last day"));
  monthDay.value = r.day != null ? String(r.day) : "1";
  const hours = el("input", { class: "pp-input", type: "number", min: "1", max: "168", value: r.minutes ? Math.max(1, Math.round(r.minutes / 60)) : 6, "aria-label": "Hours" });
  const show = () => {
    days.hidden = kind.value !== "week";
    monthDay.hidden = kind.value !== "month";
    hours.hidden = kind.value !== "minutes";
    at.hidden = kind.value === "minutes";
  };
  kind.addEventListener("change", show);
  show();
  const save = el("button", { type: "button", class: "pp-button pp-primary", onclick: async () => {
    let repeat;
    if (kind.value === "minutes") repeat = { every: "minutes", minutes: Math.max(1, Number(hours.value) || 1) * 60 };
    else if (kind.value === "day") repeat = { every: "day", at: at.value };
    else if (kind.value === "week") repeat = { every: "week", days: [...days.querySelectorAll("input:checked")].map((i) => i.value), at: at.value };
    else repeat = { every: "month", day: monthDay.value === "last" ? "last" : Number(monthDay.value), at: at.value };
    const state = await schedCall("schedule_set_repeat", { id: s.id, repeat });
    if (state) {
      schedEditing = null;
      renderSchedules(state);
      toast("Schedule changed");
    }
  } }, "Save");
  const cancel = el("button", { type: "button", class: "pp-button", onclick: () => { schedEditing = null; loadSchedules(); } }, "Cancel");
  return el("div", { class: "sched-edit" }, kind, days, monthDay, hours, at, save, cancel);
}

function renderSchedules(state) {
  $("sched-paused").checked = state.settings.paused;
  $("sched-notify").checked = state.settings.notify_failures;
  $("sched-notify-done").checked = state.settings.notify_done;
  const items = state.schedules.map((s) => {
    const last = s.last_run;
    const meta = [s.plugin_name, s.space_name && s.space !== "personal" ? s.space_name : null, s.describe,
      s.run_on === "desktop" ? "computer only" : null].filter(Boolean).join(" · ");
    const next = !s.enabled ? "Off" : state.settings.paused ? "Paused" : s.next_ms ? `Next ${when(s.next_ms)}` : "";
    const toggle = el("label", { class: "pp-checkbox" }, el("input", { type: "checkbox", checked: s.enabled, onchange: async (e) => {
      const st = await schedCall("schedule_set_enabled", { id: s.id, enabled: e.target.checked });
      if (st) renderSchedules(st);
    } }), "On");
    const actions = el("div", { class: "sched-actions" },
      toggle,
      el("button", { type: "button", class: "pp-button", disabled: !s.installed, onclick: async () => {
        const r = await schedCall("schedule_run_now", { id: s.id });
        if (r) {
          renderSchedules(r.state);
          toast(`${s.name} started`);
        }
      } }, "Run now"),
      el("button", { type: "button", class: "pp-button", onclick: () => { schedEditing = s.id; renderSchedules(state); } }, "Change time"),
      s.source === "manifest" && s.installed ? null : el("button", { type: "button", class: "pp-button pp-danger", onclick: async () => {
        if (!await window.terrariumConfirm(`Remove “${s.name}” and its history from all your devices?`)) return;
        const st = await schedCall("schedule_remove", { id: s.id });
        if (st) renderSchedules(st);
      } }, "Remove"),
    );
    return el("li", { class: "pp-list-item sched-item" },
      el("div", { class: "sched-row" },
        el("div", { class: "grow" }, el("div", { class: "sched-title" }, s.name), el("div", { class: "muted small" }, meta)),
        last ? statusChip(last.status) : null,
        el("span", { class: "muted small" }, next)),
      last && last.error && last.status !== "done" ? el("p", { class: "muted small" }, `Last run ${when(last.started_ms || last.slot_ms)}: ${last.error}`) : null,
      schedEditing === s.id ? repeatEditor(s) : actions);
  });
  $("sched-list").replaceChildren(...(items.length ? items : [el("li", { class: "pp-empty" }, "No schedules yet. Plugins add them, for example Reports → Schedules.")]));
  const runs = state.runs.map((r) => el("li", { class: "pp-list-item sched-row" },
    el("div", { class: "grow" },
      el("div", {}, r.schedule_name || r.schedule, r.manual ? el("span", { class: "muted" }, " · run by hand") : null),
      el("div", { class: "muted small" }, [when(r.started_ms || r.slot_ms), r.attempt > 1 ? `try ${r.attempt}` : null,
        r.status === "retry" && r.next_attempt_ms ? `next try ${when(r.next_attempt_ms)}` : null, r.error].filter(Boolean).join(" · "))),
    statusChip(r.status),
    ["failed", "skipped", "retry"].includes(r.status)
      ? el("button", { type: "button", class: "pp-button", onclick: async () => {
        const st = await schedCall("schedule_retry", { run: r.id });
        if (st) {
          renderSchedules(st);
          toast("Retrying");
        }
      } }, "Retry")
      : null));
  $("sched-runs").replaceChildren(...(runs.length ? runs : [el("li", { class: "pp-empty" }, "Nothing has run yet.")]));
}

async function loadSchedules() {
  const state = await schedCall("schedules");
  if (state) renderSchedules(state);
}

async function saveSchedulerSettings() {
  const state = await schedCall("scheduler_save_settings", { settings: { paused: $("sched-paused").checked, notify_failures: $("sched-notify").checked, notify_done: $("sched-notify-done").checked } });
  if (state) renderSchedules(state);
}
$("sched-paused").addEventListener("change", saveSchedulerSettings);
$("sched-notify").addEventListener("change", saveSchedulerSettings);
$("sched-notify-done").addEventListener("change", saveSchedulerSettings);

// ---- Backups ----------------------------------------------------------------

function formatSize(bytes) {
  return bytes > 1024 * 1024 ? `${(bytes / 1024 / 1024).toFixed(1)} MB` : `${Math.max(1, Math.round(bytes / 1024))} KB`;
}

function describeRestore(r) {
  const parts = [`${r.applied} changes restored`];
  if (r.duplicates) parts.push(`${r.duplicates} already here`);
  if (r.missing.length) parts.push(`not installed here: ${r.missing.join(", ")}`);
  if (r.adopted_space) parts.push("this computer now hosts the restored space");
  return parts.join(" · ");
}

async function loadBackups() {
  try {
    const backups = await call("list_backups");
    const latest = backups.reduce((newest, backup) => !newest || backup.created_ms > newest.created_ms ? backup : newest, null);
    $("backup-health").textContent = latest
      ? `Latest backup: ${ago(latest.created_ms)}. Keep a copy outside this device and keep your recovery phrase separately.`
      : "No backup yet. Create one, keep a copy outside this device, and store your recovery phrase separately.";
    $("backup-list").replaceChildren(...(backups.length
      ? backups.map((b) => el("li", { class: "pp-list-item widget-head" },
        el("span", {}, new Date(b.created_ms).toLocaleString(), el("span", { class: "muted" }, ` · ${formatSize(b.size)}`)),
        el("button", { type: "button", class: "pp-button", onclick: () => restore({ path: b.path }) }, "Restore")))
      : [el("li", { class: "pp-empty" }, "No backups yet.")]));
    $("backup-dir").textContent = backups[0] ? `Saved in ${backups[0].path.replace(/[\\/][^\\/]+$/, "")}` : "";
  } catch (e) {
    syncError(String(e));
  }
}

async function restore(source) {
  syncError("");
  const phrase = $("restore-phrase").value;
  if (!await window.terrariumConfirm("Merge this backup into this device? Newer changes are kept.")) return;
  try {
    const result = await call("restore_backup", { ...source, phrase });
    $("restore-phrase").value = "";
    toast(describeRestore(result));
    await loadPlugins();
    refreshPluginViews();
    await loadSyncStatus();
  } catch (e) {
    syncError(String(e));
  }
}

$("backup-create").addEventListener("click", async () => {
  const button = $("backup-create");
  button.disabled = true;
  syncError("");
  try {
    const b = await call("create_backup");
    toast(`Backup saved (${formatSize(b.size)})`);
    await loadBackups();
    await loadSyncStatus();
  } catch (e) {
    syncError(String(e));
  } finally {
    button.disabled = false;
  }
});

$("restore-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const file = $("restore-file").files[0];
  if (!file) {
    syncError("Choose a backup file.");
    return;
  }
  await restore({ data: await readFileBase64(file) });
  $("restore-file").value = "";
});

// ---- Spaces -------------------------------------------------------------------

async function loadSpaces() {
  try {
    spaces = await call("list_spaces");
  } catch (e) {
    spaces = [{ id: "personal", name: "Personal", removed: false }];
  }
  const active = spaces.filter((sp) => !sp.removed);
  if (!active.some((sp) => sp.id === currentSpace)) await setSpace("personal", { reload: false });
  renderSpaceChrome();
  renderSpaces();
  updateTitleStatus();
}

function current() {
  return spaces.find((sp) => sp.id === currentSpace) || { id: "personal", name: "Personal" };
}

/** The space button (desktop) and chip (phone). */
function renderSpaceChrome() {
  const space = current();
  $("space-tile").replaceWith(Object.assign(spaceTile(space), { id: "space-tile" }));
  $("space-chip-tile").replaceWith(Object.assign(spaceTile(space), { id: "space-chip-tile" }));
  $("space-name-label").textContent = space.name;
  $("space-chip-name").textContent = space.name;
  renderWorkspace();
  rails?.setSpace(space);
  $("space-button").setAttribute("aria-label", `Switch space, current: ${space.name}`);
  $("space-button").title = `Switch space, current: ${space.name}`;
  $("space-chip").setAttribute("aria-label", `Switch space, current: ${space.name}`);
}

function renderSpaces() {
  $("space-list").replaceChildren(...spaces.map((sp) => {
    const input = el("input", { class: "pp-input", type: "text", value: sp.name, maxlength: "40", "aria-label": `Name of ${sp.name}` });
    const row = el("li", { class: "pp-list-item space-row" }, spaceTile(sp), input);
    if (sp.removed) {
      row.append(el("span", { class: "muted" }, "Removed"),
        el("button", { type: "button", class: "pp-button", onclick: () => spaceAction("set_space_removed", { id: sp.id, removed: false }, `${sp.name} is back`) }, "Add back"));
    } else {
      row.append(el("button", {
        type: "button", class: "pp-button",
        onclick: () => spaceAction("rename_space", { id: sp.id, name: input.value }, "Renamed"),
      }, "Rename"));
      if (sp.id !== "personal") {
        row.append(el("button", {
          type: "button", class: "pp-button",
          onclick: async () => {
            if (!await window.terrariumConfirm(`Remove ${sp.name} from all your devices? Its data is kept and you can add it back.`)) return;
            spaceAction("set_space_removed", { id: sp.id, removed: true }, `${sp.name} removed`);
          },
        }, "Remove"));
      }
    }
    return row;
  }));
}

async function spaceAction(command, args, done) {
  syncError("");
  try {
    const result = await call(command, args);
    toast(done);
    await loadSpaces();
    return result;
  } catch (e) {
    syncError(String(e));
    return null;
  }
}

let spaceSeq = 0;
async function setSpace(id, { reload = true } = {}) {
  const seq = ++spaceSeq;
  if (!await flushPage({ unmountWidgets: true }) || seq !== spaceSeq) return false;
  rememberWorkspace();
  const location = workspaceSession.read(id);
  pageSeq++; viewSeq++; homeSeq++; badgeSeq++;
  views.page?.dispose();
  pages.reset();
  views.page = null; currentPage = null;
  $("widgets").replaceChildren(el("p", { class: "muted", role: "status" }, "Opening space…"));
  $("app-body").replaceChildren();
  currentSpace = id;
  badges = {};
  renderAppNav();
  try {
    localStorage.setItem("space", id);
  } catch (_) {}
  if (reload) {
    renderSpaceChrome();
    updateTitleStatus();
    currentView = "home";
    await showView("home");
    try {
      await loadPlugins();
      if (seq !== spaceSeq) return false;
      await restoreWorkspace(location);
    } catch (e) { toast(String(e), "error"); }
  }
  return true;
}

const overlayFocus = new Map();

function openOverlay(id, focus) {
  for (const other of ["palette", "space-sheet"]) {
    if (other !== id && !$(other).hidden) closeOverlay(other);
  }
  if ($(id).hidden) overlayFocus.set(id, document.activeElement);
  $(id).hidden = false;
  if (id === "space-sheet") {
    for (const button of [$("space-button"), $("space-chip")]) button.setAttribute("aria-expanded", "true");
  }
  document.querySelector(".shell").inert = true;
  document.querySelector(".workspace-bar").inert = true;
  $("workspace-status").inert = true;
  if (focus) focus.focus();
}

function closeOverlay(id) {
  $(id).hidden = true;
  if (id === "space-sheet") {
    for (const button of [$("space-button"), $("space-chip")]) button.setAttribute("aria-expanded", "false");
  }
  const otherOpen = ["palette", "space-sheet"].some((other) => !$(other).hidden);
  document.querySelector(".shell").inert = otherOpen;
  document.querySelector(".workspace-bar").inert = otherOpen || !!rails?.drawerOpen();
  $("workspace-status").inert = otherOpen || !!rails?.drawerOpen();
  const lastFocus = overlayFocus.get(id);
  if (!otherOpen && lastFocus?.isConnected) lastFocus.focus();
  overlayFocus.delete(id);
}

function openSpaceSheet() {
  $("space-options").replaceChildren(...spaces.filter((sp) => !sp.removed).map((sp) =>
    el("button", {
      type: "button", class: "space-option", "aria-current": sp.id === currentSpace ? "true" : false,
      onclick: async () => {
        closeOverlay("space-sheet");
        if (sp.id !== currentSpace && await setSpace(sp.id)) toast(`Switched to ${sp.name}`);
      },
    }, spaceTile(sp, "space-tile space-tile-lg"), el("span", { class: "space-option-name" }, sp.name),
    sp.id === currentSpace ? icon("check") : "")));
  openOverlay("space-sheet", $("space-options").querySelector("[aria-current='true']") || $("space-new"));
}

$("space-button").addEventListener("click", openSpaceSheet);
$("space-chip").addEventListener("click", openSpaceSheet);
$("space-new").addEventListener("click", () => {
  closeOverlay("space-sheet");
  showSettings("spaces");
  $("space-name").focus();
});

$("space-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const name = $("space-name").value;
  const created = await spaceAction("create_space", { name }, `${name.trim()} created`);
  if (created) {
    $("space-name").value = "";
    await setSpace(created.id);
    await loadSpaces();
    navigated = true;
    showView("home");
  }
});

// ---- Search and commands -----------------------------------------------------

const palette = { items: [], index: 0, action: null, mode: null, allSpaces: false, seq: 0 };

async function openCreation(plugin, action, text = "") {
  const space = currentSpace;
  const page = plugin.pages[0];
  try {
    const template = page && await call("view_template", { plugin: plugin.id, kind: "page", id: page.id });
    const creation = template?.creation;
    if (space !== currentSpace) return;
    if (!creation || creation.operation !== action.op) return openPalette({ action: { plugin, action }, query: text, formChecked: true });
    if (!$("palette").hidden) closePalette();
    if (!await openPage(plugin.id, page.id)) return;
    if (space !== currentSpace || currentPage?.plugin !== plugin.id || !views.page) return;
    views.page.setState("state.open", "");
    views.page.setState(creation.prefill, text);
    views.page.setState(creation.bind, "new");
  } catch (error) { toast(`Couldn’t open ${action.label}: ${String(error)}`, "error"); }
}

function openPalette({ action = null, mode = null, query = "", allSpaces = palette.allSpaces, formChecked = false } = {}) {
  if (action && !formChecked) return openCreation(action.plugin, action.action, query);
  palette.action = action;
  palette.mode = mode;
  palette.allSpaces = allSpaces;
  const adding = mode === "add" || !!action;
  $("palette-dialog").setAttribute("aria-label", adding ? "Quick add" : "Search and commands");
  $("palette-scope").hidden = adding;
  $("capture-destination").hidden = !adding;
  $("capture-destination").textContent = `Save in ${current().name}`;
  $("palette-label").textContent = adding ? "What would you like to add?" : "Search or run a command";
  $("scope-space").setAttribute("aria-pressed", String(!allSpaces));
  $("scope-all").setAttribute("aria-pressed", String(allSpaces));
  palette.index = 0;
  const input = $("palette-input");
  input.value = query;
  input.placeholder = action
    ? `${action.action.label}: type, then press Enter`
    : mode === "add" ? "Type what to add" : "Search or type a command";
  openOverlay("palette", input);
  updatePalette();
}

function closePalette() {
  closeOverlay("palette");
  palette.action = null;
  palette.seq++; // Ignore search results arriving after dismissal.
}

$("palette-close").addEventListener("click", closePalette);

function setScope(all) {
  palette.allSpaces = all;
  $("scope-space").setAttribute("aria-pressed", String(!all));
  $("scope-all").setAttribute("aria-pressed", String(all));
  updatePalette();
}

$("scope-space").addEventListener("click", () => setScope(false));
$("scope-all").addEventListener("click", () => setScope(true));

function runQuickAction(plugin, action, text) {
  const space = currentSpace;
  const spaceName = current().name;
  return async () => {
    const input = $("palette-input");
    if (input.disabled) return;
    input.disabled = true;
    try {
      const result = await call("run_action", { space, plugin: plugin.id, op: action.op, args: { [action.arg]: text } });
      toast(typeof result?.message === "string" ? `${plugin.name}: ${result.message}` : `Saved to ${plugin.name} · ${spaceName}`);
      closePalette();
      refreshPluginViews();
    } catch (e) {
      toast(String(e), "error");
    } finally {
      input.disabled = false;
      if (!$("palette").hidden) input.focus();
    }
  };
}

function commands() {
  const out = [];
  for (const p of pluginList.plugins.filter((x) => x.pages.length)) {
    out.push({ group: "Apps", label: `Open ${p.name}`, tile: appTile(p, "xs"), run: () => openApp(p) });
  }
  for (const sp of spaces.filter((x) => !x.removed && x.id !== currentSpace)) {
    out.push({ group: "Spaces", label: `Switch to ${sp.name}`, tile: spaceTile(sp, "space-tile space-tile-xs"), run: () => setSpace(sp.id) });
  }
  out.push({ group: "Spaces", label: "New space", tile: icon("plus"), run: () => { showSettings("spaces"); $("space-name").focus(); } });
  const settings = [["Devices and sync", "devices"], ["Backups", "backups"], ["Appearance", "appearance"], ["AI", "ai"], ["Scheduled", "scheduled"], ["Plugins", "plugins"], ["Spaces", "spaces"], ["Developer", "developer"]];
  for (const [label, section] of settings) {
    out.push({ group: "Settings", label: `Settings: ${label}`, tile: icon("sliders"), run: () => showSettings(section) });
  }
  const active = activeThemeId();
  for (const t of [DEFAULT_THEME, ...themes.map((x) => x.theme)].filter((x) => x.id !== active)) {
    out.push({ group: "Themes", label: `Use theme ${t.name}`, tile: icon("palette"), run: () => useTheme(t.id === "default" ? null : t) });
  }
  out.push({ group: "Settings", label: "Sync now", tile: icon("sync"), run: () => $("sync-now").click() });
  out.push({ group: "Settings", label: "Back up now", tile: icon("archive"), run: () => $("backup-create").click() });
  return out;
}

async function searchPlugins(q, seq) {
  const returnTo = { query: q, allSpaces: palette.allSpaces };
  const scope = palette.allSpaces ? spaces.filter((sp) => !sp.removed) : [current()];
  const found = [];
  await Promise.all(pluginList.plugins.filter((p) => p.search).flatMap((p) => scope.map(async (sp) => {
    try {
      const rows = await call("run_query", { space: sp.id, plugin: p.id, name: p.search, params: { q } });
      for (const row of rows) {
        found.push({
          group: p.name,
          label: String(row.title ?? ""),
          sub: [row.subtitle, palette.allSpaces ? sp.name : null].filter(Boolean).join(" · "),
          tile: appTile(p, "xs"),
          run: async () => {
            if (!p.pages.length) return;
            if (sp.id !== currentSpace) {
              if (await setSpace(sp.id, { reload: false }) === false) return;
              renderSpaceChrome();
              renderHome();
              loadBadges();
            }
            searchReturn = returnTo;
            await openPage(p.id, p.pages[0].id, { item: row.id, fromSearch: true });
          },
        });
      }
    } catch (_) {}
  })));
  return seq === palette.seq ? found : null;
}

async function updatePalette() {
  const q = $("palette-input").value.trim();
  const seq = ++palette.seq;
  let items = [];
  if (palette.action) {
    const { plugin, action } = palette.action;
    items.push({
      group: plugin.name, label: q ? `${action.label} “${q}”` : action.label,
      sub: q ? "" : "Type the text first", tile: appTile(plugin, "xs"),
      run: q ? runQuickAction(plugin, action, q) : null, keep: true,
    });
  } else {
    const lower = q.toLowerCase();
    for (const p of pluginList.plugins) {
      for (const a of p.quick_actions) {
        items.push({
          group: "Add", label: q ? `${a.label} “${q}”` : a.label, sub: p.name, tile: appTile(p, "xs"),
          run: () => openCreation(p, a, q),
          keep: true,
        });
      }
    }
    if (palette.mode !== "add") items.push(...commands().filter((c) => !q || c.label.toLowerCase().includes(lower)));
  }
  renderPalette(items);
  if (q && !palette.action && palette.mode !== "add") {
    const found = await searchPlugins(q, seq);
    // Found items first: on a phone the keyboard hides the rest.
    if (found && found.length) renderPalette([...found, ...items]);
  }
}

function renderPalette(items) {
  palette.items = items;
  palette.index = Math.min(palette.index, Math.max(items.length - 1, 0));
  const box = $("palette-results");
  if (!items.length) {
    box.replaceChildren(el("p", { class: "palette-empty muted" }, "Nothing found"));
    return;
  }
  const nodes = [];
  let group = null;
  items.forEach((item, i) => {
    if (item.group !== group) {
      group = item.group;
      nodes.push(el("p", { class: "palette-group" }, group));
    }
    nodes.push(el("button", {
      type: "button", role: "option", class: "palette-item",
      "aria-selected": String(i === palette.index), disabled: item.run ? false : true,
      onclick: () => choose(i),
      onmousemove: () => { if (palette.index !== i) { palette.index = i; markSelected(); } },
    }, item.tile || "", el("span", { class: "palette-text" }, el("span", {}, item.label), item.sub ? el("span", { class: "muted" }, item.sub) : "")));
  });
  box.replaceChildren(...nodes);
}

function markSelected() {
  const options = $("palette-results").querySelectorAll(".palette-item");
  options.forEach((o, i) => o.setAttribute("aria-selected", String(i === palette.index)));
  options[palette.index]?.scrollIntoView({ block: "nearest" });
}

function choose(i) {
  const item = palette.items[i];
  if (!item?.run) return;
  if (!item.keep) closePalette();
  navigated = true;
  item.run();
}

$("palette-input").addEventListener("input", () => {
  palette.index = 0;
  updatePalette();
});

$("palette-input").addEventListener("keydown", (e) => {
  if (e.key === "ArrowDown" || e.key === "ArrowUp") {
    e.preventDefault();
    const n = palette.items.length;
    if (!n) return;
    palette.index = (palette.index + (e.key === "ArrowDown" ? 1 : n - 1)) % n;
    markSelected();
  } else if (e.key === "Enter") {
    e.preventDefault();
    choose(palette.index);
  }
});

for (const id of ["palette", "space-sheet"]) {
  $(id).addEventListener("click", (e) => {
    if (e.target === $(id)) id === "palette" ? closePalette() : closeOverlay(id);
  });
}

document.addEventListener("keydown", (e) => {
  const overlay = ["palette", "space-sheet"].map($).find((node) => !node.hidden);
  if (overlay && e.key === "Tab") {
    const fields = [...overlay.querySelectorAll("button:not(:disabled), input:not(:disabled), select, textarea, a[href], [tabindex='0']")]
      .filter((node) => node.getClientRects().length);
    const first = fields[0], last = fields[fields.length - 1];
    if (fields.length && (!overlay.contains(document.activeElement) || (e.shiftKey && document.activeElement === first) || (!e.shiftKey && document.activeElement === last))) {
      e.preventDefault();
      (e.shiftKey ? last : first).focus();
    }
  }
  if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
    e.preventDefault();
    if (document.querySelector("dialog[open]")) return;
    if (e.shiftKey) openPalette({ mode: "add" });
    else if ($("palette").hidden) openPalette();
    else closePalette();
  } else if (e.key === "Escape") {
    if (!$("palette").hidden) closePalette();
    else if (!$("space-sheet").hidden) closeOverlay("space-sheet");
  }
});

for (const button of document.querySelectorAll("[data-open-search]")) {
  button.addEventListener("click", () => openPalette({ mode: button.dataset.openSearch }));
}

// ---- Navigation ----------------------------------------------------------

function markNav() {
  for (const item of document.querySelectorAll(".nav-item, .tab, .page-tab")) {
    let active = false;
    if (item.dataset.settings) active = currentView === "settings" && settingsSection === item.dataset.settings;
    else if (item.dataset.userPage) active = currentView === "home" && pages?.currentId === item.dataset.userPage;
    else if (item.dataset.view === "home") active = currentView === "home" && pages?.isHome() !== false;
    else if (item.dataset.view === "plugin") active = currentView === "plugin" && currentPage?.plugin === item.dataset.plugin;
    else if (item.dataset.view === "apps") active = currentView === "apps" || (currentView === "plugin" && phoneLayout());
    else if (item.dataset.view) active = item.dataset.view === currentView && !(item.dataset.view === "settings" && settingsSection === "plugins" && !phoneLayout());
    if (active) item.setAttribute("aria-current", "page");
    else item.removeAttribute("aria-current");
  }
}

// Older view names now live in Settings.
const LEGACY = { devices: "devices", plugins: "plugins", inspector: "developer" };

async function flushPage({ unmountWidgets = false } = {}) {
  const saved = (await profile.measure("navigation/flush", () => Promise.all([views.page?.flush({ unmount: true }), rails?.flush({ unmount: unmountWidgets }), ...views.widgets.map(view => view.flush({ unmount: unmountWidgets }))]))).every((ok) => ok !== false);
  if (!saved) toast(document.querySelector('.pp-recorder[data-state="recording"], .pp-recorder[data-state="saving"]') ? "Stop and save your recording before leaving." : "Your changes are not saved yet. Keep the editor open and choose Retry save.", "error");
  return saved;
}

let viewSeq = 0;
async function showView(name) {
  if (LEGACY[name]) return showSettings(LEGACY[name]);
  const seq = ++viewSeq;
  if (name !== "plugin") pageSeq++; // A newer navigation cancels a pending page load.
  if (name !== currentView && !await flushPage()) return;
  if (seq !== viewSeq) return;
  rememberWorkspace();
  currentView = name;
  for (const view of document.querySelectorAll(".view")) view.hidden = view.id !== `view-${name}`;
  document.body.dataset.view = name;
  const plugin = pluginList.plugins.find((p) => p.id === currentPage?.plugin);
  $("workspace-context").textContent = `${current().name} / ${name === "plugin" ? plugin?.name || "App" : name[0].toUpperCase() + name.slice(1)}`;
  renderWorkspace();
  if (name === "settings") showSection(settingsSection);
  markNav();
  $("content").scrollTop = 0;
  window.scrollTo(0, 0);
}

function showSettings(section) {
  settingsSection = section;
  showView("settings");
}

function showSection(section) {
  renderWorkspace();
  for (const s of document.querySelectorAll(".settings-section")) s.hidden = s.id !== `settings-${section}`;
  if (section === "developer") refresh().catch((e) => showError(String(e)));
  if (section === "devices") loadSyncStatus();
  if (section === "spaces") loadSpaces();
  if (section === "backups") loadBackups();
  if (section === "ai") loadAi();
  if (section === "scheduled") loadSchedules();
  if (section === "appearance") {
    renderThemes();
    loadThemes();
    if (!gallery) loadGallery();
  }
  if (section === "plugins") {
    renderInstalled();
    loadSigners();
  }
}

for (const item of document.querySelectorAll("[data-view]:not([data-view='plugin'])")) {
  item.addEventListener("click", async () => {
    navigated = true;
    if (item.dataset.view === "home" && pages.home && !pages.isHome()) return showUserPage(pages.home.id);
    showView(item.dataset.view);
  });
}

for (const item of document.querySelectorAll("[data-settings]")) {
  item.addEventListener("click", () => {
    navigated = true;
    if (!$("space-sheet").hidden) closeOverlay("space-sheet");
    showSettings(item.dataset.settings);
  });
}

$("add-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const input = $("add-title");
  const title = input.value;
  await act("add_item", { title });
  if ($("error").hidden) input.value = "";
});

$("simulate").addEventListener("click", () => act("simulate_conflict"));

$("benchmark").addEventListener("click", async () => {
  const button = $("benchmark");
  const out = $("bench-results");
  button.disabled = true;
  button.textContent = "Running…";
  try {
    const b = await call("run_benchmark");
    const f = (n) => `${n.toFixed(1)} ms`;
    const rows = [
      ["Device", `${b.os} ${b.arch} · plugins run ${b.execution === "native" ? "as native code (JIT)" : "in the interpreter"}`],
      ["App startup to ready", f(b.startup_ms)],
      ["To-do load (compile + open)", f(b.plugin_load_ms)],
      [`Operation round trip (${b.calls}×)`, `avg ${f(b.call_avg_ms)} · p95 ${f(b.call_p95_ms)}`],
      ["Query (all tasks)", f(b.query_ms)],
      [`${b.inserts.toLocaleString()} inserts in one write`, f(b.insert_total_ms)],
    ];
    out.replaceChildren(...rows.flatMap(([k, v]) => [el("dt", {}, k), el("dd", {}, v)]));
    out.hidden = false;
  } catch (e) {
    showError(String(e));
  } finally {
    button.disabled = false;
    button.textContent = "Run benchmark";
  }
});

async function start() {
  // Plugins load in the background; listen before asking so none is missed.
  if (window.__TAURI__?.event) {
    await window.__TAURI__.event.listen("plugins-changed", async ({ payload }) => {
      if (payload.error) toast(`${payload.initial ? "Could not load" : "Reload failed"}: ${payload.error}`, "error");
      else if (!payload.dir) toast(`${payload.plugin} removed`, "ok");
      else if (!payload.initial) toast(`${payload.plugin} loaded`, "ok");
      await loadPlugins();
      if (!navigated && currentView !== "home") showView("home");
    });
  }
  if (window.__TAURI__?.event) {
    await window.__TAURI__.event.listen("sync-status", () => loadSyncStatus());
    await window.__TAURI__.event.listen("spaces-changed", () => loadSpaces());
    await window.__TAURI__.event.listen("data-changed", () => {
      refreshPluginViews();
      loadThemes();
      if (currentView === "settings" && settingsSection === "developer") refresh();
      if (currentView === "settings" && settingsSection === "scheduled" && !schedEditing) loadSchedules();
      if (currentView === "settings" && settingsSection === "ai") aiCall("ai_settings").then((st) => st && renderAi(st, false));
    });
  }
  const location = workspaceSession.read(currentSpace);
  await showView("home");
  await loadSpaces();
  await loadPlugins();
  await loadSyncStatus();
  await loadThemes();
  if (!navigated) await restoreWorkspace(location);
  // Android uses an HTTPS local origin. The backend alone decides whether to
  // emit opt-in duration-only diagnostics; regular builds emit nothing.
  if (window.__TAURI__) {
    await call("profile_startup", { frontendReadyMs: performance.now() });
  }
  return null;
}

pages = new window.TerrariumPages({
  el, call, icon, appTile, bridge, toast,
  pageIcons: ["layers", "star", "heart", "bolt", "target", "calendar", "clock", "list", "check", "note", "book", "bookmark", "folder", "inbox", "chart", "grid", "code", "link", "sparkle", "palette"],
  session: workspaceSession,
  mount: (root, template, viewBridge, options) => window.TerrariumRenderer.mount(root, template, viewBridge, options),
  openApp,
  show: showUserPage,
  legacyHome: () => ({ home: rails?.layout.home ?? null, homeWide: rails?.layout.homeWide || [] }),
  onChange: () => { renderGreeting(); pages.navigation(); markNav(); renderWorkspace(); },
});

rails = new window.TerrariumRails({
  el, call, icon, appTile, bridge, toast,
  onHomeLayout: () => renderHome(),
  flushLayout: flushPage,
  openHome: () => showView("home"),
  onNavigation: renderAppNav,
  openPlugin: (id) => { const plugin = pluginList.plugins.find((p) => p.id === id); if (plugin) openApp(plugin); },
  quickAction: (plugin, action) => openPalette({ action: { plugin, action }, mode: "add" }),
  settings: (section) => { rails.closeDrawer(); showSettings(section); },
});

profile.measure("startup/ready", start).catch((e) => {
  $("space-status").textContent = "Could not open space";
  showError(String(e));
});

window.addEventListener("beforeunload", (event) => {
  if (document.querySelector("form[data-pp-unsaved='true']")) {
    event.preventDefault();
    event.returnValue = "";
  }
});
document.addEventListener("visibilitychange", () => {
  if (document.hidden) flushPage();
});
