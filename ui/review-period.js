/* Page period date resolution shared by the browser and Node tests. */
(function (root, factory) {
  const api = factory();
  if (typeof module === "object" && module.exports) module.exports = api;
  if (root) root.TerrariumReviewPeriod = api;
})(typeof globalThis === "object" ? globalThis : this, function () {
  "use strict";

  const DAY_MS = 24 * 60 * 60 * 1000;
  const DATE_RE = /^(\d{4})-(\d{2})-(\d{2})$/;

  function utcMs(year, month, day, hour = 0, minute = 0, second = 0) {
    const value = new Date(0);
    value.setUTCFullYear(year, month - 1, day);
    value.setUTCHours(hour, minute, second, 0);
    return value.getTime();
  }

  function parseDate(value) {
    const match = typeof value === "string" && DATE_RE.exec(value);
    if (!match) throw new TypeError("Date must use YYYY-MM-DD");
    const [, y, m, d] = match;
    const year = Number(y), month = Number(m), day = Number(d);
    const check = new Date(utcMs(year, month, day));
    if (year < 1 || year > 9999 || check.getUTCFullYear() !== year ||
        check.getUTCMonth() !== month - 1 || check.getUTCDate() !== day) {
      throw new RangeError("Date is not a real calendar date");
    }
    return { year, month, day, dayNumber: Math.floor(utcMs(year, month, day) / DAY_MS) };
  }

  function formatter(timezone) {
    try {
      return new Intl.DateTimeFormat("en-CA-u-ca-gregory-nu-latn", {
        timeZone: timezone,
        year: "numeric", month: "2-digit", day: "2-digit",
        hour: "2-digit", minute: "2-digit", second: "2-digit",
        hourCycle: "h23"
      });
    } catch (_) {
      throw new RangeError("Timezone must be a valid IANA timezone");
    }
  }

  function partsAt(format, timestamp) {
    const parts = Object.create(null);
    for (const item of format.formatToParts(new Date(timestamp))) {
      if (item.type !== "literal") parts[item.type] = Number(item.value);
    }
    return parts;
  }

  function midnight(date, timezone, format) {
    const target = utcMs(date.year, date.month, date.day);
    let guess = target;
    for (let i = 0; i < 6; i++) {
      const local = partsAt(format, guess);
      const represented = utcMs(local.year, local.month, local.day,
        local.hour, local.minute, local.second);
      const next = target - (represented - guess);
      if (next === guess) break;
      guess = next;
    }
    const actual = partsAt(format, guess);
    if (actual.year !== date.year || actual.month !== date.month || actual.day !== date.day ||
        actual.hour !== 0 || actual.minute !== 0 || actual.second !== 0) {
      throw new RangeError(`Local midnight does not exist for ${formatDate(date)} in ${timezone}`);
    }
    return guess;
  }

  function formatDate(date) {
    return `${String(date.year).padStart(4, "0")}-${String(date.month).padStart(2, "0")}-${String(date.day).padStart(2, "0")}`;
  }

  function addDays(date, count) {
    const moved = new Date(utcMs(date.year, date.month, date.day + count));
    return { year: moved.getUTCFullYear(), month: moved.getUTCMonth() + 1, day: moved.getUTCDate() };
  }

  function labelFor(start, end) {
    const last = addDays(end, -1);
    const monthDay = date => new Intl.DateTimeFormat("en", {
      timeZone: "UTC", month: "short", day: "numeric"
    }).format(new Date(utcMs(date.year, date.month, date.day)));
    const first = monthDay(start);
    const final = start.year === last.year && start.month === last.month
      ? String(last.day)
      : monthDay(last);
    const year = last.year;
    return start.year === year ? `${first}–${final}, ${year}` : `${first}, ${start.year}–${final}, ${year}`;
  }

  function resolve(period, now = Date.now()) {
    if (!period || typeof period !== "object" || Array.isArray(period)) {
      throw new TypeError("Page period must be an object");
    }
    const { mode, timezone } = period;
    if (!["previous_week", "this_week", "custom"].includes(mode)) {
      throw new RangeError("Unknown page period mode");
    }
    if (typeof timezone !== "string" || !timezone.trim()) throw new TypeError("Page period needs a timezone");
    const format = formatter(timezone);
    let start, end;
    if (mode === "custom") {
      start = parseDate(period.start);
      end = parseDate(period.end);
      const span = end.dayNumber - start.dayNumber;
      if (span < 1 || span > 366) throw new RangeError("Custom period must span 1 to 366 days");
    } else {
      if (period.start !== undefined || period.end !== undefined) {
        throw new TypeError("Only a custom period can have start and end dates");
      }
      if (!Number.isFinite(now)) throw new TypeError("now must be a timestamp in milliseconds");
      const local = partsAt(format, now);
      const today = { year: local.year, month: local.month, day: local.day };
      const weekday = new Date(utcMs(today.year, today.month, today.day)).getUTCDay();
      const mondayOffset = (weekday + 6) % 7;
      const thisMonday = addDays(today, -mondayOffset);
      start = mode === "this_week" ? thisMonday : addDays(thisMonday, -7);
      end = addDays(thisMonday, mode === "this_week" ? 7 : 0);
    }
    const start_ms = midnight(start, timezone, format);
    const end_ms = midnight(end, timezone, format);
    if (end_ms <= start_ms) throw new RangeError("Page period end must be after start");
    return { start_ms, end_ms, timezone, label: labelFor(start, end) };
  }

  return { resolve };
});
