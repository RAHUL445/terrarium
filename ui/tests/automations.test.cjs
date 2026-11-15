const { test } = require('node:test');
const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { join } = require('node:path');
const vm = require('node:vm');
const shell = readFileSync(join(__dirname, '../app.js'), 'utf8');
const block = shell.slice(shell.indexOf('// ---- Automations ---'), shell.indexOf('// ---- Backups ---'));
const elCode = shell.slice(shell.indexOf('function el('), shell.indexOf('function showError('));
const chipCode = shell.slice(shell.indexOf('function statusChip('), shell.indexOf('async function schedCall('));
function harness() {
  class Node {
    constructor(tag, text = '') { this.tag = tag; this.children = []; this.attrs = {}; this.listeners = {}; this.textContent = text; }
    append(child) { this.children.push(child); }
    replaceChildren(...children) { this.children = children; }
    setAttribute(key, value) { this.attrs[key] = value; if (["value", "type"].includes(key)) this[key] = value; if (key === "checked") this.checked = true; }
    addEventListener(key, value) { this.listeners[key] = value; }
    async fire(key, event = {}) { await this.listeners[key]?.({ target: this, ...event }); }
  }
  const nodes = new Map(), requests = [], confirms = [];
  const $ = id => { if (!nodes.has(id)) nodes.set(id, new Node('div')); return nodes.get(id); };
  const state = { rules: [], settings: { primary_device: 'local', paused: false }, armed: false, space: 'personal', waiting: 0, events: [{ plugin: 'todo', event: 'done', description: 'Task done' }], operations: [{ plugin: 'habits', operation: 'check', description: 'Check in' }] };
  const h = { $, state, requests, confirms, answer: true, sync: { device: 'local', devices: [] }, page: { runs: [], next: null } };
  const context = vm.createContext({ Node, document: { createElement: tag => new Node(tag), createTextNode: text => new Node('#text', text) }, $, currentSpace: 'personal', pluginList: { plugins: [{ id: 'todo', name: 'To-do' }, { id: 'habits', name: 'Habits' }] }, STATUS_LABELS: { done: 'Done', stopped: 'Stopped', skipped: 'Skipped', failed: 'Failed' }, when: ms => `time ${ms}`, window: { terrariumConfirm: async (message, options) => { confirms.push({ message, options }); return h.answer; } }, call: async (cmd, args) => {
    requests.push(JSON.parse(JSON.stringify({ cmd, ...args })));
    if (h.respond) { const result = await h.respond(cmd, args); if (result !== undefined) return result; }
    if (cmd === 'sync_status') return h.sync;
    if (args.name === 'rules') return state;
    if (args.name === 'rule_runs') return h.page;
    if (args.name === 'rule_review') return { digest: 'review-digest', consent: { summary: 'When todo emits done, run habits.check', passed_fields: ['day'], condition_fields: ['tags'], conditions: [{ field: 'tags', op: 'has_tag', value: 'gym' }], fixed_args: { habit: 'gym' }, device: 'remote', warnings: ['Only new events will run'] } };
    return { ok: true };
  } });
  vm.runInContext(elCode + chipCode + block, context);
  h.run = code => vm.runInContext(code, context);
  h.render = async () => { await h.run('loadAutomations()'); };
  return h;
}
function text(node) { return node.textContent + node.children.map(text).join(''); }
function all(node) { return [node, ...node.children.flatMap(all)]; }
function button(h, label) { return all(h.$('auto-list')).find(n => n.tag === 'button' && text(n) === label); }
const rule = changes => ({ id: 'r1', name: '<img src=x onerror=evil()>', trigger: { plugin: 'todo', event: 'done' }, action: { plugin: 'habits', operation: 'check', args: {} }, filter: [], approved: true, enabled: true, waiting: 0, ...changes });

