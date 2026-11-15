const { test } = require('node:test');
const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { resolve, join } = require('node:path');
const vm = require('node:vm');
const { rendererHarness } = require('./helpers/renderer-dom.cjs');

const packages = process.env.TERRA_PLUGIN_SOURCE || resolve(__dirname, '../../../terrarium-plugins/packages');
const template = JSON.parse(readFileSync(resolve(packages, 'tasks/ui/page-main.json'), 'utf8'));
const walk = node => !node || typeof node !== 'object' ? [] : [node, ...Object.values(node).flatMap(value => Array.isArray(value) ? value.flatMap(walk) : walk(value))];
const domNodes = (root, predicate, out = []) => {
  if (predicate(root)) out.push(root);
  for (const child of root.children || []) if (typeof child !== 'string') domNodes(child, predicate, out);
  return out;
};
const text = node => typeof node === 'string' ? node : (node.textContent || '') + (node.children || []).map(text).join('');
const hasClass = (node, name) => String(node.className || '').split(/\s+/).includes(name);
const byClass = (root, name) => domNodes(root, node => hasClass(node, name));
const turn = () => new Promise(done => setImmediate(done));

function bridge(calls, extra = {}) {
  return {
    today: () => '2026-11-12', toast() {}, changed() {},
    query: async (name, params = {}) => {
      calls.queries.push([name, params]);
      if (name === 'nav_counts') return [{ my_day: 2, inbox: 0, today: 1, upcoming: 0, ready: 3, open: 5 }];
      if (name === 'nav_lists') return [{ id: 'l1', name: 'Work', color: '#0f766e', icon: '', open: 4, filter: 'all', list_id: 'l1', nav: 'l1' }];
      if (name === 'all_group_choices') return [{ value: 'g1', label: 'Errands' }];
      if (name === 'workspace_tasks') return calls.rows || [];
      if (name === 'archived') return calls.archived || [];
      if (name === 'archive_stats') return [{ done_month: 14, tracked_month: 560, label: '14 done this month · 9h 20m tracked' }];
      if (name === 'archive_settings') return [{ auto: '7d' }];
      if (name === 'task') return calls.task ? [calls.task] : [];
      return [];
    },
    action: async (op, args) => { calls.actions.push([op, args]); return {}; },
    bulk: async () => ({}),
    listViews: async () => [{ id: 'v1', title: 'Focus', icon: 'list' }],
    ...extra,
  };
}

async function mountPage(calls, extra) {
  const ui = rendererHarness();
  const view = ui.renderer.mount(ui.root, structuredClone(template), bridge(calls, extra));
  await view.ready;
  await turn();
  return { ui, view };
}
const navButton = (root, label) => byClass(root, 'pp-nav-item').find(node => text(byClass(node, 'pp-nav-label')[0]) === label);
const lastTasksQuery = calls => calls.queries.filter(([name]) => name === 'workspace_tasks').at(-1)?.[1];

