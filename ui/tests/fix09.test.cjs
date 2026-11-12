const test = require('node:test');
const assert = require('node:assert/strict');
const {readFileSync} = require('node:fs');
const {join} = require('node:path');
const {rendererHarness} = require('./helpers/renderer-dom.cjs');
const flatten = node => [node, ...(node.children || []).filter(n => typeof n !== 'string').flatMap(flatten)];
test('grouped feed rows show locale dates, distinct read states and no repeated source', async () => {
  const template=JSON.parse(readFileSync(join(__dirname,'../../../terrarium-plugins/packages/feeds/ui/page-main.json'),'utf8'));
  const list=template.body.children.find(n=>n.query==='items');
  const date=new Date(2026,10,11,5,30).getTime();
  const ui=rendererHarness();
  const view=ui.renderer.mount(ui.root,{body:list},{today:()=> '2026-11-12',query:async()=>[
    {id:'a',title:'Unread article',feed_title:'Rust Blog',read:0,published_ms:date},
    {id:'b',title:'Read article',feed_title:'Rust Blog',read:1,published_ms:date},
  ]});await view.ready;
  const nodes=flatten(ui.root);
  assert.equal(nodes.filter(n=>n.className?.split(/\s+/).includes('pp-item-unread')).length,1);
  assert.equal(nodes.filter(n=>n.className?.split(/\s+/).includes('pp-item-read')).length,1);
  assert.equal(nodes.filter(n=>n.children?.includes(new Date(date).toLocaleDateString())).length,2);
  assert.equal(list.group,'feed_title');
  assert.equal(list.item.chips.some(c=>c.text.includes('feed_title')),false);
  const css=readFileSync(join(__dirname,'../app.css'),'utf8');
  assert.match(css,/\.pp-item-unread \.pp-item-title \{ font-weight: 700/);
  assert.match(css,/\.pp-item-read \.pp-item-main \{ color: var\(--muted\)/);
  view.dispose();
});
