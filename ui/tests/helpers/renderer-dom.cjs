const { readFileSync } = require('node:fs');
const { join } = require('node:path');
const vm = require('node:vm');

// Small DOM substitute for renderer lifecycle/query tests. Browser checks
// cover layout; allocation counts here measure the renderer's own work.
function rendererHarness(environment = {}) {
  let allocations = 0;
  class Node {
    constructor(tag) {
      allocations++;
      this.tag = tag; this.children = []; this.dataset = {}; this.style = {};
      this.attrs = {}; this.listeners = {}; this.value = '';
      this.isConnected = true; this.open = false;
      this.classList = { add() {}, remove() {}, toggle() {} };
    }
    append(...nodes) { for (const node of nodes) {
      if (node.tag === 'fragment') { this.append(...node.children); node.children = []; }
      else { node.parent = this; this.children.push(node); }
    } }
    replaceChildren(...nodes) { this.children = []; this.append(...nodes); }
    setAttribute(name, value) { this.attrs[name] = value; }
    removeAttribute(name) { delete this.attrs[name]; }
    addEventListener(name, listener) { (this.listeners[name] ||= []).push(listener); }
    dispatchEvent(event) { for (const listener of this.listeners[event.type] || []) listener(event); }
    async fire(type, event = {}) { await Promise.all((this.listeners[type] || []).map(listener => listener({ target: this, preventDefault() {}, ...event }))); }
    closest(selector) {
      for (let node = this; node; node = node.parent) {
        if (selector === '.pp-sheet' && node.tag === 'dialog') return node;
      }
      return null;
    }
    showModal() { this.open = true; }
    close() { this.open = false; queueMicrotask(() => this.dispatchEvent({ type: 'close' })); }
    contains(node) { return this === node || this.children.some(child => child.contains(node)); }
    querySelectorAll(selector) {
      const nodes = this.children.flatMap(node => [node, ...node.querySelectorAll(selector)]);
      if (selector === '[data-pp-name]') return nodes.filter(node => node.dataset.ppName);
      if (selector === 'form[data-pp-draft]') return nodes.filter(node => node.tag === 'form' && node.attrs['data-pp-draft']);
      if (selector === 'form') return nodes.filter(node => node.tag === 'form');
      if (selector.includes('.pp-recorder')) return nodes.filter(node => node.className?.includes('pp-recorder') && ['recording', 'saving'].includes(node.dataset.state));
      return [];
    }
    querySelector(selector) { return this.querySelectorAll(selector)[0] || null; }
  }
  const window = environment.window || {};
  vm.runInNewContext(readFileSync(join(__dirname, '../../renderer.js'), 'utf8'), {
    window, structuredClone, setTimeout, clearTimeout, setInterval, clearInterval, matchMedia: () => ({ matches: false }),
    CustomEvent: class { constructor(type, options) { this.type = type; this.detail = options.detail; } },
    document: { createElement: tag => new Node(tag), createElementNS: (ns, tag) => new Node(tag), createDocumentFragment: () => new Node('fragment') },
    Event: class { constructor(type) { this.type = type; } },
    ...environment,
  });
  return { renderer: window.TerrariumRenderer, root: new Node('root'), allocations: () => allocations };
}

function listTemplate() {
  return { state: { q: '' }, body: {
    type: 'list', query: 'entries', params: { q: '{{state.q}}' }, empty: 'No entries',
    item: { type: 'form', submit: { op: 'save' }, children: [
      { type: 'text-field', name: 'title', label: 'Title', value: '{{row.title}}' },
    ] },
  } };
}
module.exports = { rendererHarness, listTemplate };
