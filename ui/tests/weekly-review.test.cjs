const { test } = require('node:test');
const assert = require('node:assert/strict');
const vm = require('node:vm');
const { readFileSync } = require('node:fs');
const period = require('../review-period.js');
function setup(failSave=false) {
  const records=[], messages=[],mounted=[];
  const window={TerrariumReviewPeriod:period,TerrariumViews:{find:()=>({plugin:{id:'todo'},view:{temporal:{start_param:'start_ms',end_param:'end_ms'}}}),mount:(_,options)=>{mounted.push(options);return {ready:Promise.resolve(),refresh(){},dispose(){}};}}};
  vm.runInNewContext(readFileSync(require.resolve('../pages.js'),'utf8'),{window,Intl});
  const page=Object.create(window.TerrariumPages.prototype);
  Object.assign(page,{space:'space-1',draft:null,plugins:[{id:'todo'}],page:{title:'Review',period:{mode:'custom',timezone:'Asia/Kolkata',start:'2026-11-02',end:'2026-11-09'}},
    grid:{sizeClass:()=> 'normal'},focusOf:()=>'',sessionKey:()=> 'review',el:()=>({}),
    o:{toast:(...args)=>messages.push(args),show:async id=>records.push(['show',id]),bridge:()=>({}),session:{get:()=>null},call:async(_,v)=>{
      records.push([v.name,v.args]);
      if(v.name==='page_create')return {id:'new-review'};
      if(v.name==='page_save'&&failSave)throw Error('save failed');
      if(v.name==='pages_list')return [];
      if(v.name==='view_get')return {space:'space-1',config:{plugin:'todo',query:'review_tasks'}};
    }}});
  return {page,records,messages,mounted};
}
test('weekly review creates independent Tasks, Notes, reflection and capture placements with one period',async()=>{
  const {page,records}=setup();await page.createWeeklyReview();
  const create=records.find(r=>r[0]==='page_create')[1];assert.equal(create.period.mode,'previous_week');
  const saved=records.find(r=>r[0]==='page_save')[1];assert.equal(saved.placements.length,4);
  assert.deepEqual(Array.from(saved.placements.slice(0,2),p=>p.config.query),['review_tasks','review_notes']);
  assert.ok(saved.placements.slice(0,2).every(p=>p.config.follow_page_period===true&&!p.config.saved_view));
  assert.ok(records.some(r=>r[0]==='show'));assert.equal(page.creatingReview,false);
});
test('failed review setup removes the incomplete page and releases the creation guard',async()=>{
  const {page,records,messages}=setup(true);await page.createWeeklyReview();
  assert.ok(records.some(r=>r[0]==='page_remove'));assert.ok(!records.some(r=>r[0]==='show'));
  assert.match(messages.at(-1)[0],/save failed/);assert.equal(page.creatingReview,false);
});
test('shared view reference keeps page period binding local and evaluates refreshed periods',async()=>{
  const {page,mounted}=setup();const handle=page.viewWidget({replaceChildren(){},append(){}},{id:'p'},{saved_view:'shared',follow_page_period:true});
  await handle.ready;assert.equal(mounted.length,1);
  const first=mounted[0].period();assert.equal(first.start_ms,Date.parse('2026-11-01T18:30:00Z'));
  page.page.period={mode:'custom',timezone:'UTC',start:'2026-11-09',end:'2026-11-16'};
  assert.equal(mounted[0].period().start_ms,Date.parse('2026-11-09T00:00:00Z'));
  page.page.period=null;assert.throws(()=>mounted[0].period(),/Choose a review period/);
});