test('the To-do page replaces tabs, the collection select and help paragraphs with a view sidebar', () => {
  const nodes = walk(template.body);
  assert.equal(nodes.some(node => node.type === 'tabs'), false);
  assert.equal(nodes.some(node => node.label === 'Collection'), false);
  for (const phrase of ['Capture anything in Inbox', 'Your chosen work, including', 'When you finish a task']) {
    assert.equal(nodes.some(node => node.type === 'text' && String(node.text).includes(phrase)), false, phrase);
  }
  // The running-timer entry is its own nav group (no rows, no entry, while no timer runs).
  const timer = nodes.find(node => node.type === 'nav' && node.query === 'running_timer');
  assert.equal(timer.item.icon, 'clock');
  assert.deepEqual(timer.item.action.set, { 'state.open': '{{row.task_id}}' });
  const navs = nodes.filter(node => node.type === 'nav' && node !== timer);
  const [smart, lists, saved, more] = navs;
  assert.deepEqual(smart.items.map(item => item.label), ['My Day', 'Inbox', 'Due', 'Upcoming', 'Anytime', 'All open']);
  assert.deepEqual(smart.items.map(item => item.action.set['state.filter']), ['my_day', 'inbox', 'today', 'upcoming', 'ready', 'all']);
  assert.equal(smart.query, 'nav_counts');
  assert.equal(lists.query, 'nav_lists');
  assert.equal(lists.add.action.set['state.new_list'], 'yes');
  assert.deepEqual(lists.item.menu.map(item => item.action.set['state.edit_list']), ['{{row.id}}', '{{row.id}}']);
  assert.equal(saved.source, 'saved-views');
  assert.equal(saved.add.action.set['state.section'], 'save_view');
  assert.deepEqual(more.items.map(item => item.label), ['Done', 'Task notes', 'Archive', 'Trash']);
  const toolbars = nodes.filter(node => node.type === 'view-toolbar');
  assert.equal(toolbars.length, 1);
  assert.deepEqual(toolbars[0].layout.options.map(option => option.value), ['list', 'kanban', 'matrix']);
  assert.deepEqual(toolbars[0].filters.map(filter => filter.bind), ['state.groups', 'state.statuses', 'state.tags', 'state.tags_mode', 'state.field_filter']);
  const capture = nodes.find(node => node.type === 'form' && node.submit?.op === 'create' && node.style === 'capture');
  assert.ok(capture, 'capture is one input with its destination inside');
  for (const list of nodes.filter(node => node.type === 'list' && node.query === 'workspace_tasks')) {
    assert.equal(list.select.bind, 'state.selecting');
    assert.equal(list.item.children[0].layout, 'line');
  }
});

test('sidebar shows counts, marks the current view and switches filters in one refresh', async () => {
  const calls = { queries: [], actions: [] };
  const { ui, view } = await mountPage(calls);
  const myDay = navButton(ui.root, 'My Day');
  assert.equal(myDay.attrs['aria-current'], 'page');
  assert.equal(text(byClass(myDay, 'pp-nav-count')[0]), '2');
  assert.equal(byClass(navButton(ui.root, 'Inbox'), 'pp-nav-count').length, 0, 'zero counts stay quiet');
  const work = navButton(ui.root, 'Work');
  assert.equal(byClass(work, 'pp-nav-dot')[0].style['--pp-dot'], '#0f766e');
  assert.ok(navButton(ui.root, 'Focus'), 'saved views come from the host view library');

  const before = calls.queries.filter(([name]) => name === 'workspace_tasks').length;
  await navButton(ui.root, 'Inbox').fire('click');
  await turn();
  assert.equal(calls.queries.filter(([name]) => name === 'workspace_tasks').length - before, 1);
  assert.equal(lastTasksQuery(calls).filter, 'inbox');
  assert.equal(navButton(ui.root, 'Inbox').attrs['aria-current'], 'page');
  assert.equal(navButton(ui.root, 'My Day').attrs['aria-current'], undefined);
  assert.equal(text(byClass(ui.root, 'pp-heading-title')[0]), 'Inbox');

  view.setState('state.groups', 'g1');
  await navButton(ui.root, 'Work').fire('click');
  await turn();
  assert.equal(lastTasksQuery(calls).lists, 'l1');
  assert.equal(lastTasksQuery(calls).filter, 'all');
  assert.equal(lastTasksQuery(calls).groups, '', 'choosing a view drops temporary filters');
  assert.equal(view.state.capture_list, 'l1', 'capture goes to the open list');
  view.dispose();
});

