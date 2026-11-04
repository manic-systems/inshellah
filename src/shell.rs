// SPDX-License-Identifier: EUPL-1.2
//! the shells `complete` answers, and bash's raw command line.

use std::{
    fmt::{
        self,
        Display,
        Formatter,
    },
    iter::Peekable,
    mem,
    str::Chars,
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
pub const BASH_WORDBREAKS: &str = " \t\n\"'><=;|&(:";

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
    target:        bool,
    pending:       bool,
}

impl Lexer {
    fn start_word(&mut self) {
        if !self.in_word {
            self.in_word = true;
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
    }
}

impl BashLine {
    pub fn parse(line: &str, wordbreaks: &str) -> Self {
        let mut lex = Lexer::default();
        let mut quote = None;
        let mut prev = None;
        let mut chars = line.chars().peekable();
        while let Some(ch) = chars.next() {
            match (quote, ch) {
                (Some('\''), '\'') | (Some('"'), '"') => {
                    quote = None;
                    lex.kept = lex.unquoted_kept;
                },
                (Some('"'), '\\') if chars.peek().is_some_and(|next| "\"\\$`".contains(*next)) => {
                    lex.word.extend(chars.next());
                },
                (Some('"'), '$' | '`') if ch == '`' || chars.peek() == Some(&'(') => {
                    let mut ahead = chars.clone();
                    let open = ahead.next_if_eq(&'(').unwrap_or(ch);
                    let (body, closed) = substitution(&mut ahead, open);
                    if closed {
                        chars = ahead;
                        lex.word.push(ch);
                        if open == '(' {
                            lex.word.push(open);
                        }
                        lex.word.push_str(&body);
                    } else if let Some(inner) = Self::in_quotes(&lex, open, &body, wordbreaks) {
                        return inner;
                    } else {
                        lex.word.push(ch);
                    }
                },
                (Some(_), other) => lex.word.push(other),
                (None, ' ' | '\t' | '\n') => lex.end_word(),
                (None, '&' | '|') if matches!(prev, Some('<' | '>')) => {},
                (None, '&') if chars.peek() == Some(&'>') => {},
                (None, '$' | '<' | '>' | '`') if ch == '`' || chars.peek() == Some(&'(') => {
                    let open = chars.next_if_eq(&'(').unwrap_or(ch);
                    let (body, closed) = substitution(&mut chars, open);
                    if !closed {
                        return Self::parse(&body, wordbreaks);
                    }
                    lex.start_word();
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
                    lex.start_word();
                    lex.word.extend(chars.next());
                },
                (None, '$')
                    if chars
                        .peek()
                        .is_some_and(|next| *next == '\'' || *next == '"') =>
                {
                    lex.start_word();
                    quote = chars.next();
                    lex.unquoted_kept = lex.kept;
                    lex.kept = lex.word.len();
                },
                (None, '\'' | '"') => {
                    lex.start_word();
                    quote = Some(ch);
                    lex.unquoted_kept = lex.kept;
                    lex.kept = lex.word.len();
                },
                (None, other) => {
                    lex.start_word();
                    lex.word.push(other);
                    if wordbreaks.contains(other) {
                        lex.kept = lex.word.len();
                    }
                },
            }
            prev = Some(ch);
        }
        let redirect = lex.pending || lex.target;
        let kept = lex.word.get(..lex.kept).unwrap_or_default().to_owned();
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
        }
    }

    /// the command typed in a substitution still open inside double quotes.
    /// readline keeps all the quoted text as one word there, so `lead` carries
    /// what precedes the command's last word back in.
    fn in_quotes(lex: &Lexer, open: char, body: &str, wordbreaks: &str) -> Option<Self> {
        let mut line = Self::parse(body, wordbreaks);
        if line.redirect {
            return None;
        }
        let tail = line.words.last()?.get(line.kept.len()..)?;
        let head = body.strip_suffix(tail)?;
        let opener = if open == '(' { "$(" } else { "`" };
        line.lead = format!("{}{opener}{head}", lex.word.get(lex.kept..)?);
        Some(line)
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
                        || (idx == 0 && self.kept.is_empty() && "~#".contains(ch))
                },
            };
            if special {
                out.push('\\');
            }
            out.push(ch);
        }
        Some(format!("{}{out}", self.lead))
    }
}

