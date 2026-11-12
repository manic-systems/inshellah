// SPDX-License-Identifier: EUPL-1.2
//! the shells `complete` answers, and bash's raw command line.

use std::{
    fmt::{
        self,
        Display,
        Formatter,
    },
    mem,
};

use pound::{
    ArgValue as _,
    ValueEnum,
};

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shell {
    Nushell,
    Bash,
    Zsh,
    Fish,
}

impl Display for Shell {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// bash's default `COMP_WORDBREAKS`.
pub const BASH_WORDBREAKS: &str = " \t\n\"'@><=;|&(:";

/// wordbreaks readline keeps at the start of the word instead of dropping,
/// `rl_special_prefixes` in bash.
const SPECIAL_PREFIXES: &str = "$@";

/// reserved words after which a new command starts.
const COMMAND_PREFIXES: [&str; 11] = [
    "time", "!", "{", "if", "then", "elif", "else", "while", "until", "do", "coproc",
];

/// a bash command line up to the cursor, split into the words of the command
/// being typed. the last word is the one under the cursor, possibly empty.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct BashLine {
    pub words:    Vec<String>,
    /// the part of the last word readline won't replace, cut at its last
    /// wordbreak or opening quote.
    pub kept:     String,
    pub quote:    Option<char>,
    /// the cursor sits on a redirection target.
    pub redirect: bool,
    /// text readline replaces along with the last word, put back in front of
    /// every candidate.
    pub lead:     String,
    /// byte offset in the parsed line where the replaced tail of the last
    /// word starts.
    pub raw_kept: usize,
    /// the line ends inside a substitution still open, where readline does
    /// not close the command's own quote.
    pub nested:   bool,
}

#[derive(Default)]
struct Lexer {
    words:         Vec<String>,
    word:          String,
    in_word:       bool,
    kept:          usize,
    /// `kept` before the open quote, back in force once it closes, since
    /// readline only breaks a word at a quote that is still open.
    unquoted_kept: usize,
    word_raw:      usize,
    kept_raw:      Option<usize>,
    unquoted_raw:  Option<usize>,
    target:        bool,
    pending:       bool,
}

impl Lexer {
    fn start_word(&mut self, at: usize) {
        if !self.in_word {
            self.in_word = true;
            self.word_raw = at;
            self.target = mem::take(&mut self.pending);
        }
    }

    fn end_word(&mut self) {
        if self.in_word {
            let word = mem::take(&mut self.word);
            if !self.target {
                self.words.push(word);
            }
        }
        self.in_word = false;
        self.target = false;
        self.kept = 0;
        self.kept_raw = None;
    }

    const fn open_quote(&mut self, after: usize) {
        self.unquoted_kept = self.kept;
        self.unquoted_raw = self.kept_raw;
        self.kept = self.word.len();
        self.kept_raw = Some(after);
    }
}

/// no real line nests substitutions this deep, so what lies past it is kept
/// as one opaque word and the parser can't overflow the stack.
const MAX_NESTING: usize = 64;

#[derive(Clone)]
struct Cursor<'line> {
    line: &'line str,
    pos:  usize,
}

impl<'line> Cursor<'line> {
    const fn new(line: &'line str) -> Self {
        Self { line, pos: 0 }
    }

    fn peek(&self) -> Option<char> {
        self.line.get(self.pos..)?.chars().next()
    }

    fn next_if_eq(&mut self, want: char) -> Option<char> {
        let ch = self.peek().filter(|ch| *ch == want)?;
        self.pos += ch.len_utf8();
        Some(ch)
    }
}

impl Iterator for Cursor<'_> {
    type Item = char;

    fn next(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.pos += ch.len_utf8();
        Some(ch)
    }
}

/// where readline starts the word when it counts a quote as open, going by
/// quote characters alone as it does.
fn readline_quote(line: &str) -> Option<usize> {
    let mut open: Option<(char, usize)> = None;
    let mut chars = line.char_indices();
    while let Some((idx, ch)) = chars.next() {
        match open {
            Some((quote, _)) if ch == quote => open = None,
            Some(('\'', _)) => {},
            _ if ch == '\\' => {
                chars.next();
            },
            None if ch == '"' || ch == '\'' => open = Some((ch, idx + 1)),
            _ => {},
        }
    }
    open.map(|(_, at)| at)
}

impl BashLine {
    pub fn parse(line: &str, wordbreaks: &str) -> Self {
        Self::parse_nested(line, wordbreaks, 0)
    }

