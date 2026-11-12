//! Small RFC 4180 records shared by readable export and import.

use serde_json::Value;

/// Appends one record followed by CRLF, quoting fields without changing them.
pub(crate) fn write_record(out: &mut String, fields: &[String]) {
    for (i, field) in fields.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        if field.contains([',', '"', '\r', '\n']) || field.starts_with(' ') || field.ends_with(' ')
        {
            out.push('"');
            out.push_str(&field.replace('"', "\"\""));
            out.push('"');
        } else {
            out.push_str(field);
        }
    }
    out.push_str("\r\n");
}

/// Parses records, skipping an optional UTF-8 BOM and a final blank line.
pub(crate) fn parse(text: &str) -> Result<Vec<Vec<String>>, String> {
    let mut chars = text.trim_start_matches('\u{feff}').chars().peekable();
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut started = false;
    let mut last_blank = false;
    let mut line = 1;
    let mut quote_line = 1;
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    quoted = false;
                }
            } else {
                if c == '\n' {
                    line += 1;
                }
                field.push(c);
            }
            continue;
        }
        match c {
            '"' if field.is_empty() => {
                quoted = true;
                started = true;
                quote_line = line;
            }
            ',' => {
                row.push(std::mem::take(&mut field));
                started = true;
            }
            '\r' | '\n' => {
                if c == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                line += 1;
                last_blank = !started && row.is_empty();
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
                started = false;
            }
            _ => {
                field.push(c);
                started = true;
            }
        }
    }
    if quoted {
        return Err(format!("line {quote_line}: a quote is not closed"));
    }
    if started {
        row.push(field);
        rows.push(row);
    } else if last_blank {
        rows.pop();
    }
    Ok(rows)
}

/// A cell's text, preserving strings and using JSON for other non-null values.
pub(crate) fn cell(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        _ => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn writes_and_quotes() {
        let mut out = String::new();
        write_record(
            &mut out,
            &["a", "b,c", "say \"hi\"", "two\nlines", " x"].map(String::from),
        );
        assert_eq!(
            out,
            "a,\"b,c\",\"say \"\"hi\"\"\",\"two\nlines\",\" x\"\r\n"
        );
    }

    #[test]
    fn parses_what_it_writes() {
        let rows = vec![
            vec!["a".into(), "b,c".into(), "say \"hi\"".into()],
            vec!["two\r\nlines".into(), " x ".into(), "".into()],
            vec!["last".into(), "é\ntext".into(), "trailing ".into()],
        ];
        let mut out = String::new();
        for row in &rows {
            write_record(&mut out, row);
        }
        assert_eq!(parse(&out).unwrap(), rows);
        assert_eq!(parse("\"\"\r\n").unwrap(), vec![vec![String::new()]]);
    }

    #[test]
    fn parses_bom_crlf_and_blank_end() {
        assert_eq!(
            parse("\u{feff}a,b\r\n1,2\r\n").unwrap(),
            vec![vec!["a", "b"], vec!["1", "2"]]
        );
        assert_eq!(
            parse("a,b\n1,2\n\n").unwrap(),
            vec![vec!["a", "b"], vec!["1", "2"]]
        );
    }

    #[test]
    fn unclosed_quote_fails() {
        assert_eq!(
            parse("a,\"b\n").unwrap_err(),
            "line 1: a quote is not closed"
        );
    }

    #[test]
    fn cells() {
        for (value, text) in [
            (json!(null), ""),
            (json!(true), "true"),
            (json!(3), "3"),
            (json!("x"), "x"),
        ] {
            assert_eq!(cell(&value), text);
        }
    }
}
