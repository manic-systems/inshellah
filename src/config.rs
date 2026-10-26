// SPDX-License-Identifier: EUPL-1.2
//! complete-path knobs, read from INSHELLAH_* env (also set by the nixos
//! module).

use std::env;

pub const DEFAULT_TIMEOUT_MS: u64 = 1200;

/// 0 disables the dynamic provider.
pub const DEFAULT_DYNAMIC_TIMEOUT_MS: u64 = 5000;

/// row cap for native list commands (git for-each-ref --count N), 0 omits the
/// flag.
pub const DEFAULT_DYNAMIC_LIMIT: usize = 200;

pub const DEFAULT_FLAG_TRIGGERS: &str = "-";

/// rescrape user-cached sets older than this; 0 disables.
pub const DEFAULT_CACHE_TTL_SECS: u64 = 604_800;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// chars that, as a token's first byte, surface flags.
    pub flag_triggers:      Vec<char>,
    /// surface flags on an empty token (after a space).
    pub flag_on_empty:      bool,
    /// cap on static candidates, 0 = no cap.
    pub max_completions:    usize,
    /// --help resolve + adb timeout (ms).
    pub timeout_ms:         u64,
    /// budget across dynamic provider subprocesses, distinct from `timeout_ms`.
    pub dynamic_timeout_ms: u64,
    pub dynamic_limit:      usize,
    /// rescrape user-cached sets older than this many seconds, 0 disables.
    pub cache_ttl_secs:     u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            flag_triggers:      DEFAULT_FLAG_TRIGGERS.chars().collect(),
            flag_on_empty:      false,
            max_completions:    0,
            timeout_ms:         DEFAULT_TIMEOUT_MS,
            dynamic_timeout_ms: DEFAULT_DYNAMIC_TIMEOUT_MS,
            dynamic_limit:      DEFAULT_DYNAMIC_LIMIT,
            cache_ttl_secs:     DEFAULT_CACHE_TTL_SECS,
        }
    }
}

impl Config {
    pub fn from_env() -> Self {
        Self::from_lookup(|key| env::var(key).ok())
    }

    /// env source injected so tests don't touch the process env.
    pub fn from_lookup<Lookup>(mut get: Lookup) -> Self
    where
        Lookup: FnMut(&str) -> Option<String>,
    {
        let mut cfg = Self::default();
        if let Some(raw) = get("INSHELLAH_FLAG_TRIGGERS") {
            // whitespace can't start a token so drop it. empty value disables
            // prefix triggers, leaving only flag_on_empty.
            cfg.flag_triggers = raw.chars().filter(|ch| !ch.is_whitespace()).collect();
        }
        if let Some(raw) = get("INSHELLAH_FLAG_ON_EMPTY") {
            cfg.flag_on_empty = parse_bool(&raw);
        }
        if let Some(raw) = get("INSHELLAH_MAX_COMPLETIONS")
            && let Ok(value) = raw.trim().parse::<usize>()
        {
            cfg.max_completions = value;
        }
        if let Some(raw) = get("INSHELLAH_TIMEOUT_MS")
            && let Ok(value) = raw.trim().parse::<u64>()
        {
            cfg.timeout_ms = value;
        }
        if let Some(raw) = get("INSHELLAH_CACHE_TTL_SECS")
            && let Ok(value) = raw.trim().parse::<u64>()
        {
            cfg.cache_ttl_secs = value;
        }
        // i64 so "-1" rejects instead of wrapping, negatives and garbage fall
        // back.
        if let Some(raw) = get("INSHELLAH_DYNAMIC_TIMEOUT_MS")
            && let Ok(signed) = raw.trim().parse::<i64>()
            && let Ok(value) = u64::try_from(signed)
        {
            cfg.dynamic_timeout_ms = value;
        }
        if let Some(raw) = get("INSHELLAH_DYNAMIC_LIMIT")
            && let Ok(signed) = raw.trim().parse::<i64>()
            && let Ok(value) = usize::try_from(signed)
        {
            cfg.dynamic_limit = value;
        }
        cfg
    }

    pub fn triggers_flags(&self, token: &str) -> bool {
        token.chars().next().map_or(self.flag_on_empty, |lead| {
            self.flag_triggers.contains(&lead)
        })
    }

    /// dash keeps the dashed form so `--ver` prefers `--verbose`; any other
    /// trigger strips its lead char and matches the bare name (`+ver` ->
    /// verbose).
    pub fn flag_needle<'token>(&self, token: &'token str) -> FlagNeedle<'token> {
        let mut chars = token.chars();
        match chars.next() {
            None => {
                FlagNeedle {
                    needle: token,
                    bare:   true,
                }
            },
            Some('-') => {
                FlagNeedle {
                    needle: token,
                    bare:   false,
                }
            },
            Some(_) => {
                FlagNeedle {
                    needle: chars.as_str(),
                    bare:   true,
                }
            },
        }
    }
}

/// `bare` matches `needle` against the stripped flag name, else the dashed
/// form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlagNeedle<'token> {
    pub needle: &'token str,
    pub bare:   bool,
}

fn parse_bool(raw: &str) -> bool {
    matches!(
        raw.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn cfg_from(pairs: &[(&str, &str)]) -> Config {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        Config::from_lookup(|key| map.get(key).cloned())
    }

    #[test]
    fn defaults_match_historical_behaviour() {
        let cfg = Config::default();
        assert_eq!(cfg.flag_triggers, vec!['-']);
        assert!(!cfg.flag_on_empty);
        assert_eq!(cfg.max_completions, 0);
        assert_eq!(cfg.timeout_ms, DEFAULT_TIMEOUT_MS);
        assert_eq!(cfg.cache_ttl_secs, DEFAULT_CACHE_TTL_SECS);

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
        assert_eq!(cfg.flag_needle("--ver"), FlagNeedle {
            needle: "--ver",
            bare:   false,
        });
        assert_eq!(cfg.flag_needle("+ver"), FlagNeedle {
            needle: "ver",
            bare:   true,
        });
        assert_eq!(cfg.flag_needle(""), FlagNeedle {
            needle: "",
            bare:   true,
        });
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
            ("INSHELLAH_CACHE_TTL_SECS", "3600"),
        ]);
        assert_eq!(cfg.max_completions, 50);
        assert_eq!(cfg.timeout_ms, 1000);
        assert_eq!(cfg.cache_ttl_secs, 3600);

        let zeroed = cfg_from(&[("INSHELLAH_CACHE_TTL_SECS", "0")]);
        assert_eq!(zeroed.cache_ttl_secs, 0);

        let bad = cfg_from(&[
            ("INSHELLAH_MAX_COMPLETIONS", "lots"),
            ("INSHELLAH_TIMEOUT_MS", "soon"),
            ("INSHELLAH_CACHE_TTL_SECS", "never"),
        ]);
        assert_eq!(bad.max_completions, 0);
        assert_eq!(bad.timeout_ms, DEFAULT_TIMEOUT_MS);
        assert_eq!(bad.cache_ttl_secs, DEFAULT_CACHE_TTL_SECS);

        // u64 parse rejects negatives, no explicit guard needed.
        let negative = cfg_from(&[("INSHELLAH_CACHE_TTL_SECS", "-1")]);
        assert_eq!(negative.cache_ttl_secs, DEFAULT_CACHE_TTL_SECS);
    }
}
