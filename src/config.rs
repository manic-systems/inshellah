//! runtime configuration for the `complete` path.
//!
//! the completer reads a handful of behavioural knobs from the
//! environment. this matches the mechanism already used for the dynamic
//! nushell shim (`INSHELLAH_DYNAMIC_*`): the nixos module exports the
//! variables via `environment.variables`, and users sourcing the snippet
//! by hand can export them directly. every field has a compiled-in
//! default that reproduces the historical behaviour, so an unconfigured
//! install behaves exactly as before.

/// per-subprocess timeout default for the dynamic `--help` resolve path
/// when neither `--timeout-ms` nor `INSHELLAH_TIMEOUT_MS` is set.
pub const DEFAULT_TIMEOUT_MS: u64 = 200;

/// the historical (and default) flag-trigger set: a partial token starting
/// with `-` asks for flag completions.
pub const DEFAULT_FLAG_TRIGGERS: &str = "-";

/// behavioural configuration resolved once at startup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// characters that, when a partial token begins with one of them,
    /// cause flag completions to be emitted. defaults to `['-']` — the
    /// only trigger in the original behaviour.
    pub flag_triggers: Vec<char>,
    /// also emit flags when the partial token is empty, i.e. right after a
    /// space/tab with nothing typed yet. defaults to `false`.
    pub flag_on_empty: bool,
    /// upper bound on the number of completion candidates returned by the
    /// static completer. `0` means no inshellah-imposed cap (nushell's own
    /// `max_results` still applies).
    pub max_completions: usize,
    /// per-subprocess timeout (ms) for the dynamic `--help` resolve path.
    pub timeout_ms: u64,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            flag_triggers: DEFAULT_FLAG_TRIGGERS.chars().collect(),
            flag_on_empty: false,
            max_completions: 0,
            timeout_ms: DEFAULT_TIMEOUT_MS,
        }
    }
}

impl Config {
    /// resolve configuration from the process environment, falling back to
    /// the compiled-in defaults for anything unset or unparseable.
    pub fn from_env() -> Self {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    /// inner resolver, parameterised over the variable source so tests can
    /// drive it without mutating the real (process-global) environment.
    pub fn from_lookup(mut get: impl FnMut(&str) -> Option<String>) -> Self {
        let mut cfg = Config::default();
        if let Some(raw) = get("INSHELLAH_FLAG_TRIGGERS") {
            // tokens are split on whitespace before they reach us, so a
            // whitespace character can never be the first byte of a partial
            // token — drop any from the trigger set rather than letting it
            // silently never match. an explicitly empty value disables
            // prefix-triggered flags entirely (leaving only flag_on_empty).
            cfg.flag_triggers = raw.chars().filter(|c| !c.is_whitespace()).collect();
        }
        if let Some(raw) = get("INSHELLAH_FLAG_ON_EMPTY") {
            cfg.flag_on_empty = parse_bool(&raw);
        }
        if let Some(raw) = get("INSHELLAH_MAX_COMPLETIONS")
            && let Ok(n) = raw.trim().parse::<usize>()
        {
            cfg.max_completions = n;
        }
        if let Some(raw) = get("INSHELLAH_TIMEOUT_MS")
            && let Ok(n) = raw.trim().parse::<u64>()
        {
            cfg.timeout_ms = n;
        }
        cfg
    }

    /// whether a partial token should surface flag completions. an empty
    /// token is governed by [`Config::flag_on_empty`]; otherwise the first
    /// character is matched against the trigger set.
    pub fn triggers_flags(&self, token: &str) -> bool {
        match token.chars().next() {
            None => self.flag_on_empty,
            Some(c) => self.flag_triggers.contains(&c),
        }
    }

    /// derive the needle used to score flag candidates for a triggering
    /// token, plus whether that needle should match the *bare* flag name
    /// (dashes stripped) rather than the canonical dashed form.
    ///
    /// the `-` trigger keeps the dashed form so long-vs-short ranking is
    /// preserved exactly (`--ver` prefers `--verbose`, `-v` prefers `-v`).
    /// any other trigger character has no dash semantics, so we strip the
    /// single leading trigger char and match the remainder against the bare
    /// name — letting `+ver` match `--verbose`. an empty token yields an
    /// empty bare needle, which matches every flag.
    pub fn flag_needle<'a>(&self, token: &'a str) -> FlagNeedle<'a> {
        match token.chars().next() {
            None => FlagNeedle {
                needle: token,
                bare: true,
            },
            Some('-') => FlagNeedle {
                needle: token,
                bare: false,
            },
            Some(c) => FlagNeedle {
                needle: &token[c.len_utf8()..],
                bare: true,
            },
        }
    }
}

