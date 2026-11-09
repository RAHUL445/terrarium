const { test } = require('node:test');
const assert = require('node:assert/strict');
const createProfiler = require('../performance.js');
const TemplateCache = require('../templates.js');

test('profiling is inactive by default and preserves operation results', async () => {
  const profiler = createProfiler(false, () => { throw Error('clock used'); }, () => { throw Error('report used'); });
  assert.equal(await profiler.measure('example', async () => 42), 42);
  await assert.rejects(profiler.measure('example', async () => { throw Error('original'); }), /original/);
});
test('enabled profiling records success/failure timing without inspecting results', async () => {
  const records = [];
  let time = 0;
  const profiler = createProfiler(true, () => (time += 5), record => records.push(record));
  assert.deepEqual(await profiler.measure('read', async () => ({ private: 'not recorded' })), { private: 'not recorded' });
  await assert.rejects(profiler.measure('read', async () => { throw Error('not recorded'); }));
  assert.deepEqual(records, [{ name: 'read', durationMs: 5, ok: true }, { name: 'read', durationMs: 5, ok: false }]);
});
test('diagnostic reporting failures cannot change app results', async () => {
  const profiler = createProfiler(true, () => 0, () => { throw Error('console unavailable'); });
  assert.equal(await profiler.measure('read', async () => 42), 42);
});
test('synchronous rendering spans preserve return values and failures', () => {
  let time = 0;
  const records = [];
  const profiler = createProfiler(true, () => ++time, record => records.push(record));
  assert.equal(profiler.measureSync('render', () => 42), 42);
  assert.throws(() => profiler.measureSync('render', () => { throw Error('original'); }), /original/);
  assert.deepEqual(records.map(record => record.ok), [true, false]);
});
const args = { plugin: 'notes', kind: 'page', id: 'main' };
test('template metadata loads once for concurrent and repeated consumers', async () => {
  const cache = new TemplateCache();
  let calls = 0;
  const load = async () => { calls++; return { state: {} }; };
  const [home, companion] = await Promise.all([cache.get(args, load), cache.get(args, load)]);
  assert.equal(home, companion);
  assert.equal(await cache.get(args, load), home);
  assert.equal(calls, 1);
  assert.notEqual(await cache.get({ ...args, kind: 'widget' }, load), home);
});
test('plugin reload invalidates even same-version templates', async () => {
  const cache = new TemplateCache();
  const old = await cache.get(args, async () => 'old');
  cache.clear();
  assert.equal(old, 'old');
  assert.equal(await cache.get(args, async () => 'new'), 'new');
});
test('failed loads are retried and stale failures cannot evict a newer generation', async () => {
  const cache = new TemplateCache();
  await assert.rejects(cache.get(args, async () => { throw Error('missing'); }));
  assert.equal(await cache.get(args, async () => 'recovered'), 'recovered');
  cache.clear();
  let fail;
  const old = cache.get(args, () => new Promise((resolve, reject) => { fail = reject; }));
  await Promise.resolve();
  cache.clear();
  assert.equal(await cache.get(args, async () => 'new generation'), 'new generation');
  fail(Error('stale failure')); await assert.rejects(old);
  assert.equal(await cache.get(args, async () => 'wrong'), 'new generation');
});
test('an invalidated pending success cannot repopulate the cache', async () => {
  const cache = new TemplateCache();
  let finish;
  const old = cache.get(args, () => new Promise(resolve => { finish = resolve; }));
  await Promise.resolve();
  cache.clear();
  finish('old'); await old;
  assert.equal(await cache.get(args, async () => 'new'), 'new');
});
