const test = require("node:test");
const assert = require("node:assert/strict");
const { resolve } = require("../review-period.js");

test("week boundaries use local midnight across spring DST", () => {
  const result = resolve({ mode: "this_week", timezone: "America/New_York" }, Date.parse("2024-03-06T16:00:00Z"));
  assert.equal(result.start_ms, Date.parse("2024-03-04T05:00:00Z"));
  assert.equal(result.end_ms, Date.parse("2024-03-11T04:00:00Z"));
  assert.equal(result.end_ms - result.start_ms, 167 * 60 * 60 * 1000);
  assert.equal(result.label, "Mar 4–10, 2024");
});

test("previous week boundaries use local midnight across fall DST", () => {
  const result = resolve({ mode: "previous_week", timezone: "America/New_York" }, Date.parse("2024-11-06T16:00:00Z"));
  assert.equal(result.start_ms, Date.parse("2024-10-28T04:00:00Z"));
  assert.equal(result.end_ms, Date.parse("2024-11-04T05:00:00Z"));
  assert.equal(result.end_ms - result.start_ms, 169 * 60 * 60 * 1000);
  assert.equal(result.label, "Oct 28–Nov 3, 2024");
});

test("India week starts at midnight in UTC+05:30", () => {
  const result = resolve({ mode: "this_week", timezone: "Asia/Kolkata" }, Date.parse("2024-06-12T12:00:00Z"));
  assert.equal(result.start_ms, Date.parse("2024-06-09T18:30:00Z"));
  assert.equal(result.end_ms, Date.parse("2024-06-16T18:30:00Z"));
  assert.equal(result.end_ms - result.start_ms, 7 * 24 * 60 * 60 * 1000);
});

test("custom dates are midnight bounded and end exclusive", () => {
  const result = resolve({ mode: "custom", timezone: "Asia/Kolkata", start: "2024-02-28", end: "2024-03-01" });
  assert.equal(result.start_ms, Date.parse("2024-02-27T18:30:00Z"));
  assert.equal(result.end_ms, Date.parse("2024-02-29T18:30:00Z"));
  assert.equal(result.label, "Feb 28–29, 2024");
});

test("custom period accepts the maximum 366 calendar days", () => {
  const result = resolve({ mode: "custom", timezone: "UTC", start: "2023-01-01", end: "2024-01-02" });
  assert.equal(result.end_ms - result.start_ms, 366 * 24 * 60 * 60 * 1000);
});

test("invalid modes, dates, ranges, timezones, and skipped local dates are rejected", () => {
  for (const period of [
    { mode: "next_week", timezone: "UTC" },
    { mode: "custom", timezone: "UTC", start: "2024-02-30", end: "2024-03-01" },
    { mode: "custom", timezone: "UTC", start: "2024-03-02", end: "2024-03-01" },
    { mode: "custom", timezone: "UTC", start: "2023-01-01", end: "2024-01-03" },
    { mode: "this_week", timezone: "Not/AZone" }
  ]) assert.throws(() => resolve(period));
  assert.throws(() => resolve({ mode: "custom", timezone: "Pacific/Apia", start: "2011-12-30", end: "2011-12-31" }), /midnight does not exist/);
});
