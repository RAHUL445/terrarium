const test = require('node:test');
const assert = require('node:assert/strict');
const vm = require('node:vm');
const fs = require('node:fs');
const path = require('node:path');
const source = fs.readFileSync(path.join(__dirname, '../app.js'), 'utf8');
const cardSource = source.slice(source.indexOf('function accountCard('), source.indexOf('function renderInstalled()'));
function setup() {
  const requests = [];
  let finish;
  const el = (tag, attrs = {}, ...children) => ({ tag, attrs, children, value: attrs.value || '',
    append(...nodes) { this.children.push(...nodes); },
    replaceChildren(...nodes) { this.children = nodes; } });
  const context = { currentSpace: 'personal', el, window: { async terrariumConfirm() { return true; } },
    call(command, args) {
      requests.push({ command, args });
      if (args.name === 'vault_put_bearer') return new Promise(resolve => { finish = resolve; });
      return Promise.resolve([{ name: 'work', hosts: ['api.example.com'], readable: true, active: false, generation: 'fixture-generation' }]);
    } };
  vm.runInNewContext(cardSource, context);
  const card = context.accountCard('fixture', ['api.example.com']);
  const all = (node, pred) => typeof node !== 'object' ? [] : [...(pred(node) ? [node] : []), ...node.children.flatMap(n => all(n, pred))];
  return { card, requests, all, finish: () => finish([]) };
}
test('account form clears the password before saving completes and never displays it', async () => {
  const s = setup();
  await Promise.resolve();
  const form = s.all(s.card, n => n.tag === 'form')[0];
  const password = s.all(form, n => n.attrs.type === 'password')[0];
  const name = s.all(form, n => n.attrs['aria-label'] === 'Account name')[0];
  const host = s.all(form, n => n.tag === 'select')[0];
  name.value = 'work'; host.value = 'api.example.com'; password.value = 'test-token-1234';
  const pending = form.attrs.onsubmit({ preventDefault() {} });
  assert.equal(password.value, '');
  assert.equal(s.requests.at(-1).args.name, 'vault_put_bearer');
  assert.equal(s.requests.at(-1).args.args.space, 'personal');
  assert.equal(s.all(s.card, n => n.textContent?.includes('test-token-1234')).length, 0);
  s.finish(); await pending;
  assert.equal(password.value, '');
});
test('peer accounts show exact hosts and approve the displayed generation', async () => {
  const s = setup(); await Promise.resolve();
  const button = s.all(s.card, n => n.tag === 'button' && n.children.includes('Use this account on this device'))[0];
  assert.ok(button);
  assert.ok(s.all(s.card, n => n.tag === 'li' && n.children.some(c => typeof c === 'string' && c.includes('api.example.com'))).length);
  await button.attrs.onclick();
  assert.equal(s.requests.at(-1).args.name, 'vault_approve');
  assert.equal(s.requests.at(-1).args.args.generation, 'fixture-generation');
});
test('vault IPC is main-window-only and absent from generic services', () => {
  const tauri = fs.readFileSync(path.join(__dirname, '../../apps/tauri/src-tauri/src/ai.rs'), 'utf8');
  assert.match(tauri, /window\.label\(\) != "main"/);
  const services = fs.readFileSync(path.join(__dirname, '../../crates/runtime/src/services.rs'), 'utf8');
  assert.doesNotMatch(services, /"vault_(?:put_bearer|remove|approve|accounts)"/);
});

test('OAuth form names provider hosts, clears credentials, waits and supports cancellation', async () => {
  const requests = []; let resolveSignIn;
  const el = (tag, attrs = {}, ...children) => ({ tag, attrs, children, value: '', append(...nodes) { this.children.push(...nodes); }, replaceChildren(...nodes) { this.children = nodes; } });
  const context = { currentSpace: 'personal', el, URL, navigator: { userAgent: 'Desktop' }, window: {}, call(command, args) {
    requests.push({ command, args });
    if (command === 'vault_sign_in') return new Promise(resolve => { resolveSignIn = resolve; });
    return Promise.resolve([]);
  } };
  vm.runInNewContext(cardSource, context);
  const provider = { auth_url: 'https://login.example.com/auth', token_url: 'https://tokens.example.com/token', api_hosts: ['api.example.com'], scopes: ['items.read'] };
  const all = (n, pred) => typeof n !== 'object' ? [] : [...(pred(n) ? [n] : []), ...n.children.flatMap(c => all(c, pred))];
  const card = context.accountCard('fixture', ['api.example.com'], { work: provider });
  const form = all(card, n => n.tag === 'form')[1];
  const fields = all(form, n => n.tag === 'input');
  fields[0].value = 'work'; fields[1].value = 'synthetic-client'; fields[2].value = 'synthetic-secret';
  const pending = form.attrs.onsubmit({ preventDefault() {} });
  assert.equal(fields[1].value, ''); assert.equal(fields[2].value, '');
  assert.equal(requests.at(-1).args.request.kind, 'work');
  assert.ok(all(form, n => n.textContent === 'Waiting for the browser…').length);
  assert.ok(all(form, n => n.children.some(c => typeof c === 'string' && c.includes('Token server: tokens.example.com'))).length);
  await all(form, n => n.tag === 'button' && n.children.includes('Cancel'))[0].attrs.onclick();
  assert.equal(requests.at(-1).command, 'vault_cancel');
  resolveSignIn([]); await pending;
  context.navigator.userAgent = 'Android';
  const phone = context.accountCard('fixture', ['api.example.com'], { work: provider });
  assert.equal(all(phone, n => n.tag === 'form').length, 1);
  assert.ok(all(phone, n => n.children.includes('Sign in on your computer, then sync and approve this account on this device.')).length);
});

test('OAuth commands enforce the desktop and main-window boundaries', () => {
  const oauth = fs.readFileSync(path.join(__dirname, '../../apps/tauri/src-tauri/src/vault.rs'), 'utf8');
  assert.match(oauth, /window\.label\(\) != "main"/);
  assert.match(oauth, /!cfg!\(desktop\)/);
  const acl = JSON.parse(fs.readFileSync(path.join(__dirname, '../../apps/tauri/src-tauri/capabilities/default.json'), 'utf8'));
  assert.ok(acl.permissions.includes('allow-vault-sign-in'));
  assert.ok(acl.permissions.includes('allow-vault-cancel'));
});
