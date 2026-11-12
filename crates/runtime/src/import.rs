//! Host parsing for declared importers; plugin operations own all writes.
use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::commands::{self, Caller, CommandError, InvokeOptions, Outcome};
use crate::{ImportDef, ImportFormat, ParamSpec, ParamType, PluginHost};

pub(crate) const IMPORT_COMMAND: &str = "space.import";
pub(crate) const MAX_ITEMS: usize = 2_000;
pub(crate) const MAX_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportFile {
    pub path: String,
    pub text: String,
}

type Items = Vec<(String, Map<String, Value>)>;

fn value(text: String, spec: Option<&ParamSpec>) -> Value {
    let converted = match spec.map(|s| s.ty) {
        Some(ParamType::Integer) => text.parse::<i64>().ok().map(Value::from),
        Some(ParamType::Number) => text
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map(Value::Number),
        Some(ParamType::Boolean) => match text.to_ascii_lowercase().as_str() {
            "true" | "yes" | "1" | "x" => Some(Value::Bool(true)),
            "false" | "no" | "0" => Some(Value::Bool(false)),
            _ => None,
        },
        _ => None,
    };
    converted.unwrap_or(Value::String(text))
}

/// One labelled call per item, plus the number of blank or ignored inputs.
pub(crate) fn items(
    def: &ImportDef,
    params: Option<&BTreeMap<String, ParamSpec>>,
    files: &[ImportFile],
) -> Result<(Items, usize), CommandError> {
    if files.iter().map(|f| f.text.len()).sum::<usize>() > MAX_BYTES {
        return Err(CommandError::invalid(
            "files are too large to import at once (16 MB at most)",
        ));
    }
    if files.is_empty() {
        return Err(CommandError::invalid("choose a file to import"));
    }
    let mut items = Vec::new();
    let mut skipped = 0;
    let arguments = |cells: Vec<(&String, String)>| {
        cells
            .into_iter()
            .filter(|(_, text)| !text.is_empty())
            .map(|(key, text)| (key.clone(), value(text, params.and_then(|p| p.get(key)))))
            .collect::<Map<_, _>>()
    };
    match def.format {
        ImportFormat::Csv => {
            if files.len() != 1 {
                return Err(CommandError::invalid("choose one CSV file"));
            }
            let rows = crate::csv::parse(&files[0].text).map_err(CommandError::invalid)?;
            let header = rows.first().map(Vec::as_slice).unwrap_or_default();
            let columns = def
                .fields
                .iter()
                .map(|(key, src)| {
                    header
                        .iter()
                        .position(|h| h.trim().eq_ignore_ascii_case(src.trim()))
                        .map(|index| (key, index))
                        .ok_or_else(|| {
                            CommandError::invalid(format!("the file has no column `{src}`"))
                        })
                })
                .collect::<Result<Vec<_>, _>>()?;
            for (index, row) in rows.iter().enumerate().skip(1) {
                let cells = columns
                    .iter()
                    .map(|(key, col)| (*key, row.get(*col).cloned().unwrap_or_default()))
                    .collect::<Vec<_>>();
                if cells.iter().all(|(_, text)| text.trim().is_empty()) {
                    skipped += 1;
                } else {
                    items.push((format!("Row {}", index + 1), arguments(cells)));
                }
            }
        }
        ImportFormat::Markdown => {
            for file in files {
                let path = file.path.replace('\\', "/");
                let path = path.strip_prefix("./").unwrap_or(&path);
                let (folder, filename) = path.rsplit_once('/').unwrap_or(("", path));
                let extension = filename.rsplit_once('.').filter(|(_, ext)| {
                    ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown")
                });
                if filename.starts_with('.') || extension.is_none() {
                    skipped += 1;
                    continue;
                }
                let name = extension.unwrap().0;
                let cells = def
                    .fields
                    .iter()
                    .map(|(key, src)| {
                        let text = match src.as_str() {
                            "$name" => name.to_owned(),
                            "$folder" => folder.to_owned(),
                            "$path" => path.to_owned(),
                            "$text" => file.text.replace("\r\n", "\n"),
                            _ => unreachable!("validated manifest source"),
                        };
                        (key, text)
                    })
                    .collect();
                items.push((path.to_owned(), arguments(cells)));
            }
        }
    }
    if items.len() > MAX_ITEMS {
        return Err(CommandError::invalid(format!(
            "at most 2000 items at a time; this file has {}",
            items.len()
        )));
    }
    Ok((items, skipped))
}

