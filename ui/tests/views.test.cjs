const { test } = require("node:test");
const assert = require("node:assert/strict");
const views = require("../views.js");
const { rendererHarness } = require("./helpers/renderer-dom.cjs");

const plugins = [
  {
    id: "todo",
    name: "To-do",
    views: {
      tasks: {
        title: "Tasks",
        title_column: "title",
        params: { filter: "everything" },
        columns: [
          { key: "title", label: "Task", type: "string" },
          { key: "due", label: "Due", type: "date" },
          {
            key: "board_lane",
            label: "Status",
            type: "enum",
            values: ["todo", "doing", "done"],
            labels: ["To do", "In progress", "Done"],
            move: { op: "move", args: { id: "{{row.id}}", lane: "{{value}}" } },
          },
        ],
      },
    },
  },
  { id: "notes", name: "Notes", views: {} },
];

function taskView() {
  return plugins[0].views.tasks;
}

test("viewable queries are listed per plugin", () => {
  const listed = views.viewsOf(plugins);
  assert.equal(listed.length, 1);
  assert.equal(listed[0].plugin.id, "todo");
  assert.equal(listed[0].query, "tasks");
  assert.equal(views.find(plugins, "todo", "nope"), null);
});

test("layouts follow the columns", () => {
  assert.deepEqual(views.layouts(taskView()), ["list", "table", "board", "calendar"]);
  const stringOnly = {
    title: "Simple",
    title_column: "name",
    columns: [{ key: "name", label: "Name", type: "string" }],
  };
  assert.deepEqual(views.layouts(stringOnly), ["list", "table"]);
});

test("unsupported settings fall back safely", () => {
  const safe = views.settings(taskView(), { layout: "board", group: "title", sort: "missing" });
  assert.equal(safe.group, "board_lane");
  assert.equal(safe.sort, "");
  const stringOnly = {
    title: "Simple",
    title_column: "name",
    columns: [{ key: "name", label: "Name", type: "string" }],
  };
  const fallback = views.settings(stringOnly, { layout: "calendar" });
  assert.equal(fallback.layout, "list");
});

test("a compact view always uses the list layout", () => {
  assert.equal(views.layoutFor({ layout: "board" }, "compact"), "list");
  assert.equal(views.layoutFor({ layout: "calendar" }, "compact"), "list");
  assert.equal(views.layoutFor({ layout: "table" }, "large"), "table");
});

test("rows are filtered by focus, labelled and sorted", () => {
  const rows = [
    { title: "Pay rent", due: "2026-11-09", board_lane: "doing" },
    { title: "Call Sam", due: "2026-11-01", board_lane: "todo" },
    { title: "Rent car", due: null, board_lane: "done" },
  ];
  const filtered = views.prepareRows(rows, taskView(), { sort: "due", dir: "asc" }, "rent");
  assert.deepEqual(filtered.map((row) => row.title), ["Pay rent", "Rent car"]);
  assert.equal(filtered[0].board_lane__label, "In progress");
  const byLabel = views.prepareRows(rows, taskView(), {}, "in progress");
  assert.deepEqual(byLabel.map((row) => row.title), ["Pay rent"]);
});

test("the board template moves with the column's op", () => {
  const entry = views.find(plugins, "todo", "tasks");
  const body = views.template(entry, { layout: "board" }).body;
  assert.equal(body.layout, "board");
  assert.equal(body.column, "board_lane");
  assert.deepEqual(body.columns.map((lane) => lane.label), ["To do", "In progress", "Done"]);
  assert.equal(body.move.op, "move");
});

test("the table shows enum labels", () => {
  const entry = views.find(plugins, "todo", "tasks");
  const body = views.template(entry, { layout: "table" }).body;
  const status = body.columns.find((column) => column.label === "Status");
  assert.equal(status.key, "board_lane__label");
  assert.equal(status.type, "string");
});

test("the wrapped bridge filters what the query returns", async () => {
  const rows = [
    { id: "a", title: "Pay rent", due: "2026-11-09", board_lane: "doing" },
    { id: "b", title: "Call Sam", due: "2026-11-01", board_lane: "todo" },
    { id: "c", title: "Rent car", due: null, board_lane: "done" },
  ];
  const entry = views.find(plugins, "todo", "tasks");
  const base = {
    query: async () => rows,
    openItem() { return "open"; },
    action() { return "action"; },
  };
  const wrapped = views.wrapBridge(base, entry, {}, "rent");
  const got = await wrapped.query("tasks", {});
  assert.equal(got.length, 2);
  assert.equal(wrapped.openItem, base.openItem);
  assert.equal(wrapped.action, base.action);
});

test("views.js and the table sort the same way", () => {
  const rows = [
    { id: "a", n: 2, due: "2026-11-03" },
    { id: "b", n: null, due: null },
    { id: "c", n: 2, due: "2026-11-01" },
    { id: "d", n: 10, due: "2026-11-02" },
    { id: "e", n: "", due: "2026-11-04" },
  ];
  const renderer = rendererHarness().renderer;
  for (const dir of ["asc", "desc"]) {
    const rendererOrder = renderer.sortRows(rows, "n", "integer", dir).map((row) => row.id);
    const viewsOrder = sortWithViews(rows, "n", "integer", dir).map((row) => row.id);
    assert.deepEqual(viewsOrder, rendererOrder);
  }

  function sortWithViews(input, key, type, dir) {
    const order = dir === "desc" ? -1 : 1;
    const isEmpty = (value) => value == null || value === "" || ((type === "integer" || type === "number") && Number.isNaN(Number(value)));
    return input
      .map((row, index) => ({ row, index }))
      .sort((left, right) => {
        const leftEmpty = isEmpty(left.row?.[key]);
        const rightEmpty = isEmpty(right.row?.[key]);
        if (leftEmpty || rightEmpty) return leftEmpty === rightEmpty ? left.index - right.index : (leftEmpty ? 1 : -1);
        const cmp = views.compareValues(left.row?.[key], right.row?.[key], type);
        if (cmp === 0) return left.index - right.index;
        return cmp * order;
      })
      .map((entry) => entry.row);
  }
});
