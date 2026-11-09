const { test } = require('node:test');
const assert = require('node:assert/strict');
const grid = require('../grid.js');

const at = (id, x, y, w, h) => ({ id, x, y, w, h });
const rects = (items) => Object.fromEntries(items.map((i) => [i.id, [i.x, i.y, i.w, i.h]]));
const overlaps = (items) => items.some((a) => items.some((b) => grid.collides(a, b)));

test('size limits are completed, ordered and clamped to the grid', () => {
  assert.deepEqual(grid.limits(null), { min: [2, 2], max: [12, 12], default: [4, 3] });
  assert.deepEqual(grid.limits({ min: [6, 4], max: [3, 2], default: [1, 1] }), { min: [6, 4], max: [6, 4], default: [6, 4] });
  assert.deepEqual(grid.limits({ default: [40, 'x'] }).default, [12, 3]);
  const loose = grid.limits({ min: [6, 6] });
  assert.deepEqual(grid.limits(null).max, [12, 12], 'defaults are not shared');
  assert.deepEqual(loose.max, [12, 12]);
  assert.deepEqual(grid.limits({ default: [40, 2] }).default, [12, 2]);
});

test('clamp keeps placements inside 12 columns and their limits', () => {
  assert.deepEqual(rects([grid.clamp(at('a', 10, -3, 5, 1), { min: [3, 2] })]), { a: [7, 0, 5, 2] });
  assert.deepEqual(rects([grid.clamp({ id: 'a' }, { default: [6, 4] })]), { a: [0, 0, 6, 4] });
});

test('compact closes gaps and separates merged overlaps', () => {
  const out = grid.compact([at('a', 0, 5, 4, 2), at('b', 0, 5, 4, 2), at('c', 4, 9, 4, 1)]);
  assert.deepEqual(rects(out), { a: [0, 0, 4, 2], b: [0, 2, 4, 2], c: [4, 0, 4, 1] });
  assert.equal(overlaps(out), false);
  assert.deepEqual(out.map((i) => i.id), ['a', 'b', 'c'], 'input order is kept');
});

test('moving onto a neighbour pushes it out of the way', () => {
  const items = [at('a', 0, 0, 4, 2), at('b', 4, 0, 4, 2), at('c', 0, 2, 8, 2)];
  const out = grid.place(items, 'c', { y: 0 });
  assert.equal(overlaps(out), false);
  assert.deepEqual(rects(out).c, [0, 0, 8, 2]);
  assert.deepEqual(rects(out).a, [0, 2, 4, 2]);
});

test('dragging down past a taller neighbour swaps them', () => {
  const items = [at('a', 0, 0, 4, 2), at('b', 0, 2, 4, 5)];
  const out = grid.place(items, 'a', { y: 5 });
  assert.deepEqual(rects(out), { a: [0, 5, 4, 2], b: [0, 0, 4, 5] });
});

test('resize respects limits and reflows others', () => {
  const items = [at('a', 0, 0, 4, 2), at('b', 0, 2, 4, 2)];
  const out = grid.place(items, 'a', { w: 20, h: 3 }, { max: [8, 6] });
  assert.deepEqual(rects(out), { a: [0, 0, 8, 3], b: [0, 3, 4, 2] });
});

test('first fit finds the earliest free slot', () => {
  const items = [at('a', 0, 0, 6, 2), at('b', 6, 0, 3, 4)];
  assert.deepEqual(grid.firstFit(items, 3, 2), { x: 9, y: 0, w: 3, h: 2 });
  assert.deepEqual(grid.firstFit(items, 6, 2), { x: 0, y: 2, w: 6, h: 2 });
  assert.deepEqual(grid.firstFit([], 20, 2), { x: 0, y: 0, w: 12, h: 2 });
  assert.equal(grid.bottom(items), 4);
});

test('keyboard nudges move, swap and resize', () => {
  const items = [at('a', 0, 0, 4, 3), at('b', 0, 3, 4, 1)];
  assert.deepEqual(rects(grid.nudge(items, 'a', 'ArrowDown', false)), { a: [0, 1, 4, 3], b: [0, 0, 4, 1] });
  assert.deepEqual(rects(grid.nudge(items, 'a', 'ArrowRight', false)).a, [1, 0, 4, 3]);
  assert.deepEqual(rects(grid.nudge(items, 'a', 'ArrowLeft', false)).a, [0, 0, 4, 3]);
  assert.deepEqual(rects(grid.nudge(items, 'a', 'ArrowRight', true)), { a: [0, 0, 5, 3], b: [0, 3, 4, 1] });
  assert.deepEqual(rects(grid.nudge(items, 'a', 'ArrowUp', true)), { a: [0, 0, 4, 2], b: [0, 2, 4, 1] });
  assert.equal(grid.nudge(items, 'zz', 'ArrowUp', false), items);
  let many = grid.compact(Array.from({ length: 9 }, (_, i) => at(`w${i}`, (i % 2) * 6, i, 6, 3)));
  for (let i = 0; i < 9; i += 1) many = grid.nudge(many, 'w8', i % 3 ? 'ArrowUp' : 'ArrowRight', false);
  assert.deepEqual(rects(grid.compact(many)), rects(many), 'keyboard moves leave no gaps');
});

test('narrow screens stack in reading order; size classes follow width', () => {
  const items = [at('c', 0, 4, 4, 2), at('b', 6, 0, 6, 2), at('a', 0, 0, 6, 2)];
  assert.deepEqual(grid.stack(items).map((i) => i.id), ['a', 'b', 'c']);
  assert.equal(grid.sizeClass(at('x', 0, 0, 3, 4)), 'compact');
  assert.equal(grid.sizeClass(at('x', 0, 0, 6, 2)), 'compact');
  assert.equal(grid.sizeClass(at('x', 0, 0, 6, 3)), 'regular');
  assert.equal(grid.sizeClass(at('x', 0, 0, 8, 3)), 'expanded');
});

test('random edits never leave overlaps', () => {
  let seed = 7;
  const rand = (n) => { seed = (seed * 1103515245 + 12345) % 2147483648; return seed % n; };
  let items = Array.from({ length: 10 }, (_, i) => ({ id: `w${i}`, ...grid.firstFit([], 4, 2) }));
  items = grid.compact(items);
  for (let step = 0; step < 300; step += 1) {
    const id = `w${rand(10)}`;
    items = rand(2)
      ? grid.place(items, id, { x: rand(12), y: rand(12) })
      : grid.place(items, id, { w: 1 + rand(12), h: 1 + rand(6) }, { min: [1, 1] });
    assert.equal(overlaps(items), false, `overlap after step ${step}`);
    assert.deepEqual(rects(grid.compact(items)), rects(items), `gap left after step ${step}`);
    assert.ok(items.every((i) => i.x >= 0 && i.x + i.w <= 12 && i.y >= 0));
  }
});