/// the scoring needle for flag candidates: [`FlagNeedle::needle`] is matched
/// against the bare flag name when [`FlagNeedle::bare`] is set, else against
/// the dashed form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlagNeedle<'a> {
    pub needle: &'a str,
    pub bare: bool,
}

/// permissive truthy parse for boolean env vars.
fn parse_bool(s: &str) -> bool {
    matches!(
        s.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn cfg_from(pairs: &[(&str, &str)]) -> Config {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Config::from_lookup(|k| map.get(k).cloned())
    }

    #[test]
    fn defaults_match_historical_behaviour() {
        let cfg = Config::default();
        assert_eq!(cfg.flag_triggers, vec!['-']);
        assert!(!cfg.flag_on_empty);
        assert_eq!(cfg.max_completions, 0);
        assert_eq!(cfg.timeout_ms, DEFAULT_TIMEOUT_MS);

        // only "-" prefixes trigger; empty does not.
        assert!(cfg.triggers_flags("-"));
        assert!(cfg.triggers_flags("--verbose"));
        assert!(!cfg.triggers_flags(""));
        assert!(!cfg.triggers_flags("build"));
    }

    #[test]
    fn flag_on_empty_opens_flags_after_a_space() {
        let cfg = cfg_from(&[("INSHELLAH_FLAG_ON_EMPTY", "true")]);
        assert!(cfg.flag_on_empty);
        assert!(cfg.triggers_flags(""));
        // a bare word still does not trigger flags.
        assert!(!cfg.triggers_flags("sub"));
    }

    #[test]
    fn custom_trigger_chars_replace_the_dash() {
        let cfg = cfg_from(&[("INSHELLAH_FLAG_TRIGGERS", "-+")]);
        assert_eq!(cfg.flag_triggers, vec!['-', '+']);
        assert!(cfg.triggers_flags("+ver"));
        assert!(cfg.triggers_flags("-v"));
        assert!(!cfg.triggers_flags("/x"));
    }

    #[test]
    fn whitespace_in_triggers_is_dropped() {
        let cfg = cfg_from(&[("INSHELLAH_FLAG_TRIGGERS", "- ")]);
        assert_eq!(cfg.flag_triggers, vec!['-']);
    }

    #[test]
    fn dash_needle_keeps_dashes_other_triggers_go_bare() {
        let cfg = cfg_from(&[("INSHELLAH_FLAG_TRIGGERS", "-+")]);
        assert_eq!(
            cfg.flag_needle("--ver"),
            FlagNeedle {
                needle: "--ver",
                bare: false
            }
        );
        assert_eq!(
            cfg.flag_needle("+ver"),
            FlagNeedle {
                needle: "ver",
                bare: true
            }
        );
        assert_eq!(
            cfg.flag_needle(""),
            FlagNeedle {
                needle: "",
                bare: true
            }
        );
    }

    #[test]
    fn numeric_knobs_parse_and_fall_back() {
        let cfg = cfg_from(&[
            ("INSHELLAH_MAX_COMPLETIONS", "50"),
            ("INSHELLAH_TIMEOUT_MS", "1000"),
        ]);
        assert_eq!(cfg.max_completions, 50);
        assert_eq!(cfg.timeout_ms, 1000);

        // garbage leaves the default intact.
        let bad = cfg_from(&[
            ("INSHELLAH_MAX_COMPLETIONS", "lots"),
            ("INSHELLAH_TIMEOUT_MS", "soon"),
        ]);
        assert_eq!(bad.max_completions, 0);
        assert_eq!(bad.timeout_ms, DEFAULT_TIMEOUT_MS);
    }
}
