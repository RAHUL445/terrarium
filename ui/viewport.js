// Layout viewport units can include the area behind a mobile keyboard.
// Publish only geometry; no field values or keyboard/input events are recorded.
(function (root) {
  function followViewport(win) {
    const viewport = win.visualViewport;
    if (!viewport) return () => {};
    const update = () => {
      const style = win.document.documentElement.style;
      style.setProperty('--visible-height', `${viewport.height}px`);
      style.setProperty('--visible-top', `${viewport.offsetTop}px`);
    };
    viewport.addEventListener('resize', update);
    viewport.addEventListener('scroll', update);
    update();
    return () => {
      viewport.removeEventListener('resize', update);
      viewport.removeEventListener('scroll', update);
      win.document.documentElement.style.removeProperty('--visible-height');
      win.document.documentElement.style.removeProperty('--visible-top');
    };
  }
  if (typeof module !== 'undefined' && module.exports) module.exports = followViewport;
  else followViewport(root);
})(typeof window !== 'undefined' ? window : globalThis);