test('toolbar filters show removable chips with Temporary, Save view and Reset', async () => {
  const calls = { queries: [], actions: [] };
  const { ui, view } = await mountPage(calls);
  const filterButton = () => byClass(byClass(ui.root, 'pp-toolbar-filter')[0], 'pp-menu-button')[0];
  assert.equal(text(filterButton()), 'Filter');
  assert.equal(byClass(ui.root, 'pp-toolbar-chips')[0].hidden, true);

  view.setStates([['state.groups', 'g1'], ['state.statuses', '@active,@completed']]);
  await turn();
  assert.equal(text(filterButton()), 'Filter · 2');
  const chips = () => byClass(ui.root, 'pp-toolbar-chip').map(text);
  assert.deepEqual(chips(), ['Group: Errands', 'Status: In progress (default)', 'Status: Done (default)']);
  const temp = byClass(ui.root, 'pp-toolbar-temp')[0];
  assert.deepEqual(byClass(temp, 'pp-toolbar-link').map(text), ['Save view', 'Reset']);

  await byClass(ui.root, 'pp-toolbar-chip')[1].children.find(node => hasClass(node, 'pp-toolbar-chip-remove')).fire('click');
  assert.equal(view.state.statuses, '@completed');

  await byClass(temp, 'pp-toolbar-link')[0].fire('click');
  assert.equal(view.state.section, 'save_view');
  assert.equal(view.state.draft_title, 'My Day');
  view.setState('state.section', 'tasks');
  await turn();
  await byClass(byClass(ui.root, 'pp-toolbar-temp')[0], 'pp-toolbar-link')[1].fire('click');
  assert.equal(view.state.groups, '');
  assert.equal(view.state.statuses, '');
  assert.equal(byClass(ui.root, 'pp-toolbar-chips')[0].hidden, true);
  view.dispose();
});

test('sort menu and page menu run through state, and Select tasks starts selection', async () => {
  const calls = { queries: [], actions: [], rows: [{ id: 't1', title: 'Write brief', done: 0, tags: 'work', due_label: 'Today', list_name: 'Work' }] };
  const { ui, view } = await mountPage(calls);
  const menuButton = label => byClass(ui.root, 'pp-menu-button').find(node => node.attrs['aria-label']?.startsWith(label));
  const sort = menuButton('Sort tasks');
  assert.equal(text(sort), 'Sort');
  await sort.fire('click');
  const due = domNodes(sort.parent, node => node.attrs?.role === 'menuitemradio').find(node => text(node) === 'Due date');
  await due.fire('click');
  await turn();
  assert.equal(view.state.sort, 'due');
  assert.equal(lastTasksQuery(calls).sort, 'due');
  assert.equal(text(menuButton('Sort tasks')), 'Sort: Due date');
  assert.equal(menuButton('Sort tasks').parent.dataset.active, 'true');

  const list = byClass(ui.root, 'pp-select-list')[0];
  assert.equal(list.dataset.selecting, 'false');
  const more = menuButton('View actions');
  await more.fire('click');
  const select = domNodes(more.parent, node => node.attrs?.role === 'menuitem').find(node => text(node) === 'Select tasks');
  await select.fire('click');
  await turn();
  assert.equal(view.state.selecting, 'yes');
  assert.equal(list.dataset.selecting, 'true');
  const cancel = domNodes(list, node => node.tag === 'button' && text(node) === 'Cancel')[0];
  await cancel.fire('click');
  assert.equal(view.state.selecting, '');
  view.dispose();
});