test('sentences use labels, mention conditions, and fall back for missing plugins', async () => {
  const h = harness(); h.state.rules = [rule()]; await h.render();
  assert.match(text(h.$('auto-list')), /When To-do: Task done → Habits: Check in/);
  h.state.rules[0].filter = [{ field: 'tags', op: 'has_tag', value: 'private' }]; await h.render();
  assert.match(text(h.$('auto-list')), /\(with conditions\)/);
  assert.doesNotMatch(text(h.$('auto-list')), /private/);
  h.state.events = []; await h.render(); assert.match(text(h.$('auto-list')), /To-do: done/);
});
for (const [name, changes, expected, disabled] of [
  ['normal', {}, 'On', false],
  ['problem', { problem: 'Update the app to use this plugin', approved: false }, 'Update the app to use this plugin', true],
  ['unapproved', { approved: false }, 'Needs your OK on this device', false],
  ['waiting', { waiting: 3 }, '3 events waiting', false],
  ['expired', { last_run: { started_ms: 12, status: 'skipped', error: 'expired' } }, 'Event expired', false],
  ['stopped', { last_run: { started_ms: 12, status: 'stopped', error: 'stopped' } }, 'Too many automations in a row', false],
]) test(`automation row: ${name}`, async () => {
  const h = harness(); h.state.rules = [rule(changes)]; await h.render();
  assert.ok(text(h.$('auto-list')).includes(expected));
  const toggle = all(h.$('auto-list')).find(n => n.tag === 'input');
  assert.equal('disabled' in toggle.attrs, disabled);
  assert.equal(toggle.attrs.role, 'switch');
  assert.equal(all(h.$('auto-list')).some(n => n.tag === 'img'), false);
  assert.equal(!!button(h, 'Review'), changes.approved === false);
});

test('toggle and confirmed delete send space-scoped mutations and reload', async () => {
  const h = harness(); h.state.rules = [rule()]; await h.render();
  const toggle = all(h.$('auto-list')).find(n => n.tag === 'input');
  await toggle.fire('change', { target: { checked: false } });
  assert.deepEqual(h.requests.find(r => r.name === 'rule_set_enabled').args, { id: 'r1', enabled: false, space: 'personal' });
  h.answer = false; await button(h, 'Delete').fire('click');
  assert.equal(h.requests.some(r => r.name === 'rule_remove'), false);
  h.answer = true; await button(h, 'Delete').fire('click');
  assert.deepEqual(h.requests.find(r => r.name === 'rule_remove').args, { id: 'r1', space: 'personal' });
});

test('review shows transfer, condition-only fields, fixed values and remote guidance; approval uses digest', async () => {
  const h = harness(); h.state.rules = [rule({ approved: false })]; await h.render();
  await button(h, 'Review').fire('click');
  const message = h.confirms[0].message;
  for (const expected of ['Passed from To-do to Habits: day', 'Used only for conditions: tags', 'habit = "gym"', 'Review again on the device that runs automations']) assert.ok(message.includes(expected));
  assert.equal(h.requests.find(r => r.name === 'rule_approve').args.digest, 'review-digest');
  h.requests.length = 0; h.answer = false; await button(h, 'Review').fire('click');
  assert.equal(h.requests.some(r => r.name === 'rule_approve'), false);
});

test('stale review rejection is visible and switching space during consent cannot approve', async () => {
  const h = harness(); h.state.rules = [rule({ approved: false })]; await h.render();
  h.respond = async (cmd, args) => { if (args?.name === 'rule_approve') throw new Error('Rule changed; review again'); };
  await button(h, 'Review').fire('click');
  assert.match(h.$('auto-error').textContent, /Rule changed; review again/);
  h.respond = undefined; h.requests.length = 0;
  h.run('window.terrariumConfirm = async () => { currentSpace = "work"; return true; }');
  await button(h, 'Review').fire('click');
  assert.equal(h.requests.some(r => r.name === 'rule_approve'), false);
});

