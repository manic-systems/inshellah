//! groff escape/formatting stripping and line classification.
//!
//! also exports `make_macro_walker!`: the "scan lines, run a handler per
//! .MACRO_NAME, advance, accumulate" loop shared by every strategy_*.

/// on each macro named `$mname`, invoke the body with `(lines, i, args)`. the
/// body returns `Option<(T, usize)>`: `Some((value, new_i))` pushes and advances
/// the cursor to `new_i`; `None` advances by one and keeps scanning.
#[macro_export]
macro_rules! make_macro_walker {
    (pub $name:ident -> Vec<$t:ty>, on macro $mname:expr =>
     |$lines:ident, $i:ident, $args:ident| $body:expr) => {
        pub fn $name(lines_input: &[$crate::parsers::manpage::GroffLine]) -> Vec<$t> {
            let mut out = Vec::new();
            let mut cursor = 0;
            let $lines: &[$crate::parsers::manpage::GroffLine] = lines_input;
            while cursor < $lines.len() {
                if let $crate::parsers::manpage::GroffLine::Macro {
                    name: macro_name,
                    args: $args,
                } = &$lines[cursor]
                {
                    if macro_name == $mname {
                        let $i = cursor;
                        // IIFE so an early `return None` exits the handler, not
                        // the strategy function.
                        #[allow(clippy::redundant_closure_call)]
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

fn is_alnum(c: u8) -> bool {
    c.is_ascii_alphanumeric()
}

/// replace named chars with text equivalents and discard formatting directives.
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
                        // \fB--max-results\fR\fIcount\fR -> "--max-results count"
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
                }
                b'-' => {
                    buffer.push('-');
                    prev_char = b'-';
                    pos += 2;
                }
                b'&' | b'/' | b',' => {
                    // zero-width characters
                    pos += 2;
                }
                b'(' => {
                    // two-char named char: \(aq, \(lq, \(rq, etc.
                    if let Some(name) = source.get(pos + 2..pos + 4) {
                        if let Some(c) = named_char_of(name) {
                            buffer.push(c);
                            prev_char = c as u8;
                        }
                        pos += 4;
                    } else {
                        pos += 2;
                    }
                }
                b'[' => {
                    // bracketed named char: \[aq], \[lq], etc.
                    pos += 2;
                    let start = pos;
                    skip_to_byte(bytes, len, &mut pos, b']');
                    if pos < len {
                        let name = &source[start..pos];
                        if let Some(c) = named_char_of(name) {
                            buffer.push(c);
                            prev_char = c as u8;
                        }
                        pos += 1;
                    }
                }
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
                }
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
                }
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
                }
                b'*' => {
                    // string variable: \*X or \*(XX or \*[...]
                    pos += 2;
                    skip_groff_reference(bytes, len, &mut pos);
                }
                b'n' => {
                    // number register: \nX or \n(XX or \n[...]
                    pos += 2;
                    skip_groff_reference(bytes, len, &mut pos);
                }
                b'e' => {
                    buffer.push('\\');
                    prev_char = b'\\';
                    pos += 2;
                }
                b'\\' => {
                    buffer.push('\\');
                    prev_char = b'\\';
                    pos += 2;
                }
                b' ' | b'~' => {
                    // escaped/non-breaking space
                    buffer.push(' ');
                    prev_char = b' ';
                    pos += 2;
                }
                _ => {
                    // unknown escape, skip the two-char sequence
                    pos += 2;
                }
            }
        } else {
            let c = source[pos..].chars().next().unwrap();
            buffer.push(c);
            prev_char = if c.is_ascii() { c as u8 } else { 0 };
            pos += c.len_utf8();
        }
    }
    buffer
}

fn skip_to_byte(bytes: &[u8], len: usize, pos: &mut usize, delim: u8) {
    while *pos < len && bytes[*pos] != delim {
        *pos += 1;
    }
}

/// skip a groff reference in one of three sub-forms:
///   single char, e.g. \*X or \nX
///   ( + 2 chars, e.g. \*(XX or \n(XX
///   [ to ], e.g. \*[name] or \n[name]
fn skip_groff_reference(bytes: &[u8], len: usize, pos: &mut usize) {
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

/// render inline alternating-font macros (.BI, .BR, .IR, ...). args concatenate
/// without spaces, matching groff:
///   .BI "--output " "FILE"  ->  "--outputFILE"
/// quoted strings keep inner spaces (quotes stripped); unquoted spaces consumed.
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
                let c = text[pos..].chars().next().unwrap();
                buffer.push(c);
                pos += c.len_utf8();
            }
            if pos < len {
                pos += 1;
            }
        } else if bytes[pos] == b' ' || bytes[pos] == b'\t' {
            // unquoted whitespace skipped, args concatenate
            pos += 1;
        } else {
            let c = text[pos..].chars().next().unwrap();
            buffer.push(c);
            pos += c.len_utf8();
        }
    }
    buffer
}

/// render same-font macro args (.B/.I), space-separated. quotes group args in
/// roff source but aren't part of the visible text.
pub fn strip_space_macro_args(text: &str) -> String {
    strip_groff_escapes(&text.replace('"', ""))
        .trim()
        .to_string()
}

pub fn strip_groff(line: &str) -> String {
    strip_groff_escapes(line).trim().to_string()
}

/// `.\"` and `\"` comment forms.
fn is_comment_line(line: &str) -> bool {
    let bytes = line.as_bytes();
    let len = bytes.len();
    (len >= 3 && bytes[0] == b'.' && bytes[1] == b'\\' && bytes[2] == b'"')
        || (len >= 2 && bytes[0] == b'\\' && bytes[1] == b'"')
}

/// classify a single line of manpage source. macro lines start with '.' or '\''
/// (groff alternate control char); name splits from args at the first space/tab,
/// double-quoted args are unquoted.
pub fn classify_line(line: &str) -> GroffLine {
    if is_comment_line(line) {
        return GroffLine::Comment;
    }
    let len = line.len();
    if len == 0 {
        return GroffLine::Blank;
    }
    let bytes = line.as_bytes();
    // dot-backslash forms are also comments
    if len >= 2 && bytes[0] == b'.' && bytes[1] == b'\\' && (len < 3 || bytes[2] == b'"') {
        return GroffLine::Comment;
    }
    if len >= 3 && bytes[0] == b'\\' && bytes[1] == b'"' {
        return GroffLine::Comment;
    }
    if bytes[0] == b'.' || bytes[0] == b'\'' {
        let rest = line[1..].trim();
        let split_at = rest.find([' ', '\t']);
        match split_at {
            Some(idx) => {
                let name = rest[..idx].to_string();
                let args = rest[idx + 1..].trim();
                let args = if args.len() >= 2
                    && args.starts_with('"')
                    && args.ends_with('"')
                    && !args[1..args.len() - 1].contains('"')
                {
                    args[1..args.len() - 1].to_string()
                } else {
                    args.to_string()
                };
                GroffLine::Macro { name, args }
            }
            None => GroffLine::Macro {
                name: rest.to_string(),
                args: String::new(),
            },
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
        for input in ["\\(é", "\\(λx", "a \\(€ b", "\\("] {
            let _ = strip_groff_escapes(input);
        }
    }

    #[test]
    fn two_char_named_escape_still_resolves() {
        assert_eq!(strip_groff_escapes("\\(aq"), "'");
    }
}
