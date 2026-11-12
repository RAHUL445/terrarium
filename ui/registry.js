/* Device-local catalog view. Search never leaves this process. */
(function (root) {
  "use strict";
  function create({ $, el, icon, call, showReview, renderInstalled, phone = false, clock = Date.now }) {
    let state = { plugins: [], fresh: false, configured: false, automatic_checks: true };
    let busy = false;
    let expiryTimer;
    function capabilityLabel(id) {
      const labels = { notifications: "Show notifications", speech: "Transcribe audio locally", ai: "Send chosen text to your AI provider", "jobs:any": "Run scheduled work on your devices", "jobs:desktop": "Run scheduled work on your computer" };
      if (labels[id]) return labels[id];
      if (id.startsWith("uses:")) return `Read and change ${id.slice(5)} through other plugins`;
      if (id.startsWith("provides:")) return `Offer ${id.slice(9)} to other plugins`;
      if (id.startsWith("limits:")) return "Use more resources";
      return id;
    }
    function fresh() { return state.fresh && state.expires_ms > clock(); }
    function updateFor(id) { return fresh() ? state.plugins.find(p => p.id === id && p.update) : null; }
    function warningFor(id) { return state.plugins.find(p => p.id === id)?.warning; }
    function render() {
      const valid = fresh();
      $("registry-message").textContent = busy ? "Checking for plugins…" : phone ? "Plugin downloads are available on desktop." : state.fresh && !valid ? "This list is out of date." : state.message || "";
      $("registry-check").disabled = busy || !state.configured || phone;
      $("registry-cancel").hidden = !busy;
      $("registry-automatic").checked = state.automatic_checks;
      $("registry-automatic").disabled = phone;
      const count = valid ? state.plugins.filter(p => p.update).length : 0;
      for (const badge of document.querySelectorAll("[data-registry-badge]")) {
        badge.hidden = !count; badge.textContent = count ? String(count) : "";
      }
      const q = $("registry-search").value.trim().toLowerCase();
      const cards = state.plugins.filter(p => `${p.id} ${p.name} ${p.description}`.toLowerCase().includes(q));
      $("registry-list").replaceChildren(...(cards.length ? cards.map(p => el("li", { class: "pp-list-item plugin-row" },
        el("div", { class: "widget-head" }, icon(p.icon), el("strong", {}, p.name), el("span", { class: "muted" }, p.version)),
        el("p", {}, p.description),
        el("p", { class: "muted" }, `Publisher ${p.publisher}`),
        el("p", { class: "muted" }, p.capabilities.length ? `Can: ${p.capabilities.map(capabilityLabel).join("; ")}` : "No permissions requested"),
        p.warning ? el("p", { class: "muted" }, p.warning) : "",
        el("button", { type: "button", class: "pp-button", disabled: busy || !valid || !p.can_install || phone,
          onclick: () => stage(p.id, p.version) }, p.update ? "Update" : p.installed_version ? "Installed" : "Install")
      )) : [el("li", { class: "pp-empty" }, "No plugins found")]));
    }
    async function load(refresh = false) {
      if (phone || busy) { render(); return; }
      if (refresh) { busy = true; render(); }
      try {
        state = await call("registry_index", { refresh });
        clearTimeout(expiryTimer);
        if (state.fresh) expiryTimer = setTimeout(() => { render(); renderInstalled(); }, Math.max(0, state.expires_ms - clock()) + 1);
      } catch (e) { state.message = String(e); }
      finally { busy = false; render(); renderInstalled(); }
    }
    async function stage(id, version) {
      if (!fresh() || busy || phone) return;
      busy = true; render();
      $("registry-message").textContent = "Downloading plugin…";
      try { showReview(await call("registry_stage", { id, version })); }
      catch (_) { state.message = "Couldn't verify this plugin. Check for plugins and try again."; }
      finally { busy = false; render(); }
    }
    $("registry-search").addEventListener("input", render);
    $("registry-check").addEventListener("click", () => load(true));
    $("registry-cancel").addEventListener("click", () => call("registry_cancel").catch(() => {}));
    $("registry-automatic").addEventListener("change", async () => {
      try { await call("registry_settings", { automaticChecks: $("registry-automatic").checked }); await load(); }
      catch (e) { state.message = String(e); render(); }
    });
    for (const tab of document.querySelectorAll("[data-plugin-tab]")) tab.addEventListener("click", () => {
      const browse = tab.dataset.pluginTab === "browse";
      $("registry-browse").hidden = !browse;
      $("plugins-installed").hidden = browse;
      for (const t of document.querySelectorAll("[data-plugin-tab]")) t.setAttribute("aria-selected", String(t === tab));
      if (browse) load(false);
    });
    return { load, render, stage, updateFor, warningFor };
  }
  root.TerrariumRegistry = { create };
})(typeof module === "object" ? module.exports : window);
