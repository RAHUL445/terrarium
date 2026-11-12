use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager, State};
use terrarium_runtime::SHARE_TYPES;

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(tag = "action", rename_all = "lowercase")]
pub enum Link {
    Open {
        plugin: String,
        page: Option<String>,
        item: Option<String>,
        space: Option<String>,
    },
    Share {
        text: String,
        subject: Option<String>,
        #[serde(rename = "type")]
        kind: String,
    },
}

#[derive(Default)]
pub struct Inbox(pub Mutex<InboxState>);

#[derive(Default)]
pub struct InboxState {
    pending: Vec<Link>,
    last: Option<(String, Instant)>,
}

pub fn parse(url: &tauri::Url) -> Result<Link, String> {
    if !url.scheme().eq_ignore_ascii_case("terrarium") {
        return Err("scheme must be terrarium".into());
    }
    let Some(host) = url.host_str() else {
        return Err("host must be open or share".into());
    };
    if host.eq_ignore_ascii_case("open") {
        return parse_open(url);
    }
    if host.eq_ignore_ascii_case("share") {
        return parse_share(url);
    }
    Err("host must be open or share".into())
}

fn parse_open(url: &tauri::Url) -> Result<Link, String> {
    if url.as_str().len() > 2048 {
        return Err("link is too long".into());
    }

    let mut segments = url
        .path_segments()
        .ok_or_else(|| "link path is invalid".to_string())?
        .collect::<Vec<_>>();
    while segments.last().is_some_and(|segment| segment.is_empty()) {
        segments.pop();
    }
    if segments.iter().any(|segment| segment.contains('%')) {
        return Err("path segments must not be encoded".into());
    }
    if !(segments.len() == 1 || segments.len() == 2) {
        return Err("path must be /<plugin> or /<plugin>/<page>".into());
    }

    let plugin = segments[0];
    if !valid_plugin_id(plugin) {
        return Err("plugin id must use a-z, 0-9 and - and start with a letter".into());
    }
    let page = segments.get(1).map(|value| (*value).to_string());
    if page.as_deref().is_some_and(|value| !valid_target_id(value)) {
        return Err("page must use A-Z, a-z, 0-9, _, - or .".into());
    }

    let mut item = None;
    let mut space = None;
    let mut seen_item = false;
    let mut seen_space = false;
    for (key, value) in url.query_pairs() {
        if key == "item" && !seen_item {
            seen_item = true;
            if value.is_empty() {
                continue;
            }
            if !valid_target_id(&value) {
                return Err("item must use A-Z, a-z, 0-9, _, - or .".into());
            }
            item = Some(value.into_owned());
        } else if key == "space" && !seen_space {
            seen_space = true;
            if value.is_empty() {
                continue;
            }
            if !valid_target_id(&value) {
                return Err("space must use A-Z, a-z, 0-9, _, - or .".into());
            }
            space = Some(value.into_owned());
        }
    }

    Ok(Link::Open {
        plugin: plugin.to_string(),
        page,
        item,
        space,
    })
}

fn parse_share(url: &tauri::Url) -> Result<Link, String> {
    if url.as_str().len() > 40_000 {
        return Err("link is too long".into());
    }
    let segments = url
        .path_segments()
        .ok_or_else(|| "link path is invalid".to_string())?
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    if !segments.is_empty() {
        return Err("share path must be empty".into());
    }
    let mut text = None::<String>;
    let mut subject = None::<String>;
    let mut kind = None::<String>;
    for (key, value) in url.query_pairs() {
        if key == "text" && text.is_none() {
            text = Some(value.into_owned());
        } else if key == "subject" && subject.is_none() {
            subject = Some(value.into_owned());
        } else if key == "type" && kind.is_none() {
            kind = Some(value.into_owned());
        }
    }
    let text = text.ok_or_else(|| "share text is required".to_string())?;
    let text = clean_controls(&text).trim().to_string();
    if text.is_empty() {
        return Err("share text is required".into());
    }
    if text.chars().count() > 10_000 {
        return Err("share text is too long".into());
    }
    let subject = subject
        .map(|value| clean_controls(&value).chars().take(200).collect::<String>())
        .filter(|value| !value.is_empty());
    let kind = kind.unwrap_or_else(|| "text/plain".to_string());
    if !SHARE_TYPES.contains(&kind.as_str()) {
        return Err("share type must be text/plain or text/uri-list".into());
    }
    Ok(Link::Share {
        text,
        subject,
        kind,
    })
}

fn clean_controls(input: &str) -> String {
    input
        .chars()
        .filter(|ch| !ch.is_control() || matches!(ch, '\n' | '\t'))
        .collect()
}

fn valid_plugin_id(value: &str) -> bool {
    (1..=63).contains(&value.len())
        && value
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_lowercase())
        && value
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
}

fn valid_target_id(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
}

pub fn push(state: &mut InboxState, url: &str, link: Link, now: Instant) -> bool {
    if let Some((last_url, last_at)) = &state.last {
        if last_url == url && now.duration_since(*last_at) < Duration::from_secs(2) {
            state.last = Some((url.to_string(), now));
            return false;
        }
    }
    state.last = Some((url.to_string(), now));
    state.pending.push(link);
    if state.pending.len() > 20 {
        let drop = state.pending.len() - 20;
        state.pending.drain(0..drop);
    }
    true
}

