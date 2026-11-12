const test = require('node:test');
const assert = require('node:assert/strict');
const { rendererHarness } = require('./helpers/renderer-dom.cjs');
const bridge = { today: () => '2026-11-12', query: async () => [] };
const flatten = node => [node, ...(node.children || []).filter(n => typeof n !== 'string').flatMap(flatten)];

test('Today undated group starts collapsed, expands and survives a view snapshot', async () => {
  const template = {state: {expanded: false, filter: 'today'}, body: {type: 'list', query: 'tasks', group: 'section',
    collapse_group: {value: 'No date', when: '{{state.filter == today}}', bind: 'state.expanded'},
    item: {type: 'text', text: '{{row.title}}'}}};
  const data = {...bridge, query: async () => [{title: 'Due today', section: 'Today'}, {title: 'Undated', section: 'No date'}]};
  const ui = rendererHarness(); const view = ui.renderer.mount(ui.root, template, data); await view.ready;
  assert.equal(flatten(ui.root).some(n => n.textContent === 'Undated'), false);
  const toggle = flatten(ui.root).find(n => n.attrs?.['aria-expanded'] === 'false');
  assert.ok(toggle); await toggle.fire('click'); await view.refresh();
  assert.equal(flatten(ui.root).some(n => n.textContent === 'Undated'), true);
  const next = rendererHarness(); const restored = next.renderer.mount(next.root, template, data, {snapshot: view.snapshot()}); await restored.ready;
  assert.equal(flatten(next.root).some(n => n.textContent === 'Undated'), true);
  view.setState('state.filter', 'all'); view.setState('state.expanded', false); await view.refresh();
  assert.equal(flatten(ui.root).some(n => n.textContent === 'Undated'), true);
  restored.dispose(); view.dispose();
});

test('reminder stays blank and disabled until a due date is entered, then clears when date is removed', async () => {
  const ui = rendererHarness(); const view = ui.renderer.mount(ui.root, {body: {type:'form', children:[
    {type:'date', name:'due', value:''}, {type:'time', name:'reminder', value:'12:30', enabled_when:'{{form.due}}'},
  ]}}, bridge); await view.ready;
  const [due, reminder] = ui.root.querySelectorAll('[data-pp-name]');
  assert.equal(reminder.value, ''); assert.equal(reminder.disabled, true);
  due.value='2026-11-12'; await ui.root.children[0].fire('change'); assert.equal(reminder.disabled, false);
  reminder.value='18:00'; due.value=''; await ui.root.children[0].fire('change');
  assert.equal(reminder.value, ''); assert.equal(reminder.disabled, true); view.dispose();
});

test('display times follow locale, percentages include a unit, and week dots expose full dates', async () => {
  const ui = rendererHarness(); const view = ui.renderer.mount(ui.root, {body:{type:'stack',children:[
    {type:'text',text:'Every Sunday at 18:00',format:'local-time'},
    {type:'bar',query:'rates',label:'label',value:'pct',suffix:'%'},
    {type:'week-strip',days:'d',end:'2026-11-12'},
  ]}}, {...bridge,query:async()=>[{label:'Walk',pct:100}]}); await view.ready;
  const time = new Date(2000,0,1,18).toLocaleTimeString([], {hour:'numeric',minute:'2-digit'});
  assert.ok(flatten(ui.root).some(n=>n.textContent===`Every Sunday at ${time}`));
  assert.ok(flatten(ui.root).some(n=>n.textContent==='100%'));
  const dot=flatten(ui.root).find(n=>String(n.className).includes('pp-wdot-d'));
  assert.match(dot.title,/2026/);assert.equal(dot.title,dot.attrs['aria-label']); view.dispose();
});

test('habit completion does not add deletion styling', async () => {
  const ui=rendererHarness();const view=ui.renderer.mount(ui.root,{body:{type:'item',title:'Walk',done:true,strike_done:false}},bridge);await view.ready;
  assert.equal(ui.root.children[0].className,'pp-item');view.dispose();
});

test('bound query-backed filters preserve selection and refresh options after data changes', async () => {
  let rows=[{id:'first',title:'First feed'}];const ui=rendererHarness();const view=ui.renderer.mount(ui.root,{state:{feed:''},body:{type:'select',bind:'state.feed',options:[{value:'',label:'All feeds'}],options_query:{query:'feeds',value:'id',label:'title'}}},{...bridge,query:async()=>rows});await view.ready;
  const select=ui.root.querySelectorAll('select')[0];select.value='first';await select.fire('change');await view.refresh();
  assert.equal(view.state.feed,'first');rows=[...rows,{id:'second',title:'Second feed'}];await view.refresh();
  assert.equal(select.value,'first');assert.equal(select.children.length,3);view.dispose();
});

test('Home habit streaks renders a useful empty state instead of a blank activity grid', async () => {
  const path = require('node:path');
  const template = require(path.join(process.env.TERRA_PLUGIN_SOURCE, 'habits/ui/widget-streaks.json'));
  const ui = rendererHarness();
  const view = ui.renderer.mount(ui.root, template, {...bridge, query: async () => [{habits: 0}]}); await view.ready;
  assert.ok(flatten(ui.root).some(n => n.textContent === 'No habits yet. Open Habits to add one.'));
  assert.equal(flatten(ui.root).some(n => n.className === 'pp-heatmap'), false);
  view.dispose();
});
