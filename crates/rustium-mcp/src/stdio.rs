use std::io::{BufRead, Write};

use serde_json::{Value, json};
use ureq::Agent;

use crate::error::Error;

/// JSON-RPC "internal error", used when the editor cannot be reached.
const INTERNAL_ERROR: i64 = -32603;

/// Forwards MCP messages from `input` (one JSON-RPC message per line, as MCP stdio clients send them) to the
/// streamable HTTP endpoint of a running editor at `url`, and writes each answer to `output` as one line.
///
/// This is what `rustium --mcp-stdio` runs, so clients that can only spawn a process still talk to the editor
/// that is already open. When the editor is not reachable, requests are answered with a JSON-RPC error that
/// says so, instead of ending the session.
///
/// # Errors
///
/// [`Error::Stdio`] when reading `input` or writing `output` fails.
pub fn run_stdio_bridge(url: &str, input: impl BufRead, mut output: impl Write) -> Result<(), Error> {
    let agent: Agent = Agent::config_builder().http_status_as_error(false).build().into();
    let mut protocol_version: Option<String> = None;
    for line in input.lines() {
        let line = line.map_err(Error::Stdio)?;
        if line.trim().is_empty() {
            continue;
        }
        for answer in forward(&agent, url, &line, &mut protocol_version) {
            writeln!(output, "{answer}").map_err(Error::Stdio)?;
            output.flush().map_err(Error::Stdio)?;
        }
    }
    Ok(())
}

/// Sends one message and returns the lines to print (none for notifications).
fn forward(agent: &Agent, url: &str, message: &str, protocol_version: &mut Option<String>) -> Vec<String> {
    let parsed: Option<Value> = serde_json::from_str(message).ok();
    let id = parsed.as_ref().and_then(|value| value.get("id")).cloned();
    let is_initialize = parsed
        .as_ref()
        .is_some_and(|value| value.get("method").and_then(Value::as_str) == Some("initialize"));
    let per_request_version = parsed
        .as_ref()
        .and_then(|value| value.pointer("/params/_meta"))
        .and_then(|meta| meta.get("io.modelcontextprotocol/protocolVersion"))
        .and_then(Value::as_str)
        .map(str::to_owned);

    let mut request = agent
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream");
    if let Some(version) = per_request_version.as_ref().or(protocol_version.as_ref()) {
        request = request.header("MCP-Protocol-Version", version);
    }
    let response = match request.send(message) {
        Ok(response) => response,
        Err(error) => {
            return error_line(
                id,
                &format!("Rustium is not reachable at {url} ({error}). Open Rustium and switch the MCP server on."),
            );
        }
    };
    let status = response.status();
    let is_event_stream = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/event-stream"));
    let body = match response.into_body().read_to_string() {
        Ok(body) => body,
        Err(error) => return error_line(id, &format!("cannot read the answer from Rustium: {error}")),
    };

    let lines = if is_event_stream {
        event_data(&body)
    } else if body.trim().is_empty() {
        Vec::new()
    } else {
        vec![body.trim().to_owned()]
    };
    if is_initialize {
        for line in &lines {
            if let Some(version) = serde_json::from_str::<Value>(line).ok().and_then(|value| {
                value
                    .pointer("/result/protocolVersion")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            }) {
                *protocol_version = Some(version);
            }
        }
    }
    if lines.is_empty() && !status.is_success() {
        return error_line(id, &format!("Rustium answered with HTTP {status}"));
    }
    // One message per line: compact anything the server pretty-printed.
    lines
        .into_iter()
        .map(|line| serde_json::from_str::<Value>(&line).map_or(line, |value| value.to_string()))
        .collect()
}

/// The `data:` payload of every server-sent event in `body`.
fn event_data(body: &str) -> Vec<String> {
    let mut events = Vec::new();
    let mut data = Vec::new();
    for line in body.lines().chain(std::iter::once("")) {
        if let Some(rest) = line.strip_prefix("data:") {
            data.push(rest.strip_prefix(' ').unwrap_or(rest));
        } else if line.is_empty() && !data.is_empty() {
            let joined = data.join("\n");
            if !joined.trim().is_empty() {
                events.push(joined);
            }
            data.clear();
        }
    }
    events
}

/// A JSON-RPC error for `id`; nothing for a notification, which has no id and expects no answer.
fn error_line(id: Option<Value>, message: &str) -> Vec<String> {
    id.map(|id| json!({"jsonrpc": "2.0", "id": id, "error": {"code": INTERNAL_ERROR, "message": message}}).to_string())
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_stream_data_lines_become_messages() {
        let body = "id: 0\ndata:\n\nid: 1\ndata: {\"a\":1}\n\ndata: {\"b\":\ndata: 2}\n\n";
        assert_eq!(event_data(body), ["{\"a\":1}", "{\"b\":\n2}"]);
    }

    #[test]
    fn an_unreachable_editor_is_answered_with_a_json_rpc_error() {
        // Port 9 (discard) on localhost refuses connections on a normal machine.
        let input = b"{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"ping\"}\n{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n";
        let mut output = Vec::new();
        run_stdio_bridge("http://127.0.0.1:9/mcp", &input[..], &mut output).unwrap();
        let text = String::from_utf8(output).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 1, "notifications get no answer: {text}");
        let answer: Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(answer["id"], 7);
        assert!(answer["error"]["message"].as_str().unwrap().contains("not reachable"));
    }
}
