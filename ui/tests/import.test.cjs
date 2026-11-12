const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

const source = fs.readFileSync(path.join(__dirname, '../app.js'), 'utf8');
const start = source.indexOf('// ---- Settings → Spaces → Import ----');
const end = source.indexOf('// ---- Spaces ----', start);
assert.ok(start >= 0 && end > start);
const code = source.slice(start, end);
const csv = { id: 'tasks_csv', label: 'Tasks from a CSV file', format: 'csv' };
const markdown = { id: 'notes_md', label: 'Markdown notes', format: 'markdown' };

function harness({ isPhone = false, imports = [csv], result = { message: '1 imported', failed: 0, failures: [] } } = {}) {
  const elements = new Map(), calls = [], toasts = [];
  let refreshes = 0;
  const get = id => {
    if (!elements.has(id)) elements.set(id, {
      disabled: false, hidden: false, children: [], files: [], listeners: {}, attrs: {},
      replaceChildren(...children) { this.children = children; },
      get selectedOptions() { return this.children.length ? [this.children[this.selectedIndex || 0]] : []; },
      setAttribute(key, value) { this.attrs[key] = value; },
      removeAttribute(key) { delete this.attrs[key]; },
      addEventListener(type, listener) { this.listeners[type] = listener; },
    });
    return elements.get(id);
  };
  const context = vm.createContext({
    $: get,
    el: (tag, attrs, ...children) => ({ tag, attrs, children, value: attrs.value, dataset: { format: attrs['data-format'] } }),
    toast: (text, kind) => toasts.push({ text, kind }),
    isPhone, currentSpace: 'work',
    refreshPluginViews: async () => { refreshes++; },
    async call(command, args) {
      calls.push({ command, args: JSON.parse(JSON.stringify(args)) });
      if (args.id === 'plugin.list') return [{ id: 'todo', name: 'To-do', imports }];
      if (result instanceof Error) throw result;
      return result;
    },
  });
  vm.runInContext(code, context);
  return { get, calls, toasts, context, get refreshes() { return refreshes; },
    load: () => context.loadImporters(), run: dry => context.runImport(dry),
    read: (files, format) => context.readImportFiles({ files }, format) };
}
const file = (name = 'tasks.csv', text = 'abc') => ({ name, size: text.length, text: async () => text });

test('importers fill the picker', async () => {
  const h = harness();
  await h.load();
  const [option] = h.get('import-kind').children;
  assert.equal(option.value, 'todo.tasks_csv');
  assert.equal(option.children[0], 'To-do · Tasks from a CSV file');
  assert.equal(h.get('import-card').hidden, false);
  assert.equal(h.get('import-files').accept, '.csv,text/csv');
});

test('markdown picks a folder and switching to CSV resets it', async () => {
  const h = harness({ imports: [markdown, csv] });
  await h.load();
  assert.equal(h.get('import-files').attrs.webkitdirectory, '');
  assert.equal(h.get('import-files').multiple, true);
  assert.equal(h.get('import-files-label').textContent, 'Folder');
  h.get('import-kind').selectedIndex = 1;
  h.get('import-kind').listeners.change();
  assert.equal(h.get('import-files').attrs.webkitdirectory, undefined);
  assert.equal(h.get('import-files').multiple, false);
  assert.equal(h.get('import-files-label').textContent, 'File');
});

test('paths drop the chosen folder', async () => {
  const h = harness();
  const files = [{ ...file('Plan.md'), webkitRelativePath: 'Notes/Work/Plan.md' },
    { ...file('pic.png'), webkitRelativePath: 'Notes/pic.png' }, file('Loose.markdown')];
  assert.deepEqual(JSON.parse(JSON.stringify(await h.read(files, 'markdown'))), [
    { path: 'Work/Plan.md', text: 'abc' }, { path: 'Loose.markdown', text: 'abc' },
  ]);
});

test('check is a dry run and shows failures', async () => {
  const message = 'Dry run: 1 imported, 1 failed. Nothing was kept.';
  const h = harness({ result: { message, failed: 1, failures: [{ item: 'Row 3', message: 'Priority must be 0 to 3' }] } });
  await h.load();
  h.get('import-files').files = [file()];
  const pending = h.run(true);
  assert.equal(h.get('import-check').disabled, true);
  assert.equal(h.get('import-run').disabled, true);
  await pending;
  assert.deepEqual(h.calls[1], { command: 'command', args: { kind: 'action', id: 'space.import', space: 'work',
    args: { plugin: 'todo', importer: 'tasks_csv', files: [{ path: 'tasks.csv', text: 'abc' }], dry_run: true } } });
  assert.equal(h.get('import-result').textContent, message);
  assert.equal(h.get('import-failures').children[0].children[0], 'Row 3: Priority must be 0 to 3');
  assert.deepEqual(h.toasts, []);
  assert.equal(h.refreshes, 0);
  assert.equal(h.get('import-check').disabled, false);
  assert.equal(h.get('import-run').disabled, false);
});

test('too large is refused before sending', async () => {
  const h = harness();
  await h.load();
  h.get('import-files').files = [{ ...file(), size: 17 * 1024 * 1024 }];
  await h.run(false);
  assert.equal(h.calls.length, 1);
  assert.deepEqual(h.toasts, [{ text: 'Error: These files are too large to import at once (16 MB at most).', kind: 'error' }]);
});

test('empty selection uses the correct prompt', async () => {
  const h = harness();
  await assert.rejects(h.read([], 'csv'), /Choose a file to import/);
  await assert.rejects(h.read([file('pic.png')], 'markdown'), /Choose a folder to import/);
});

test('real imports refresh views and cap visible failures', async () => {
  const h = harness({ result: { message: '1 imported, 12 failed', failed: 12,
    failures: Array.from({ length: 12 }, (_, i) => ({ item: `Row ${i}`, message: 'Invalid' })) } });
  await h.load();
  h.get('import-files').files = [file()];
  await h.get('import-run').listeners.click();
  assert.equal(h.calls[1].args.args.dry_run, false);
  assert.equal(h.refreshes, 1);
  assert.equal(h.get('import-failures').children.length, 10);
  assert.deepEqual(h.toasts, [{ text: '1 imported, 12 failed', kind: 'error' }]);
});

test('successful imports toast and errors re-enable buttons', async () => {
  for (const result of [{ message: '1 imported', failed: 0 }, new Error('Import failed')]) {
    const h = harness({ result });
    await h.load();
    h.get('import-files').files = [file()];
    await h.run(false);
    assert.equal(h.toasts[0].kind, result instanceof Error ? 'error' : 'ok');
    assert.equal(h.get('import-check').disabled, false);
    assert.equal(h.get('import-run').disabled, false);
  }
});

test('phones keep CSV importers and hide only Markdown', async () => {
  const h = harness({ isPhone: true, imports: [markdown, csv] });
  await h.load();
  assert.equal(h.get('import-kind').children.length, 1);
  assert.equal(h.get('import-kind').children[0].value, 'todo.tasks_csv');
  assert.equal(h.get('import-card').hidden, false);
  for (const imports of [[], [markdown]]) {
    const empty = harness({ isPhone: true, imports });
    await empty.load();
    assert.equal(empty.get('import-card').hidden, true);
  }
});