    fn parse_nested(line: &str, wordbreaks: &str, depth: usize) -> Self {
        let mut lex = Lexer::default();
        let mut quote = None;
        let mut prev = None;
        let mut chars = Cursor::new(line);
        while let Some(ch) = chars.next() {
            let after = chars.pos;
            let here = after - ch.len_utf8();
            match (quote, ch) {
                (Some('\''), '\'') | (Some('"'), '"') => {
                    quote = None;
                    lex.kept = lex.unquoted_kept;
                    lex.kept_raw = lex.unquoted_raw;
                },
                (Some('"'), '\\') if chars.peek().is_some_and(|next| "\"\\$`".contains(next)) => {
                    lex.word.extend(chars.next());
                },
                (Some('"'), '$' | '`') if ch == '`' || chars.peek() == Some('(') => {
                    let mut ahead = chars.clone();
                    let open = ahead.next_if_eq('(').unwrap_or(ch);
                    let body_at = ahead.pos;
                    let (body, closed) = substitution(&mut ahead, open);
                    if closed {
                        chars = ahead;
                        lex.word.push(ch);
                        if open == '(' {
                            lex.word.push(open);
                        }
                        lex.word.push_str(&body);
                    } else if let Some(inner) =
                        Self::in_quotes(line, &lex, &body, body_at, wordbreaks, depth)
                    {
                        return inner;
                    } else {
                        lex.word.push(ch);
                    }
                },
                (Some(_), other) => lex.word.push(other),
                (None, ' ' | '\t' | '\n') => lex.end_word(),
                (None, '&' | '|') if matches!(prev, Some('<' | '>')) => {},
                (None, '&') if chars.peek() == Some('>') => {},
                (None, '$' | '<' | '>' | '`') if ch == '`' || chars.peek() == Some('(') => {
                    let open = chars.next_if_eq('(').unwrap_or(ch);
                    let body_at = chars.pos;
                    let (body, closed) = substitution(&mut chars, open);
                    if !closed {
                        let mut inner = Self::parse_inner(&body, wordbreaks, depth);
                        inner.raw_kept += body_at;
                        inner.nested = true;
                        return inner;
                    }
                    lex.start_word(here);
                    lex.word.push(ch);
                    if open == '(' {
                        lex.word.push(open);
                    }
                    lex.word.push_str(&body);
                },
                (None, ';' | '|' | '&' | '(') => lex = Lexer::default(),
                (None, '<' | '>') => {
                    let fd = lex.in_word
                        && !lex.target
                        && lex.word.bytes().all(|byte| byte.is_ascii_digit());
                    if fd {
                        lex.word.clear();
                        lex.in_word = false;
                    }
                    lex.end_word();
                    lex.pending = true;
                },
                (None, '\\') => {
                    lex.start_word(here);
                    lex.word.extend(chars.next());
                },
                (None, '$') if chars.peek().is_some_and(|next| next == '\'' || next == '"') => {
                    lex.start_word(here);
                    quote = chars.next();
                    lex.open_quote(chars.pos);
                },
                (None, '\'' | '"') => {
                    lex.start_word(here);
                    quote = Some(ch);
                    lex.open_quote(after);
                },
                (None, other) => {
                    lex.start_word(here);
                    lex.word.push(other);
                    if wordbreaks.contains(other) {
                        if SPECIAL_PREFIXES.contains(other) {
                            lex.kept = lex.word.len() - other.len_utf8();
                            lex.kept_raw = Some(here);
                        } else {
                            lex.kept = lex.word.len();
                            lex.kept_raw = Some(after);
                        }
                    }
                },
            }
            prev = Some(ch);
        }
        Self::finish(lex, quote, line.len())
    }

    fn finish(mut lex: Lexer, quote: Option<char>, len: usize) -> Self {
        let redirect = lex.pending || lex.target;
        let kept = lex.word.get(..lex.kept).unwrap_or_default().to_owned();
        let raw_kept = if lex.in_word {
            lex.kept_raw.unwrap_or(lex.word_raw)
        } else {
            len
        };
        if !redirect {
            lex.words.push(mem::take(&mut lex.word));
        }
        let mut words = lex.words;
        while words.len() > 1 && COMMAND_PREFIXES.contains(&words[0].as_str()) {
            let timed = words.remove(0) == "time";
            if timed && words.len() > 1 && words[0] == "-p" {
                words.remove(0);
            }
        }
        let assignments = words
            .iter()
            .take(words.len().saturating_sub(1))
            .take_while(|word| is_assignment(word))
            .count();
        words.drain(..assignments);
        Self {
            words,
            kept,
            quote,
            redirect,
            lead: String::new(),
            raw_kept,
            nested: false,
        }
    }

