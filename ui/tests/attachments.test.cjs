const { test } = require('node:test');
const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { join } = require('node:path');
const vm = require('node:vm');

const rendererSource = readFileSync(join(__dirname, '../renderer.js'), 'utf8');
const BLOB_ID = `b1-0123abcd-${'a'.repeat(64)}`;
const PNG_BASE64 = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9s7f2xgAAAAASUVORK5CYII=';

class TextNode {
  constructor(text) {
    this.tag = '#text';
    this.textContent = String(text);
    this.parent = null;
  }
  querySelectorAll() { return []; }
  contains(node) { return this === node; }
}

class Node {
  constructor(tag) {
    this.tag = tag;
    this.parent = null;
    this.children = [];
    this.dataset = {};
    this.attrs = {};
    this.listeners = {};
    this.style = {};
    this.className = '';
    this.hidden = false;
    this.value = '';
    this.selectionStart = 0;
    this.selectionEnd = 0;
    this.classList = {
      add: (...names) => this.#classes().add(...names),
      remove: (...names) => this.#classes().delete(...names),
      toggle: (name, on) => {
        if (on === undefined) on = !this.#classes().has(name);
        if (on) this.#classes().add(name); else this.#classes().delete(name);
      },
      contains: (name) => this.#classes().has(name),
    };
  }
  #classes() {
    const set = new Set(String(this.className || '').split(/\s+/).filter(Boolean));
    return {
      add: (...names) => { for (const n of names) set.add(n); this.className = [...set].join(' '); },
      delete: (...names) => { for (const n of names) set.delete(n); this.className = [...set].join(' '); },
      has: (name) => set.has(name),
    };
  }
  append(...nodes) {
    for (const entry of nodes) {
      if (entry == null) continue;
      if (entry.tag === 'fragment') {
        this.append(...entry.children);
        entry.children = [];
        continue;
      }
      const node = typeof entry === 'string' ? new TextNode(entry) : entry;
      node.parent = this;
      this.children.push(node);
    }
  }
  prepend(...nodes) {
    const existing = this.children.slice();
    this.children = [];
    this.append(...nodes, ...existing);
  }
  replaceChildren(...nodes) { this.children = []; this.append(...nodes); }
  setAttribute(name, value) { this.attrs[name] = String(value); }
  removeAttribute(name) { delete this.attrs[name]; }
  addEventListener(type, handler) { (this.listeners[type] ||= []).push(handler); }
  dispatchEvent(event) {
    event.target ||= this;
    for (const handler of this.listeners[event.type] || []) handler(event);
  }
  focus() {}
  querySelector(selector) { return this.querySelectorAll(selector)[0] || null; }
  querySelectorAll(selector) {
    const parts = selector.trim().split(/\s+/);
    const all = this.#descendants();
    return all.filter((node) => node.tag !== '#text' && this.#matches(node, parts[parts.length - 1]));
  }
  #descendants() {
    const out = [];
    for (const child of this.children) {
      out.push(child);
      if (child instanceof Node) out.push(...child.#descendants());
    }
    return out;
  }
  #matches(node, selector) {
    if (selector.startsWith('.')) {
      return String(node.className || '').split(/\s+/).includes(selector.slice(1));
    }
    if (selector.startsWith('[') && selector.endsWith(']')) {
      const name = selector.slice(1, -1).replace(/^data-/, '');
      return node.dataset && Object.hasOwn(node.dataset, name.replace(/-([a-z])/g, (_, c) => c.toUpperCase()));
    }
    return node.tag === selector;
  }
  contains(node) {
    if (this === node) return true;
    return this.children.some((child) => child.contains?.(node));
  }
  setRangeText(text, start, end) {
    this.value = this.value.slice(0, start) + text + this.value.slice(end);
    this.selectionStart = this.selectionEnd = start + text.length;
  }
  setSelectionRange(start, end) { this.selectionStart = start; this.selectionEnd = end; }
}

class Event {
  constructor(type) { this.type = type; this.defaultPrevented = false; }
  preventDefault() { this.defaultPrevented = true; }
}
class CustomEvent extends Event {
  constructor(type, options = {}) { super(type); this.detail = options.detail; }
}

function text(node) {
  if (!node) return '';
  if (node.tag === '#text') return node.textContent;
  return (node.textContent || '') + node.children.map(text).join('');
}

