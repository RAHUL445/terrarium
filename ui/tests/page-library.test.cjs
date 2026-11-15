const { test } = require("node:test");
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const vm = require("node:vm");
const { buildFolderTree, folderPath, mount } = require("../page-library.js");
const { rendererHarness } = require("./helpers/renderer-dom.cjs");

function flatten(root) {
  return [root, ...root.children.filter(node => typeof node !== "string").flatMap(flatten)];
}

test("folder tree keeps deterministic reachability with missing parents and cycles", () => {
  const roots = buildFolderTree([
    { id: "b", parent_id: "a", title: "Bee" },
    { id: "orphan", parent_id: "missing", title: "Orphan" },
    { id: "a", parent_id: "b", title: "Aye" },
    { id: "child", parent_id: "a", title: "Child" },
    { id: "root", title: "Root" },
  ]);
  const all = [];
  const walk = items => items.forEach(item => { all.push(item.id); walk(item.children); });
  walk(roots);
  assert.deepEqual([...all].sort(), ["a", "b", "child", "orphan", "root"]);
  assert.equal(new Set(all).size, all.length);
  assert.deepEqual(roots.map(folder => folder.id), ["a", "orphan", "root"]);
  const reversed = buildFolderTree([
    { id: "root", title: "Root" }, { id: "child", parent_id: "a", title: "Child" },
    { id: "a", parent_id: "b", title: "Aye" }, { id: "orphan", parent_id: "missing", title: "Orphan" },
    { id: "b", parent_id: "a", title: "Bee" },
  ]);
  assert.deepEqual(JSON.stringify(reversed.map(folder => [folder.id, folder.children.map(child => child.id)])), JSON.stringify(roots.map(folder => [folder.id, folder.children.map(child => child.id)])));
  assert.equal(folderPath("child", new Map([
    ["a", { id: "a", title: "Aye", parent_id: "b" }],
    ["b", { id: "b", title: "Bee", parent_id: "a" }],
    ["child", { id: "child", title: "Child", parent_id: "a" }],
  ])), "Bee / Aye / Child");
});

test("mount scopes list requests and records to the selected space, then opens listed pages", async () => {
  const { root: base } = rendererHarness();
  class Node extends base.constructor {
    get tagName() { return this.tag.toUpperCase(); }
    get options() { return this.children.filter(child => child.tag === "option"); }
  }
  const calls = [], opened = [];
  const window = {};
  vm.runInNewContext(readFileSync(require.resolve("../page-library.js"), "utf8"), {
    window, document: { createElement: tag => new Node(tag) },
  });
  const api = window.TerrariumPageLibrary;
  const host = new Node("root");
  const library = api.mount(host, {
    space: "home",
    service: async (name, args) => {
      calls.push({ name, args });
      if (name === "folders_list") return [
        { id: "f1", space: "home", title: "Reading", parent_id: null, position: 1 },
        { id: "other", space: "work", title: "Secret", parent_id: null },
        { id: "deleted-folder", space: "home", title: "Old", archived: true, recovered: true },
      ];
      if (name === "pages_list") return [
        { id: "p1", space: "home", title: "Book notes", folder_id: "f1", recovered: true },
        { id: "p-other", space: "work", title: "Private page" },
      ];
      if (name === "pages_trash") return [{ id: "t1", space: "home", title: "Discarded", archived: true }];
      throw new Error(`Unexpected service: ${name}`);
    },
    onOpen: id => opened.push(id),
  });
  await library.ready;
  assert.equal(JSON.stringify(calls.slice(0, 3)), JSON.stringify([
    { name: "folders_list", args: { space: "home", include_archived: true } },
    { name: "pages_list", args: { space: "home" } },
    { name: "pages_trash", args: { space: "home" } },
  ]));
  const labels = flatten(host).map(node => node.textContent).filter(Boolean);
  assert.ok(labels.includes("Reading"));
  assert.ok(labels.includes("Book notes"));
  assert.ok(labels.includes("Old · Folder"));
  assert.ok(labels.includes("Discarded · Page"));
  assert.ok(!labels.includes("Secret"));
  assert.ok(!labels.includes("Private page"));
  const open = flatten(host).find(node => node.tag === "button" && node.textContent === "Book notes");
  await open.fire("click");
  assert.deepEqual(opened, ["p1"]);
  library.dispose();
});