    fn parse_inner(body: &str, wordbreaks: &str, depth: usize) -> Self {
        if depth >= MAX_NESTING {
            return Self {
                words: vec![body.to_owned()],
                raw_kept: body.len(),
                ..Self::default()
            };
        }
        Self::parse_nested(body, wordbreaks, depth + 1)
    }

    /// the command typed in a substitution still open inside double quotes.
    /// readline keeps the text it counts as the word as one candidate prefix
    /// there, so `lead` carries what precedes the command's last word back in.
    fn in_quotes(
        line: &str,
        lex: &Lexer,
        body: &str,
        body_at: usize,
        wordbreaks: &str,
        depth: usize,
    ) -> Option<Self> {
        let mut inner = Self::parse_inner(body, wordbreaks, depth);
        if inner.redirect {
            return None;
        }
        inner.raw_kept += body_at;
        inner.nested = true;
        let from = readline_quote(line).unwrap_or_else(|| lex.unquoted_raw.unwrap_or(lex.word_raw));
        line.get(from..inner.raw_kept)?.clone_into(&mut inner.lead);
        Some(inner)
    }

    /// `value` as readline should insert it over the replaced tail of the
    /// word, or `None` when it can't be inserted there.
    pub fn insertion(&self, value: &str) -> Option<String> {
        let tail = value.strip_prefix(self.kept.as_str())?;
        let mut out = String::with_capacity(tail.len());
        for (idx, ch) in tail.char_indices() {
            let special = match self.quote {
                // `\!` keeps its backslash inside double quotes, so history
                // expansion can't be escaped there.
                Some('\'') if ch == '\'' => return None,
                Some('"') if ch == '!' => return None,
                Some('\'') => false,
                Some(_) => "\"\\$`".contains(ch),
                None => {
                    " \"'\\$`<>|&;()*?[]{}!".contains(ch)
                        || (idx == 0 && self.kept.is_empty() && ch == '#')
                },
            };
            if special {
                out.push('\\');
            }
            out.push(ch);
        }
        if let (true, Some(quote), false) = (self.nested, self.quote, value.ends_with(['/', '='])) {
            out.push(quote);
        }
        Some(format!("{}{out}", self.lead))
    }
}

/// a substitution's body up to and including its closing `)` or backtick,
/// and whether the line got that far.
fn substitution(chars: &mut Cursor<'_>, open: char) -> (String, bool) {
    let close = if open == '(' { ')' } else { open };
    let mut body = String::new();
    let mut depth = 1_usize;
    let mut quote = None;
    while let Some(ch) = chars.next() {
        body.push(ch);
        match (quote, ch) {
            (Some('\''), '\'') | (Some('"'), '"') => quote = None,
            (Some('\''), _) => {},
            (_, '\\') => body.extend(chars.next()),
            (None, '\'' | '"') => quote = Some(ch),
            (None, '(') if open == '(' => depth += 1,
            (None, other) if other == close => {
                depth -= 1;
                if depth == 0 {
                    return (body, true);
                }
            },
            _ => {},
        }
    }
    (body, false)
}

fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        name.starts_with(|ch: char| ch.is_ascii_alphabetic() || ch == '_')
            && name
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    })
}

#[cfg(test)]
mod tests {
    use std::time::{
        Duration,
        Instant,
    };

    use super::*;

    fn parse(line: &str) -> BashLine {
        BashLine::parse(line, BASH_WORDBREAKS)
    }

    fn words(line: &str) -> Vec<String> {
        parse(line).words
    }

