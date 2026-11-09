// The host owns the rails. Plugins contribute existing declared pages,
// widgets and quick actions; preferences contain IDs, never executable UI.
(function () {
  "use strict";
  window.TerrariumRails = class {
    constructor(options) {
      this.o = options;
      this.el = options.el;
      this.layout = window.TerrariumLayout.normalize(null);
      this.space = null;
      this.plugins = [];
      this.catalog = { nav: [], widgets: [], actions: [], home: [] };
      this.mounted = new Map();
      this.drafts = new Map();
      try { this.storage = localStorage; } catch (_) { this.storage = { getItem: () => null, setItem: () => { throw Error("unavailable"); } }; }
      this.seq = 0;
      this.tab = "pinned";
      this.conversations = new Map();
      this.events = [];
      this.activitySeq = 0;
      this.contextSeq = 0;
      this.mode = "nav";
      this.compact = matchMedia("(max-width: 1119px)");
      this.phone = matchMedia("(max-width: 760px)");
      this.opened = false;
      this.navigationExpanded = this.readPreference("pp-navigation-expanded", "false") === "true";
      this.$("sidebar-toggle").addEventListener("click", () => {
        this.navigationExpanded = !this.navigationExpanded;
        this.remember("pp-navigation-expanded", String(this.navigationExpanded));
        this.renderChrome();
      });
      this.compact.addEventListener("change", () => this.renderChrome());
      this.phone.addEventListener("change", () => this.renderChrome());
      document.querySelectorAll("[data-companion-toggle]").forEach((button) => button.addEventListener("click", () => this.setOpen(!this.isOpen())));
      this.$("companion-close").addEventListener("click", () => this.setOpen(false));
      this.$("companion-backdrop").addEventListener("click", () => this.setOpen(false));
      document.querySelectorAll("[data-rail-tab]").forEach((button) => button.addEventListener("click", () => { if (!this.phone.matches && this.isOpen() && this.tab === button.dataset.railTab) this.setOpen(false); else this.showTab(button.dataset.railTab); }));
      const tabs = [...document.querySelectorAll("[data-rail-tab][role='tab']")];
      tabs.forEach((button, index) => {
        button.tabIndex = index === 0 ? 0 : -1;
        button.addEventListener("keydown", (event) => {
          if (!["ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
          event.preventDefault();
          const next = event.key === "Home" ? tabs[0] : event.key === "End" ? tabs[tabs.length - 1] : tabs[(index + (["ArrowRight", "ArrowDown"].includes(event.key) ? 1 : tabs.length - 1)) % tabs.length];
          this.showTab(next.dataset.railTab); next.focus();
        });
      });
      window.addEventListener("popstate", (event) => {
        if (this.compact.matches) this.setOpen(event.state?.ppCompanion === true, { fromHistory: true });
      });
      document.querySelectorAll("[data-customize-rail]").forEach((button) => button.addEventListener("click", () => this.customize(button.dataset.customizeRail)));
      document.querySelectorAll("[data-rail-mode]").forEach((button) => button.addEventListener("click", () => this.customize(button.dataset.railMode)));
      this.$("rail-custom-done").addEventListener("click", async () => {
        const space = this.space.id;
        if (!await this.o.flushLayout() || space !== this.space.id) return;
        this.editing = false; this.originalLayout = null; this.saveLayout(); this.showTab("pinned"); this.o.onHomeLayout(); (this.compact.matches ? this.$("companion-close") : this.$("rail-pinned-tab")).focus();
      });
      this.$("rail-custom-cancel").addEventListener("click", async () => { await this.showTab("pinned"); (this.compact.matches ? this.$("companion-close") : this.$("rail-pinned-tab")).focus(); });
      document.querySelectorAll("[data-rail-settings]").forEach((button) => button.addEventListener("click", () => this.o.settings(button.dataset.railSettings)));
      this.$("rail-assistant-form").addEventListener("submit", (event) => { event.preventDefault(); this.sendAssistant(); });
      this.$("rail-message").addEventListener("input", () => { this.conversation().draft = this.$("rail-message").value; });
      this.$("rail-attach-tasks").addEventListener("change", () => {
        this.conversation().include = this.$("rail-attach-tasks").checked;
        this.previewContext();
      });
      this.$("rail-plan-day").addEventListener("click", () => {
        this.conversation().draft = "Help me plan the rest of my day.";
        this.renderConversation(); this.$("rail-message").focus();
      });
      this.$("rail-assistant-retry").addEventListener("click", () => {
        const state = this.conversation();
        if (state.pending && state.pollError) this.pollAssistant(state, this.space.id);
        else this.sendAssistant(true);
      });
      this.$("rail-chat-clear").addEventListener("click", () => {
        if (this.conversation().pending) return;
        this.conversations.delete(this.space.id); this.renderConversation(); this.previewContext();
      });
      this.$("rail-activity-refresh").addEventListener("click", () => this.loadActivity());
      this.$("rail-reset").addEventListener("click", async () => {
        const space = this.space.id;
        if (!await this.o.flushLayout() || space !== this.space.id) return;
        this.layout = window.TerrariumLayout.normalize(null);
        this.saveLayout(); this.renderPins(); this.o.onHomeLayout(); this.renderCustom();
      });
      document.addEventListener("keydown", (event) => {
        if (!this.drawerOpen() || document.querySelector("dialog[open]") || !this.$("palette").hidden || !this.$("space-sheet").hidden) return;
        if (event.key === "Escape") { event.preventDefault(); this.setOpen(false); }
        if (event.key === "Tab") {
          const fields = [...this.$("companion").querySelectorAll("button:not(:disabled), input, textarea, select, summary")].filter((node) => node.getClientRects().length);
          const first = fields[0], last = fields[fields.length - 1];
          if (first && (!this.$("companion").contains(document.activeElement) || (event.shiftKey && document.activeElement === first) || (!event.shiftKey && document.activeElement === last))) {
            event.preventDefault(); (event.shiftKey ? last : first).focus();
          }
        }
      });
      this.renderChrome();
    }
    $(id) { return document.getElementById(id); }
    readPreference(key, fallback) { try { return localStorage.getItem(key) || fallback; } catch (_) { return fallback; } }
    remember(key, value) { try { localStorage.setItem(key, value); } catch (_) {} }
    saveLayout() {
      if (this.editing) return; // Preview until Apply; Cancel restores the snapshot.
      if (!window.TerrariumLayout.write(this.storage, this.space.id, this.layout)) this.o.toast("Layout changed for this session; device storage is unavailable.", "error");
    }
    ids(kind) {
      const items = this.catalog[kind];
      return window.TerrariumLayout.selected(this.layout[kind], items.map((item) => item.key), items.filter((item) => item.default !== false).map((item) => item.key));
    }
    navigation(plugins) {
      const byId = new Map(plugins.map((plugin) => [plugin.id, plugin]));
      return this.ids("nav").map((id) => byId.get(id)).filter(Boolean);
    }
    setSpace(space) {
      if (space.id === this.space?.id) { this.space = space; this.renderChrome(); return; }
      for (const [key, mounted] of this.mounted) if (this.space && mounted.view) this.drafts.set(`${this.space.id}/${key}`, mounted.view.snapshot());
      this.editing = false; this.originalLayout = null;
      this.space = space;
      this.seq++;
      for (const { node, view } of this.mounted.values()) { view?.dispose(); node.remove(); }
      this.mounted.clear();
      this.$("rail-widgets").setAttribute("aria-busy", "true");
      this.layout = window.TerrariumLayout.read(this.storage, space.id);
      this.renderChrome();
      this.renderConversation();
      this.previewContext();
      this.loadAi();
      if (this.tab === "activity") this.loadActivity();
      if (this.plugins.length) this.update(this.plugins);
    }
    async update(plugins) {
      const seq = ++this.seq;
      if (!await this.flush({ unmount: true }) || seq !== this.seq) return;
      this.plugins = plugins;
      const items = await Promise.all(plugins.flatMap((plugin) => plugin.widgets.map(async (widget) => {
        try {
          const template = await this.o.call("view_template", { plugin: plugin.id, kind: "widget", id: widget.id });
          return { key: `${plugin.id}/${widget.id}`, plugin, widget, template, name: widget.title, default: template.rail?.default === true };
        } catch (error) {
          return { key: `${plugin.id}/${widget.id}`, plugin, widget, name: widget.title, error, default: false };
        }
      })));
      if (seq !== this.seq) return;
      this.catalog.nav = plugins.filter((p) => p.pages.length).map((p) => ({ key: p.id, plugin: p, name: p.name, default: true }));
      this.catalog.widgets = items.sort((a, b) => (a.template?.rail?.order ?? a.template?.home?.order ?? 100) - (b.template?.rail?.order ?? b.template?.home?.order ?? 100));
      this.catalog.home = items.slice().sort((a, b) => (a.template?.home?.order ?? 100) - (b.template?.home?.order ?? 100)).map((item) => ({ ...item, default: !item.template?.home?.hidden }));
      this.catalog.actions = plugins.flatMap((plugin) => plugin.quick_actions.map((action) => ({ key: `${plugin.id}/${action.op}/${action.arg}`, plugin, action, name: action.label, default: true })));
      for (const [key, mounted] of this.mounted) {
        const item = items.find((entry) => entry.key === key);
        const stamp = item && `${this.space.id}:${item.plugin.version}:${JSON.stringify(item.template)}`;
        if (!item || mounted.stamp !== stamp) { mounted.view?.dispose(); mounted.node.remove(); this.mounted.delete(key); }
      }
      this.renderPins();
      this.renderCustom();
      this.$("rail-widgets").setAttribute("aria-busy", "false");
    }
    renderPins() {
      this.o.onNavigation();
      const target = this.$("rail-widgets");
      const chosen = this.ids("widgets");
      for (const [key, mounted] of this.mounted) mounted.node.hidden = !chosen.includes(key);
      for (const key of chosen) {
        let mounted = this.mounted.get(key);
        if (mounted && !mounted.stamp.startsWith(`${this.space.id}:`)) {
          mounted.view?.dispose(); mounted.node.remove(); this.mounted.delete(key); mounted = null;
        }
        if (!mounted) {
          const item = this.catalog.widgets.find((entry) => entry.key === key);
          const body = this.el("div", { class: "rail-widget-body" });
          const node = this.el("details", { class: "rail-widget", open: !this.layout.folded.includes(key) },
            this.el("summary", {}, this.o.appTile(item.plugin, "xs"), this.el("span", {}, item.name), this.el("small", {}, item.plugin.name)), body);
          node.addEventListener("toggle", () => {
            const next = window.TerrariumLayout.toggle(this.layout.folded, key, !node.open);
            if (JSON.stringify(next) !== JSON.stringify(this.layout.folded)) { this.layout.folded = next; this.saveLayout(); }
          });
          target.append(node);
          const view = item.error ? null : window.TerrariumRenderer.mount(body, item.template, this.o.bridge(item.plugin.id), { snapshot: this.drafts.get(`${this.space.id}/${key}`) });
          if (item.error) body.append(this.el("p", { class: "error", role: "alert" }, `Couldn’t load ${item.name}. Reopen the app to retry.`));
          mounted = { node, view, stamp: `${this.space.id}:${item.plugin.version}:${JSON.stringify(item.template)}` };
          this.mounted.set(key, mounted);
        }
        mounted.node.hidden = false;
        target.append(mounted.node);
      }
      this.$("rail-empty").hidden = chosen.length > 0;
      const actions = this.ids("actions").map((id) => this.catalog.actions.find((item) => item.key === id));
      this.$("rail-actions-list").replaceChildren(...actions.map((item) => this.el("button", {
        type: "button", class: "rail-action", onclick: () => { this.closeDrawer(); this.o.quickAction(item.plugin, item.action); },
      }, this.o.appTile(item.plugin, "xs"), this.el("span", {}, item.name), this.el("small", {}, item.plugin.name))));
      this.$("rail-actions-empty").hidden = actions.length > 0;
    }
    async customize(mode) {
      const space = this.space?.id;
      if (!space || !await this.o.flushLayout() || space !== this.space.id) return;
      if (mode === "home") await this.o.openHome();
      if (space !== this.space.id) return;
      if (!this.editing) { this.originalLayout = structuredClone(this.layout); this.editing = true; }
      this.mode = mode; this.showTab("customize"); this.renderCustom(); this.o.onHomeLayout();
    }
    renderCustom(focusKey = null) {
      const mode = this.mode, space = this.space?.id;
      document.querySelectorAll("[data-rail-mode]").forEach((button) => button.setAttribute("aria-pressed", String(button.dataset.railMode === this.mode)));
      const selected = this.ids(mode);
      this.$("rail-layout-preview").textContent = `Preview: ${this.ids("home").length} Home widgets · ${this.ids("nav").length} apps · ${this.ids("widgets").length} widgets · ${this.ids("actions").length} quick actions. Apply to keep changes.`;
      const items = this.catalog[mode];
      const ordered = [...selected.map((id) => items.find((item) => item.key === id)), ...items.filter((item) => !selected.includes(item.key))];
      this.$("rail-custom-list").replaceChildren(...ordered.map((item) => {
        const checked = selected.includes(item.key), index = selected.indexOf(item.key);
        const input = this.el("input", { type: "checkbox", "aria-label": `${item.name} in ${this.mode === "home" ? "Home" : this.mode === "nav" ? "navigation" : this.mode === "widgets" ? "widgets" : "quick actions"}` });
        input.dataset.railKey = item.key;
        input.checked = checked;
        input.addEventListener("change", async () => {
          if (!await this.o.flushLayout() || space !== this.space.id) { input.checked = checked; return; }
          this.layout[mode] = window.TerrariumLayout.toggle(selected, item.key, input.checked);
          this.saveLayout(); this.renderPins(); if (mode === "home") this.o.onHomeLayout(); this.renderCustom(item.key);
        });
        return this.el("div", { class: "rail-custom-row" },
          this.el("label", {}, input, this.el("span", {}, item.name, this.el("small", {}, item.plugin.name))),
          ...[-1, 1].map((delta) => this.el("button", {
            type: "button", class: "icon-button", disabled: !checked || index + delta < 0 || index + delta >= selected.length,
            "aria-label": `Move ${item.name} ${delta < 0 ? "up" : "down"}`,
            onclick: async () => { if (!await this.o.flushLayout() || space !== this.space.id) return; this.layout[mode] = window.TerrariumLayout.move(selected, item.key, delta); this.saveLayout(); this.renderPins(); if (mode === "home") this.o.onHomeLayout(); this.renderCustom(item.key); },
          }, this.o.icon(delta < 0 ? "chevron-up" : "chevron-down"))),
          ...(mode === "home" && checked ? [this.el("button", { type: "button", "aria-label": `Wide ${item.name}`, "aria-pressed": String(this.layout.homeWide.includes(item.key)), onclick: async () => {
            if (!await this.o.flushLayout() || space !== this.space.id) return;
            this.layout.homeWide = window.TerrariumLayout.toggle(this.layout.homeWide, item.key, !this.layout.homeWide.includes(item.key));
            this.o.onHomeLayout(); this.renderCustom(item.key);
          } }, "Wide")] : []));
      }));
      if (focusKey) [...this.$("rail-custom-list").querySelectorAll("input")].find((input) => input.dataset.railKey === focusKey)?.focus();
    }
    isOpen() { return this.compact.matches ? !!this.drawer : this.opened; }
    drawerOpen() { return this.compact.matches && !!this.drawer; }
    closeDrawer() { if (this.drawerOpen()) this.setOpen(false); }
    setOpen(open, { fromHistory = false } = {}) {
      if (open && !this.isOpen()) this.returnFocus = document.activeElement;
      if (this.compact.matches) {
        if (!fromHistory && open && !this.drawer) history.pushState({ ...history.state, ppCompanion: true }, "");
        if (!fromHistory && !open && this.drawer && history.state?.ppCompanion) history.back();
        this.drawer = open;
      } else { this.opened = open;  }
      this.renderChrome();
      if (this.drawerOpen()) this.$("companion-close").focus();
      else if (!open && !this.compact.matches) {
        (this.$(`rail-${this.tab}-tab`) || this.$("rail-assistant-tab")).focus();
        this.returnFocus = null;
      }
      else if (this.returnFocus?.isConnected) { this.returnFocus.focus(); this.returnFocus = null; }
    }
    renderChrome() {
      const tools = this.$("companion-tools");
      const companion = this.$("companion");
      if (this.phone.matches && tools.parentElement !== companion) companion.insertBefore(tools, companion.querySelector("[data-rail-panel]"));
      else if (!this.phone.matches && tools.parentElement === companion) document.querySelector(".shell").insertBefore(tools, document.querySelector(".tabbar"));
      tools.setAttribute("aria-orientation", this.phone.matches ? "horizontal" : "vertical");
      document.documentElement.dataset.sidebar = this.navigationExpanded ? "expanded" : "fixed";
      const navigationToggle = this.$("sidebar-toggle");
      navigationToggle.setAttribute("aria-expanded", String(this.navigationExpanded));
      navigationToggle.setAttribute("aria-label", this.navigationExpanded ? "Collapse navigation" : "Expand navigation");
      navigationToggle.title = navigationToggle.getAttribute("aria-label");
      document.documentElement.dataset.companion = this.isOpen() ? "open" : "closed";
      document.querySelectorAll("[data-rail-tab][role='tab']").forEach((button, index) => {
        const selected = this.isOpen() && button.dataset.railTab === this.tab;
        button.setAttribute("aria-selected", String(selected));
        button.tabIndex = selected || (index === 0 && (!this.isOpen() || !["assistant", "activity", "actions", "pinned"].includes(this.tab))) ? 0 : -1;
      });
      this.$("companion-space").textContent = this.space?.name || "Personal";
      this.$("rail-layout-scope").textContent = `${this.space?.name || "Personal"} · this device`;
      this.$("companion").hidden = !this.isOpen();
      this.$("companion-backdrop").hidden = !this.drawerOpen();
      this.$("companion-tools").hidden = false;
      this.$("companion").setAttribute("role", this.drawerOpen() ? "dialog" : "complementary");
      if (this.drawerOpen()) this.$("companion").setAttribute("aria-modal", "true");
      else this.$("companion").removeAttribute("aria-modal");
      document.querySelectorAll("[data-companion-toggle]").forEach((button) => {
        button.setAttribute("aria-expanded", String(this.isOpen()));
        button.setAttribute("aria-label", this.isOpen() ? "Close companion" : "Open companion");
        button.title = this.isOpen() ? "Close companion" : "Open companion";
      });
      for (const node of document.querySelectorAll(".main, .sidebar, .tabbar, .fab, .workspace-status, .workspace-bar")) node.inert = this.drawerOpen();
      tools.inert = !this.phone.matches && this.drawerOpen();
    }
    async showTab(name) {
      if (name !== "customize" && this.editing) {
        const space = this.space.id;
        if (!await this.o.flushLayout() || space !== this.space.id) return;
        this.layout = this.originalLayout; this.originalLayout = null; this.editing = false;
        this.renderPins(); this.o.onHomeLayout(); this.renderCustom();
      }
      this.tab = name;
      if (!this.isOpen()) this.setOpen(true);
      document.querySelectorAll("[data-rail-tab][role='tab']").forEach((button, index) => {
        button.setAttribute("aria-selected", String(button.dataset.railTab === name));
        button.tabIndex = button.dataset.railTab === name || (!["pinned", "assistant", "activity"].includes(name) && index === 0) ? 0 : -1;
      });
      document.querySelectorAll("[data-rail-panel]").forEach((panel) => panel.hidden = panel.dataset.railPanel !== name);
      if (name === "assistant") { this.loadAi(); this.renderConversation(); this.previewContext(); }
      if (name === "activity") this.loadActivity();
    }
    conversation() {
      const id = this.space?.id || "personal";
      if (!this.conversations.has(id)) this.conversations.set(id, { messages: [], draft: "", include: false, pending: false, id: null, error: null, pollError: false, lastPayload: null, sources: "" });
      return this.conversations.get(id);
    }
    service(name, args) { return this.o.call("service", { name, args }); }
    async loadAi() {
      try { this.setAiState(await this.service("ai_settings")); }
      catch (_) {
        this.$("rail-ai-status").textContent = "Couldn’t read AI settings. Open AI settings to retry.";
        this.$("rail-send").disabled = true;
      }
    }
    setAiState(state) {
      this.aiState = state;
      const provider = state.presets.find((item) => item.id === state.config.preset)?.name || "Custom provider";
      let host = "";
      try { host = new URL(state.config.base_url).host; } catch (_) {}
      this.$("rail-ai-status").textContent = state.status.ready
        ? `${provider}${host ? ` (${host})` : ""} · ${state.config.model}`
        : state.status.reason || "Set up AI in Settings.";
      this.renderConversation();
    }
    renderConversation() {
      const state = this.conversation();
      this.$("rail-chat").replaceChildren(...state.messages.map((message) => {
        const body = message.role === "assistant"
          ? window.TerrariumRenderer.markdown(message.content, { open: (url) => this.o.call("open_external", { url }).catch((error) => this.o.toast(String(error), "error")) })
          : this.el("p", { class: "rail-user-text" }, message.content);
        return this.el("section", { class: "rail-chat-message" }, this.el("strong", {}, message.role === "user" ? "You" : "Assistant"), body,
          message.model ? this.el("small", { class: "muted" }, `Written with ${message.model}`) : "");
      }));
      this.$("rail-chat-empty").hidden = state.messages.length > 0;
      this.$("rail-plan-day").hidden = state.messages.length > 0;
      if (this.$("rail-message").value !== state.draft) this.$("rail-message").value = state.draft;
      this.$("rail-attach-tasks").checked = state.include;
      this.$("rail-send").disabled = state.pending || !this.aiState?.status.ready;
      this.$("rail-send").textContent = state.pending ? "Waiting for reply…" : "Send";
      this.$("rail-chat-clear").disabled = state.pending || !state.messages.length;
      this.$("rail-assistant-error").hidden = !state.error;
      this.$("rail-assistant-error").textContent = state.error || "";
      this.$("rail-assistant-retry").hidden = !state.error || !state.lastPayload;
      this.$("rail-assistant-retry").textContent = state.pending ? "Check reply again" : "Retry last message";
      const status = this.$("rail-reply-state");
      status.hidden = !state.pending && !state.sources;
      status.textContent = state.pending ? "The request runs on this device while the app is open. You can keep working." : state.sources;
    }
    async previewContext() {
      const state = this.conversation(), space = this.space?.id, seq = ++this.contextSeq;
      this.$("rail-context").hidden = !state.include;
      this.$("rail-context-list").replaceChildren();
      if (!state.include || !space) return;
      this.$("rail-context-status").textContent = "Reading today and overdue task titles…";
      try {
        const context = await this.service("assistant_context", { space });
        if (seq !== this.contextSeq || this.space.id !== space || !state.include) return;
        this.$("rail-context-status").textContent = context.sources;
        this.$("rail-context-list").replaceChildren(...context.tasks.map((task) => this.el("li", {}, task.title, task.due ? ` · ${task.due}` : "")));
      } catch (error) {
        if (seq === this.contextSeq) this.$("rail-context-status").textContent = String(error);
      }
    }
    async sendAssistant(retry = false) {
      const space = this.space.id, state = this.conversation();
      if (state.pending) return;
      const content = state.draft.trim();
      if (!retry && !content) { this.$("rail-message").focus(); return; }
      const nonce = () => window.crypto?.randomUUID?.() || `${Date.now()}-${Math.random().toString(36).slice(2)}`;
      const payload = retry && state.lastPayload ? { ...state.lastPayload, client_id: state.completedFailure ? nonce() : state.lastPayload.client_id } : {
        space, messages: [...state.messages.slice(-18).map(({ role, content }) => ({ role, content })), { role: "user", content }], include_today: state.include, client_id: nonce(),
      };
      if (!retry) state.userAppended = false;
      state.completedFailure = false;
      state.pending = true; state.pollError = false; state.error = null; state.lastPayload = payload; state.id = null;
      this.renderConversation();
      try {
        const made = await this.service("assistant_submit", payload);
        if (!state.userAppended) { state.messages.push({ role: "user", content: payload.messages[payload.messages.length - 1].content }); state.userAppended = true; }
        state.id = made.id; state.sources = made.sources; state.draft = "";
        if (this.space.id === space) this.renderConversation();
        this.pollAssistant(state, space);
      } catch (error) {
        state.pending = false; state.error = String(error);
        if (this.space.id === space) this.renderConversation();
      }
    }
    async pollAssistant(state, space) {
      if (state.polling || !state.id) return;
      state.polling = true; state.pollError = false;
      try {
        const reply = await this.service("assistant_reply", { space, id: state.id });
        if (reply.status === "done") {
          state.messages.push({ role: "assistant", content: reply.text, model: reply.model });
          state.messages = state.messages.slice(-40);
          state.pending = false; state.error = null; state.sources = reply.sources;
          this.record({ space, title: "Assistant reply ready", at: Date.now() });
          this.loadAi();
        } else if (reply.status === "failed") {
          state.completedFailure = true;
          state.pending = false; state.error = reply.error || "AI could not reply. Retry the message or check AI settings.";
          this.record({ space, title: "Assistant request failed", at: Date.now() });
          this.loadAi();
        } else setTimeout(() => this.pollAssistant(state, space), 1000);
      } catch (error) {
        state.pollError = true; state.error = `Couldn’t check the reply: ${String(error)}`;
      } finally {
        state.polling = false;
        if (this.space.id === space) this.renderConversation();
      }
    }
    record(event) {
      this.events.unshift(event); this.events = this.events.slice(0, 100);
      if (this.tab === "activity") this.loadActivity();
    }
    setSync(text, state) { this.sync = { text, state }; }
    async loadActivity() {
      const seq = ++this.activitySeq, space = this.space?.id;
      if (!space) return;
      this.$("rail-activity-error").hidden = true;
      let runs = [];
      try {
        const data = await this.service("schedules");
        runs = data.runs.filter((run) => run.space === space).slice(0, 25).map((run) => ({
          title: `${run.schedule_name || "Scheduled work"} · ${({ done: "Done", failed: "Failed", retry: "Will retry", running: "Running", waiting: "Waiting for AI", skipped: "Skipped" })[run.status] || run.status}`,
          detail: run.error || "", at: run.finished_ms || run.started_ms || run.slot_ms, run,
        }));
      } catch (_) {
        if (seq === this.activitySeq) {
          this.$("rail-activity-error").hidden = false;
          this.$("rail-activity-error").textContent = "Couldn’t load scheduled activity. Refresh to retry.";
        }
      }
      if (seq !== this.activitySeq || this.space.id !== space) return;
      const events = [...runs, ...this.events.filter((event) => event.space === space)].sort((a, b) => b.at - a.at).slice(0, 30);
      const renderEvent = (event) => {
        const actions = [];
        if (event.run?.plugin && this.plugins.some((plugin) => plugin.id === event.run.plugin)) actions.push(this.el("button", { type: "button", class: "link-button", onclick: () => { this.closeDrawer(); this.o.openPlugin(event.run.plugin); } }, "Open app"));
        if (event.run?.status === "failed") actions.push(this.el("button", { type: "button", onclick: async (e) => {
          e.currentTarget.disabled = true;
          try { await this.service("schedule_retry", { run: event.run.id }); await this.loadActivity(); }
          catch (error) { this.o.toast(String(error), "error"); e.target.disabled = false; }
        } }, "Retry"));
        return this.el("article", { class: "rail-event" }, this.el("strong", {}, event.title),
          this.el("small", { class: "muted" }, new Date(event.at).toLocaleString()), event.detail ? this.el("p", {}, event.detail) : "", ...actions);
      };
      const groups = new Map();
      for (const event of events) {
        const key = `${new Date(event.at).toDateString()}:${event.title}`;
        if (!groups.has(key)) groups.set(key, []);
        groups.get(key).push(event);
      }
      const nodes = [...groups.values()].map(group => group.length === 1 ? renderEvent(group[0]) : this.el("details", { class: "rail-event-group" },
        this.el("summary", {}, `${group[0].title} · ${group.length} events`, this.el("small", { class: "muted" }, new Date(group[0].at).toLocaleDateString())), ...group.map(renderEvent)));

      if (!events.length) nodes.push(this.el("p", { class: "muted" }, "Completed actions in this session and scheduled runs in this space appear here."));
      this.$("rail-activity-list").replaceChildren(...nodes);
    }
    refresh() {
      for (const { view } of this.mounted.values()) view?.refresh();
      if (this.tab === "activity") this.loadActivity();
      if (this.tab === "assistant") this.loadAi();
    }
    async flush(options) { return (await Promise.all([...this.mounted.values()].map(({ view }) => view?.flush(options)))).every((ok) => ok !== false); }
  };
})();