/// Parses incoming deep links and notifies the UI to drain queued links.
pub fn receive(app: &AppHandle, urls: Vec<tauri::Url>) {
    let now = Instant::now();
    let inbox = app.state::<Inbox>();
    for url in urls {
        match parse(&url) {
            Ok(link) => {
                if let Ok(mut state) = inbox.0.lock() {
                    let _ = push(&mut state, url.as_str(), link, now);
                }
            }
            Err(reason) => eprintln!("ignored link: {reason}"),
        }
    }
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
    let _ = app.emit("deep-link", ());
}

#[tauri::command]
pub fn take_links(inbox: State<'_, Inbox>) -> Vec<Link> {
    let mut state = inbox.0.lock().expect("deep-link inbox mutex poisoned");
    std::mem::take(&mut state.pending)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_plugin_page_item_and_space() {
        let url = tauri::Url::parse("terrarium://open/todo/main?item=0192abc&space=personal")
            .expect("url parses");
        let link = parse(&url).expect("link parses");
        assert_eq!(
            link,
            Link::Open {
                plugin: "todo".into(),
                page: Some("main".into()),
                item: Some("0192abc".into()),
                space: Some("personal".into())
            }
        );
        assert_eq!(
            serde_json::to_value(link).expect("serializes"),
            json!({
                "action": "open",
                "plugin": "todo",
                "page": "main",
                "item": "0192abc",
                "space": "personal"
            })
        );
    }

    #[test]
    fn page_and_query_are_optional() {
        let url = tauri::Url::parse("terrarium://open/notes").expect("url parses");
        assert_eq!(
            parse(&url).expect("link parses"),
            Link::Open {
                plugin: "notes".into(),
                page: None,
                item: None,
                space: None
            }
        );
        let empty_item = tauri::Url::parse("terrarium://open/notes?item=").expect("url parses");
        assert_eq!(
            parse(&empty_item).expect("link parses"),
            Link::Open {
                plugin: "notes".into(),
                page: None,
                item: None,
                space: None
            }
        );
    }

    #[test]
    fn rejects_bad_links() {
        let item65 = format!("terrarium://open/todo/main?item={}", "a".repeat(65));
        let too_long = format!("terrarium://open/todo?item={}", "a".repeat(2026));
        for raw in [
            "https://open/todo",
            "terrarium://share/todo",
            "terrarium://open/",
            "terrarium://open/Todo",
            "terrarium://open/todo/main/extra",
            "terrarium://open/todo/ma%20in",
            "terrarium://open/todo?item=a b",
            &item65,
            &too_long,
        ] {
            let url = tauri::Url::parse(raw).expect("url parses");
            assert!(parse(&url).is_err(), "{raw} should be rejected");
        }
    }

    #[test]
    fn parses_a_share() {
        let url = tauri::Url::parse(
            "terrarium://share?text=Hi%20there%0Ahttps%3A%2F%2Fa.b&type=text%2Fplain",
        )
        .expect("url parses");
        let link = parse(&url).expect("link parses");
        assert_eq!(
            link,
            Link::Share {
                text: "Hi there\nhttps://a.b".into(),
                subject: None,
                kind: "text/plain".into(),
            }
        );
        assert_eq!(
            serde_json::to_value(link).expect("serializes"),
            json!({
                "action": "share",
                "text": "Hi there\nhttps://a.b",
                "subject": null,
                "type": "text/plain"
            })
        );
    }

    #[test]
    fn rejects_bad_shares() {
        let text_10001 = format!("terrarium://share?text={}", "a".repeat(10_001));
        for raw in [
            "terrarium://share?type=text/plain",
            "terrarium://share?text=%20%20%20&type=text/plain",
            "terrarium://share?text=ok&type=image%2Fpng",
            &text_10001,
        ] {
            let url = tauri::Url::parse(raw).expect("url parses");
            assert!(parse(&url).is_err(), "{raw} should be rejected");
        }
    }

    #[test]
    fn share_cuts_a_long_subject_and_strips_controls() {
        let long_subject = "x".repeat(250);
        let url = tauri::Url::parse(&format!(
            "terrarium://share?text=A%00B%0AC&subject={}&type=text%2Fplain",
            long_subject
        ))
        .expect("url parses");
        assert_eq!(
            parse(&url).expect("link parses"),
            Link::Share {
                text: "AB\nC".into(),
                subject: Some("x".repeat(200)),
                kind: "text/plain".into(),
            }
        );
    }

    #[test]
    fn inbox_drops_a_quick_duplicate_and_caps_its_size() {
        let mut state = InboxState::default();
        let start = Instant::now();
        let first = Link::Open {
            plugin: "todo".into(),
            page: Some("main".into()),
            item: None,
            space: None,
        };

        assert!(push(
            &mut state,
            "terrarium://open/todo/main",
            first.clone(),
            start
        ));
        assert!(!push(
            &mut state,
            "terrarium://open/todo/main",
            first.clone(),
            start + Duration::from_millis(1500)
        ));
        assert!(push(
            &mut state,
            "terrarium://open/todo/main",
            first.clone(),
            start + Duration::from_millis(3600)
        ));

        for index in 0..25 {
            let link = Link::Open {
                plugin: "todo".into(),
                page: Some(format!("p{index}")),
                item: None,
                space: None,
            };
            assert!(push(
                &mut state,
                &format!("terrarium://open/todo/p{index}"),
                link,
                start + Duration::from_secs(5 + index as u64)
            ));
        }

        assert_eq!(state.pending.len(), 20);
        match &state.pending[0] {
            Link::Open { page, .. } => assert_eq!(page.as_deref(), Some("p5")),
            Link::Share { .. } => panic!("expected open link"),
        }
    }
}