test('unpaired identity, phone and missing-device warnings and explicit local rearm', async () => {
  const h = harness(); await h.render();
  assert.ok(text(h.$('auto-device')).includes('This device'));
  assert.equal(h.$('auto-start').hidden, false);
  h.answer = false; await h.$('auto-start').fire('click');
  assert.equal(h.requests.some(r => r.name === 'rules_start'), false);
  h.answer = true; await h.$('auto-start').fire('click');
  assert.ok(h.requests.some(r => r.name === 'rules_start'));
  h.sync.devices = [{ device: 'remote', name: 'Phone', kind: 'phone' }]; h.state.settings.primary_device = 'remote'; await h.render();
  assert.match(h.$('auto-device-note').textContent, /Waiting for Phone/);
  assert.match(h.$('auto-device-note').textContent, /Phones run automations only while the app is open/);
  assert.equal(h.$('auto-start').hidden, true);
  h.state.settings.primary_device = 'gone'; await h.render(); assert.match(h.$('auto-device-note').textContent, /Choose a device to run/);
  h.state.settings.primary_device = null; await h.render(); assert.match(h.$('auto-device-note').textContent, /Choose a device to start/);
});

test('settings require Start confirmation locally; pause and remote selection persist', async () => {
  const h = harness(); await h.render();
  h.$('auto-device').value = 'remote'; await h.$('auto-device').fire('change');
  assert.ok(h.confirms[0].message.includes('duplicate actions'));
  assert.deepEqual(h.requests.find(r => r.name === 'rules_save_settings').args, { primary_device: 'remote', paused: false, space: 'personal' });
  h.requests.length = 0; h.$('auto-device').value = 'local'; h.answer = false; await h.$('auto-device').fire('change');
  assert.equal(h.requests.some(r => r.name === 'rules_save_settings'), false);
  h.$('auto-paused').checked = true; await h.$('auto-paused').fire('change');
  assert.equal(h.requests.find(r => r.name === 'rules_save_settings').args.paused, true);
});

test('history pages stay bounded, show fixed reasons and never display payload or raw errors', async () => {
  const h = harness(); h.page = { runs: [{ rule_name: '<script>evil</script>', status: 'skipped', error: 'expired', device: 'local', started_ms: 10, payload: 'private' }, { status: 'failed', error: 'secret' }], next: 'cursor' };
  await h.render(); assert.match(text(h.$('auto-runs')), /Event expired/); assert.doesNotMatch(text(h.$('auto-runs')), /secret|private/);
  await h.$('auto-more').fire('click');
  assert.deepEqual(h.requests.filter(r => r.name === 'rule_runs').at(-1).args, { limit: 20, before: 'cursor', space: 'personal' });
  assert.equal(h.$('auto-latest').hidden, false);
  await h.$('auto-latest').fire('click'); assert.equal(h.$('auto-latest').hidden, true);
});

test('late loads cannot replace a different space and service errors stay visible', async () => {
  const h = harness();
  h.respond = async (cmd, args) => { if (args?.name === 'rule_runs') throw new Error('History unavailable'); };
  await h.render(); assert.equal(h.$('auto-error').textContent, 'Error: History unavailable');
  let release; h.respond = (cmd, args) => args?.name === 'rules' ? new Promise(resolve => { release = resolve; }) : undefined;
  const pending = h.render(); h.run('currentSpace = "work"'); release(h.state); await pending;
  assert.equal(h.run('autoState.space'), 'personal');
});

test('navigation, phone settings, refresh and mutation notifications include automations', () => {
  const html = readFileSync(join(__dirname, '../index.html'), 'utf8');
  assert.match(html, /data-settings="automations"/); assert.match(html, /id="settings-automations"/);
  assert.match(html, /id="auto-new">/); assert.match(html, /id="auto-editor" hidden/);
  assert.match(shell, /\["Automations", "automations"\]/);
  assert.match(shell, /if \(section === "automations"\) loadAutomations\(\)/);
  assert.match(shell, /settingsSection === "automations"\) loadAutomations\(\)/);
  const native = readFileSync(join(__dirname, '../../apps/tauri/src-tauri/src/ai.rs'), 'utf8');
  assert.match(native, /name.starts_with\("rule"\)/);
  assert.match(native, /"rules" \| "rule_list" \| "rule_runs" \| "rule_review"/);
});