/// a substitution's body up to and including its closing `)` or backtick,
/// and whether the line got that far.
fn substitution(chars: &mut Peekable<Chars<'_>>, open: char) -> (String, bool) {
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
    use super::*;

    fn parse(line: &str) -> BashLine {
        BashLine::parse(line, BASH_WORDBREAKS)
    }

    fn words(line: &str) -> Vec<String> {
        parse(line).words
    }

    #[test]
    fn trailing_space_opens_an_empty_word() {
        assert_eq!(words("git "), ["git", ""]);
        assert_eq!(words("git ch"), ["git", "ch"]);
        assert_eq!(words(""), [""]);
    }

    #[test]
    fn quotes_and_escapes_are_removed() {
        assert_eq!(words(r#"cmd "a b" 'c d' e\ f "g\"h""#), [
            "cmd", "a b", "c d", "e f", "g\"h"
        ]);
        let open = parse(r#"cmd --out="fi"#);
        assert_eq!(open.words, ["cmd", "--out=fi"]);
        assert_eq!(open.kept, "--out=");
        assert_eq!(open.quote, Some('"'));
    }

    #[test]
    fn wordbreaks_mark_the_kept_prefix() {
        let line = parse("nix build nixpkgs#foo:ba");
        assert_eq!(line.words, ["nix", "build", "nixpkgs#foo:ba"]);
        assert_eq!(line.kept, "nixpkgs#foo:");
        assert_eq!(parse("cmd --flag=va").kept, "--flag=");
        assert_eq!(BashLine::parse("cmd a:b", " ").kept, "");
    }

    #[test]
    fn only_an_open_quote_moves_the_word_start() {
        let closed = parse(r#"cmd a"l"p"#);
        assert_eq!(closed.words, ["cmd", "alp"]);
        assert_eq!(closed.kept, "");
        assert_eq!(closed.insertion("alpha").as_deref(), Some("alpha"));
        assert_eq!(parse("cmd --x=\"a\"b").kept, "--x=");
        let ansi = parse("cmd $'al");
        assert_eq!(ansi.words, ["cmd", "al"]);
        assert_eq!(ansi.kept, "");
        assert_eq!(ansi.quote, Some('\''));
    }

    #[test]
    fn separators_start_a_new_command() {
        assert_eq!(words("echo hi | git ch"), ["git", "ch"]);
        assert_eq!(words("make && cargo b"), ["cargo", "b"]);
        assert_eq!(words("FOO=1 BAR=2 cargo b"), ["cargo", "b"]);
        assert_eq!(words("FOO=1"), ["FOO=1"]);
        assert_eq!(words("time -p cargo b"), ["cargo", "b"]);
        assert_eq!(words("time"), ["time"]);
    }

    #[test]
    fn reserved_words_start_a_command() {
        assert_eq!(words("if frob --col"), ["frob", "--col"]);
        assert_eq!(words("while ! git sw"), ["git", "sw"]);
        assert_eq!(words("{ git sw"), ["git", "sw"]);
        assert_eq!(words("x; then time -p git sw"), ["git", "sw"]);
        assert_eq!(words("do FOO=1 git sw"), ["git", "sw"]);
        assert_eq!(words("if"), ["if"]);
    }

    #[test]
    fn redirections_are_skipped() {
        assert_eq!(words("cmd > out.txt --fl"), ["cmd", "--fl"]);
        assert_eq!(words("cmd 2>/dev/null --fl"), ["cmd", "--fl"]);
        assert_eq!(words("cmd 2>&1 --fl"), ["cmd", "--fl"]);
        assert_eq!(words("cmd>out --fl"), ["cmd", "--fl"]);
        assert!(parse("cmd > ou").redirect);
        assert!(parse("cmd >").redirect);
        assert!(!parse("cmd > out ").redirect);
        assert_eq!(words("cmd &>/dev/null --fl"), ["cmd", "--fl"]);
        assert_eq!(words("cmd &>> log --fl"), ["cmd", "--fl"]);
    }

    #[test]
    fn substitutions_stay_inside_their_word() {
        assert_eq!(words("cmd $(echo x) --fl"), ["cmd", "$(echo x)", "--fl"]);
        assert_eq!(words("diff <(ls a) --ig"), ["diff", "<(ls a)", "--ig"]);
        assert_eq!(words("ls $(pwd)/x --al"), ["ls", "$(pwd)/x", "--al"]);
        assert_eq!(words("cmd `date` --fl"), ["cmd", "`date`", "--fl"]);
        assert_eq!(words("cmd $(a $(b)) --fl"), ["cmd", "$(a $(b))", "--fl"]);
        assert_eq!(words("cmd $(echo \")\") --fl"), [
            "cmd",
            "$(echo \")\")",
            "--fl"
        ]);
        assert_eq!(words("cmd $(echo '(') --fl"), [
            "cmd",
            "$(echo '(')",
            "--fl"
        ]);
    }

    #[test]
    fn an_open_substitution_completes_its_own_command() {
        assert_eq!(words("curl $(git ch"), ["git", "ch"]);
        assert_eq!(words("diff <(git log --on"), ["git", "log", "--on"]);
        assert_eq!(words("echo `git st"), ["git", "st"]);
        assert_eq!(words("echo $(a | git "), ["git", ""]);
        assert_eq!(words("echo \"$(frob --col"), ["frob", "--col"]);
        assert_eq!(words("x=\"$(git sw"), ["git", "sw"]);
        assert_eq!(words("echo \"`git st"), ["git", "st"]);
        let nested = parse("echo \"x$(frob --color=al");
        assert_eq!(nested.words, ["frob", "--color=al"]);
        assert_eq!(nested.lead, "x$(frob --color=");
        assert_eq!(
            nested.insertion("--color=always").as_deref(),
            Some("x$(frob --color=always")
        );
        assert_eq!(words("echo \"$(a) b\" --fl"), ["echo", "$(a) b", "--fl"]);
        assert_eq!(words("echo \"\\$(a"), ["echo", "$(a"]);
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
        assert_eq!(parse("cmd ").insertion("~x").as_deref(), Some(r"\~x"));
        assert_eq!(
            parse(r#"cmd "a"#).insertion("a $b").as_deref(),
            Some(r"a \$b")
        );
        assert_eq!(parse("cmd 'a").insertion("it's"), None);
        assert_eq!(parse("cmd \"p").insertion("p!q"), None);
        assert_eq!(parse("cmd p").insertion("p!q").as_deref(), Some(r"p\!q"));
    }
}
