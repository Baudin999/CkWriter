//! Shared JSON-extraction helpers for LLM responses.
//!
//! Local models often wrap JSON in code fences or trailing commentary even when
//! told not to. We tolerate that by locating the first balanced `{...}` block
//! and parsing that.

use serde::Deserialize;

pub fn parse_json_object<T: for<'de> Deserialize<'de>>(buf: &str, label: &str) -> Option<T> {
    let extracted = extract_json_object(buf);
    let sanitized = sanitize_json_strings(extracted);
    match serde_json::from_str::<T>(&sanitized) {
        Ok(v) => Some(v),
        Err(e) => {
            log::warn!(
                "{label} parse failed at line {} col {}: {e}; raw response ({} bytes): {}; sanitized preview: {}",
                e.line(),
                e.column(),
                buf.len(),
                preview(buf, 800),
                preview(&sanitized, 800),
            );
            None
        }
    }
}

/// Recover the elements of a `"<key>": [ ... ]` array when the surrounding
/// document is malformed. Local models occasionally hallucinate stray
/// non-JSON tokens between array elements (e.g. a bare `im,` between two
/// objects); strict parsing then drops the whole batch. We walk the array
/// element-by-element, parse each balanced `{...}` block independently, and
/// silently skip the ones that don't deserialize. Use only as a fallback
/// after `parse_json_object` returns `None` — partial output is better than
/// none, but unconditional salvage would mask real schema regressions.
pub fn salvage_array<T>(buf: &str, key: &str) -> Vec<T>
where
    T: for<'de> serde::de::DeserializeOwned,
{
    let extracted = extract_json_object(buf);
    let sanitized = sanitize_json_strings(extracted);
    let bytes = sanitized.as_bytes();

    let Some(start) = find_array_start(&sanitized, key) else {
        return Vec::new();
    };

    let mut out: Vec<T> = Vec::new();
    let mut i = start;
    let mut in_str = false;
    let mut escape = false;
    while i < bytes.len() {
        let b = bytes[i];
        if escape {
            escape = false;
            i += 1;
            continue;
        }
        if in_str {
            match b {
                b'\\' => escape = true,
                b'"' => in_str = false,
                _ => {}
            }
            i += 1;
            continue;
        }
        match b {
            b'"' => {
                in_str = true;
                i += 1;
            }
            b']' => break,
            b'{' => {
                let Some(end) = find_balanced_close(bytes, i) else {
                    break;
                };
                let elem = &sanitized[i..=end];
                if let Ok(v) = serde_json::from_str::<T>(elem) {
                    out.push(v);
                }
                i = end + 1;
            }
            _ => i += 1,
        }
    }
    out
}

/// Locate `"<key>"` followed (after whitespace) by `:` then `[`, and return
/// the byte index just past the `[`. The `:` check filters out incidental
/// occurrences of the key as a value substring.
fn find_array_start(s: &str, key: &str) -> Option<usize> {
    let needle = format!("\"{key}\"");
    let bytes = s.as_bytes();
    let mut search_from = 0usize;
    loop {
        let rel = s[search_from..].find(&needle)?;
        let after_key = search_from + rel + needle.len();
        let mut k = after_key;
        while k < bytes.len() && bytes[k].is_ascii_whitespace() {
            k += 1;
        }
        if k < bytes.len() && bytes[k] == b':' {
            k += 1;
            while k < bytes.len() && bytes[k].is_ascii_whitespace() {
                k += 1;
            }
            if k < bytes.len() && bytes[k] == b'[' {
                return Some(k + 1);
            }
        }
        search_from = after_key;
    }
}