test("create folder and page trash use explicit service operations", async () => {
  const { root: base } = rendererHarness();
  class Node extends base.constructor { get tagName() { return this.tag.toUpperCase(); } }
  const calls = [];
  const window = {};
  vm.runInNewContext(readFileSync(require.resolve("../page-library.js"), "utf8"), {
    window, document: { createElement: tag => new Node(tag) },
  });
  const host = new Node("root");
  const saved = { folders: [{ id: "f1", space: "home", title: "Reading", parent_id: null }] };
  const library = window.TerrariumPageLibrary.mount(host, {
    space: "home",
    service: async (name, args) => {
      calls.push({ name, args });
      if (name === "folders_list") return saved.folders;
      if (name === "pages_list") return [{ id: "p1", space: "home", title: "Draft", folder_id: "f1" }];
      if (name === "pages_trash") return [];
      if (name === "folder_create") { saved.folders.push({ id: "f2", space: "home", title: args.title, parent_id: null }); return { id: "f2" }; }
      return { ok: true };
    },
    onChanged: async name => {
      await new Promise(resolve => setImmediate(resolve));
      if (name === "folder_create") throw new Error("parent changed spaces");
    },
  });
  await library.ready;
  let nodes = flatten(host);
  const reading = nodes.find(node => node.tag === "details" && node.className === "pl-folder");
  reading.open = true;
  await reading.fire("toggle");
  nodes = flatten(host);
  const newFolder = nodes.find(node => node.tag === "button" && node.textContent === "New folder");
  await newFolder.fire("click");
  nodes = flatten(host);
  const form = nodes.find(node => node.tag === "form" && node.className.includes("pl-create-form"));
  const input = form.children.find(node => node.tag === "label").children.find(node => node.tag === "input");
  input.value = "Archive";
  await form.fire("submit");
  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(JSON.stringify(calls.find(call => call.name === "folder_create")), JSON.stringify({
    name: "folder_create", args: { space: "home", title: "Archive" },
  }));
  assert.ok(flatten(host).some(node => node.attrs?.role === "alert" && node.textContent.includes("parent changed spaces")));
  const folderDisclosure = () => flatten(host).find(node => node.tag === "details" && node.className === "pl-folder" && flatten(node).some(child => child.className === "pl-folder-title" && child.textContent === "Reading"));
  assert.equal(folderDisclosure()?.open, true);

  nodes = flatten(host);
  const confirm = nodes.find(node => node.tag === "details" && node.className === "pl-remove-options" && node.children.some(child => child.textContent === "Move to Trash"));
  assert.ok(confirm);
  const trashButton = nodes.find(node => node.tag === "button" && node.textContent === "Move page to Trash");
  await trashButton.fire("click");
  await new Promise(resolve => setImmediate(resolve));
  await new Promise(resolve => setImmediate(resolve));
  assert.ok(calls.some(call => call.name === "page_trash" && call.args.page === "p1"));
  assert.equal(folderDisclosure()?.open, true);
  assert.ok(!calls.some(call => call.name === "page_remove"));
  library.dispose();
});

test("initial load errors show a retry action", async () => {
  const { root: base } = rendererHarness();
  class Node extends base.constructor { get tagName() { return this.tag.toUpperCase(); } }
  let failed = false;
  const window = {};
  vm.runInNewContext(readFileSync(require.resolve("../page-library.js"), "utf8"), {
    window, document: { createElement: tag => new Node(tag) },
  });
  const host = new Node("root");
  const library = window.TerrariumPageLibrary.mount(host, {
    space: "home",
    service: async name => {
      if (name === "folders_list" && !failed) { failed = true; throw new Error("Storage offline"); }
      if (name === "folders_list" || name === "pages_list" || name === "pages_trash") return [];
      throw new Error(name);
    },
  });
  await library.ready;
  let nodes = flatten(host);
  assert.ok(nodes.some(node => node.attrs?.role === "alert" && node.textContent === "Storage offline"));
  const retry = nodes.find(node => node.tag === "button" && node.textContent === "Retry loading");
  assert.equal(retry.hidden, false);
  await retry.fire("click");
  await new Promise(resolve => setImmediate(resolve));
  nodes = flatten(host);
  assert.equal(nodes.find(node => node.tag === "button" && node.textContent === "Retry loading").hidden, true);
  library.dispose();
});