    #[test]
    fn quotes_escapes_and_trailing_space() {
        assert_eq!(words("git "), ["git", ""]);
        assert_eq!(words(r#"cmd "a b" 'c d' e\ f "g\"h""#), [
            "cmd", "a b", "c d", "e f", "g\"h"
        ]);
        let open = parse(r#"cmd --out="fi"#);
        assert_eq!(open.words, ["cmd", "--out=fi"]);
        assert_eq!(open.kept, "--out=");
        assert_eq!(open.quote, Some('"'));
        let closed = parse(r#"cmd a"l"p"#);
        assert_eq!(closed.words, ["cmd", "alp"]);
        assert_eq!(closed.insertion("alpha").as_deref(), Some("alpha"));
        assert_eq!(parse("cmd $'al").quote, Some('\''));
    }

    #[test]
    fn wordbreaks_and_special_prefixes_mark_the_kept_prefix() {
        let line = parse("nix build nixpkgs#foo:ba");
        assert_eq!(line.words, ["nix", "build", "nixpkgs#foo:ba"]);
        assert_eq!(line.kept, "nixpkgs#foo:");
        assert_eq!(parse("cmd --flag=va").kept, "--flag=");
        assert_eq!(BashLine::parse("cmd a:b", " ").kept, "");
        let unit = parse("systemctl restart autovt@");
        assert_eq!(
            unit.insertion("autovt@tty1.service").as_deref(),
            Some("@tty1.service")
        );
        assert_eq!(parse("cmd a@b@c").kept, "a@b");
    }

    #[test]
    fn separators_reserved_words_and_redirections_are_skipped() {
        assert_eq!(words("echo hi | git ch"), ["git", "ch"]);
        assert_eq!(words("FOO=1 BAR=2 cargo b"), ["cargo", "b"]);
        assert_eq!(words("time -p cargo b"), ["cargo", "b"]);
        assert_eq!(words("while ! git sw"), ["git", "sw"]);
        assert_eq!(words("x; then time -p git sw"), ["git", "sw"]);
        assert_eq!(words("if"), ["if"]);
        assert_eq!(words("cmd 2>&1 --fl"), ["cmd", "--fl"]);
        assert_eq!(words("cmd &>> log --fl"), ["cmd", "--fl"]);
        assert!(parse("cmd > ou").redirect);
        assert!(!parse("cmd > out ").redirect);
    }

    #[test]
    fn closed_substitutions_stay_inside_their_word() {
        assert_eq!(words("diff <(ls a) --ig"), ["diff", "<(ls a)", "--ig"]);
        assert_eq!(words("cmd `date` --fl"), ["cmd", "`date`", "--fl"]);
        assert_eq!(words("cmd $(a $(b)) --fl"), ["cmd", "$(a $(b))", "--fl"]);
        assert_eq!(words("cmd $(echo \")\") --fl"), [
            "cmd",
            "$(echo \")\")",
            "--fl"
        ]);
        assert_eq!(words("echo \"$(a) b\" --fl"), ["echo", "$(a) b", "--fl"]);
    }

    #[test]
    fn an_open_substitution_completes_its_own_command() {
        assert_eq!(words("curl $(git ch"), ["git", "ch"]);
        assert_eq!(words("diff <(git log --on"), ["git", "log", "--on"]);
        assert_eq!(words("echo `git st"), ["git", "st"]);
        assert_eq!(words("echo \"$(frob --col"), ["frob", "--col"]);
        assert_eq!(words("echo \"\\$(a"), ["echo", "$(a"]);
        let nested = parse("echo \"x$(frob --color=al");
        assert_eq!(nested.words, ["frob", "--color=al"]);
        assert_eq!(nested.lead, "x$(frob --color=");
        assert_eq!(
            nested.insertion("--color=always").as_deref(),
            Some("x$(frob --color=always")
        );
    }

    #[test]
    fn a_substitution_in_quotes_keeps_the_raw_text_in_the_lead() {
        let escaped = parse(r#"echo "$(frob --output=dir\ o"#);
        assert_eq!(escaped.words, ["frob", "--output=dir o"]);
        assert_eq!(
            escaped.insertion("--output=dir one/").as_deref(),
            Some(r"$(frob --output=dir\ one/")
        );
        let nested = parse(r#"echo "$(frob --output="dir o"#);
        assert_eq!(nested.lead, r#""$(frob --output=""#);
        assert_eq!(
            nested.insertion("--output=dir one/").as_deref(),
            Some(r#""$(frob --output="dir one/"#)
        );
        assert_eq!(parse(r#"echo "$(frob "x" --col"#).lead, " ");
        assert_eq!(parse(r#"echo "a\$b $(frob --co"#).lead, r"a\$b $(frob ");
    }

    #[test]
    fn insertion_strips_the_kept_prefix_and_escapes() {
        let line = parse("cmd --flag=va");
        assert_eq!(line.insertion("--flag=value").as_deref(), Some("value"));
        assert_eq!(line.insertion("--other"), None);
        assert_eq!(
            parse("cmd ").insertion("a b&c").as_deref(),
            Some(r"a\ b\&c")
        );
        assert_eq!(parse("cmd ").insertion("~x/").as_deref(), Some("~x/"));
        assert_eq!(parse("cmd ").insertion("#x").as_deref(), Some(r"\#x"));
        assert_eq!(parse("cmd 'a").insertion("it's"), None);
        assert_eq!(parse("cmd p").insertion("p!q").as_deref(), Some(r"p\!q"));
    }

    #[test]
    fn long_and_deeply_nested_lines_parse_in_linear_time() {
        let start = Instant::now();
        let parsed = parse(&"cmd --flag value ".repeat(20_000));
        assert_eq!(parsed.words.len(), 3 * 20_000 + 1);
        assert!(parse(&format!("echo {}git ch", "$(".repeat(20_000))).nested);
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}