const plain = value => JSON.parse(JSON.stringify(value));
function editorState(h) {
  h.state.events[0].payload = {
    tags: { type: 'string' }, day: { type: 'date', required: true },
    title: { type: 'string', required: true }, n: { type: 'integer', required: true },
    flag: { type: 'boolean' }, choice: { type: 'enum', values: ['done', 'skip'] },
    data: { type: 'json' },
  };
  h.state.operations[0].params = { habit: { type: 'id', required: true }, day: { type: 'date', required: true }, flag: { type: 'boolean' } };
}
function control(h, label) { return all(h.$('auto-editor')).find(n => n.attrs['aria-label'] === label); }
function editorButton(h, label) { return all(h.$('auto-editor')).find(n => n.tag === 'button' && text(n) === label); }
async function submit(h) { await all(h.$('auto-editor')).find(n => n.tag === 'form').fire('submit', { preventDefault() {} }); }
async function openNew(h) {
  editorState(h); await h.render(); await h.$('auto-new').fire('click');
  const inputs = all(h.$('auto-editor'));
  inputs.find(n => n.attrs.maxlength === '60').value = 'Gym';
  control(h, 'When').value = '["todo","done"]'; await control(h, 'When').fire('change');
  control(h, 'Then').value = '["habits","check"]'; await control(h, 'Then').fire('change');
  control(h, 'habit').value = 'gym'; control(h, 'day').value = '2026-11-11';
}

test('ruleFromForm builds exact fixed and bound JSON with omitted optional arguments', () => {
  const h = harness();
  h.run(`form = {name: 'Gym', trigger: {plugin: 'todo', event: 'done'}, filter: [], action: {plugin: 'habits', operation: 'check'}, enabled: true,
    args: {habit: {value: 'gym'}, day: {value: '2026-11-11'}, flag: {omit: true}}}`);
  assert.deepEqual(plain(h.run('ruleFromForm(form)')), { name: 'Gym', trigger: { plugin: 'todo', event: 'done' }, filter: [], action: { plugin: 'habits', operation: 'check', args: { habit: 'gym', day: '2026-11-11' } }, enabled: true });
  h.run(`form.filter = [{field:'tags', op:'has_tag', value:'gym'}]; form.args.day = {source:'day'}; form.enabled = false`);
  const bound = plain(h.run('ruleFromForm(form)'));
  assert.deepEqual(bound.filter, [{ field: 'tags', op: 'has_tag', value: 'gym' }]);
  assert.equal(bound.action.args.day, '{{event.day}}'); assert.equal(bound.enabled, false);
});

test('binding choices follow host compatibility including requiredness and enums', () => {
  const h = harness();
  for (const [source, target, expected] of [
    [{type:'date',required:true}, {type:'date',required:true}, true],
    [{type:'date'}, {type:'date',required:true}, false],
    [{type:'integer'}, {type:'number'}, true],
    [{type:'string'}, {type:'id'}, false],
    [{type:'id'}, {type:'text'}, true],
    [{type:'boolean'}, {type:'json'}, true],
    [{type:'enum',values:['a']}, {type:'enum',values:['a','b']}, true],
    [{type:'enum',values:['c']}, {type:'enum',values:['a','b']}, false],
  ]) assert.equal(h.run(`autoCompatible(${JSON.stringify(source)}, ${JSON.stringify(target)})`), expected);
});

