//! Themes: installable colour schemes for the app.
//!
//! A theme is one JSON file (`<id>.theme.json`) naming colours for the
//! app's design tokens, for light mode, dark mode or both, and optionally a
//! font family. Values are checked strictly — only hex colours and a font
//! from a fixed list — so a theme cannot inject CSS or load anything.
//! Tokens a theme leaves out keep the app's defaults.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::{Result, RuntimeError};

pub const THEME_EXT: &str = "theme.json";
const MAX_BYTES: usize = 64 * 1024;

/// The design tokens a theme may set, as CSS custom properties without the
/// leading `--`.
pub const THEME_TOKENS: &[&str] = &[
    "bg",
    "surface",
    "border",
    "ink",
    "muted",
    "accent",
    "accent-soft",
    "accent-ink",
    "accent-strong",
    "ok",
    "warn-bg",
    "warn-ink",
    "danger",
];

/// Font families a theme may pick.
pub const THEME_FONTS: &[&str] = &["system", "serif", "rounded", "mono"];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Theme {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub light: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dark: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font: Option<String>,
}

fn bad(message: impl Into<String>) -> RuntimeError {
    RuntimeError::Theme(message.into())
}

fn is_hex_colour(value: &str) -> bool {
    value.strip_prefix('#').is_some_and(|hex| {
        matches!(hex.len(), 3 | 4 | 6 | 8) && hex.chars().all(|c| c.is_ascii_hexdigit())
    })
}

/// `#rgb`, `#rgba`, `#rrggbb` or `#rrggbbaa` as linear-light RGB, ignoring alpha.
fn rgb(value: &str) -> Option<[f64; 3]> {
    let hex = value.strip_prefix('#')?;
    let digits: Vec<u8> = match hex.len() {
        3 | 4 => hex
            .chars()
            .take(3)
            .map(|c| c.to_digit(16).map(|d| (d * 17) as u8))
            .collect::<Option<_>>()?,
        6 | 8 => (0..3)
            .map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok())
            .collect::<Option<_>>()?,
        _ => return None,
    };
    let linear = |c: u8| {
        let c = f64::from(c) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    Some([linear(digits[0]), linear(digits[1]), linear(digits[2])])
}

