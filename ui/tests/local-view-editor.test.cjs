const { test } = require("node:test");
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const vm = require("node:vm");
const { rendererHarness } = require("./helpers/renderer-dom.cjs");
const views = require("../views.js");

function libraryHarness() {
  const { root: base } = rendererHarness();
  class Node extends base.constructor {
    get tagName() { return this.tag.toUpperCase(); }
    get options() { return this.children.filter(child => child.tag === "option"); }
    get selectedOptions() { return this.options.filter(option => option.selected); }
  }
  const mounts = [];
  const window = { TerrariumViews: { ...views, mount: (_node, options) => {
    mounts.push(options);
    return {
      config: options.config,
      ready: Promise.resolve(),
      refresh: async () => {},
      flush: async () => true,
      dispose() {},
    };
  } } };
  vm.runInNewContext(readFileSync(require.resolve("../view-library.js"), "utf8"), {
    window,
    document: { createElement: tag => new Node(tag) },
  });
  return { Node, library: window.TerrariumViewLibrary, mounts };
}

const view = {
  title: "Tasks",
  title_column: "title",
  params: {},
  columns: [
    { key: "title", label: "Task", type: "string" },
    { key: "due_at", label: "Due", type: "date" },
    { key: "project", label: "Project", type: "enum", values: ["work", "home"] },
  ],
  filters: [
    { param: "after", label: "After", type: "date" },
    { param: "limit", label: "Minimum", type: "number" },
    { param: "completed", label: "Completed", type: "boolean" },
    { param: "project", label: "Project", type: "enum", values: ["work", "home"] },
  ],
  sorts: [{ param: "sort", values: ["due_at"] }],
  temporal: { start_param: "start_ms", end_param: "end_ms" },
};

function flatten(node) {
  return [node, ...node.children.filter(child => typeof child !== "string").flatMap(flatten)];
}

test("local editor uses native filter types and only declared paginated sort values", async () => {
  const { Node, library: api, mounts } = libraryHarness();
  const root = new Node("root");
  const editor = api.mount(root, { query: "tasks" }, {
    plugin: "todo",
    plugins: [{ id: "todo", name: "To-do", views: { tasks: view } }],
    bridge: { query: async () => [] },
    service: async name => name === "views_list" ? [] : Promise.reject(new Error(name)),
    localEditor: true,
    config: { plugin: "todo", query: "tasks", filters: '{"after":"2026-05-01","limit":12}', layout: "table" },
    focus: () => "keyword scope",
    period: () => ({ start_ms: 100, end_ms: 200, timezone: "UTC" }),
  });
  await editor.ready;
  assert.equal(mounts[0].focus, "keyword scope");
  assert.deepEqual(mounts[0].period(), { start_ms: 100, end_ms: 200, timezone: "UTC" });

  const nodes = flatten(root);
  assert.ok(nodes.some(node => node.tag === "input" && node.type === "date" && node.value === "2026-05-01"));
  assert.ok(nodes.some(node => node.tag === "input" && node.type === "number" && node.value === "12"));
  const completed = nodes.find(node => node.tag === "select" && node.attrs["aria-label"] === "Completed");
  assert.deepEqual(completed.options.map(option => option.value), ["", "true", "false"]);
  completed.value = "false";
  await completed.fire("change");
  assert.equal(JSON.parse(editor.config().filters).completed, false);
  const sort = nodes.find(node => node.tag === "select" && node.parent?.children?.some(child => child.textContent === "Sort by"));
  assert.ok(sort);
  assert.deepEqual(sort.options.map(option => option.value), ["", "due_at"]);
  const selects = nodes.filter(node => node.tag === "select");
  const layout = selects[2];
  const group = selects[5];
  assert.deepEqual(group.options.map(option => option.value), ["", "title", "project"], "list grouping includes string and enum columns");
  layout.value = "board";
  await layout.fire("change");
  assert.deepEqual(group.options.map(option => option.value), ["", "project"], "board grouping stays enum-only");
  assert.equal(editor.config().plugin, "todo");
  assert.equal(editor.validate(), true);
  editor.dispose();
});

test("local shared reuse is explicit and saving creates a new shared definition", async () => {
  const { Node, library: api } = libraryHarness();
  const root = new Node("root");
  const record = { id: "shared-1", title: "Work tasks", config: { plugin: "todo", query: "tasks", filters: '{"limit":3}' } };
  const writes = [];
  const editor = api.mount(root, { query: "tasks" }, {
    plugin: "todo",
    plugins: [{ id: "todo", name: "To-do", views: { tasks: view } }],
    bridge: { query: async () => [] },
    service: async (name, args) => {
      if (name === "views_list") return [record];
      if (name === "view_get") return record;
      if (name === "view_save") { writes.push(args); return { ...record, id: "shared-2", ...args }; }
      throw new Error(name);
    },
    localEditor: true,
    config: { plugin: "todo", query: "tasks", filters: "{}" },
  });
  await editor.ready;

  const nodes = flatten(root);
  const savedSelect = nodes.find(node => node.tag === "select" && node.attrs["aria-label"] === "Saved view");
  savedSelect.value = record.id;
  await savedSelect.fire("change");
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(editor.config().saved_view, undefined, "selecting a shared view does not bind the placement yet");

  const number = nodes.find(node => node.tag === "input" && node.type === "number");
  number.value = "99";
  await number.fire("input");
  const reuse = nodes.find(node => node.tag === "button" && node.textContent === "Use shared view");
  await reuse.fire("click");
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(editor.config().saved_view, "shared-1");
  assert.equal(editor.snapshot().config.filters, '{"limit":3}', "binding a shared view discards the dirty local draft");

  const copy = nodes.find(node => node.tag === "button" && node.textContent === "Make independent copy");
  await copy.fire("click");
  assert.equal(editor.config().saved_view, undefined);

  const title = nodes.find(node => node.tag === "input" && node.maxLength === 40);
  title.value = "Shared copy";
  const save = nodes.find(node => node.tag === "button" && node.textContent === "Save as shared view");
  await save.fire("click");
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(writes.length, 1);
  assert.equal(writes[0].id, undefined, "local editor never updates the selected shared view");
  assert.equal(writes[0].title, "Shared copy");
  editor.dispose();
});