test('consent lists mapped fields separately, uses plugin names, and never claims title transfer', async () => {
  const h = harness(); await h.render();
  const message = h.run(`consentText(${JSON.stringify(rule())}, autoState, {passed_fields:['day','day'], condition_fields:['tags','day'], conditions:[], fixed_args:{habit:'gym'}, device:'local'})`);
  assert.match(message, /When To-do: Task done, Habits will Check in/);
  assert.match(message, /Passed from To-do to Habits: day\n/);
  assert.match(message, /Used only for conditions: tags\n/);
  assert.doesNotMatch(message, /title/);
});

test('new form saves before consent, Not now leaves saved unapproved, Allow uses reviewed digest', async () => {
  for (const allow of [false, true]) {
    const h = harness(); await openNew(h); h.answer = allow;
    h.respond = async (cmd, args) => args?.name === 'rule_create' ? rule({ ...args.args.rule, approved: false }) : undefined;
    await submit(h);
    const saved = h.requests.find(r => r.name === 'rule_create');
    assert.equal(saved.args.rule.action.args.habit, 'gym');
    assert.equal(saved.args.rule.action.args.day, '2026-11-11');
    assert.equal(Object.hasOwn(saved.args.rule.action.args, 'flag'), false);
    assert.equal(h.confirms[0].options.title, 'Allow this automation?');
    assert.equal(h.confirms[0].options.confirm, 'Allow');
    assert.equal(h.requests.some(r => r.name === 'rule_approve'), allow);
    if (allow) assert.equal(h.requests.find(r => r.name === 'rule_approve').args.digest, 'review-digest');
    assert.equal(h.$('auto-editor').hidden, true);
  }
});

test('edit pre-fills bindings and conditions, preserves disabled, and rename skips consent', async () => {
  const h = harness(); editorState(h);
  h.state.rules = [rule({ name:'Gym', enabled: false, filter:[{field:'tags',op:'has_tag',value:'gym'}], action:{plugin:'habits',operation:'check',args:{habit:'gym',day:'{{event.day}}'}} })];
  await h.render(); await button(h, 'Edit').fire('click');
  assert.equal(control(h, 'Condition value').value, 'gym');
  assert.equal(control(h, 'day source').value, 'day');
  assert.deepEqual(control(h, 'day source').children.map(n => n.attrs.value), ['', 'day']);
  all(h.$('auto-editor')).find(n => n.attrs.maxlength === '60').value = 'New name';
  h.respond = async (cmd,args) => args?.name === 'rule_update' ? rule({ ...args.args.rule, approved: true }) : undefined;
  await submit(h);
  assert.equal(h.requests.find(r => r.name === 'rule_update').args.rule.enabled, false);
  assert.equal(h.confirms.length, 0);
});

test('changed rules require consent and validation failures retain editor and show error', async () => {
  const h = harness(); await openNew(h);
  h.respond = async (cmd,args) => { if (args?.name === 'rule_create') throw new Error('Unknown action argument'); };
  await submit(h); assert.match(h.$('auto-error').textContent, /Unknown action argument/);
  assert.equal(h.$('auto-editor').hidden, false); assert.equal(h.confirms.length, 0);
  h.respond = undefined;
  h.run('currentSpace = "work"'); await submit(h);
  assert.equal(h.requests.filter(r => r.name === 'rule_create').length, 1);
  await h.render(); assert.equal(h.$('auto-editor').hidden, true);
});