/// WCAG contrast ratio between two colours.
pub fn contrast(a: &str, b: &str) -> Option<f64> {
    let lum = |c: [f64; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    let (la, lb) = (lum(rgb(a)?), lum(rgb(b)?));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    Some((hi + 0.05) / (lo + 0.05))
}

impl Theme {
    pub fn parse(bytes: &[u8]) -> Result<Theme> {
        if bytes.len() > MAX_BYTES {
            return Err(bad("a theme file must be under 64 KB"));
        }
        let theme: Theme = serde_json::from_slice(bytes).map_err(|e| bad(e.to_string()))?;
        theme.validate()?;
        Ok(theme)
    }

    /// A theme stored by this or another version of the app. Fields and
    /// tokens this version does not know are dropped instead of refused, so
    /// a theme installed on a newer device still shows here.
    pub fn from_stored(text: &str) -> Result<Theme> {
        let mut value: serde_json::Value =
            serde_json::from_str(text).map_err(|e| bad(e.to_string()))?;
        if let Some(obj) = value.as_object_mut() {
            const FIELDS: &[&str] = &[
                "id",
                "name",
                "version",
                "description",
                "author",
                "light",
                "dark",
                "font",
            ];
            obj.retain(|k, _| FIELDS.contains(&k.as_str()));
            if obj
                .get("font")
                .and_then(|f| f.as_str())
                .is_some_and(|f| !THEME_FONTS.contains(&f))
            {
                obj.remove("font");
            }
            for mode in ["light", "dark"] {
                if let Some(palette) = obj.get_mut(mode).and_then(|p| p.as_object_mut()) {
                    palette.retain(|k, v| {
                        THEME_TOKENS.contains(&k.as_str()) && v.as_str().is_some_and(is_hex_colour)
                    });
                }
            }
        }
        let theme: Theme = serde_json::from_value(value).map_err(|e| bad(e.to_string()))?;
        theme.validate()?;
        Ok(theme)
    }

    pub fn validate(&self) -> Result<()> {
        let id_ok = !self.id.is_empty()
            && self.id.len() <= 40
            && self
                .id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            && !self.id.starts_with('-');
        if !id_ok {
            return Err(bad(format!(
                "id `{}` must be 1–40 lowercase letters, digits or dashes",
                self.id
            )));
        }
        if self.id == "default" {
            return Err(bad("the id `default` is taken by the built-in theme"));
        }
        let name = self.name.trim();
        if name.is_empty() || name.chars().count() > 40 {
            return Err(bad("name must be 1–40 characters"));
        }
        if self.version.trim().is_empty() || self.version.len() > 20 {
            return Err(bad("version must be set, e.g. 1.0.0"));
        }
        if self.description.chars().count() > 200 {
            return Err(bad("description must be at most 200 characters"));
        }
        if self.author.as_ref().is_some_and(|a| a.chars().count() > 60) {
            return Err(bad("author must be at most 60 characters"));
        }
        if self.light.is_none() && self.dark.is_none() {
            return Err(bad("set colours for `light`, `dark` or both"));
        }
        for (mode, palette) in [("light", &self.light), ("dark", &self.dark)] {
            for (token, value) in palette.iter().flatten() {
                if !THEME_TOKENS.contains(&token.as_str()) {
                    return Err(bad(format!(
                        "{mode}.{token} is not a theme colour; use one of {}",
                        THEME_TOKENS.join(", ")
                    )));
                }
                if !is_hex_colour(value) {
                    return Err(bad(format!(
                        "{mode}.{token} must be a hex colour like #1f2937, not `{value}`"
                    )));
                }
            }
        }
        if let Some(font) = &self.font {
            if !THEME_FONTS.contains(&font.as_str()) {
                return Err(bad(format!(
                    "font `{font}` must be one of {}",
                    THEME_FONTS.join(", ")
                )));
            }
        }
        Ok(())
    }

    /// Readability problems: pairs of colours the app draws together whose
    /// contrast is below WCAG AA. Tokens the theme leaves out are not checked.
    pub fn warnings(&self) -> Vec<String> {
        const PAIRS: &[(&str, &str, f64)] = &[
            ("ink", "bg", 4.5),
            ("ink", "surface", 4.5),
            ("muted", "surface", 4.5),
            ("muted", "bg", 4.5),
            ("accent", "surface", 3.0),
            ("accent-ink", "accent-soft", 4.5),
            ("warn-ink", "warn-bg", 4.5),
            ("danger", "surface", 3.0),
        ];
        let mut out = Vec::new();
        for (mode, palette) in [("light", &self.light), ("dark", &self.dark)] {
            let Some(palette) = palette else { continue };
            for &(fg, bg, min) in PAIRS {
                let (Some(a), Some(b)) = (palette.get(fg), palette.get(bg)) else {
                    continue;
                };
                if let Some(ratio) = contrast(a, b).filter(|r| *r < min) {
                    out.push(format!(
                        "{mode}: {fg} on {bg} has contrast {ratio:.1}:1, below {min}:1"
                    ));
                }
            }
            if let Some(ratio) = palette
                .get("accent-strong")
                .and_then(|c| contrast(c, "#ffffff"))
                .filter(|r| *r < 4.5)
            {
                out.push(format!(
                    "{mode}: white on accent-strong has contrast {ratio:.1}:1, below 4.5:1"
                ));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme(json: &str) -> Result<Theme> {
        Theme::parse(json.as_bytes())
    }

    #[test]
    fn accepts_a_light_only_theme() {
        let t = theme(r##"{"id":"paper","name":"Paper","version":"1.0.0","light":{"bg":"#fafaf7","ink":"#222"}}"##).unwrap();
        assert_eq!(t.light.unwrap()["bg"], "#fafaf7");
        assert!(t.dark.is_none());
    }

    #[test]
    fn rejects_css_injection_and_unknown_tokens() {
        let base =
            |palette: &str| format!(r#"{{"id":"x","name":"X","version":"1","light":{palette}}}"#);
        assert!(theme(&base(r#"{"bg":"red; background:url(https://e.com)"}"#)).is_err());
        assert!(theme(&base(r#"{"bg":"url(x)"}"#)).is_err());
        assert!(theme(&base(r##"{"font-family":"#fff"}"##)).is_err());
        assert!(theme(&base(r##"{"bg":"#ggg"}"##)).is_err());
        assert!(theme(r#"{"id":"x","name":"X","version":"1","light":{},"css":"body{}"}"#).is_err());
        assert!(theme(r#"{"id":"x","name":"X","version":"1"}"#).is_err());
        assert!(theme(r#"{"id":"Bad Id","name":"X","version":"1","light":{}}"#).is_err());
        assert!(theme(r#"{"id":"default","name":"X","version":"1","light":{}}"#).is_err());
        assert!(
            theme(r#"{"id":"x","name":"X","version":"1","light":{},"font":"Comic Sans"}"#).is_err()
        );
    }

    #[test]
    fn stored_themes_from_newer_apps_drop_what_this_one_does_not_know() {
        let t = Theme::from_stored(r##"{"id":"x","name":"X","version":"2","radius":4,"font":"cursive","light":{"bg":"#fff","glow":"#f0f","ink":"url(x)"}}"##).unwrap();
        assert_eq!(t.light.unwrap().len(), 1);
        assert!(t.font.is_none());
        assert!(Theme::from_stored(r#"{"id":"Bad","name":"X","version":"1","light":{}}"#).is_err());
    }

    #[test]
    fn warns_about_low_contrast() {
        let t = theme(r##"{"id":"x","name":"X","version":"1","dark":{"bg":"#111111","ink":"#333333","accent-strong":"#ffff00"}}"##).unwrap();
        let w = t.warnings();
        assert!(w.iter().any(|w| w.starts_with("dark: ink on bg")), "{w:?}");
        assert!(w.iter().any(|w| w.contains("accent-strong")), "{w:?}");
        assert!((contrast("#000", "#fff").unwrap() - 21.0).abs() < 0.01);
    }
}
