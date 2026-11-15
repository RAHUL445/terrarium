const { test } = require('node:test');
const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const vm = require('node:vm');
const views = require('../views.js');
const entry = { query: 'review', view: { title: 'Review', version: 1,
  columns: [{ key: 'title', type: 'string' }, { key: 'completed_at', type: 'integer' }],
  params: { sort: 'completed_at', direction: 'desc' }, filters: [],
  pagination: { page_size_param: 'page_size', offset_param: 'offset', default_page_size: 2 },
  sorts: [{ param: 'sort', values: ['completed_at','title'], direction_param: 'direction', directions: ['asc','desc'] }],
  temporal: { start_param: 'start_ms', end_param: 'end_ms' } } };

test('paged views send temporal bounds and sorting before SQL, overfetch without dropping subsequent records', async () => {
  const data = Array.from({length: 7}, (_,i) => ({id: String(i), title: 'Same title', completed_at: i}));
  const calls = [], paging = {size: 2, offset: 0};
  const bridge = views.wrapBridge({query: async (_,p) => { calls.push(p); return data.slice(p.offset,p.offset+p.page_size); }}, entry,
    {sort:'completed_at',dir:'desc'}, '', paging, {start_ms: 10,end_ms: 20});
  const rows = [];
  do { rows.push(...await bridge.query('review', entry.view.params)); paging.offset += paging.size; } while (paging.more);
  assert.deepEqual(rows.map(r=>r.id),data.map(r=>r.id));
  assert.equal(calls.length,4);
  assert.deepEqual(calls[0],{sort:'completed_at',direction:'desc',start_ms:10,end_ms:20,page_size:3,offset:0});
  assert.equal(paging.count,1);
});

test('paged views reject unsupported focus and sort rather than silently filtering a partial batch', async () => {
  const base={query:async()=>[]};
  await assert.rejects(views.wrapBridge(base,entry,{},'project').query('review',{}), /does not support page focus/);
  const bad={...entry,view:{...entry.view,sorts:[{param:'sort',values:['completed_at']} ]}};
  await assert.rejects(views.wrapBridge(base,bad,{sort:'title'},'').query('review',{}), /sort is unavailable/);
});

test('mount pager advances, resets on refresh and disposes its child', async () => {
  class Node { constructor(){this.children=[];this.dataset={};this.classList={add(){}};this.listeners={};}
    append(...nodes){this.children.push(...nodes);} replaceChildren(...nodes){this.children=nodes;}
    setAttribute(){} addEventListener(name,fn){this.listeners[name]=fn;} }
  const window={}; vm.runInNewContext(readFileSync(require.resolve('../views.js'),'utf8'),{window,structuredClone,document:{createElement:()=>new Node()}});
  const body=new Node(), calls=[];let disposed=false;
  const handle=window.TerrariumViews.mount(body,{entry,config:{},bridge:{query:async(_,p)=>{calls.push(p.offset);return [{id:'a'},{id:'b'},{id:'c'}];}},
    mount:(target,template,bridge)=>{const refresh=()=>bridge.query('review',entry.view.params);return {ready:refresh(),refresh,flush:async()=>true,dispose(){disposed=true;}};}});
  await handle.ready;
  const controls=body.children[1];assert.equal(controls.children[0].disabled,true);
  await controls.children[2].listeners.click();assert.deepEqual(calls,[0,2]);
  await handle.refresh();assert.deepEqual(calls,[0,2,0]);
  assert.equal(controls.children[0].disabled,true);
  handle.dispose();assert.equal(disposed,true);
});

test('date presentation preserves UTC timestamps and uses the page timezone for calendar days',()=>{
  const timestamp=Date.parse('2026-11-08T20:00:00Z');
  const view={columns:[{key:'completed_at',type:'date'}]};
  const [india]=views.prepareRows([{completed_at:timestamp}],view,{},'','Asia/Kolkata');
  const [newYork]=views.prepareRows([{completed_at:timestamp}],view,{},'','America/New_York');
  assert.equal(india.completed_at,timestamp);assert.equal(india.completed_at__day,'2026-11-09');
  assert.equal(newYork.completed_at__day,'2026-11-08');assert.ok(!String(india.completed_at__label).includes(String(timestamp)));
  assert.equal(views.prepareRows([{completed_at:'2026-11-09'}],view,{},'','America/New_York')[0].completed_at__day,'2026-11-09');
});