test('all condition operators serialize typed values and conditions cap at ten with removal', async () => {
  const h = harness(); await openNew(h); await editorButton(h, 'Add condition').fire('click');
  for (const [field, op, value, expected] of [
    ['tags','eq','gym','gym'], ['tags','ne','gym','gym'], ['tags','contains','gym','gym'],
    ['tags','has_tag','gym','gym'], ['n','gt','3',3], ['day','lt','2026-11-12','2026-11-12'],
    ['flag','eq',true,true], ['choice','eq','done','done'], ['data','eq','{"a":1}',{a:1}], ['tags','exists',true,true],
  ]) {
    control(h,'Condition field').value = field; await control(h,'Condition field').fire('change');
    control(h,'Condition comparison').value = op; await control(h,'Condition comparison').fire('change');
    const input = control(h,'Condition value'); if (typeof value === 'boolean') input.checked = value; else input.value = value;
    h.respond = async (cmd,args) => {
      if (args?.name === 'rule_create') throw new Error('Keep editor open');
    };
    await submit(h);
    assert.deepEqual(h.requests.filter(r => r.name === 'rule_create').at(-1).args.rule.filter, [{field,op,value:expected}]);
  }
  for (let i=0;i<15;i++) await editorButton(h,'Add condition').fire('click');
  assert.equal(all(h.$('auto-editor')).filter(n=>n.attrs['aria-label']==='Condition field').length,10);
  assert.equal(editorButton(h,'Add condition').disabled,true);
  await editorButton(h,'Remove').fire('click');
  assert.equal(editorButton(h,'Add condition').disabled,false);
});

test('editing an action asks for consent again and submits exact event binding', async () => {
  const h = harness(); editorState(h);
  h.state.rules = [rule({ action:{plugin:'habits',operation:'check',args:{habit:'gym',day:'2026-11-11'}} })];
  await h.render(); await button(h,'Edit').fire('click');
  control(h,'day source').value = 'day'; await control(h,'day source').fire('change');
  h.answer = false;
  h.respond = async (cmd,args) => args?.name === 'rule_update' ? rule({...args.args.rule, approved:false}) : undefined;
  await submit(h);
  assert.equal(h.requests.find(r=>r.name==='rule_update').args.rule.action.args.day,'{{event.day}}');
  assert.equal(h.requests.filter(r=>r.name==='rule_review').length,1);
  assert.equal(h.requests.some(r=>r.name==='rule_approve'),false);
});

test('stale editor responses cannot open consent in another space or close a new draft', async () => {
  const h = harness(); await openNew(h);
  let release;
  h.respond = (cmd,args) => args?.name === 'rule_create' ? new Promise(resolve=>{release=resolve;}) : undefined;
  const pending = submit(h);
  await h.$('auto-new').fire('click');
  release(rule({approved:false})); await pending;
  assert.equal(h.$('auto-editor').hidden,false);
  assert.equal(h.confirms.length,0);
});

test('review refuses a token for a trigger or action different from the displayed row', async () => {
  const h = harness(); h.state.rules = [rule({approved:false})]; await h.render();
  h.respond = async (cmd,args) => args?.name === 'rule_review' ? {digest:'new-target',consent:{summary:'When todo emits done, run notes.create'}} : undefined;
  await button(h,'Review').fire('click');
  assert.match(h.$('auto-error').textContent,/Automation changed/);
  assert.equal(h.confirms.length,0);
  assert.equal(h.requests.some(r=>r.name==='rule_approve'),false);
});