test("missing saved references remain unavailable instead of falling back to a default view", async () => {
  const { Node, library: api } = libraryHarness();
  const root = new Node("root");
  const editor = api.mount(root, { query: "tasks" }, {
    plugin: "todo",
    plugins: [{ id: "todo", name: "To-do", views: { tasks: view } }],
    bridge: { query: async () => [] },
    service: async name => name === "views_list" ? [] : Promise.reject(new Error(name)),
    localEditor: true,
    config: { saved_view: "missing-shared" },
  });
  await editor.ready;
  assert.equal(editor.validate(), false);
  assert.equal(editor.config().saved_view, "missing-shared");
  assert.ok(flatten(root).some(node => String(node.textContent || "").includes("unavailable in this space")));
  editor.dispose();
});

test("page settings resolves a saved reference's plugin before mounting the local editor", async () => {
  class Node {
    constructor(tag) { this.tag = tag; this.children = []; this.dataset = {}; this.attrs = {}; this.listeners = {}; this.hidden = false; }
    append(...nodes) { this.children.push(...nodes); }
    replaceChildren(...nodes) { this.children = nodes; }
    setAttribute(name, value) { this.attrs[name] = value; }
    addEventListener(name, listener) { (this.listeners[name] ||= []).push(listener); }
  }
  const notesView = { ...view, title: "Review notes" };
  const plugins = [
    { id: "todo", name: "To-do", views: { tasks: view } },
    { id: "notes", name: "Notes", views: { review_notes: notesView } },
  ];
  const record = { id: "shared-notes", title: "Notes", config: { plugin: "notes", query: "review_notes", filters: "{}" } };
  const mounts = [];
  const window = {
    TerrariumViews: views,
    TerrariumReviewPeriod: { resolve: period => ({ start_ms: 100, end_ms: 200, timezone: period.timezone }) },
    TerrariumViewLibrary: { mount: (_node, _spec, options) => {
      mounts.push(options);
      return { config: () => ({ saved_view: record.id }), validate: () => true, dispose() {} };
    } },
  };
  vm.runInNewContext(readFileSync(require.resolve("../pages.js"), "utf8"), {
    window,
    document: { createElement: tag => new Node(tag) },
  });
  const page = Object.create(window.TerrariumPages.prototype);
  page.plugins = plugins;
  page.space = "space-a";
  page.page = { focus: "page focus", period: { mode: "custom", timezone: "Asia/Kolkata" } };
  page.o = { bridge: plugin => ({ plugin }) };
  page.service = async name => name === "view_get" ? record : Promise.reject(new Error(name));
  page.el = (tag, attrs = {}, ...children) => {
    const node = new Node(tag);
    Object.assign(node, attrs);
    node.append(...children);
    return node;
  };
  page.focusControls = () => ({ read: () => undefined, rows: [] });
  page.filterParts = () => [];
  const summary = { textContent: "" };
  const settings = page.viewSettings(
    { id: "placement", config: { saved_view: record.id, follow_page_period: true } },
    {},
    summary,
    (title, rows) => ({ title, rows }),
  );
  await new Promise(resolve => setImmediate(resolve));

  assert.equal(mounts.length, 1);
  assert.equal(mounts[0].plugin, "notes");
  assert.equal(mounts[0].config.saved_view, record.id);
  assert.equal(mounts[0].focus(), "page focus");
  assert.deepEqual(mounts[0].period(), { start_ms: 100, end_ms: 200, timezone: "Asia/Kolkata" });
  assert.equal(settings.read().saved_view, record.id);
  assert.equal(settings.read().follow_page_period, true);
  const periodCheckbox = settings.groups[0].rows[2].children[0];
  periodCheckbox.checked = false;
  periodCheckbox.listeners.change[0]();
  assert.match(summary.textContent, /Preview changes to refresh focus and review period/);
  assert.equal(mounts[0].period(), null);
  mounts[0].onPreview();
  assert.doesNotMatch(summary.textContent, /Preview changes to refresh focus and review period/);
  periodCheckbox.checked = true;
  page.page.period = null;
  assert.throws(() => mounts[0].period(), /Choose a review period in page settings/);
  settings.dispose();
});
