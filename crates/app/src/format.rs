//! Short, readable figures for time, tokens and cost.

use agent_desk_core::thread::{TokenCount, TurnStats};

/// `870` → "0.9s", `12_400` → "12s", `152_100` → "2m 32s".
pub fn duration(ms: u64) -> String {
    if ms < 10_000 {
        format!("{:.1}s", ms as f64 / 1_000.0)
    } else {
        whole_seconds(ms / 1_000)
    }
}

/// `7` → "7s", `152` → "2m 32s"; for a clock that ticks each second.
pub fn whole_seconds(seconds: u64) -> String {
    if seconds >= 60 {
        format!("{}m {}s", seconds / 60, seconds % 60)
    } else {
        format!("{seconds}s")
    }
}

/// `812` → "812", `43_120` → "43.1k", `1_260_000` → "1.3M".
pub fn tokens(count: u64) -> String {
    let scaled = |value: f64, unit: &str| {
        let text = format!("{value:.1}");
        format!("{}{unit}", text.strip_suffix(".0").unwrap_or(&text))
    };
    match count {
        0..1_000 => count.to_string(),
        1_000..999_950 => scaled(count as f64 / 1_000.0, "k"),
        _ => scaled(count as f64 / 1_000_000.0, "M"),
    }
}

/// `0.5357` → "$0.54"; amounts under a cent read "<$0.01".
pub fn cost(usd: f64) -> String {
    if usd > 0.0 && usd < 0.01 {
        "<$0.01".to_owned()
    } else {
        format!("${usd:.2}")
    }
}

/// "12s · 43.1k in · 2.8k out · $0.54"; tokens and cost appear when known.
pub fn turn_summary(stats: &TurnStats) -> String {
    let mut parts = vec![duration(stats.duration_ms)];
    parts.extend(token_parts(stats.tokens));
    if let Some(usd) = stats.cost_usd {
        parts.push(cost(usd));
    }
    parts.join(" · ")
}

/// "Working · 12s · 43.1k in · 1.2k out" for a turn still running.
pub fn turn_progress(elapsed_seconds: u64, used: TokenCount) -> String {
    let mut parts = vec!["Working".to_owned(), whole_seconds(elapsed_seconds)];
    parts.extend(token_parts(used));
    parts.join(" · ")
}

fn token_parts(used: TokenCount) -> Vec<String> {
    if used == TokenCount::default() {
        return Vec::new();
    }
    vec![
        format!("{} in", tokens(used.input)),
        format!("{} out", tokens(used.output)),
    ]
}

#[cfg(test)]
mod tests {
    use agent_desk_core::thread::{TokenCount, TurnStats};

    use super::{cost, duration, tokens, turn_progress, turn_summary};

    #[test]
    fn durations_read_naturally() {
        assert_eq!(duration(870), "0.9s");
        assert_eq!(duration(4_380), "4.4s");
        assert_eq!(duration(12_400), "12s");
        assert_eq!(duration(152_100), "2m 32s");
    }

    #[test]
    fn token_counts_are_short() {
        assert_eq!(tokens(812), "812");
        assert_eq!(tokens(1_000), "1k");
        assert_eq!(tokens(43_120), "43.1k");
        assert_eq!(tokens(999_999), "1M");
        assert_eq!(tokens(1_260_000), "1.3M");
    }

    #[test]
    fn costs_round_to_cents() {
        assert_eq!(cost(0.5357), "$0.54");
        assert_eq!(cost(0.003), "<$0.01");
        assert_eq!(cost(0.0), "$0.00");
    }

    #[test]
    fn a_turn_summary_leaves_out_what_is_unknown() {
        let mut stats = TurnStats {
            duration_ms: 12_400,
            tokens: TokenCount {
                input: 43_120,
                output: 2_800,
            },
            cost_usd: Some(0.5357),
        };
        assert_eq!(turn_summary(&stats), "12s · 43.1k in · 2.8k out · $0.54");
        stats.cost_usd = None;
        stats.tokens = TokenCount::default();
        assert_eq!(turn_summary(&stats), "12s");
    }

    #[test]
    fn a_running_turn_counts_whole_seconds() {
        assert_eq!(turn_progress(4, TokenCount::default()), "Working · 4s");
        let used = TokenCount {
            input: 43_120,
            output: 1_200,
        };
        assert_eq!(
            turn_progress(152, used),
            "Working · 2m 32s · 43.1k in · 1.2k out"
        );
    }
}
