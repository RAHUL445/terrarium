// In-app confirmation. The native WebView (WKWebView in Tauri) has no
// window.confirm panel and answers `false` without asking, so every
// destructive action asks through this dialog instead.
(function () {
  "use strict";
  let open = null;

  function build() {
    const dialog = document.createElement("dialog");
    dialog.className = "confirm-dialog";
    dialog.setAttribute("aria-labelledby", "confirm-dialog-title");
    dialog.setAttribute("aria-describedby", "confirm-dialog-message");
    const form = document.createElement("form");
    form.method = "dialog";
    const title = document.createElement("h2");
    title.id = "confirm-dialog-title";
    const message = document.createElement("p");
    message.id = "confirm-dialog-message";
    const actions = document.createElement("div");
    actions.className = "confirm-dialog-actions";
    const cancel = document.createElement("button");
    cancel.type = "submit";
    cancel.value = "cancel";
    const ok = document.createElement("button");
    ok.type = "submit";
    ok.value = "ok";
    actions.append(cancel, ok);
    form.append(title, message, actions);
    dialog.append(form);
    document.body.append(dialog);
    return { dialog, title, message, cancel, ok };
  }

  function buildAsk() {
    const dialog = document.createElement("dialog");
    dialog.className = "confirm-dialog";
    dialog.setAttribute("aria-labelledby", "ask-dialog-title");
    dialog.setAttribute("aria-describedby", "ask-dialog-message");
    const form = document.createElement("form");
    form.method = "dialog";
    const title = document.createElement("h2");
    title.id = "ask-dialog-title";
    const message = document.createElement("p");
    message.id = "ask-dialog-message";
    const field = document.createElement("label");
    const prompt = document.createElement("span");
    const input = document.createElement("input");
    input.type = "text";
    input.className = "pp-input";
    input.maxLength = 200;
    field.append(prompt, input);
    const actions = document.createElement("div");
    actions.className = "confirm-dialog-actions";
    const cancel = document.createElement("button");
    cancel.type = "submit";
    cancel.value = "cancel";
    const ok = document.createElement("button");
    ok.type = "submit";
    ok.value = "ok";
    actions.append(cancel, ok);
    form.append(title, message, field, actions);
    dialog.append(form);
    document.body.append(dialog);
    return { dialog, title, message, field, prompt, input, cancel, ok };
  }

  function closeOpen(defaultValue) {
    if (!open) return;
    open.resolve(defaultValue);
  }

  // ask("Delete this?", { title, confirm: "Delete", cancel: "Keep", danger: true })
  // resolves true only when the person picks the confirm button.
  window.terrariumConfirm = function (message, options = {}) {
    if (typeof document === "undefined" || !document.body || typeof HTMLDialogElement === "undefined") {
      return Promise.resolve(typeof window.confirm === "function" ? window.confirm(message) : false);
    }
    closeOpen(false);
    const ui = window.terrariumConfirm.ui || (window.terrariumConfirm.ui = build());
    ui.title.textContent = options.title || "Are you sure?";
    ui.title.hidden = !options.title && !message;
    ui.message.textContent = message || "";
    ui.cancel.textContent = options.cancel || "Cancel";
    ui.ok.textContent = options.confirm || "OK";
    ui.ok.className = options.danger ? "danger-solid" : "primary";
    return new Promise((resolve) => {
      const done = (value) => {
        if (open?.resolve !== finish) return;
        open = null;
        resolve(value);
      };
      const finish = done;
      open = { resolve: finish };
      ui.dialog.returnValue = "";
      ui.dialog.onclose = () => done(ui.dialog.returnValue === "ok");
      ui.dialog.showModal();
      // Destructive choices start on Cancel so Enter never deletes by accident.
      (options.danger ? ui.cancel : ui.ok).focus();
    });
  };

  // ask("Tag", { title, label: "Tag", value: "work", confirm: "Add tag" })
  // resolves to trimmed text, or null when cancelled or empty.
  window.terrariumAsk = function (message, options = {}) {
    if (typeof document === "undefined" || !document.body || typeof HTMLDialogElement === "undefined") {
      if (typeof window.prompt !== "function") return Promise.resolve(null);
      const raw = window.prompt(message || "", options.value || "");
      const value = raw == null ? null : String(raw).trim();
      return Promise.resolve(value || null);
    }
    closeOpen(null);
    const ui = window.terrariumAsk.ui || (window.terrariumAsk.ui = buildAsk());
    ui.title.textContent = options.title || "";
    ui.title.hidden = !options.title;
    ui.message.textContent = message || "";
    ui.message.hidden = !message;
    ui.field.hidden = !(options.label || options.title || message);
    ui.prompt.textContent = options.label || "";
    ui.input.value = options.value || "";
    ui.cancel.textContent = options.cancel || "Cancel";
    ui.ok.textContent = options.confirm || "OK";
    ui.ok.className = "primary";
    return new Promise((resolve) => {
      const done = (value) => {
        if (open?.resolve !== finish) return;
        open = null;
        resolve(value);
      };
      const finish = done;
      open = { resolve: finish };
      ui.dialog.returnValue = "";
      ui.dialog.onclose = () => {
        if (ui.dialog.returnValue !== "ok") {
          done(null);
          return;
        }
        const value = String(ui.input.value || "").trim();
        done(value || null);
      };
      ui.dialog.showModal();
      ui.input.focus();
      ui.input.select?.();
    });
  };
})();
