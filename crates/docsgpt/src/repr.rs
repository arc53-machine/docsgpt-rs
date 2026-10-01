//! Best-effort reading of Python `repr()` output as JSON.
//!
//! DocsGPT reports a tool's result as the Python `repr` of a dict (single
//! quotes, `True`/`False`/`None`), so the files and images a tool produced have
//! to be dug out of that text.

use serde_json::Value;

/// Convert a Python `repr` of a dict or list into JSON.
///
/// Valid JSON is returned as is. Otherwise quotes, `True`/`False`/`None` and
/// the `\x`/`\U` string escapes are translated. Returns `None` when the text
/// still does not parse (tuples, sets, objects and other shapes JSON lacks).
pub fn python_repr_to_json(input: &str) -> Option<Value> {
    if let Ok(v) = serde_json::from_str::<Value>(input) {
        return Some(v);
    }
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len() + 8);
    let mut i = 0;
    let mut in_str: Option<char> = None;
    while i < chars.len() {
        let c = chars[i];
        match in_str {
            Some(q) => {
                if c == '\\' && i + 1 < chars.len() {
                    let n = chars[i + 1];
                    match n {
                        _ if n == q && q == '\'' => out.push('\''),
                        _ if n == q => out.push_str("\\\""),
                        'x' => {
                            let hex: String = chars.iter().skip(i + 2).take(2).collect();
                            out.push_str(&format!("\\u00{hex}"));
                            i += 4;
                            continue;
                        }
                        'U' => {
                            let hex: String = chars.iter().skip(i + 2).take(8).collect();
                            let ch = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32)?;
                            push_json_char(&mut out, ch);
                            i += 10;
                            continue;
                        }
                        _ => {
                            out.push(c);
                            out.push(n);
                        }
                    }
                    i += 2;
                    continue;
                }
                if c == q {
                    out.push('"');
                    in_str = None;
                } else {
                    push_json_char(&mut out, c);
                }
            }
            None => {
                if c == '\'' || c == '"' {
                    in_str = Some(c);
                    out.push('"');
                } else if c.is_alphabetic() {
                    let start = i;
                    while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                        i += 1;
                    }
                    let word: String = chars[start..i].iter().collect();
                    out.push_str(match word.as_str() {
                        "True" => "true",
                        "False" => "false",
                        "None" => "null",
                        other => other,
                    });
                    continue;
                } else {
                    out.push(c);
                }
            }
        }
        i += 1;
    }
    serde_json::from_str(&out).ok()
}

/// Append one literal character from inside a Python string, escaped for JSON.
fn push_json_char(out: &mut String, c: char) {
    match c {
        '"' => out.push_str("\\\""),
        '\\' => out.push_str("\\\\"),
        '\n' => out.push_str("\\n"),
        '\r' => out.push_str("\\r"),
        '\t' => out.push_str("\\t"),
        c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
        c => out.push(c),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_python_repr() {
        let v = python_repr_to_json(
            "{'status': 'ok', 'stdout_tail': '1\\n', 'artifacts': [{'artifact_id': 'a', 'size': 18, 'ok': True, 'x': None}]}",
        )
        .unwrap();
        assert_eq!(v["status"], "ok");
        assert_eq!(v["stdout_tail"], "1\n");
        assert_eq!(v["artifacts"][0]["artifact_id"], "a");
        assert_eq!(v["artifacts"][0]["ok"], true);
        assert!(v["artifacts"][0]["x"].is_null());
    }

    #[test]
    fn handles_quotes_inside() {
        let v = python_repr_to_json(r#"{'msg': "it's fine", 'q': 'say "hi"', 'e': 'it\'s'}"#).unwrap();
        assert_eq!(v["msg"], "it's fine");
        assert_eq!(v["q"], "say \"hi\"");
        assert_eq!(v["e"], "it's");
    }

    #[test]
    fn handles_python_only_escapes_and_unicode() {
        let v = python_repr_to_json(r"{'a': 'tab\there', 'b': 'nul\x00', 'c': 'smile \U0001f600', 'd': 'naïve ✓'}")
            .unwrap();
        assert_eq!(v["a"], "tab\there");
        assert_eq!(v["b"], "nul\u{0}");
        assert_eq!(v["c"], "smile 😀");
        assert_eq!(v["d"], "naïve ✓");
    }

    #[test]
    fn json_passes_through_and_garbage_is_none() {
        assert_eq!(python_repr_to_json(r#"{"a": 1}"#).unwrap()["a"], 1);
        assert!(python_repr_to_json("<object at 0x1>").is_none());
        assert!(python_repr_to_json("(1, 2)").is_none());
    }
}