test('single-line rows show only chosen fields and keep row actions apart', async () => {
  const ui = rendererHarness();
  const view = ui.renderer.mount(ui.root, { state: { fields: 'due' }, body: {
    type: 'item', layout: 'line', title: 'Write brief', note: '1/2', note_icon: 'steps', fields: '{{state.fields}}',
    chips: [
      { text: 'work, review', format: 'tags', style: 'tag', field: 'tags' },
      { text: 'Today', style: 'time', tone: 'accent', field: 'due' },
      { text: 'Work', style: 'source', field: 'list' },
    ],
    actions: [{ type: 'button', label: 'Plan today', action: { set: { 'state.fields': 'tags' } } }],
  } }, bridge({ queries: [], actions: [] }));
  await view.ready;
  const row = byClass(ui.root, 'pp-item-line')[0];
  assert.ok(row);
  assert.equal(text(byClass(row, 'pp-item-note')[0]), '1/2');
  assert.deepEqual(byClass(row, 'pp-meta').map(node => [node.className, text(node)]), [['pp-meta pp-meta-time pp-tone-accent', 'Today']]);
  assert.deepEqual(byClass(row, 'pp-item-actions')[0].children.map(text), ['Plan today']);
  view.dispose();

  const tags = rendererHarness();
  const tagged = tags.renderer.mount(tags.root, { body: { type: 'item', layout: 'line', title: 'x', fields: 'tags,list',
    chips: [{ text: 'work, review', format: 'tags', style: 'tag', field: 'tags' }, { text: 'Work', style: 'source', field: 'list' }] } }, bridge({ queries: [], actions: [] }));
  await tagged.ready;
  assert.deepEqual(byClass(tags.root, 'pp-meta').map(text), ['#work #review', 'Work']);
  tagged.dispose();
});

test('workspace sidebar collapses through its bound state and keeps a View menu', async () => {
  const ui = rendererHarness();
  const view = ui.renderer.mount(ui.root, { state: { collapsed: false, title: 'Inbox' }, body: {
    type: 'workspace', label: 'To-do views', menu_label: '{{state.title}}', collapsible: true, collapsed: 'state.collapsed',
    navigation: { type: 'text', text: 'nav' }, content: { type: 'text', text: 'content' },
  } }, bridge({ queries: [], actions: [] }));
  await view.ready;
  const node = byClass(ui.root, 'pp-workspace')[0];
  const toggle = byClass(node, 'pp-workspace-toggle')[0];
  assert.equal(text(toggle), 'Inbox');
  assert.equal(node.dataset.collapsed, 'false');
  await byClass(node, 'pp-workspace-collapse')[0].fire('click');
  assert.equal(view.state.collapsed, true);
  assert.equal(node.dataset.collapsed, 'true');
  await toggle.fire('click');
  assert.equal(node.dataset.navOpen, 'true');
  assert.equal(toggle.attrs['aria-expanded'], 'true');
  view.setState('state.title', 'Work');
  assert.equal(text(toggle), 'Work');
  view.dispose();
});

test('floating pin buttons are gone; pins live in menus and the pin bar hides when empty', () => {
  const html = readFileSync(join(__dirname, '../index.html'), 'utf8');
  assert.doesNotMatch(html, /id="app-pin"[^>]*>Pin app</);
  assert.match(html, /<details class="head-menu" id="page-menu">[\s\S]*id="page-pin"[\s\S]*id="page-pins-manage"/);
  assert.match(html, /id="app-menu"/);
  const pagesSource = readFileSync(join(__dirname, '../pages.js'), 'utf8');
  assert.doesNotMatch(pagesSource, /Pin pages & apps/);
  const window = { TerrariumPins: require('../pins.js') };
  vm.runInNewContext(pagesSource, { window });
  const elements = {};
  const fake = () => ({ hidden: false, children: [], replaceChildren(...nodes) { this.children = nodes; }, setAttribute() {} });
  const pages = Object.create(window.TerrariumPages.prototype);
  Object.assign(pages, {
    space: 'a', pinStates: new Map(), pinStorage: { getItem: () => null, setItem() {} }, pages: [], plugins: [],
    o: { icon: () => ({}) }, el: (tag, attrs, ...children) => ({ tag, attrs, children }),
    $: id => (elements[id] ||= fake()),
  });
  pages.navigation();
  pages.pinStates.set('a', window.TerrariumPins.normalize({ pins: [{ kind: 'app', id: 'todo' }] }));
  pages.navigation();
  // Pins live in the left rail only; there is no pinned-tabs bar above page content.
  assert.equal(elements['page-tabs'], undefined);
  assert.doesNotMatch(html, /id="page-tabs"/);
  assert.equal(elements['page-nav'].children.length, 1);
});

