const { test } = require('node:test');
const assert = require('node:assert/strict');
const { rendererHarness } = require('./helpers/renderer-dom.cjs');
const wait = () => new Promise(resolve => setTimeout(resolve, 200));
const turn = () => new Promise(resolve => setImmediate(resolve));
const walk = n => typeof n === 'string' ? [] : [n, ...n.children.flatMap(walk)];
const hit = { ref: 'todo/tasks/t1', title: 'Buy lamp', kind: 'Task', subtitle: 'Private text' };
const action = { link: { from: '{{row.$ref}}', to: '{{value.ref}}' } };
async function setup(options = {}) {
  const ui = rendererHarness(); const calls = [], toasts = []; let changes = 0;
  const view = ui.renderer.mount(ui.root, { state: { picked: '', pickedRef: '' }, body: { type: 'list', query: 'one', table: 'notes', item: { type: 'ref-picker', action: options.action || action } } }, {
    today: () => '2026-11-11', plugin: 'notes', query: async () => [{ id: 'n1' }],
    command: async (...args) => {
      calls.push(args);
      if (options.command) return options.command(...args);
      return args[1] === 'search.query' ? { results: [hit, {title:'No ref'}, {...hit,ref:'bad'}] } : {id:'l1'};
    },
    action: options.operation || (() => assert.fail('unexpected plugin operation')),
    changed: () => changes++, toast: (...args) => toasts.push(args),
  });
  await view.ready;
  const input = walk(ui.root).find(n => n.tag === 'input');
  const picker = walk(ui.root).find(n => n.className === 'pp-mention');
  const type = async value => { input.value = value; await input.fire('input'); };
  return {...ui,view,input,picker,calls,toasts,type,changes:()=>changes};
}

test('choosing an item links it to the row', async () => {
  const ui = await setup();
  assert.equal(walk(ui.root).find(n => n.tag === 'label').textContent, 'Link to…');
  assert.equal(ui.input.type,'search'); assert.equal(ui.input.placeholder,'Search items');
  assert.equal(ui.input.attrs.role,'combobox'); assert.equal(ui.input.attrs['aria-controls'],ui.picker.id);
  await ui.type(' lam '); await wait();
  assert.deepEqual(JSON.parse(JSON.stringify(ui.calls[0])), ['query','search.query',{q:'lam',limit:8}]);
  assert.equal(ui.picker.children.length,1); await ui.input.fire('keydown',{key:'Enter'});
  assert.deepEqual(JSON.parse(JSON.stringify(ui.calls[1])), ['action','link.add',{source:'notes/notes/n1',target:hit.ref,kind:'related'}]);
  assert.equal(ui.changes(),1); assert.deepEqual(ui.toasts,[['Linked','ok']]);
  assert.equal(ui.input.value,''); assert.equal(ui.picker.hidden,true); assert.equal(ui.input.disabled,false); ui.view.dispose();
});

test('value carries only the ref', async () => {
  const ui = await setup({ action: {set: {'state.picked':'{{value.title}}','state.pickedRef':'{{value.ref}}'}} });
  await ui.type('lamp'); await wait(); await ui.picker.children[0].fire('mousedown');
  assert.equal(ui.view.state.picked,undefined); assert.equal(ui.view.state.pickedRef,hit.ref);
  assert.equal(ui.calls.filter(c=>c[0]==='action').length,0); ui.view.dispose();
});

test('a failed link shows an error and keeps the picker open', async () => {
  let reject;
  const ui = await setup({ command: async (kind,id) => id==='search.query' ? {results:[hit]} : new Promise((resolve,no)=>{reject=no;}) });
  await ui.type('lamp'); await wait();
  const choosing = ui.input.fire('keydown',{key:'Tab'}); await turn();
  assert.equal(ui.input.disabled,true); await ui.input.fire('blur'); await wait();
  assert.equal(ui.picker.hidden,false); reject(new Error('Link failed')); await choosing;
  assert.equal(ui.picker.hidden,false); assert.equal(ui.input.value,'lamp'); assert.equal(ui.input.disabled,false);
  assert.match(ui.toasts[0][0],/Link failed/); assert.equal(ui.toasts[0][1],'error'); assert.equal(ui.changes(),0); ui.view.dispose();
});

test('link runs before op, passes its ref and can show a custom success', async () => {
  const order = [];
  const ui = await setup({ action: {...action, op:'save', args:{ref:'{{value.ref}}'}, success:'Connected'},
    command: async (kind,id) => { if(id==='search.query')return {results:[hit]};order.push('link'); return {}; },
    operation: async (op,args) => { order.push(op); assert.equal(args.ref,hit.ref); return {}; },
  });
  await ui.type('lamp'); await wait(); await ui.input.fire('keydown',{key:'Enter'});
  assert.deepEqual(order,['link','save']); assert.equal(ui.toasts[0][0],'Connected'); ui.view.dispose();
});

test('failed links never fall through to plugin code', async () => {
  const ui = await setup({ action: {...action,op:'save'}, command: async (kind,id) => {if(id==='search.query')return {results:[hit]};throw new Error('offline');} });
  await ui.type('lamp'); await wait(); await ui.picker.children[0].fire('click');
  assert.equal(ui.picker.hidden,false); assert.equal(ui.changes(),0); ui.view.dispose();
});

test('mouse compatibility click does not submit a failed choice twice', async () => {
  const ui = await setup({ command: async (kind,id) => {if(id==='search.query')return {results:[hit]};throw new Error('offline');} });
  await ui.type('lamp'); await wait(); const item=ui.picker.children[0];
  await item.fire('mousedown'); await item.fire('click');
  assert.equal(ui.calls.filter(c=>c[1]==='link.add').length,1); ui.view.dispose();
});

test('shared picker supports arrows, Escape, empty input and stale search replies', async () => {
  let first;
  const ui = await setup({ command: async (kind,id,{q}) => q==='old' ? new Promise(resolve=>{first=resolve;}) : {results:[hit,{...hit,ref:'notes/notes/n2',title:'Second'}]} });
  await ui.type('old'); await wait(); await ui.type('new'); await wait();
  first({results:[{...hit,title:'Old'}]}); await turn(); assert.equal(ui.picker.children[0].textContent,'Buy lamp');
  await ui.input.fire('keydown',{key:'ArrowDown'}); assert.equal(ui.picker.children[1].attrs['aria-selected'],'true');
  await ui.input.fire('keydown',{key:'Escape'}); assert.equal(ui.picker.hidden,true); assert.equal(ui.input.value,'new');
  const count=ui.calls.length; await ui.type('   '); await wait(); assert.equal(ui.calls.length,count); ui.view.dispose();
});

test('disposal cancels debounce and ignores pending choices', async () => {
  const ui=await setup(); await ui.type('lamp'); ui.view.dispose(); await wait(); assert.equal(ui.calls.length,0);
  let finish;
  const pending=await setup({command: async(kind,id)=>id==='search.query'?{results:[hit]}:new Promise(resolve=>{finish=resolve;})});
  await pending.type('lamp'); await wait(); const choice=pending.input.fire('keydown',{key:'Enter'}); await turn();
  pending.view.dispose(); finish({id:'l1'}); await choice;
  assert.equal(pending.toasts.length,0); assert.equal(pending.changes(),1); assert.equal(pending.input.value,'lamp');
});
