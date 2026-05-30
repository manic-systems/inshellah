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
        if $completed.exit_code != 0 {
            null
        } else {
            let parsed = (try { $completed.stdout | from json } catch { null })
            if $parsed == null {
                null
            } else {
                let parsed_type = ($parsed | describe)
                if (($parsed_type | str starts-with "list") or ($parsed_type | str starts-with "table")) {
                    if ($parsed | is-empty) { null } else { $parsed }
                } else {
                    null
                }
            }
        }
    } catch {
        null
    }
}

def inshellah-complete-commandline [line: string, cursor: int] {
    let prefix = if $cursor <= ($line | str length) {
        $line | str substring 0..<$cursor
    } else {
        $line
    }
    let current = ($prefix | split row "\n" | last)
    let spans = ($current | split row " ")
    inshellah-complete-spans $spans
}

let inshellah_complete = { |spans| inshellah-complete-spans $spans }

$env.config.completions.external = {enable: true, max_results: $inshellah_max_results, completer: $inshellah_complete}
