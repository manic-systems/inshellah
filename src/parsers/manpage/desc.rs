//! one parameterized description collector for the boundary-style bodies.
//!
//! several extractors share the shape: accumulate Text and rendered inline
//! macros until a boundary macro, optionally skipping nested `.RS/.RE` blocks
//! and optionally stopping at the first blank after some text. the knobs below
//! express every difference.

use crate::parsers::manpage::groff::{GroffLine, strip_groff_escapes, strip_space_macro_args};

/// which inline-format macros render to text inside a description body.
#[derive(Clone, Copy)]
pub enum TagMacros {
    None,
    /// bold/italic family: B/BI/BR/I/IR/RI.
    Common,
    /// Common plus the reverse-order IB/RB forms.
    Wide,
}

impl TagMacros {
    fn renders(self, name: &str) -> bool {
        match self {
            TagMacros::None => false,
            TagMacros::Common => matches!(name, "B" | "BI" | "BR" | "I" | "IR" | "RI"),
            TagMacros::Wide => {
                matches!(name, "B" | "BI" | "BR" | "I" | "IR" | "IB" | "RB" | "RI")
            }
        }
    }
}

/// knobs for [`collect`]. defaults match the most common boundary collector.
#[derive(Clone, Copy)]
pub struct DescOpts {
    /// macro names that terminate the body (e.g. `["TP", "SH", "SS"]`).
    pub boundaries: &'static [&'static str],
    /// skip whole `.RS/.RE` blocks (sub-value example listings).
    pub skip_rs: bool,
    /// a blank line ends the body once text has been collected (leading
    /// blanks between tag and first line are always skipped).
    pub stop_on_blank: bool,
    pub tags: TagMacros,
}

/// render an inline-format macro's args as the strategies do: .B/.I keep
/// spaces, alternating-font macros concatenate.
fn render_tag(name: &str, args: &str) -> String {
    match name {
        "B" | "I" => strip_space_macro_args(args),
        _ => strip_groff_escapes(&crate::parsers::manpage::groff::strip_inline_macro_args(args))
            .trim()
            .to_string(),
    }
}

/// collect a description body from `start`, returning `(joined_text, next_i)`.
pub fn collect(lines: &[GroffLine], start: usize, opts: DescOpts) -> (String, usize) {
    let mut acc: Vec<String> = Vec::new();
    let mut i = start;
    while i < lines.len() {
        match &lines[i] {
            GroffLine::Macro { name, .. } if opts.boundaries.contains(&name.as_str()) => break,
            GroffLine::Macro { name, .. } if opts.skip_rs && name == "RS" => {
                i = skip_rs_block(lines, i + 1);
            }
            GroffLine::Text(t) => {
                acc.push(t.clone());
                i += 1;
            }
            GroffLine::Macro { name, args } if opts.tags.renders(name) => {
                let text = render_tag(name, args);
                if !text.is_empty() {
                    acc.push(text);
                }
                i += 1;
            }
            GroffLine::Blank if opts.stop_on_blank && !acc.is_empty() => break,
            _ => i += 1,
        }
    }
    (acc.join(" "), i)
}

/// advance past a `.RS` block (the opener already consumed), depth-tracked,
/// returning the index just after its matching `.RE` (or EOF).
fn skip_rs_block(lines: &[GroffLine], start: usize) -> usize {
    let mut i = start;
    let mut depth: u32 = 1;
    while i < lines.len() && depth > 0 {
        if let GroffLine::Macro { name, .. } = &lines[i] {
            if name == "RS" {
                depth += 1;
            } else if name == "RE" {
                depth -= 1;
            }
        }
        i += 1;
    }
    i
}
