// Themes: applies the chosen theme and light/dark mode before the app
// draws, from what this device last used. app.js owns choosing them.
//
// A theme only sets the design tokens app.css declares; its colours were
// checked when it was installed. The sheet is constructed rather than
// inline, which the app's content security policy allows.
(function () {
  const FONTS = {
    system: '-apple-system, BlinkMacSystemFont, "Inter", system-ui, sans-serif',
    serif: '"Iowan Old Style", "Charter", "Source Serif Pro", Georgia, serif',
    rounded: 'ui-rounded, "SF Pro Rounded", "Nunito", "Quicksand", system-ui, sans-serif',
    mono: 'ui-monospace, "JetBrains Mono", SFMono-Regular, Menlo, monospace',
  };
  const MODES = ["system", "light", "dark"];
  const HEX = /^#(?:[0-9a-f]{3,4}|[0-9a-f]{6}|[0-9a-f]{8})$/i;
  const TOKEN = /^[a-z][a-z-]*$/;
  const sheet = typeof CSSStyleSheet === "function" && "adoptedStyleSheets" in document ? new CSSStyleSheet() : null;
  if (sheet) document.adoptedStyleSheets = [...document.adoptedStyleSheets, sheet];

  function read(key) {
    try { return localStorage.getItem(key); } catch (_) { return null; }
  }
  function write(key, value) {
    try {
      if (value == null) localStorage.removeItem(key);
      else localStorage.setItem(key, value);
    } catch (_) {}
  }

  function block(selector, palette) {
    const decls = Object.entries(palette || {})
      .filter(([k, v]) => TOKEN.test(k) && HEX.test(v))
      .map(([k, v]) => `--${k}: ${v};`);
    return decls.length ? `${selector} { ${decls.join(" ")} }` : "";
  }

  // The mode a theme can show: one with colours for one mode only keeps it.
  function modeFor(theme, mode) {
    if (theme && theme.light && !theme.dark) return "light";
    if (theme && theme.dark && !theme.light) return "dark";
    return MODES.includes(mode) ? mode : "system";
  }

  function apply(theme, mode) {
    const root = document.documentElement;
    const effective = modeFor(theme, mode);
    root.dataset.mode = effective;
    root.dataset.theme = theme ? theme.id : "default";
    if (!sheet) return effective;
    const css = theme
      ? [
          theme.font && FONTS[theme.font] ? `:root { --font: ${FONTS[theme.font]}; }` : "",
          block(':root[data-mode="light"]', theme.light),
          `@media (prefers-color-scheme: light) { ${block(':root[data-mode="system"]', theme.light)} }`,
          block(':root[data-mode="dark"]', theme.dark),
          `@media (prefers-color-scheme: dark) { ${block(':root[data-mode="system"]', theme.dark)} }`,
        ].join("\n")
      : "";
    sheet.replaceSync(css);
    return effective;
  }

  // Whether the screen shows dark colours now.
  function isDark() {
    const mode = document.documentElement.dataset.mode;
    return mode === "dark" || (mode === "system" && window.matchMedia("(prefers-color-scheme: dark)").matches);
  }

  let cached = null;
  try { cached = JSON.parse(read("pp-theme-data") || "null"); } catch (_) {}
  apply(cached, read("pp-mode"));
  // Device preference, applied before paint alongside the theme.
  document.documentElement.dataset.iconStyle = read("pp-icon-style") === "color" ? "color" : "mono";

  window.ppTheme = {
    FONTS,
    apply,
    isDark,
    modeFor,
    mode: () => read("pp-mode") || "system",
    current: () => cached,
    iconStyle: () => document.documentElement.dataset.iconStyle,
    useIconStyle(value) {
      const style = value === "color" ? "color" : "mono";
      document.documentElement.dataset.iconStyle = style;
      write("pp-icon-style", style);
    },
    // Remembers and applies `theme` (null for the built-in one) and `mode`.
    use(theme, mode) {
      cached = theme || null;
      write("pp-theme-data", cached ? JSON.stringify(cached) : null);
      if (mode !== undefined) write("pp-mode", mode);
      return apply(cached, read("pp-mode"));
    },
  };
})();
