//! groff escape/formatting stripping and line classification.
//!
//! also exports `make_macro_walker!`: the "scan lines, run a handler per
//! .`MACRO_NAME`, advance, accumulate" loop shared by every strategy_*.

/// on each macro named `$mname`, invoke the body with `(lines, i, args)`.
///
/// the body returns `Option<(T, usize)>`: `Some((value, new_i))` pushes and
/// advances the cursor to `new_i`; `None` advances by one and keeps scanning.
#[macro_export]
macro_rules! make_macro_walker {
    (
        pub
        $name:ident -> Vec <
        $t:ty > ,on macro
        $mname:expr => |
        $lines:ident,
        $i:ident,
        $args:ident |
        $body:expr
    ) => {
        pub fn $name(lines_input: &[$crate::parsers::manpage::groff::GroffLine]) -> Vec<$t> {
            let mut out = Vec::new();
            let mut cursor = 0;
            let $lines: &[$crate::parsers::manpage::groff::GroffLine] = lines_input;
            while cursor < $lines.len() {
                if let $crate::parsers::manpage::groff::GroffLine::Macro {
                    name: macro_name,
                    args: $args,
                } = &$lines[cursor]
                {
                    if macro_name == $mname {
                        let $i = cursor;
                        // IIFE so an early `return None` exits the handler, not
                        // the strategy function.
                        let result: Option<($t, usize)> = (|| $body)();
                        if let Some((value, new_i)) = result {
                            out.push(value);
                            cursor = new_i;
                            continue;
                        }
                    }
                }
                cursor += 1;
            }
            out
        }
    };
}

/// strategies pattern-match on sequences of these classified lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GroffLine {
    Macro { name: String, args: String },
    Text(String),
    Blank,
    Comment,
}

/// two-letter named char codes: "aq" apostrophe, "lq"/"rq" quotes, "em"/"en"
/// dashes.
fn named_char_of(name: &str) -> Option<char> {
    match name {
        "aq" => Some('\''),
        "lq" | "Lq" | "rq" | "Rq" => Some('"'),
        "em" | "en" => Some('-'),
        _ => None,
    }
}

const fn is_alnum(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
}

/// replace named chars with text equivalents and discard formatting directives.
#[expect(
    clippy::too_many_lines,
    reason = "one match arm per escape kind over a shared byte cursor"
)]
pub fn strip_groff_escapes(source: &str) -> String {
    let bytes = source.as_bytes();
    let len = bytes.len();
    let mut buffer = String::with_capacity(len);
    let mut pos = 0;
    let mut prev_char: u8 = 0;

    while pos < len {
        if bytes[pos] == b'\\' && pos + 1 < len {
            let next = bytes[pos + 1];
            match next {
                b'f' => {
                    // font escape: \fB, \fI, \fP, \fR, \f(XX, \f[...]
                    if pos + 2 < len {
                        let font_char = bytes[pos + 2];
                        // space before italic font preserves word boundaries:
                        // \fB--max-results\fR\fIcount\fR -> "--max-results
                        // count"
                        if font_char == b'I' && is_alnum(prev_char) {
                            buffer.push(' ');
                            prev_char = b' ';
                        }
                        if font_char == b'(' {
                            pos += 5; // \f(XX, two-character font name
                        } else if font_char == b'[' {
                            pos += 3;
                            skip_to_byte(bytes, len, &mut pos, b']');
                            if pos < len {
                                pos += 1;
                            }
                        } else {
                            pos += 3; // \fX, single-character font selector
                        }
                    } else {
                        pos += 2;
                    }
                },
                b'-' => {
                    buffer.push('-');
                    prev_char = b'-';
                    pos += 2;
                },
                b'&' | b'/' | b',' => {
                    // zero-width characters
                    pos += 2;
                },
                b'(' => {
                    // two-char named char: \(aq, \(lq, \(rq, etc.
                    if let Some(name) = source.get(pos + 2..pos + 4) {
                        if let Some(named) = named_char_of(name) {
                            buffer.push(named);
                            prev_char = u8::try_from(named).unwrap_or(0);
                        }
                        pos += 4;
                    } else {
                        pos += 2;
                    }
                },
                b'[' => {
                    // bracketed named char: \[aq], \[lq], etc.
                    pos += 2;
                    let start = pos;
                    skip_to_byte(bytes, len, &mut pos, b']');
                    if pos < len {
                        if let Some(named) = source.get(start..pos).and_then(named_char_of) {
                            buffer.push(named);
                            prev_char = u8::try_from(named).unwrap_or(0);
                        }
                        pos += 1;
                    }
                },
                b's' => {
                    // size escape: \sN, \s+N, \s-N
                    pos += 2;
                    if pos < len && (bytes[pos] == b'+' || bytes[pos] == b'-') {
                        pos += 1;
                    }
                    if pos < len && bytes[pos].is_ascii_digit() {
                        pos += 1;
                    }
                    if pos < len && bytes[pos].is_ascii_digit() {
                        pos += 1;
                    }
                },
                b'm' => {
                    // color escape: \m[...]
                    pos += 2;
                    if pos < len && bytes[pos] == b'[' {
                        pos += 1;
                        skip_to_byte(bytes, len, &mut pos, b']');
                        if pos < len {
                            pos += 1;
                        }
                    }
                },
                b'X' => {
                    // device control: \X'...'
                    pos += 2;
                    if pos < len && bytes[pos] == b'\'' {
                        pos += 1;
                        skip_to_byte(bytes, len, &mut pos, b'\'');
                        if pos < len {
                            pos += 1;
                        }
                    }
                },
                b'*' => {
                    // string variable: \*X or \*(XX or \*[...]
                    pos += 2;
                    skip_groff_reference(bytes, len, &mut pos);
                },
                b'n' => {
                    // number register: \nX or \n(XX or \n[...]
                    pos += 2;
                    skip_groff_reference(bytes, len, &mut pos);
                },
                b'e' | b'\\' => {
                    buffer.push('\\');
                    prev_char = b'\\';
                    pos += 2;
                },
                b' ' | b'~' => {
                    // escaped/non-breaking space
                    buffer.push(' ');
                    prev_char = b' ';
                    pos += 2;
                },
                _ => {
                    // unknown escape, skip the two-char sequence
                    pos += 2;
                },
            }
        } else if let Some(ch) = source.get(pos..).and_then(|rest| rest.chars().next()) {
            buffer.push(ch);
            prev_char = u8::try_from(ch).unwrap_or(0);
            pos += ch.len_utf8();
        } else {
            pos += 1;
        }
    }
    buffer
}