function loadRenderer(bridge = {}) {
  const window = {};
  const document = {
    createElement: (tag) => new Node(tag),
    createElementNS: (_ns, tag) => new Node(tag),
    createDocumentFragment: () => new Node('fragment'),
  };
  class FileReaderMock {
    readAsDataURL(file) {
      this.result = `data:${file.type || 'application/octet-stream'};base64,${file._base64 || ''}`;
      queueMicrotask(() => this.onload && this.onload());
    }
  }
  vm.runInNewContext(rendererSource, {
    window,
    document,
    structuredClone,
    setTimeout,
    clearTimeout,
    setInterval,
    clearInterval,
    Event,
    CustomEvent,
    FileReader: FileReaderMock,
    atob: (input) => Buffer.from(input, 'base64').toString('binary'),
    matchMedia: () => ({ matches: false }),
  });
  const root = new Node('root');
  const defaults = {
    today: () => '2026-11-11',
    open() {},
    toast() {},
    changed() {},
    ...bridge,
  };
  return { renderer: window.TerrariumRenderer, root, bridge: defaults };
}

test('image_node_renders_data_url_for_allowed_mime', async () => {
  const { renderer, root, bridge } = loadRenderer({
    blobGet: async () => ({ data_base64: PNG_BASE64 }),
  });
  const value = JSON.stringify({ blob: BLOB_ID, name: 'pic.png', mime: 'image/png', size: 12 });
  const view = renderer.mount(root, { body: { type: 'image', file: value } }, bridge);
  await view.ready;
  await new Promise((resolve) => setTimeout(resolve, 0));
  const image = root.querySelector('.pp-blob-image-img');
  assert.ok(image);
  assert.match(image.src, /^data:image\/png;base64,/);
});

test('svg_and_unknown_mime_are_not_rendered_as_images', async () => {
  const { renderer, root, bridge } = loadRenderer({
    blobGet: async () => ({ data_base64: PNG_BASE64 }),
  });
  const svg = JSON.stringify({ blob: BLOB_ID, name: 'bad.svg', mime: 'image/svg+xml', size: 2 });
  const unknown = JSON.stringify({ blob: BLOB_ID, name: 'bad.bin', mime: 'application/pdf', size: 2 });
  const view = renderer.mount(root, { body: { type: 'stack', children: [{ type: 'image', file: svg }, { type: 'image', file: unknown }] } }, bridge);
  await view.ready;
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(root.querySelectorAll('img').length, 0);
});

test('missing_blob_shows_placeholder_text', async () => {
  const { renderer, root, bridge } = loadRenderer({
    blobGet: async () => ({ missing: true }),
  });
  const value = JSON.stringify({ blob: BLOB_ID, name: 'pic.png', mime: 'image/png', size: 12 });
  const view = renderer.mount(root, { body: { type: 'image', file: value } }, bridge);
  await view.ready;
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.match(text(root), /Not on this device yet/);
});

test('markdown_blob_reference_becomes_image', async () => {
  const { renderer } = loadRenderer();
  const node = renderer.markdown(`![preview](terrarium:blob/${BLOB_ID})`, {
    blobGet: async () => ({ data_base64: PNG_BASE64 }),
    open() {},
  });
  await new Promise((resolve) => setTimeout(resolve, 0));
  const image = node.querySelector('img');
  assert.ok(image);
  assert.match(image.src, /^data:image\/png;base64,/);
});

test('paste_inserts_reference_at_cursor', async () => {
  let called = 0;
  const { renderer, root, bridge } = loadRenderer({
    blobPut: async ({ name, mime }) => {
      called++;
      return JSON.stringify({ blob: BLOB_ID, name, mime, size: 10 });
    },
  });
  const view = renderer.mount(root, { body: { type: 'markdown-editor', name: 'body', value: 'Hello ' } }, bridge);
  await view.ready;
  const area = root.querySelector('textarea');
  area.selectionStart = area.selectionEnd = area.value.length;
  const file = { name: 'pic.png', type: 'image/png', size: 10, _base64: PNG_BASE64 };
  area.dispatchEvent({
    type: 'paste',
    clipboardData: { files: [file] },
    preventDefault() {},
  });
  await new Promise((resolve) => setTimeout(resolve, 0));
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(called, 1);
  assert.match(area.value, new RegExp(`!\\[pic\\.png\\]\\(terrarium:blob/${BLOB_ID}\\)`));
});
