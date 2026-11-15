// Host-owned page and folder browser. The page grid remains in pages.js;
// this module only lists and organizes pages in the current space.
(function (root) {
  "use strict";

  // The system page is stored as "Home" and shown as the Dashboard.
  function shownPage(page) {
    return page?.system && (!page.title || page.title === "Home") ? { ...page, title: "Dashboard" } : page;
  }

  function sortRecords(a, b) {
    const position = Number(a?.position || 0) - Number(b?.position || 0);
    return position || String(a?.title || "").localeCompare(String(b?.title || "")) || String(a?.id || "").localeCompare(String(b?.id || ""));
  }

  // Missing parents and corrupt parent cycles are detached so every folder
  // remains reachable from the rendered root.
  function buildFolderTree(records) {
    const byId = new Map((records || []).filter(folder => folder?.id).map(folder => [String(folder.id), { ...folder, id: String(folder.id), children: [] }]));
    const parent = new Map();
    for (const folder of byId.values()) {
      const candidate = folder.parent_id == null ? null : String(folder.parent_id);
      parent.set(folder.id, candidate && byId.has(candidate) ? candidate : null);
    }
    for (const id of [...byId.keys()].sort()) {
      const visited = new Set([id]);
      let at = parent.get(id);
      let cyclic = false;
      while (at) {
        if (visited.has(at)) { cyclic = true; break; }
        visited.add(at);
        at = parent.get(at);
      }
      if (cyclic) parent.set(id, null);
    }
    const roots = [];
    for (const folder of byId.values()) {
      const parentId = parent.get(folder.id);
      if (parentId) byId.get(parentId).children.push(folder);
      else roots.push(folder);
    }
    const sort = nodes => nodes.sort(sortRecords).forEach(folder => sort(folder.children));
    sort(roots);
    return roots;
  }

  function folderPath(folderId, byId) {
    const labels = [], seen = new Set();
    let id = folderId == null ? null : String(folderId);
    while (id && byId.has(id) && !seen.has(id)) {
      seen.add(id);
      const folder = byId.get(id);
      labels.unshift(String(folder.title || "Untitled folder"));
      id = folder.parent_id == null ? null : String(folder.parent_id);
    }
    return labels.join(" / ");
  }

  function pageSearchEntries(pages, folders) {
    const byId = new Map((folders || []).filter(f => f?.id).map(f => [String(f.id), f]));
    return (pages || []).filter(page => page?.id).map(page => ({
      page,
      path: folderPath(page.folder_id, byId),
      text: `${page.title || "Untitled page"} ${folderPath(page.folder_id, byId)}`.toLocaleLowerCase(),
    }));
  }

  function mount(node, options) {
    const opts = options || {};
    const service = opts.service;
    const space = opts.space;
    let folders = [], activePages = Array.isArray(opts.pages) ? opts.pages.map(shownPage) : [], trashedPages = [];
    let disposed = false, revision = 0, pending = 0;
    const expandedFolders = new Set();
    // Opening the library at a folder (from a collapsed rail pin) expands
    // its ancestors once the folders load, then scrolls it into view.
    let focusFolder = opts.focusFolder == null ? null : String(opts.focusFolder);
    const el = (tag, className, text) => {
      const item = document.createElement(tag);
      if (className) item.className = className;
      if (text != null) item.textContent = String(text);
      return item;
    };
    const button = (label, className, handler) => {
      const item = el("button", className || "pp-button", label);
      item.type = "button";
      item.addEventListener("click", handler);
      return item;
    };
    const title = (record) => String(record?.title || "Untitled page");
    const folderById = () => new Map(folders.map(folder => [String(folder.id), folder]));

    const shell = el("section", "pl-library");
    const toolbar = el("div", "pl-toolbar");
    const search = el("input", "pp-input pl-search");
    search.type = "search"; search.placeholder = "Search pages and folders";
    search.setAttribute("aria-label", "Search pages and folder paths");
    const newFolder = button("New folder", "pp-button pl-new-folder", () => { createForm.hidden = !createForm.hidden; if (!createForm.hidden) createTitle.focus(); });
    const status = el("p", "pl-status"); status.setAttribute("role", "status");
    const error = el("p", "pl-error"); error.setAttribute("role", "alert"); error.hidden = true;
    const retry = button("Retry loading", "pp-button pl-retry", () => void refresh()); retry.hidden = true;
    const createForm = el("form", "pl-form pl-create-form"); createForm.hidden = true;
    const createLabel = el("label", "pl-field"); createLabel.append(el("span", "pl-label", "Folder name"));
    const createTitle = el("input", "pp-input"); createTitle.type = "text"; createTitle.maxLength = 40; createTitle.required = true; createLabel.append(createTitle);
    const createParentLabel = el("label", "pl-field"); createParentLabel.append(el("span", "pl-label", "Parent folder"));
    const createParent = el("select", "pp-input"); createParentLabel.append(createParent);
    const createSubmit = el("button", "pp-button pp-primary", "Create folder"); createSubmit.type = "submit";
    const createCancel = button("Cancel", "pp-button pp-ghost", () => { createForm.hidden = true; createTitle.value = ""; });
    createForm.append(createLabel, createParentLabel, createSubmit, createCancel);
    const tree = el("div", "pl-tree");
    const trash = el("section", "pl-trash");
    trash.append(el("h3", "pl-heading", "Trash"));
    shell.append(toolbar, error, retry, createForm, status, tree, trash);
    toolbar.append(search, newFolder);
    node.replaceChildren(shell);

    function report(message) {
      error.textContent = message || "";
      error.hidden = !message;
      retry.hidden = !message;
    }
    function syncPending() {
      shell.classList.toggle("pl-pending", pending > 0);
      const disable = item => {
        if (["button", "input", "select"].includes(item.tagName?.toLowerCase?.() || item.tag)) item.disabled = pending > 0;
        for (const child of item.children || []) if (typeof child !== "string") disable(child);
      };
      disable(shell);
    }
    function setPending(value) { pending = Math.max(0, pending + value); syncPending(); }
    function filteredFolders() { return folders.filter(folder => !folder.archived); }
    function updateParentSelect(select, selected, excludeId) {
      const selectedValue = selected == null ? "" : String(selected);
      const choices = filteredFolders().filter(folder => String(folder.id) !== String(excludeId || ""));
      // Prevent moving a folder below itself or any of its descendants.
      const forbidden = new Set();
      if (excludeId) {
        const walk = id => { if (forbidden.has(id)) return; forbidden.add(id); for (const child of choices.filter(f => String(f.parent_id || "") === id)) walk(String(child.id)); };
        walk(String(excludeId));
      }
      select.replaceChildren();
      const rootOption = el("option", "", "No parent (root)"); rootOption.value = ""; select.append(rootOption);
      const byId = folderById();
      for (const folder of choices.filter(f => !forbidden.has(String(f.id))).sort(sortRecords)) {
        const option = el("option", "", folderPath(folder.id, byId) || title(folder));
        option.value = String(folder.id); select.append(option);
      }
      select.value = selectedValue;
    }
    updateParentSelect(createParent, null);

    async function act(name, args, message) {
      if (disposed) return;
      setPending(1); report("");
      try {
        const result = await service(name, args);
        if (disposed) return result;
        let callbackError = "";
        try { await opts.onChanged?.(name, args, result); }
        catch (err) { callbackError = err?.message || String(err); }
        if (message) opts.toast?.(message);
        await refresh();
        if (callbackError && !disposed) report(`Change saved, but the page list could not refresh: ${callbackError}`);
        return result;
      } catch (err) {
        if (!disposed) report(err?.message || String(err));
        return undefined;
      } finally { setPending(-1); }
    }

    function inlineForm(parent, className) {
      const form = el("form", `pl-form ${className || ""}`);
      parent.append(form);
      return form;
    }
    function addRenameMove(folder, holder) {
      const form = inlineForm(holder, "pl-folder-edit");
      const nameLabel = el("label", "pl-field"); nameLabel.append(el("span", "pl-label", "Folder name"));
      const input = el("input", "pp-input"); input.type = "text"; input.required = true; input.maxLength = 40; input.value = title(folder); nameLabel.append(input);
      const parentLabel = el("label", "pl-field"); parentLabel.append(el("span", "pl-label", "Parent folder"));
      const parentSelect = el("select", "pp-input"); updateParentSelect(parentSelect, folder.parent_id, folder.id); parentLabel.append(parentSelect);
      const save = el("button", "pp-button pp-primary", "Save folder"); save.type = "submit";
      form.append(nameLabel, parentLabel, save);
      form.addEventListener("submit", event => {
        event.preventDefault();
        const args = { id: folder.id, title: input.value.trim() };
        args.parent_id = parentSelect.value || null;
        void act("folder_update", args, "Folder updated");
      });
    }
    function addFolderRemove(folder, holder) {
      const disclosure = el("details", "pl-remove-options");
      const summary = el("summary", "pl-action-summary", "Remove folder");
      const message = el("p", "pl-warning", `Choose what happens to “${title(folder)}”.`);
      const keep = button("Remove folder, keep contents", "pp-button", () => void act("folder_remove", { id: folder.id, subtree: false }, "Folder removed; its contents were kept"));
      const subtree = button("Trash folder and everything inside", "pp-button pl-danger", () => void act("folder_remove", { id: folder.id, subtree: true }, "Folder moved to Trash"));
      disclosure.append(summary, message, keep, subtree); holder.append(disclosure);
    }
    function addPageControls(page, holder) {
      const actions = el("details", "pl-page-actions");
      actions.append(el("summary", "pl-action-summary", "Organize page"));
      const moveForm = inlineForm(actions, "pl-page-move");
      const label = el("label", "pl-field"); label.append(el("span", "pl-label", "Move to folder"));
      const select = el("select", "pp-input"); updateParentSelect(select, page.folder_id); label.append(select);
      const move = el("button", "pp-button", "Move page"); move.type = "submit"; moveForm.append(label, move);
      moveForm.addEventListener("submit", event => { event.preventDefault(); void act("page_move", { page: page.id, folder_id: select.value || null }, "Page moved"); });
      const trashConfirm = el("details", "pl-remove-options");
      trashConfirm.append(el("summary", "pl-action-summary", "Move to Trash"), el("p", "pl-warning", `Move “${title(page)}” to Trash?`));
      trashConfirm.append(button("Move page to Trash", "pp-button pl-danger", () => void act("page_trash", { page: page.id }, "Page moved to Trash")));
      actions.append(trashConfirm); holder.append(actions);
    }
    function pageRow(page, path) {
      const row = el("article", "pl-page");
      const open = button(title(page), "pl-page-open", () => opts.onOpen?.(page.id));
      open.setAttribute("aria-label", `Open page ${title(page)}`);
      if (page.recovered) row.append(el("span", "pl-recovered", "Recovered"));
      row.append(open);
      if (path) { const crumb = el("span", "pl-page-path", path); row.append(crumb); }
      addPageControls(page, row);
      return row;
    }

    function render() {
      if (disposed) return;
      const query = search.value.trim().toLocaleLowerCase();
      const activeFolders = filteredFolders();
      const byId = new Map(activeFolders.map(folder => [String(folder.id), folder]));
      const pagesByFolder = new Map();
      for (const page of activePages.filter(p => !p.archived)) {
        if (page.space != null && page.space !== space) continue;
        const id = page.folder_id != null && byId.has(String(page.folder_id)) ? String(page.folder_id) : "";
        if (!pagesByFolder.has(id)) pagesByFolder.set(id, []);
        pagesByFolder.get(id).push(page);
      }
      for (const pages of pagesByFolder.values()) pages.sort(sortRecords);
      const paths = pageSearchEntries(activePages, activeFolders);
      const matchIds = new Set(paths.filter(entry => !query || entry.text.includes(query)).map(entry => String(entry.page.id)));
      const folderMatches = new Map();
      const scan = folder => {
        const own = `${folder.title || ""} ${folderPath(folder.id, byId)}`.toLocaleLowerCase().includes(query);
        const ownPages = (pagesByFolder.get(String(folder.id)) || []).some(page => matchIds.has(String(page.id)));
        const descendants = folder.children.map(scan).some(Boolean);
        const matched = !query || own || ownPages || descendants;
        folderMatches.set(String(folder.id), matched);
        return matched;
      };
      const treeRoots = buildFolderTree(activeFolders);
      treeRoots.forEach(scan);
      tree.replaceChildren();
      const renderFolder = folder => {
        if (query && !folderMatches.get(String(folder.id))) return null;
        const details = el("details", "pl-folder"); details.open = Boolean(query) || expandedFolders.has(String(folder.id));
        details.addEventListener("toggle", () => {
          if (details.open) expandedFolders.add(String(folder.id));
          else expandedFolders.delete(String(folder.id));
        });
        const summary = el("summary", "pl-folder-summary");
        summary.append(el("span", "pl-folder-title", title(folder)));
        if (folder.recovered) summary.append(el("span", "pl-recovered", "Recovered"));
        details.append(summary);
        const content = el("div", "pl-folder-content");
        (pagesByFolder.get(String(folder.id)) || []).filter(page => matchIds.has(String(page.id))).forEach(page => content.append(pageRow(page, folderPath(folder.id, byId))));
        if (opts.onTogglePin) {
          const pinned = Boolean(opts.isPinned?.(folder.id));
          const pin = button(pinned ? "Unpin from rail" : "Pin to rail", "pp-button pl-folder-pin", () => { opts.onTogglePin(folder.id); render(); });
          pin.setAttribute("aria-pressed", String(pinned));
          pin.setAttribute("aria-label", `${pinned ? "Unpin" : "Pin"} folder ${title(folder)} ${pinned ? "from" : "to"} the rail`);
          content.append(pin);
        }
        const edit = el("details", "pl-folder-actions"); edit.append(el("summary", "pl-action-summary", "Folder options"));
        addRenameMove(folder, edit); addFolderRemove(folder, edit); content.append(edit);
        for (const child of folder.children) { const rendered = renderFolder(child); if (rendered) content.append(rendered); }
        details.append(content);
        if (focusTarget === String(folder.id)) { details.classList?.add("pl-folder-focus"); focusNode = details; }
        return details;
      };
      let focusNode = null;
      const focusTarget = focusFolder;
      if (focusTarget && byId.has(focusTarget)) {
        for (let id = focusTarget, seen = new Set(); id && byId.has(id) && !seen.has(id); id = byId.get(id).parent_id == null ? null : String(byId.get(id).parent_id)) { seen.add(id); expandedFolders.add(id); }
      }
      for (const folder of treeRoots) { const rendered = renderFolder(folder); if (rendered) tree.append(rendered); }
      const rootPages = pagesByFolder.get("") || [];
      const visibleRootPages = rootPages.filter(page => matchIds.has(String(page.id)));
      if (visibleRootPages.length) {
        const unfiled = el("section", "pl-unfiled"); unfiled.append(el("h3", "pl-heading", "Pages without a folder"));
        visibleRootPages.forEach(page => unfiled.append(pageRow(page, ""))); tree.prepend(unfiled);
      }
      if (!tree.childNodes?.length && !tree.children.length) tree.append(el("p", "pl-empty", query ? "No pages or folders match your search." : "No pages or folders yet."));
      const archivedFolders = folders.filter(folder => folder.archived).sort(sortRecords);
      const archivedPageList = trashedPages.filter(page => page?.id && (page.space == null || page.space === space)).sort(sortRecords);
      const trashItems = [];
      for (const folder of archivedFolders) {
        const item = el("div", "pl-trash-item");
        item.append(el("span", "pl-trash-title", `${title(folder)} · Folder`));
        if (folder.recovered) item.append(el("span", "pl-recovered", "Recovered"));
        item.append(button("Restore folder", "pp-button", () => void act("folder_restore", { id: folder.id }, "Folder restored")));
        trashItems.push(item);
      }
      for (const page of archivedPageList) {
        const item = el("div", "pl-trash-item"); item.append(el("span", "pl-trash-title", `${title(page)} · Page`));
        if (page.recovered) item.append(el("span", "pl-recovered", "Recovered"));
        item.append(button("Restore page", "pp-button", () => void act("page_restore", { page: page.id }, "Page restored")));
        trashItems.push(item);
      }
      trash.replaceChildren(el("h3", "pl-heading", "Trash"), ...(trashItems.length ? trashItems : [el("p", "pl-empty", "Trash is empty.")]));
      status.textContent = `${activePages.filter(page => !page.archived && (page.space == null || page.space === space)).length} pages · ${activeFolders.length} folders`;
      updateParentSelect(createParent, createParent.value);
      syncPending();
      if (focusNode) {
        focusFolder = null;
        focusNode.scrollIntoView?.({ block: "nearest" });
        focusNode.querySelector?.("summary")?.focus?.({ preventScroll: true });
      }
    }

    createForm.addEventListener("submit", event => {
      event.preventDefault();
      const value = createTitle.value.trim();
      if (!value) return;
      const args = { space, title: value };
      if (createParent.value) args.parent_id = createParent.value;
      void act("folder_create", args, "Folder created");
      createTitle.value = ""; createForm.hidden = true;
    });
    search.addEventListener("input", render);

    async function refresh() {
      if (disposed) return false;
      const request = ++revision;
      report(""); setPending(1);
      try {
        const [listedFolders, listedPages, listedTrash] = await Promise.all([
          service("folders_list", { space, include_archived: true }),
          service("pages_list", { space }),
          service("pages_trash", { space }),
        ]);
        if (disposed || request !== revision) return false;
        folders = (Array.isArray(listedFolders) ? listedFolders : []).filter(folder => folder?.id && (folder.space == null || folder.space === space));
        activePages = (Array.isArray(listedPages) ? listedPages : []).filter(page => page?.id && !page.archived && (page.space == null || page.space === space)).map(shownPage);
        trashedPages = Array.isArray(listedTrash) ? listedTrash.filter(page => page?.id && (page.space == null || page.space === space)) : [];
        render();
        return true;
      } catch (err) {
        if (!disposed && request === revision) report(err?.message || String(err));
        return false;
      } finally { setPending(-1); }
    }
    const ready = refresh();
    return {
      ready,
      refresh,
      dispose() { if (disposed) return; disposed = true; revision++; node.replaceChildren(); },
    };
  }

  const api = { mount, buildFolderTree, folderPath, pageSearchEntries, shownPage };
  if (typeof module !== "undefined" && module.exports) module.exports = api;
  else root.TerrariumPageLibrary = api;
})(typeof window === "undefined" ? globalThis : window);