test('Archive section lists archived tasks by group, filters them and autosaves Auto-archive (FIX-19)', async () => {
  const calls = { queries: [], actions: [], archived: [
    { id: 'a1', title: 'Ship the brief', done_line: 'Sent to Ana', group_label: 'Today', list_name: 'Work', completed_label: '15 Nov', time_label: '1h / 2h', subtasks_label: '2/2' },
    { id: 'a2', title: 'Pay rent', group_label: 'October 2026', list_name: 'Inbox', completed_label: '1 Oct' },
  ], task: { id: 'a1', title: 'Ship the brief', done: 1, done_label: 'Mark not done', archived: 1, archived_label: 'Archived 15 Nov',
    can_archive: 0, can_start_timer: 0, running: 0, can_have_subtasks: 1, time_summary: '1h of 2h' } };
  const { ui, view } = await mountPage(calls);
  assert.ok(navButton(ui.root, 'Archive'), 'Archive is in the More group');
  await navButton(ui.root, 'Archive').fire('click');
  await turn();
  assert.equal(view.state.section, 'archive');
  assert.equal(view.state.title, 'Archive');
  assert.equal(navButton(ui.root, 'Archive').attrs['aria-current'], 'page');
  const asked = () => calls.queries.filter(([name]) => name === 'archived').at(-1)[1];
  assert.deepEqual(Object.keys(asked()).sort(), ['list_id', 'now_ms', 'q', 'today', 'utc_offset_ms']);
  assert.equal(asked().q, '');
  const shown = text(ui.root);
  for (const phrase of ['14 done this month · 9h 20m tracked', 'Ship the brief', 'Sent to Ana', 'Pay rent', 'Today', 'October 2026', '1h / 2h', '2/2']) assert.ok(shown.includes(phrase), phrase);
  assert.ok(!shown.includes('Nothing archived yet.'));
  view.setState('state.archive_q', 'rent');
  await turn();
  assert.equal(asked().q, 'rent');
  view.setState('state.archive_list', '@inbox');
  await turn();
  assert.equal(asked().list_id, '@inbox');
  // Clicking a row opens the task panel.
  const row = byClass(ui.root, 'pp-item-main').find(node => text(node).includes('Ship the brief'));
  assert.ok(row, 'the row is a button');
  await row.fire('click');
  await turn();
  assert.equal(view.state.open, 'a1');
  // The panel of an archived task: banner with Unarchive and Move to trash; no Start timer, Add subtask or Mark done.
  const panel = text(ui.root);
  for (const phrase of ['Archived 15 Nov', 'Unarchive', 'Move to trash']) assert.ok(panel.includes(phrase), phrase);
  const buttons = domNodes(ui.root, node => node.tag === 'button').map(text);
  for (const hidden of ['Start timer', 'Add subtask', 'Mark not done', 'Mark done']) assert.ok(!buttons.includes(hidden), hidden);
  view.dispose();
});

test('Archive section says so when empty (FIX-19)', async () => {
  const { ui, view } = await mountPage({ queries: [], actions: [] });
  await navButton(ui.root, 'Archive').fire('click');
  await turn();
  assert.ok(text(ui.root).includes('Nothing archived yet.'));
  view.dispose();
});

