//! `space.export`: a space's data as an unencrypted ZIP file.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use serde::Serialize;
use serde_json::{json, Map, Value};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use crate::commands::{fill_context, CommandError};
use crate::csv;
use crate::manifest::{ExportFormat, CONTEXT_PARAMS};
use crate::PluginHost;

pub(crate) const FORMAT: &str = "terrarium-export";
pub(crate) const VERSION: u32 = 1;

pub(crate) struct ExportOptions {
    pub include_trash: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct ExportSummary {
    pub path: String,
    pub plugins: usize,
    pub rows: usize,
    pub bytes: u64,
}

const README: &str = "Terrarium export\n\nThis file holds the data of one space as plain JSON (and Markdown or\nCSV copies where apps offer them). It is not encrypted: anyone who\nhas this file can read everything in it. Keep it somewhere safe and\ndelete it when you no longer need it.\n\ndata/<app>/<table>.json  every item, one JSON object per item\nfiles/<app>/...             Markdown and CSV copies, where apps offer them\nmanifest.json            what was exported and when\n";

fn failed(error: impl std::fmt::Display) -> CommandError {
    CommandError::new("failed", error.to_string())
}

pub(crate) fn export_space(
    host: &PluginHost,
    space: &str,
    path: &Path,
    options: ExportOptions,
) -> Result<ExportSummary, CommandError> {
    let invalid = |message| CommandError::new("invalid", message);
    if !path.is_absolute() {
        return Err(invalid("`path` must be an absolute path"));
    }
    if !path
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("zip"))
    {
        return Err(invalid("`path` must end in .zip"));
    }
    if !path.parent().is_some_and(Path::is_dir) {
        return Err(invalid("the folder for `path` doesn't exist"));
    }
    let mut open = OpenOptions::new();
    open.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        open.mode(0o600);
    }
    let file = open.open(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            invalid("`path` already exists; choose a new name")
        } else {
            failed(e)
        }
    })?;
    let result = (|| {
        let mut zip = ZipWriter::new(file);
        let zip_options =
            SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        let mut plugins = Vec::new();
        let mut total_rows = 0;
        for plugin in host.manifests() {
            let store = host
                .store_in(space, &plugin.id)
                .map_err(CommandError::from)?;
            let mut tables = BTreeMap::new();
            for (table, mut rows) in store.dump().map_err(failed)? {
                if !options.include_trash {
                    rows.retain(|row| row.get("_deleted").is_none_or(Value::is_null));
                    for row in &mut rows {
                        row.remove("_deleted");
                    }
                }
                total_rows += rows.len();
                tables.insert(table.clone(), rows.len());
                zip.start_file(format!("data/{}/{table}.json", plugin.id), zip_options)
                    .map_err(failed)?;
                zip.write_all(&serde_json::to_vec_pretty(&rows).map_err(failed)?)
                    .map_err(failed)?;
            }
            let mut files = 0;
            let mut used_paths = BTreeSet::new();
            for entry in &plugin.export {
                let q = &plugin.queries[&entry.query];
                let context: Vec<_> = q
                    .params
                    .iter()
                    .filter(|p| CONTEXT_PARAMS.contains(&p.as_str()))
                    .cloned()
                    .collect();
                let mut values = Map::new();
                fill_context(host, &context, &mut values);
                let params: Vec<_> = q
                    .params
                    .iter()
                    .map(|p| values.get(p).cloned().unwrap_or(Value::Null))
                    .collect();
                let rows = store
                    .query(&q.sql, &params)
                    .map_err(|e| failed(format!("{}: {e}", plugin.id)))?;
                match entry.format {
                    ExportFormat::Csv => {
                        let mut text = String::new();
                        csv::write_record(&mut text, &entry.columns);
                        for row in rows {
                            let fields: Vec<_> = entry
                                .columns
                                .iter()
                                .map(|c| csv::cell(row.get(c).unwrap_or(&Value::Null)))
                                .collect();
                            csv::write_record(&mut text, &fields);
                        }
                        zip.start_file(format!("files/{}/{}", plugin.id, entry.path), zip_options)
                            .map_err(failed)?;
                        zip.write_all(text.as_bytes()).map_err(failed)?;
                        files += 1;
                    }
                    ExportFormat::Markdown => {
                        for row in rows {
                            let path = markdown_path(&entry.path, &row);
                            let mut unique = path.clone();
                            let mut n = 2;
                            while !used_paths.insert(unique.clone()) {
                                unique = format!("{} ({n}).md", path.strip_suffix(".md").unwrap());
                                n += 1;
                            }
                            let text = csv::cell(
                                row.get(entry.body.as_ref().unwrap())
                                    .unwrap_or(&Value::Null),
                            );
                            zip.start_file(format!("files/{}/{unique}", plugin.id), zip_options)
                                .map_err(failed)?;
                            zip.write_all(text.as_bytes()).map_err(failed)?;
                            files += 1;
                        }
                    }
                }
            }
            plugins.push(json!({
                "id": plugin.id, "name": plugin.name, "version": plugin.version,
                "schema_version": store.schema().version, "tables": tables, "files": files,
            }));
        }
        let name = host
            .spaces()
            .list()
            .map_err(CommandError::from)?
            .into_iter()
            .find(|s| s.id == space)
            .map(|s| s.name)
            .unwrap_or_else(|| space.to_string());
        let manifest = json!({
            "format": FORMAT, "version": VERSION,
            "created_ms": jiff::Timestamp::now().as_millisecond(),
            "space": { "id": space, "name": name },
            "include_trash": options.include_trash, "plugins": plugins,
        });
        zip.start_file("manifest.json", zip_options)
            .map_err(failed)?;
        zip.write_all(&serde_json::to_vec_pretty(&manifest).map_err(failed)?)
            .map_err(failed)?;
        zip.start_file("README.txt", zip_options).map_err(failed)?;
        zip.write_all(README.as_bytes()).map_err(failed)?;
        zip.finish().map_err(failed)?;
        Ok(ExportSummary {
            path: path.to_string_lossy().into_owned(),
            plugins: plugins.len(),
            rows: total_rows,
            bytes: fs::metadata(path).map_err(failed)?.len(),
        })
    })();
    if result.is_err() {
        fs::remove_file(path).map_err(failed)?;
    }
    result
}