/// Given an opening `{` at `start`, return the byte index of its matching
/// `}`, tracking string state so braces inside strings don't count. Returns
/// `None` if the object is unbalanced.
fn find_balanced_close(bytes: &[u8], start: usize) -> Option<usize> {
    debug_assert!(bytes.get(start) == Some(&b'{'));
    let mut depth = 1i32;
    let mut i = start + 1;
    let mut in_str = false;
    let mut escape = false;
    while i < bytes.len() {
        let b = bytes[i];
        if escape {
            escape = false;
            i += 1;
            continue;
        }
        if in_str {
            match b {
                b'\\' => escape = true,
                b'"' => in_str = false,
                _ => {}
            }
        } else {
            match b {
                b'"' => in_str = true,
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    None
}

/// Extract the first balanced `{...}` block. Tolerates code fences,
/// leading/trailing commentary, and stray whitespace. Falls back to the
/// trimmed input if no `{` is found.
pub fn extract_json_object(s: &str) -> &str {
    let bytes = s.as_bytes();
    let Some(start) = bytes.iter().position(|&b| b == b'{') else {
        return s.trim();
    };
    let mut depth = 0i32;
    let mut in_str = false;
    let mut escape = false;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        if escape {
            escape = false;
            continue;
        }
        match b {
            b'\\' if in_str => escape = true,
            b'"' => in_str = !in_str,
            b'{' if !in_str => depth += 1,
            b'}' if !in_str => {
                depth -= 1;
                if depth == 0 {
                    return &s[start..=i];
                }
            }
            _ => {}
        }
    }
    &s[start..]
}

fn preview(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.replace('\n', "\\n")
    } else {
        let cap = floor_char_boundary(s, max);
        format!(
            "{}…[+{} bytes]",
            s[..cap].replace('\n', "\\n"),
            s.len() - cap
        )
    }
}

fn floor_char_boundary(s: &str, mut i: usize) -> usize {
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Escape raw control characters that appear inside JSON string contexts.
///
/// Gemma (and many other local models) sometimes embed a literal newline or
/// tab inside a string value — e.g. `"evidence": "line1\nline2"` written with
/// an actual `\n` byte. RFC 8259 §7 forbids that: control chars (U+0000..U+001F)
/// inside strings MUST be escaped. `serde_json` is strict and rejects the
/// whole document. We do a single pass that tracks string boundaries and
/// rewrites the offending bytes in place.
fn sanitize_json_strings(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_str = false;
    let mut escape = false;
    for ch in s.chars() {
        if escape {
            // Pass through the char following a backslash unchanged. This
            // keeps already-escaped sequences like `\"`, `\\`, `\n`, `\uXXXX`
            // intact.
            out.push(ch);
            escape = false;
            continue;
        }
        if in_str {
            match ch {
                '\\' => {
                    out.push('\\');
                    escape = true;
                }
                '"' => {
                    out.push('"');
                    in_str = false;
                }
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                c if (c as u32) < 0x20 => {
                    use std::fmt::Write;
                    let _ = write!(out, "\\u{:04x}", c as u32);
                }
                c => out.push(c),
            }
        } else {
            if ch == '"' {
                in_str = true;
            }
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_balanced_object_with_preamble() {
        let s = "Sure, here you go:\n```json\n{\"a\":1,\"b\":[2,3]}\n```\nthanks!";
        assert_eq!(extract_json_object(s), "{\"a\":1,\"b\":[2,3]}");
    }

    #[test]
    fn handles_strings_with_braces() {
        let s = r#"prefix {"q":"weird }{ inside","ok":true} suffix"#;
        assert_eq!(
            extract_json_object(s),
            r#"{"q":"weird }{ inside","ok":true}"#
        );
    }

    #[test]
    fn falls_back_to_trim_when_no_brace() {
        assert_eq!(extract_json_object("  no json here  "), "no json here");
    }

    #[test]
    fn sanitizer_escapes_raw_newlines_inside_strings() {
        // raw `\n` byte inside a string value — what gemma produces and
        // what serde_json refuses to parse.
        let bad = "{\"evidence\": \"line one\nline two\"}";
        let fixed = sanitize_json_strings(bad);
        assert_eq!(fixed, r#"{"evidence": "line one\nline two"}"#);
        // Round-trips through serde_json now.
        let v: serde_json::Value = serde_json::from_str(&fixed).expect("parse fixed");
        assert_eq!(v["evidence"], "line one\nline two");
    }

    #[test]
    fn sanitizer_preserves_already_escaped_sequences() {
        let good = r#"{"x": "a\nb", "y": "é"}"#;
        // Already-valid input is left unchanged byte-for-byte.
        assert_eq!(sanitize_json_strings(good), good);
    }

    #[test]
    fn sanitizer_keeps_control_chars_outside_strings() {
        // Whitespace between tokens is fine in JSON; the sanitizer only
        // escapes inside string literals.
        let s = "{\n  \"k\": 1\n}";
        let out = sanitize_json_strings(s);
        assert!(out.contains('\n'));
        let v: serde_json::Value = serde_json::from_str(&out).expect("parse");
        assert_eq!(v["k"], 1);
    }

    #[test]
    fn sanitizer_handles_escaped_quote_inside_string() {
        // The `\"` should not terminate the string scan; the following raw
        // newline must still be escaped.
        let bad = "{\"q\": \"he said \\\"hi\\\"\nthen left\"}";
        let fixed = sanitize_json_strings(bad);
        let v: serde_json::Value = serde_json::from_str(&fixed).expect("parse");
        assert_eq!(v["q"], "he said \"hi\"\nthen left");
    }

    #[derive(Debug, serde::Deserialize, PartialEq)]
    struct TestFlag {
        #[serde(default)]
        kind: String,
        #[serde(default)]
        quote: String,
    }

    #[test]
    fn salvage_recovers_elements_around_stray_token() {
        // Reproduces the failing case from the log: a bare `im,` between two
        // valid array elements. Strict parse fails; salvage should return the
        // three valid elements.
        let bad = r#"{
  "flags": [
    { "kind": "spelling", "quote": "judgement" },
    { "kind": "punctuation", "quote": "Wua, were perfect" },
    im,
    { "kind": "spelling", "quote": "ammassed" }
  ]
}"#;
        assert!(
            serde_json::from_str::<serde_json::Value>(bad).is_err(),
            "input must actually be malformed"
        );
        let flags: Vec<TestFlag> = salvage_array(bad, "flags");
        assert_eq!(flags.len(), 3);
        assert_eq!(flags[0].quote, "judgement");
        assert_eq!(flags[1].quote, "Wua, were perfect");
        assert_eq!(flags[2].quote, "ammassed");
    }

    #[test]
    fn salvage_returns_empty_when_key_missing() {
        let s = r#"{"other": [{"kind":"x"}]}"#;
        let flags: Vec<TestFlag> = salvage_array(s, "flags");
        assert!(flags.is_empty());
    }

    #[test]
    fn salvage_ignores_truncated_trailing_object() {
        // Last element is missing its closing brace — give up cleanly and
        // return the elements parsed so far.
        let bad = r#"{"flags":[{"kind":"a","quote":"q1"},{"kind":"b","quote":"q2"#;
        let flags: Vec<TestFlag> = salvage_array(bad, "flags");
        assert_eq!(flags.len(), 1);
        assert_eq!(flags[0].quote, "q1");
    }

    #[test]
    fn salvage_skips_value_with_matching_substring() {
        // A `"flags"` substring inside a why-text must not be picked as the
        // array key (it's not followed by `:`).
        let s = r#"{"why":"contains \"flags\" word","flags":[{"kind":"k","quote":"q"}]}"#;
        let flags: Vec<TestFlag> = salvage_array(s, "flags");
        assert_eq!(flags.len(), 1);
        assert_eq!(flags[0].quote, "q");
    }

    #[test]
    fn sanitizer_handles_utf8_inside_strings() {
        // Multi-byte chars must survive the sanitizer untouched.
        let s = "{\"name\": \"Übermut\"}";
        let out = sanitize_json_strings(s);
        let v: serde_json::Value = serde_json::from_str(&out).expect("parse");
        assert_eq!(v["name"], "Übermut");
    }
}