pub(crate) fn invoke(
    host: &mut PluginHost,
    caller: Caller,
    space: &str,
    args: &Map<String, Value>,
    options: InvokeOptions,
) -> Result<Outcome, CommandError> {
    let plugin = commands::text(args, "plugin")?;
    let importer = commands::text(args, "importer")?;
    let files = serde_json::from_value::<Vec<ImportFile>>(
        args.get("files").cloned().unwrap_or(Value::Null),
    )
    .map_err(|_| CommandError::invalid("`files` must be a list of {path, text}"))?;
    let dry_run = options.dry_run || args.get("dry_run") == Some(&Value::Bool(true));
    let manifest = host.manifests().find(|m| m.id == plugin);
    let def = manifest
        .and_then(|m| m.import.iter().find(|i| i.id == importer))
        .ok_or_else(|| {
            CommandError::new("not-found", format!("no importer `{plugin}.{importer}`"))
        })?;
    let op = def.op.clone();
    let (items, skipped) = items(
        def,
        manifest.unwrap().operations[&op].params.as_ref(),
        &files,
    )?;
    let labels = items
        .iter()
        .map(|(label, _)| label.clone())
        .collect::<Vec<_>>();
    let calls = items
        .into_iter()
        .map(|(_, args)| (None, Value::Object(args)))
        .collect();
    let (results, changes) = crate::bulk::run_each(
        host,
        space,
        plugin,
        &op,
        calls,
        caller != Caller::App || dry_run,
        dry_run,
    );
    let done = results.iter().filter(|i| i.ok).count();
    let failed = results.len() - done;
    let failures = results
        .iter()
        .filter_map(|i| {
            i.error
                .as_ref()
                .map(|e| json!({"item": labels[i.index], "code": e.code, "message": e.message}))
        })
        .take(50)
        .collect::<Vec<_>>();
    let mut message = format!("{done} imported");
    if failed > 0 {
        message.push_str(&format!(", {failed} failed"));
    }
    if skipped > 0 {
        message.push_str(&format!(", {skipped} skipped"));
    }
    if dry_run {
        message = format!("Dry run: {message} Nothing was kept.");
    }
    Ok(Outcome {
        result: json!({"plugin": plugin, "importer": importer, "total": results.len(), "done": done, "failed": failed, "skipped": skipped, "message": message, "failures": failures}),
        changes,
        dry_run,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def(format: ImportFormat, fields: &[(&str, &str)]) -> ImportDef {
        ImportDef {
            id: "test".into(),
            label: "Test".into(),
            format,
            op: "create".into(),
            fields: fields
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    fn file(path: &str, text: &str) -> ImportFile {
        ImportFile {
            path: path.into(),
            text: text.into(),
        }
    }

    #[test]
    fn csv_rows_become_items() {
        let def = def(
            ImportFormat::Csv,
            &[("title", "title"), ("due", "due"), ("done", "done")],
        );
        let params = BTreeMap::from([("done".into(), ParamSpec::new(ParamType::Boolean))]);
        let (items, skipped) = items(
            &def,
            Some(&params),
            &[file(
                "tasks.csv",
                "Title,Due, Done \nPay rent,2026-11-20,yes\n,,\nCall Sam,,no",
            )],
        )
        .unwrap();
        assert_eq!(skipped, 1);
        assert_eq!(
            items,
            vec![
                (
                    "Row 2".into(),
                    json!({"title":"Pay rent", "due":"2026-11-20", "done":true})
                        .as_object()
                        .unwrap()
                        .clone()
                ),
                (
                    "Row 4".into(),
                    json!({"title":"Call Sam", "done":false})
                        .as_object()
                        .unwrap()
                        .clone()
                )
            ]
        );
    }

    #[test]
    fn missing_column_is_refused() {
        let error = items(
            &def(ImportFormat::Csv, &[("due", "due")]),
            None,
            &[file("a.csv", "Title\na")],
        )
        .unwrap_err();
        assert_eq!(error.code, "invalid");
        assert_eq!(error.message, "the file has no column `due`");
    }

    #[test]
    fn markdown_files_become_items() {
        let def = def(
            ImportFormat::Markdown,
            &[
                ("title", "$name"),
                ("body", "$text"),
                ("project", "$folder"),
                ("source", "$path"),
            ],
        );
        let (items, skipped) = items(
            &def,
            None,
            &[
                file("./Work\\Plan.MD", "a\r\nb"),
                file("Ideas.markdown", "Idea"),
                file(".hidden.md", "hidden"),
                file("photo.png", "image"),
            ],
        )
        .unwrap();
        assert_eq!(skipped, 2);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].0, "Work/Plan.MD");
        assert_eq!(
            items[0].1,
            json!({"title":"Plan", "body":"a\nb", "project":"Work", "source":"Work/Plan.MD"})
                .as_object()
                .unwrap()
                .clone()
        );
        assert_eq!(items[1].1["title"], "Ideas");
        assert!(!items[1].1.contains_key("project"));
    }

    #[test]
    fn limits() {
        let def = def(ImportFormat::Csv, &[("title", "title")]);
        let error = items(
            &def,
            None,
            &[file("a.csv", &format!("title\n{}", "a\n".repeat(2001)))],
        )
        .unwrap_err();
        assert_eq!(
            error,
            CommandError::invalid("at most 2000 items at a time; this file has 2001")
        );
        let error = items(&def, None, &[file("a.csv", &"x".repeat(17 * 1024 * 1024))]).unwrap_err();
        assert_eq!(
            error,
            CommandError::invalid("files are too large to import at once (16 MB at most)")
        );
        assert_eq!(
            items(&def, None, &[]).unwrap_err().message,
            "choose a file to import"
        );
        assert_eq!(
            items(&def, None, &[file("a", ""), file("b", "")])
                .unwrap_err()
                .message,
            "choose one CSV file"
        );
        assert!(items(&def, None, &[file("a", "title\n\"oops")])
            .unwrap_err()
            .message
            .contains("quote"));
    }

    #[test]
    fn typed_values_preserve_unconvertible_text() {
        for (ty, input, expected) in [
            (ParamType::Integer, "-42", json!(-42)),
            (ParamType::Number, "1.25", json!(1.25)),
            (ParamType::Number, "NaN", json!("NaN")),
            (ParamType::Integer, "bad", json!("bad")),
            (ParamType::Boolean, "YES", json!(true)),
            (ParamType::Boolean, "x", json!(true)),
            (ParamType::Boolean, "0", json!(false)),
            (ParamType::Boolean, "maybe", json!("maybe")),
        ] {
            assert_eq!(value(input.into(), Some(&ParamSpec::new(ty))), expected);
        }
    }
}
