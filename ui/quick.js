(function (root) {
  "use strict";

  const invoke = root.__TAURI__ ? root.__TAURI__.core.invoke : null;
  const events = root.__TAURI__ ? root.__TAURI__.event : null;

  function actionsFrom(pluginList) {
    const plugins = Array.isArray(pluginList?.plugins) ? pluginList.plugins : [];
    const out = [];
    for (const plugin of plugins) {
      const actions = Array.isArray(plugin?.quick_actions) ? plugin.quick_actions : [];
      for (const action of actions) {
        if (!action?.op || !action?.arg || !action?.label) continue;
        out.push({
          key: `${plugin.id}/${action.op}/${action.arg}`,
          label: `${action.label} · ${plugin.name}`,
          plugin: plugin.id,
          name: plugin.name,
          op: action.op,
          arg: action.arg,
        });
      }
    }
    return out.sort((a, b) => Number(b.plugin === "tasks") - Number(a.plugin === "tasks"));
  }

  function argsFor(action, text) {
    return { [action.arg]: text };
  }

  function storageGet(key) {
    try {
      return (window.terrariumStorage || localStorage).getItem(key);
    } catch (_) {
      return null;
    }
  }

  function storageSet(key, value) {
    try {
      (window.terrariumStorage || localStorage).setItem(key, value);
    } catch (_) {}
  }

  function expose() {
    return { actionsFrom, argsFor };
  }

  if (typeof module !== "undefined" && module.exports) {
    module.exports = expose();
    return;
  }

  const $ = (id) => document.getElementById(id);
  const form = $("quick-form");
  const textInput = $("quick-text");
  const actionSelect = $("quick-action");
  const status = $("quick-status");
  const errorBox = $("quick-error");
  let actions = [];

  function setError(message) {
    errorBox.textContent = message || "";
    errorBox.hidden = !message;
  }

  function setStatus(message) {
    status.textContent = message || "";
  }

  async function loadActions() {
    setError("");
    setStatus("");
    if (!invoke) return;
    const pluginList = await invoke("plugins");
    actions = actionsFrom(pluginList);
    const last = storageGet("quick-capture-action");
    actionSelect.replaceChildren(
      ...actions.map((action) => {
        const option = document.createElement("option");
        option.value = action.key;
        option.textContent = action.label;
        return option;
      }),
    );
    if (last && actions.some((action) => action.key === last)) {
      actionSelect.value = last;
    }
    const hasActions = actions.length > 0;
    textInput.disabled = !hasActions;
    actionSelect.disabled = !hasActions;
    if (!hasActions) {
      setStatus("No app offers quick capture.");
      return;
    }
    textInput.focus();
    textInput.select();
  }

  function selectedAction() {
    const key = actionSelect.value;
    return actions.find((action) => action.key === key) || actions[0] || null;
  }

  async function complete(saved) {
    if (!invoke) return;
    await invoke("quick_capture_done", { saved });
  }

  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    const action = selectedAction();
    const text = textInput.value.trim();
    if (!action || !text || !invoke) return;

    setError("");
    textInput.disabled = true;
    actionSelect.disabled = true;

    const space = storageGet("space") || "personal";
    try {
      await invoke("run_action", {
        space,
        plugin: action.plugin,
        op: action.op,
        args: argsFor(action, text),
      });
      setStatus(`Saved to ${action.name}`);
      textInput.value = "";
      storageSet("quick-capture-action", action.key);
      setTimeout(() => {
        complete(true).catch(() => {});
      }, 600);
    } catch (err) {
      setError(String(err));
      textInput.disabled = false;
      actionSelect.disabled = false;
      textInput.focus();
      textInput.select();
    }
  });

  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      event.preventDefault();
      complete(false).catch(() => {});
    }
  });

  if (events?.listen) {
    events.listen("quick-capture-open", () => {
      loadActions().catch((err) => setError(String(err)));
    });
  }

  loadActions().catch((err) => setError(String(err)));
})(typeof window === "undefined" ? globalThis : window);
