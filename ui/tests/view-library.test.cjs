const { test } = require("node:test");
const assert = require("node:assert/strict");
const { readFileSync } = require('node:fs');
const vm = require('node:vm');
const { rendererHarness } = require('./helpers/renderer-dom.cjs');
const { readFilters, isChoiceFilter, filterDefaults, savedConfig } = require("../view-library.js");

const definitions = [
  { param: "project", label: "Project" },
  { param: "status", label: "Status", multiple: true },
];

test("saved filters accept only declared scalar parameters", () => {
  assert.deepEqual(readFilters('{"project":"work","status":"open,done"}', definitions), {
    values: { project: "work", status: "open,done" }, error: "",
  });
  assert.equal(readFilters('{"unknown":"x"}', definitions).error, "This view uses an unavailable filter: unknown.");
  assert.equal(readFilters('{"project":[]}', definitions).error, "Filter project has an invalid value.");
});

test("malformed and non-text saved filter configs fail closed", () => {
  assert.equal(readFilters("not json", definitions).error, "Saved filters are not valid JSON.");
  assert.equal(readFilters({ project: "work" }, definitions).error, "Saved filters must be JSON text.");
  assert.equal(readFilters("[]", definitions).error, "Saved filters must be a JSON object.");
});

test("choice filters require actual values or a dynamic choices query", () => {
  assert.equal(isChoiceFilter({ values: [] }), false);
  assert.equal(isChoiceFilter({}), false);
  assert.equal(isChoiceFilter({ values: [""] }), true);
  assert.equal(isChoiceFilter({ values: [], choices_query: "tags" }), true);
});

test("filter controls start from declared query params with saved filters overriding", () => {
  assert.deepEqual(filterDefaults(
    { params: { filter: "open", tag: "", unrelated: "x" } },
    [{ param: "filter" }, { param: "tag" }],
    { filter: "done", tag: "work", stale: "ignored" },
  ), { filter: "done", tag: "work" });
  assert.deepEqual(filterDefaults(
    { params: { filter: "open", tag: "" } },
    [{ param: "filter" }, { param: "tag" }],
    { filter: null },
  ), { tag: "" });
});

test("saved configs omit empty optional column settings", () => {
  assert.deepEqual(savedConfig("todo", "workspace_tasks", "list", "", "asc", "", "", "{}"), {
    plugin: "todo", query: "workspace_tasks", layout: "list", dir: "asc", filters: "{}",
  });
});

test('checkbox multi-picker preserves hidden selections, clears explicitly, and supports Escape', async () => {
  const { root } = rendererHarness();
  const Node = root.constructor;
  const window = {};
  vm.runInNewContext(readFileSync(require.resolve('../view-library.js'), 'utf8'), {
    window, document: { createElement: tag => new Node(tag) },
  });
  let changes = 0;
  const picker = window.TerrariumViewLibrary.choicePicker('Lists', [{ value: 'a', label: 'Alpha' }, { value: 'b', label: 'Beta' }], ['b', 'missing'], () => changes++);
  const [summary, panel] = picker.node.children;
  const [search, clear, options] = panel.children;
  assert.equal(picker.read(), 'b,missing');
  search.value = 'Alpha'; await search.fire('input');
  assert.equal(options.children[1].hidden, true);
  assert.equal(picker.read(), 'b,missing'); // Filtering options never clears selected filters.
  options.children[0].children[0].checked = true;
  await options.children[0].children[0].fire('change');
  assert.equal(picker.read(), 'b,missing,a');
  assert.match(summary.textContent, /3 selected/);
  picker.node.open = true;
  await picker.node.fire('keydown', { key: 'Escape', stopPropagation() {} });
  assert.equal(picker.node.open, false);
  assert.equal(summary.focused, true);
  await clear.fire('click');
  assert.equal(picker.read(), ''); assert.equal(changes, 2);
});

test('a restored draft keeps unapplied filters separate from the preview and saves display settings', async () => {
  const { root: base } = rendererHarness();
  class Node extends base.constructor {
    get tagName() { return this.tag.toUpperCase(); }
    get options() { return this.children.filter(child => child.tag === 'option'); }
  }
  const previews = [], writes = [];
  const view = { title: 'Tasks', title_column: 'title', params: { q: '' },
    columns: [{ key: 'title', label: 'Task', type: 'string' }, { key: 'tags', label: 'Tags', type: 'string' }],
    filters: [{ param: 'q', label: 'Search' }] };
  const record = { id: 'v1', title: 'Work', config: { plugin: 'todo', query: 'tasks', layout: 'table', filters: '{}' } };
  const window = { TerrariumViews: { ...require('../views.js'), mount: (body, options) => {
    previews.push(options.config);
    return { ready: Promise.resolve(), refresh: async () => {}, dispose() {} };
  } } };
  vm.runInNewContext(readFileSync(require.resolve('../view-library.js'), 'utf8'), {
    window, document: { createElement: tag => new Node(tag) },
  });
  const root = new Node('root');
  const library = window.TerrariumViewLibrary.mount(root, { query: 'tasks' }, {
    plugin: 'todo', plugins: [{ id: 'todo', views: { tasks: view } }], bridge: { changed() {} },
    snapshot: { selectedId: 'v1', title: 'Work draft', open: true, dirty: true,
      config: { ...record.config, filters: '{"q":"unapplied"}', density: 'compact', fields: 'title' },
      appliedConfig: record.config },
    service: async (name, args) => {
      if (name === 'views_list') return [record];
      if (name === 'view_save') { writes.push(args); Object.assign(record, args); return record; }
      throw new Error(name);
    },
  });
  await library.ready;
  assert.equal(previews.at(-1).filters, '{}');
  assert.equal(library.snapshot().config.filters, '{"q":"unapplied"}');
  assert.equal(library.snapshot().title, 'Work draft');
  const flatten = node => [node, ...node.children.filter(n => typeof n !== 'string').flatMap(flatten)];
  const save = flatten(root).find(node => node.tag === 'button' && node.textContent === 'Save changes');
  await save.fire('click');
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(writes.length, 1);
  assert.equal(writes[0].config.density, 'compact');
  assert.equal(writes[0].config.fields, 'title');
  assert.equal(writes[0].title, 'Work draft');
  assert.equal(previews.at(-1).filters, '{"q":"unapplied"}');
  library.dispose();
});
