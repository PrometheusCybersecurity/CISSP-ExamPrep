//! Anthropic streaming messages.

use super::{ChatMessage, LlmError, LlmEvent};
use async_stream::stream;
use futures_util::{Stream, StreamExt};
use serde::Serialize;
use serde_json::json;

#[derive(Debug, Clone, Serialize)]
pub struct AnthropicOptions<'a> {
    pub model: &'a str,
    pub max_tokens: u32,
    pub system: &'a str,
}

pub fn stream_chat<'a>(
    http: reqwest::Client,
    api_key: String,
    opts: AnthropicOptions<'a>,
    messages: Vec<ChatMessage>,
) -> impl Stream<Item = LlmEvent> + 'a {
    let model = opts.model.to_string();
    let max_tokens = opts.max_tokens;
    let system = opts.system.to_string();

    stream! {
        // Anthropic only accepts user/assistant turns; no system role in the array.
        let messages: Vec<_> = messages.into_iter()
            .filter(|m| m.role == "user" || m.role == "assistant")
            .collect();

        let body = json!({
            "model": model,
            "max_tokens": max_tokens,
            "system": system,
            "messages": messages.iter().map(|m| json!({"role": m.role, "content": m.content})).collect::<Vec<_>>(),
            "stream": true,
        });

        let res = match http
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                yield LlmEvent::Error { message: format!("network: {e}") };
                return;
            }
        };

        if !res.status().is_success() {
            let status = res.status().as_u16();
            let txt = res.text().await.unwrap_or_default();
            yield LlmEvent::Error {
                message: format!("Anthropic HTTP {status}: {}", truncate(&txt, 400)),
            };
            return;
        }

        let mut byte_stream = res.bytes_stream();
        let mut buf = String::new();

        while let Some(chunk) = byte_stream.next().await {
            match chunk {
                Ok(bytes) => buf.push_str(&String::from_utf8_lossy(&bytes)),
                Err(e) => {
                    yield LlmEvent::Error { message: format!("stream read: {e}") };
                    return;
                }
            }
            while let Some(idx) = buf.find('\n') {
                let line = buf[..idx].trim_end_matches('\r').to_string();
                buf.drain(..=idx);
                let payload = match line.strip_prefix("data: ") {
                    Some(p) => p.trim().to_string(),
                    None => continue,
                };
                if payload.is_empty() {
                    continue;
                }
                let v: serde_json::Value = match serde_json::from_str(&payload) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let kind = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
                match kind {
                    "content_block_delta" => {
                        if let Some(text) = v
                            .get("delta")
                            .and_then(|d| d.get("text"))
                            .and_then(|t| t.as_str())
                        {
                            if !text.is_empty() {
                                yield LlmEvent::Token { text: text.to_string() };
                            }
                        }
                    }
                    "message_stop" => {
                        yield LlmEvent::Done;
                        return;
                    }
                    "error" => {
                        let m = v
                            .get("error")
                            .and_then(|e| e.get("message"))
                            .and_then(|m| m.as_str())
                            .unwrap_or("unknown error")
                            .to_string();
                        yield LlmEvent::Error { message: m };
                        return;
                    }
                    _ => {}
                }
            }
        }
        yield LlmEvent::Done;
    }
}

pub async fn complete_chat<'a>(
    http: reqwest::Client,
    api_key: String,
    opts: AnthropicOptions<'a>,
    messages: Vec<ChatMessage>,
    on_progress: impl Fn(&str) + Send,
) -> Result<String, LlmError> {
    let s = stream_chat(http, api_key, opts, messages);
    futures_util::pin_mut!(s);
    let mut out = String::new();
    while let Some(ev) = s.next().await {
        match ev {
            LlmEvent::Token { text } => {
                out.push_str(&text);
                on_progress(&out);
            }
            LlmEvent::Done => return Ok(out),
            LlmEvent::Error { message } => return Err(LlmError::Network(message)),
        }
    }
    Ok(out)
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n { s.to_string() } else { format!("{}…", &s[..n]) }
}
