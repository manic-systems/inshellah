// SPDX-License-Identifier: EUPL-1.2
//! complete-path knobs, read from INSHELLAH_* env (also set by the nixos module).

pub const DEFAULT_TIMEOUT_MS: u64 = 200;

/// 0 disables the dynamic provider.
pub const DEFAULT_DYNAMIC_TIMEOUT_MS: u64 = 5000;

/// row cap for native list commands (git for-each-ref --count N), 0 omits the flag.
pub const DEFAULT_DYNAMIC_LIMIT: usize = 200;

pub const DEFAULT_FLAG_TRIGGERS: &str = "-";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// chars that, as a token's first byte, surface flags.
    pub flag_triggers: Vec<char>,
    /// surface flags on an empty token (after a space).
    pub flag_on_empty: bool,
    /// cap on static candidates, 0 = no cap.
    pub max_completions: usize,
    /// --help resolve + adb timeout (ms).
    pub timeout_ms: u64,
    /// budget across dynamic provider subprocesses, distinct from timeout_ms.
    pub dynamic_timeout_ms: u64,
    pub dynamic_limit: usize,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            flag_triggers: DEFAULT_FLAG_TRIGGERS.chars().collect(),
            flag_on_empty: false,
            max_completions: 0,
            timeout_ms: DEFAULT_TIMEOUT_MS,
            dynamic_timeout_ms: DEFAULT_DYNAMIC_TIMEOUT_MS,
            dynamic_limit: DEFAULT_DYNAMIC_LIMIT,
        }
    }
}

impl Config {
    pub fn from_env() -> Self {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    /// env source injected so tests don't touch the process env.
    pub fn from_lookup(mut get: impl FnMut(&str) -> Option<String>) -> Self {
        let mut cfg = Config::default();
        if let Some(raw) = get("INSHELLAH_FLAG_TRIGGERS") {
            // whitespace can't start a token so drop it. empty value disables
            // prefix triggers, leaving only flag_on_empty.
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
        // i64 so "-1" rejects instead of wrapping, negatives and garbage fall back.
        if let Some(raw) = get("INSHELLAH_DYNAMIC_TIMEOUT_MS")
            && let Ok(n) = raw.trim().parse::<i64>()
            && n >= 0
        {
            cfg.dynamic_timeout_ms = n as u64;
        }
        if let Some(raw) = get("INSHELLAH_DYNAMIC_LIMIT")
            && let Ok(n) = raw.trim().parse::<i64>()
            && n >= 0
        {
            cfg.dynamic_limit = n as usize;
        }
        cfg
    }

    pub fn triggers_flags(&self, token: &str) -> bool {
        match token.chars().next() {
            None => self.flag_on_empty,
            Some(c) => self.flag_triggers.contains(&c),
        }
    }

    /// dash keeps the dashed form so `--ver` prefers `--verbose`; any other
    /// trigger strips its lead char and matches the bare name (`+ver` -> verbose).
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

/// `bare` matches `needle` against the stripped flag name, else the dashed form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlagNeedle<'a> {
    pub needle: &'a str,
    pub bare: bool,
}

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
    fn dynamic_knobs_parse_and_disable() {
        let cfg = cfg_from(&[
            ("INSHELLAH_DYNAMIC_TIMEOUT_MS", "1000"),
            ("INSHELLAH_DYNAMIC_LIMIT", "50"),
        ]);
        assert_eq!(cfg.dynamic_timeout_ms, 1000);
        assert_eq!(cfg.dynamic_limit, 50);

        let zeroed = cfg_from(&[
            ("INSHELLAH_DYNAMIC_TIMEOUT_MS", "0"),
            ("INSHELLAH_DYNAMIC_LIMIT", "0"),
        ]);
        assert_eq!(zeroed.dynamic_timeout_ms, 0);
        assert_eq!(zeroed.dynamic_limit, 0);

        let bad = cfg_from(&[
            ("INSHELLAH_DYNAMIC_TIMEOUT_MS", "-1"),
            ("INSHELLAH_DYNAMIC_LIMIT", "nope"),
        ]);
        assert_eq!(bad.dynamic_timeout_ms, DEFAULT_DYNAMIC_TIMEOUT_MS);
        assert_eq!(bad.dynamic_limit, DEFAULT_DYNAMIC_LIMIT);
    }

    #[test]
    fn numeric_knobs_parse_and_fall_back() {
        let cfg = cfg_from(&[
            ("INSHELLAH_MAX_COMPLETIONS", "50"),
            ("INSHELLAH_TIMEOUT_MS", "1000"),
        ]);
        assert_eq!(cfg.max_completions, 50);
        assert_eq!(cfg.timeout_ms, 1000);

        let bad = cfg_from(&[
            ("INSHELLAH_MAX_COMPLETIONS", "lots"),
            ("INSHELLAH_TIMEOUT_MS", "soon"),
        ]);
        assert_eq!(bad.max_completions, 0);
        assert_eq!(bad.timeout_ms, DEFAULT_TIMEOUT_MS);
    }
}