/// A row value can supply one path segment, never a folder or traversal.
fn segment(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_control() || "/\\:*?\"<>|".contains(c) {
                '-'
            } else {
                c
            }
        })
        .collect::<String>()
        .trim_matches([' ', '.'])
        .chars()
        .take(80)
        .collect()
}

fn markdown_path(template: &str, row: &Map<String, Value>) -> String {
    let mut rendered = String::new();
    let mut rest = template;
    while let Some((before, after)) = rest.split_once("{{") {
        rendered.push_str(before);
        let Some((name, tail)) = after.split_once("}}") else {
            rendered.push_str("{{");
            rendered.push_str(after);
            rest = "";
            break;
        };
        rendered.push_str(&segment(&csv::cell(row.get(name).unwrap_or(&Value::Null))));
        rest = tail;
    }
    rendered.push_str(rest);
    let mut parts: Vec<_> = rendered
        .split('/')
        .filter(|s| !s.is_empty())
        // Literal dots beside an empty placeholder can form a traversal
        // segment even though neither the template nor the value has one.
        .map(|s| if matches!(s, "." | "..") { "-" } else { s })
        .collect();
    if parts.last().is_none_or(|s| *s == ".md") {
        if parts.last() == Some(&".md") {
            parts.pop();
        }
        parts.push("Untitled.md");
    }
    parts.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_segments_are_safe() {
        assert_eq!(segment(" ..a/b\\c:*?\"<>|\n.. "), "a-b-c--------");
        assert_eq!(segment(&"é".repeat(100)).chars().count(), 80);
        assert_eq!(
            markdown_path("..{{missing}}/{{title}}.md", &Map::new()),
            "-/Untitled.md"
        );
        assert_eq!(
            markdown_path("{{project}}/{{title}}.md", &Map::new()),
            "Untitled.md"
        );
        assert_eq!(
            markdown_path(
                "{{title}}.md",
                &serde_json::from_value(json!({"title": "../a/b"})).unwrap()
            ),
            "-a-b.md"
        );
    }
}