const fn skip_to_byte(bytes: &[u8], len: usize, pos: &mut usize, delim: u8) {
    while *pos < len && bytes[*pos] != delim {
        *pos += 1;
    }
}

/// skip a groff reference in one of three sub-forms:
///   single char, e.g. \*X or \nX
///   ( + 2 chars, e.g. \*(XX or \n(XX
///   [ to ], e.g. \*[name] or \n[name]
const fn skip_groff_reference(bytes: &[u8], len: usize, pos: &mut usize) {
    if *pos < len {
        if bytes[*pos] == b'(' {
            *pos += 3; // skip past '(' + two-character name
        } else if bytes[*pos] == b'[' {
            *pos += 1;
            skip_to_byte(bytes, len, pos, b']');
            if *pos < len {
                *pos += 1;
            }
        } else {
            *pos += 1;
        }
    }
}

/// render inline alternating-font macros (.BI, .BR, .IR, ...).
///
/// args concatenate without spaces, matching groff:
///   .BI "--output " "FILE"  ->  "--outputFILE"
/// quoted strings keep inner spaces (quotes stripped); unquoted spaces
/// consumed.
pub fn strip_inline_macro_args(text: &str) -> String {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut buffer = String::with_capacity(len);
    let mut pos = 0;
    while pos < len {
        if bytes[pos] == b'"' {
            // quoted arg: keep inner spaces, copy to closing quote
            pos += 1;
            while pos < len && bytes[pos] != b'"' {
                let Some(ch) = text.get(pos..).and_then(|rest| rest.chars().next()) else {
                    break;
                };
                buffer.push(ch);
                pos += ch.len_utf8();
            }
            if pos < len {
                pos += 1;
            }
        } else if bytes[pos] == b' ' || bytes[pos] == b'\t' {
            // unquoted whitespace skipped, args concatenate
            pos += 1;
        } else {
            let Some(ch) = text.get(pos..).and_then(|rest| rest.chars().next()) else {
                break;
            };
            buffer.push(ch);
            pos += ch.len_utf8();
        }
    }
    buffer
}

/// render same-font macro args (.B/.I), space-separated. quotes group args in
/// roff source but aren't part of the visible text.
pub fn strip_space_macro_args(text: &str) -> String {
    strip_groff_escapes(&text.replace('"', ""))
        .trim()
        .to_owned()
}

pub fn strip_groff(line: &str) -> String {
    strip_groff_escapes(line).trim().to_owned()
}

/// `.\"` and `\"` comment forms.
const fn is_comment_line(line: &str) -> bool {
    matches!(line.as_bytes(), [b'.', b'\\', b'"', ..] | [b'\\', b'"', ..])
}

/// classify a single line of manpage source. macro lines start with '.' or '\''
/// (groff alternate control char); name splits from args at the first
/// space/tab, double-quoted args are unquoted.
pub fn classify_line(line: &str) -> GroffLine {
    if is_comment_line(line) {
        return GroffLine::Comment;
    }
    let bytes = line.as_bytes();
    if bytes.is_empty() {
        return GroffLine::Blank;
    }
    // dot-backslash forms are also comments
    if matches!(bytes, [b'.', b'\\'] | [b'.', b'\\', b'"', ..]) {
        return GroffLine::Comment;
    }
    if matches!(bytes, [b'\\', b'"', _, ..]) {
        return GroffLine::Comment;
    }
    if let Some(after_control) = line.strip_prefix(['.', '\'']) {
        let rest = after_control.trim();
        let Some((name, raw_args)) = rest.split_once([' ', '\t']) else {
            return GroffLine::Macro {
                name: rest.to_owned(),
                args: String::new(),
            };
        };
        let trimmed = raw_args.trim();
        let args = trimmed
            .strip_prefix('"')
            .and_then(|inner| inner.strip_suffix('"'))
            .filter(|inner| !inner.contains('"'))
            .unwrap_or(trimmed)
            .to_owned();
        GroffLine::Macro {
            name: name.to_owned(),
            args,
        }
    } else {
        let stripped = strip_groff(line);
        if stripped.is_empty() {
            GroffLine::Blank
        } else {
            GroffLine::Text(stripped)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_char_named_escape_followed_by_multibyte_does_not_panic() {
        for input in ["\\(\u{e9}", "\\(\u{3bb}x", "a \\(\u{20ac} b", "\\("] {
            drop(strip_groff_escapes(input));
        }
    }

    #[test]
    fn two_char_named_escape_still_resolves() {
        assert_eq!(strip_groff_escapes("\\(aq"), "'");
    }
}
