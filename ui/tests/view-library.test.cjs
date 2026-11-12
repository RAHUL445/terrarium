const { test } = require("node:test");
const assert = require("node:assert/strict");
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
