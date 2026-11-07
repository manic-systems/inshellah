@complete external
def --wrapped sudo [...args] {
    ^sudo ...$args
}

@complete external
def --wrapped doas [...args] {
    ^doas ...$args
}

# nushell's own cap on how many external completions it will display.
# mirrors the Rust completer's INSHELLAH_MAX_COMPLETIONS cap so both ends
# agree. 0 (or unset) keeps the historical default of 200.
let inshellah_default_max_results = 200

let inshellah_max_results = do {
    let raw = (try {
        $env.INSHELLAH_MAX_COMPLETIONS? | default 0 | into int
    } catch { 0 })
    if $raw > 0 { $raw } else { $inshellah_default_max_results }
}

def inshellah-complete-spans [spans: list<string>] {
    try {
        let completed = (^inshellah complete ...$spans | complete)
        if $completed.exit_code != 0 { return null }
        let parsed = ($completed.stdout | from json)
        let kind = { describe | str replace --regex '<.*' '' }
        match ($parsed | do $kind) {
            list | table => $parsed
            record if ($parsed.completions? | do $kind) in [list table] => $parsed
            _ => null
        }
    } catch {
        null
    }
}

let inshellah_complete = { |place| inshellah-complete-spans $place.command }

$env.config.completions.external = {enable: true, max_results: $inshellah_max_results, completer: $inshellah_complete}