test('Gym example offers titled habit choices, requires a selection and saves a fixed ID', async () => {
  const h = harness();
  const packages = process.env.TERRA_PLUGIN_SOURCE || join(__dirname, '../../../terrarium-plugins/packages');
  const manifest = JSON.parse(readFileSync(join(packages, 'habits/manifest.json'), 'utf8'));
  const services = readFileSync(join(__dirname, '../../crates/runtime/src/services.rs'), 'utf8');
  const example = JSON.parse(services.match(/json!\(\[({\s*"name": "Gym task done[^]*?})\]\)/)[1]);
  h.state.events = [{plugin:'tasks',event:'task_done',description:'A task was marked done.',payload:{tags:{type:'string'},completed_date:{type:'date',required:true}}}];
  h.run("pluginList.plugins.push({ id: 'tasks', name: 'Task manager' })");
  h.state.operations = [{plugin:'habits',operation:'checkin',...manifest.operations.checkin}];
  h.state.examples = [example];
  h.respond = async cmd => cmd === 'run_query' ? [{id:'habit-gym',title:'Gym'}] : undefined;
  await h.render(); await h.$('auto-new').fire('click');
  const examples = control(h, 'Start from an example');
  assert.ok(examples.children.some(n => text(n) === example.name));
  examples.value='0'; await examples.fire('change');
  assert.ok(control(h, 'Then').children.some(n => text(n) === 'Habits: Record a check-in'));
  assert.equal(control(h,'id').tag,'select');
  await submit(h); assert.match(h.$('auto-error').textContent,/Choose an available habit/);
  assert.equal(h.requests.some(r=>r.name==='rule_create'),false);
  const habit = control(h,'id'); assert.ok(habit.children.some(n=>text(n)==='Gym' && n.value==='habit-gym'));
  habit.value='habit-gym'; await submit(h);
  const saved=h.requests.find(r=>r.name==='rule_create').args.rule;
  assert.equal(saved.action.args.id,'habit-gym');
  assert.equal(saved.action.args.status,'done');
  assert.equal(saved.action.args.day,'{{event.completed_date}}');
  assert.equal(saved.filter[0].value,'gym');
  const summary=h.run(`ruleSentence(${JSON.stringify(saved)}, autoState)`);
  assert.match(summary,/When Task manager: A task is marked done \(with conditions\) → Habits: Record a check-in for Gym/);
  assert.doesNotMatch(summary,/default today|daily target/);
  assert.ok(h.requests.some(r=>r.cmd==='run_query' && r.plugin==='habits' && r.name==='habits' && r.space==='personal'));
});

test('action labels are curated and required IDs need a picker or matching app event', () => {
  const h = harness();
  assert.equal(h.run('autoActionLabel({plugin:"todo",operation:"create"})'), 'Create a task');
  assert.equal(h.run('autoActionLabel({plugin:"tasks",operation:"create"})'), 'Create a task');
  assert.equal(h.run('autoActionAvailable({plugin:"voice",operation:"capture_audio"}, {})'), false);
  assert.equal(h.run('autoActionAvailable({plugin:"reports",operation:"on_draft"}, {})'), false);
  const operation = {plugin:'habits', operation:'checkin', params:{id:{type:'id',required:true}}};
  const event = {plugin:'habits',payload:{id:{type:'id',required:true}}};
  assert.equal(h.run(`autoActionAvailable(${JSON.stringify(operation)}, ${JSON.stringify(event)})`), true);
  event.plugin='todo';
  assert.equal(h.run(`autoActionAvailable(${JSON.stringify(operation)}, ${JSON.stringify(event)})`), true);
  operation.params={other_id:{type:'id',required:true}};
  assert.equal(h.run(`autoActionAvailable(${JSON.stringify(operation)}, ${JSON.stringify(event)})`), false);
});

test('example chooser is absent without available examples', async () => {
  const h = harness(); await openNew(h);
  assert.equal(control(h, 'Start from an example'), undefined);
});


test('consent renders conditions and fixed values as lines with editor labels', () => {
  const h = harness();
  h.state.events[0].description = 'Task done.';
  h.state.operations[0].description = 'Check in.';
  const message = h.run(`consentText(${JSON.stringify(rule())}, ${JSON.stringify(h.state)}, {
    conditions: [{field: 'tags', op: 'contains', value: 'gym'}, {field: 'status', op: 'ne', value: 'done'}, {field: 'day', op: 'exists', value: false}],
    fixed_args: {status: 'done', count: 2}, device: 'local'
  })`);
  assert.match(message, /^When To-do: Task done, Habits will Check in\./);
  assert.doesNotMatch(message, /\.\./);
  assert.ok(message.includes('Conditions: tags contains "gym"\nstatus is not "done"\nday is not set'));
  assert.ok(message.includes('Fixed values: status = "done"\ncount = 2'));
  const empty = h.run(`consentText(${JSON.stringify(rule())}, {}, {})`);
  assert.ok(empty.includes('Conditions: None'));
  assert.ok(empty.includes('Fixed values: None'));
});
