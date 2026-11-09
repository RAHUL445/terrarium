# Making a theme

A theme changes the app's colours and, optionally, its font. It is one
JSON file, `<id>.theme.json`, with no code in it, so it installs without a
permission review. Installed themes sync to all your devices like plugins;
which theme and mode (System, Light, Dark) is in use is picked per device,
in Settings → Appearance.

```json
{
  "id": "forest",
  "name": "Forest",
  "version": "1.0.0",
  "description": "Calm greens and pine accents.",
  "author": "You",
  "font": "system",
  "light": { "bg": "#f3f6f1", "surface": "#ffffff", "ink": "#1b2a1f", "accent": "#2f6b3f" },
  "dark":  { "bg": "#0e1511", "surface": "#151f19", "ink": "#e6efe7", "accent": "#7cc58f" }
}
```

- `id`: 1–40 lowercase letters, digits or dashes; not `default`. Installing
  a theme with an id that is already installed updates it.
- `light`, `dark`: at least one. A theme with only one keeps the app in that
  mode while it is in use.
- Colours are hex only (`#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`). Tokens you
  leave out keep the app's defaults.
- `font`: `system`, `serif`, `rounded` or `mono`.
- Anything else is refused, so a theme can't inject CSS or load files.

## Tokens

| Token | Used for |
|---|---|
| `bg` | page background, inputs |
| `surface` | cards, sidebar, sheets |
| `border` | borders and dividers |
| `ink` | text |
| `muted` | secondary text, icons |
| `accent` | links, selection, charts, progress |
| `accent-soft` | selected nav item, soft badges |
| `accent-ink` | text on `accent-soft` |
| `accent-strong` | primary buttons (white text on it) |
| `ok` | done, positive changes |
| `warn-bg`, `warn-ink` | warning notices |
| `danger` | errors, delete, negative changes |

## Checking contrast

```bash
terra theme my.theme.json
```

prints the theme and any colour pairs below WCAG AA (4.5:1 for text, 3:1
for accents and danger), for example `light: ink on bg has contrast 1.9:1`.
The app installs a theme with warnings but says so, and marks its card.

The themes that come with the app are in `ui/themes/` (listed in
`ui/themes/index.json`); Settings → Appearance offers them under More themes.
