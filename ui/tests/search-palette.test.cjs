const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const source = fs.readFileSync(require('node:path').join(__dirname, '../app.js'), 'utf8');
const code = source.slice(source.indexOf('async function searchPlugins('), source.indexOf('function renderPalette('));
function harness(result) {
  const rendered = [];
  const ctx = {
    call: async () => typeof result === 'function' ? result() : result,
    pluginList: {plugins: [{id:'todo', name:'To-do', quick_actions:[]}]},
    spaces: [{id:'personal', name:'Personal'}], currentSpace:'personal',
    palette: {seq:0, mode:'search', allSpaces:false},
    appTile: () => '', icon: () => '', commands: () => [],
    $: () => ({value:'lamp'}), renderPalette: items => rendered.push(items),
  };
  vm.runInNewContext(code, ctx); return {ctx, rendered};
}
test('indexing shows a disabled line', async () => {
  const h = harness({results:[], errors:[], truncated:false, indexing:['notes']});
  await h.ctx.updatePalette();
  const item = h.rendered.at(-1).find(item => item.label === 'Indexing…');
  assert.ok(item); assert.equal(item.run, null);
});
test('kind and subtitle form the second line', async () => {
  const h = harness({results:[{plugin:'todo', space:'personal', title:'Lamp', kind:'Task', subtitle:'Tomorrow'}], indexing:[]});
  await h.ctx.updatePalette(); assert.equal(h.rendered.at(-1)[0].sub, 'Task · Tomorrow');
});
test('snippet is shown when the subtitle is empty', async () => {
  const h = harness({results:[{plugin:'todo', title:'Lamp', kind:'Task', subtitle:'', snippet:'A lamp by the desk'}]});
  await h.ctx.updatePalette(); assert.equal(h.rendered.at(-1)[0].sub, 'Task · A lamp by the desk');
});
test('stale results are ignored', async () => {
  let resolve; const h = harness(() => new Promise(r => {resolve=r;}));
  const pending = h.ctx.updatePalette(); h.ctx.palette.seq++;
  resolve({results:[], indexing:['todo']}); await pending;
  assert.equal(h.rendered.length, 1); assert.equal(h.rendered[0].length, 0);
});
