//! A stand-in for the `claude` CLI that speaks just enough stream-json for
//! tests. Tests rely on this behavior exactly; see Linear VRU2-245.

use std::io::{self, BufRead, Write};
use std::process::ExitCode;

use serde_json::{Value, json};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version") {
        println!("9.9.9 (Claude Code)");
        return ExitCode::SUCCESS;
    }
    let session_id = args
        .iter()
        .position(|a| a == "--resume")
        .and_then(|i| args.get(i + 1))
        .map_or("fake-session-1", String::as_str);

    let stdout = io::stdout();
    let mut out = stdout.lock();
    let mut initialized = false;
    let streams = args.iter().any(|a| a == "--include-partial-messages");

    for line in io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let Some(text) = user_text(&line) else {
            continue;
        };

        if !initialized {
            initialized = true;
            emit(
                &mut out,
                &json!({"type": "system", "subtype": "init", "session_id": session_id, "model": "fake"}),
            );
        }
        let reply = match text.as_str() {
            "crash" => {
                eprintln!("boom");
                return ExitCode::from(1);
            }
            "garbage" => {
                let _ = writeln!(out, "not json");
                format!("echo: {text}")
            }
            "args" => args.join(" "),
            _ => format!("echo: {text}"),
        };
        if text == "think" {
            emit(
                &mut out,
                &json!({"type": "stream_event", "event": {"type": "content_block_delta", "index": 0,
                        "delta": {"type": "thinking_delta", "thinking": "pondering"}}}),
            );
            emit(
                &mut out,
                &json!({"type": "assistant", "message": {"content": [{"type": "thinking", "thinking": "pondering", "signature": "x"}]}}),
            );
        }
        if streams {
            // Like Claude Code: the text in pieces, then the whole block.
            let mid = reply
                .char_indices()
                .nth(reply.chars().count() / 2)
                .map_or(0, |(ix, _)| ix);
            for piece in [&reply[..mid], &reply[mid..]] {
                emit(
                    &mut out,
                    &json!({"type": "stream_event", "event": {"type": "content_block_delta", "index": 0,
                            "delta": {"type": "text_delta", "text": piece}}}),
                );
            }
        }
        emit(
            &mut out,
            &json!({"type": "assistant", "message": {"content": [{"type": "text", "text": reply}]}}),
        );
        emit(
            &mut out,
            &json!({"type": "result", "subtype": "success", "is_error": false, "result": reply, "total_cost_usd": 0.001, "duration_ms": 1}),
        );
    }
    ExitCode::SUCCESS
}

/// The text of a stream-json user message, or `None` for anything else.
fn user_text(line: &str) -> Option<String> {
    let value: Value = serde_json::from_str(line).ok()?;
    value
        .pointer("/message/content")?
        .as_array()?
        .iter()
        .find(|block| block.get("type").and_then(Value::as_str) == Some("text"))?
        .get("text")?
        .as_str()
        .map(str::to_owned)
}

fn emit(out: &mut impl Write, value: &Value) {
    let _ = writeln!(out, "{value}");
    let _ = out.flush();
}