test('Archive template: actions, panel banner and hidden controls for archived tasks (FIX-19)', () => {
  const nodes = walk(template.body);
  // View actions: "Archive completed" runs clear_done and undoes with unarchive {at}.
  const bulk = nodes.find(n => n.label === 'Archive completed');
  assert.equal(bulk.action.op, 'clear_done');
  assert.equal(bulk.action.confirm, 'Archive all completed tasks? They stay in the Archive and still count in reviews and reports.');
  assert.equal(bulk.action.success, 'Archived {{result.cleared}} tasks');
  assert.deepEqual(bulk.action.undo, { op: 'unarchive', args: { at: '{{result.at}}' }, success: 'Brought {{result.cleared}} tasks back' });
  assert.equal(nodes.some(n => n.label === 'Clear completed'), false);
  // Nav: Archive sits right before Trash with its own state.
  const more = nodes.find(n => n.type === 'nav' && n.title === 'More');
  const labels = more.items.map(i => i.label);
  assert.equal(labels.indexOf('Archive') + 1, labels.indexOf('Trash'));
  const archive = more.items.find(i => i.label === 'Archive');
  assert.equal(archive.icon, 'archive');
  assert.equal(archive.action.set['state.section'], 'archive');
  assert.match(more.items.find(i => i.label === 'Done').tooltip, /after you archive them/);
  // Task actions menu.
  const sheet = nodes.find(n => n.type === 'sheet' && n.bind === 'state.open' && n.table === 'tasks');
  const archiveItem = sheet.menu.items.find(i => i.label === 'Archive');
  assert.equal(archiveItem.if, '{{row.can_archive}}');
  assert.equal(archiveItem.action.op, 'archive');
  assert.equal(archiveItem.action.undo.op, 'unarchive');
  const unarchiveItem = sheet.menu.items.find(i => i.label === 'Unarchive');
  assert.equal(unarchiveItem.if, '{{row.archived}}');
  // The banner: Unarchive (undo: archive) and Move to trash (delete, undo restore).
  // The label grows on one line; the buttons wrap below it on a narrow panel.
  assert.equal(sheet.refresh, true, 'the panel re-queries its row after an op so the banner and menu follow archive state');
  const label = walk(sheet).find(n => n.type === 'text' && n.text === '{{row.archived_label}}');
  const banner = walk(sheet).find(n => n.type === 'stack' && (n.children || []).includes(label));
  assert.equal(banner.if, '{{row.archived}}');
  assert.equal(banner.direction, 'row');
  assert.equal(banner.wrap, true);
  assert.equal(label.grow, 1);
  assert.ok(label.basis >= 100);
  const [, unarchive, trash] = banner.children;
  assert.deepEqual([unarchive.label, unarchive.action.op, unarchive.action.undo.op], ['Unarchive', 'unarchive', 'archive']);
  assert.deepEqual([trash.label, trash.action.op, trash.action.undo.op], ['Move to trash', 'delete', 'restore']);
  // Start timer, Add subtask and Mark done are hidden for archived tasks.
  const start = walk(sheet).find(n => n.label === 'Start timer');
  assert.equal(start.if, '{{row.can_start_timer}}');
  const addSubtask = walk(sheet).find(n => n.type === 'form' && n.submit_label === 'Add subtask');
  assert.equal(addSubtask.if, '{{!row.archived}}');
  const markDone = walk(sheet).find(n => n.type === 'checkbox' && n.action?.op === 'toggle');
  assert.equal(markDone.if, '{{!row.archived}}');
});

test('the panel of a done, not archived task offers Archive and keeps Start timer, Add subtask and the done check (FIX-19)', async () => {
  const calls = { queries: [], actions: [], task: { id: 'd1', title: 'Finished', done: 1, done_label: 'Mark not done', archived: 0, archived_label: null,
    can_archive: 1, can_start_timer: 1, running: 0, can_have_subtasks: 1, time_summary: 'Nothing tracked yet' } };
  const { ui, view } = await mountPage(calls);
  view.setState('state.open', 'd1');
  await turn();
  const buttons = domNodes(ui.root, node => node.tag === 'button').map(text);
  assert.ok(domNodes(ui.root, node => node.tag === 'input' && node.attrs?.['aria-label'] === 'Mark Finished done' || node.tag === 'input' && node.type === 'checkbox').length > 0, 'the header check');
  for (const shown of ['Start timer', 'Add subtask']) assert.ok(buttons.includes(shown), shown);
  const panel = text(ui.root);
  for (const hidden of ['Move to trash', 'Archived ']) assert.ok(!panel.includes(hidden), hidden);
  view.dispose();
});
