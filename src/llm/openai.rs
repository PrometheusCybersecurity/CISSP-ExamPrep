//! OpenAI streaming chat completions.

use super::{ChatMessage, LlmError, LlmEvent};
use async_stream::stream;
use futures_util::{Stream, StreamExt};
use serde::Serialize;
use serde_json::json;

#[derive(Debug, Clone, Serialize)]
pub struct OpenAiOptions<'a> {
    pub model: &'a str,
    pub max_tokens: u32,
    pub temperature: f32,
    pub json_mode: bool,
}

/// Returns a `Stream<Item = LlmEvent>`. Always ends with `Done` or `Error`.
pub fn stream_chat<'a>(
    http: reqwest::Client,
    api_key: String,
    opts: OpenAiOptions<'a>,
    messages: Vec<ChatMessage>,
) -> impl Stream<Item = LlmEvent> + 'a {
    let model = opts.model.to_string();
    let max_tokens = opts.max_tokens;
    let temperature = opts.temperature;
    let json_mode = opts.json_mode;

    stream! {
        let mut body = json!({
            "model": model,
            "messages": messages.iter().map(|m| json!({"role": m.role, "content": m.content})).collect::<Vec<_>>(),
            "stream": true,
            "max_tokens": max_tokens,
            "temperature": temperature,
        });
        if json_mode {
            body["response_format"] = json!({"type": "json_object"});
        }

        let res = match http
            .post("https://api.openai.com/v1/chat/completions")
            .bearer_auth(&api_key)
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
                message: format!("OpenAI HTTP {status}: {}", truncate(&txt, 400)),
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

            // Process complete SSE lines (terminated by \n).
            while let Some(idx) = buf.find('\n') {
                let line = buf[..idx].trim_end_matches('\r').to_string();
                buf.drain(..=idx);

                let payload = match line.strip_prefix("data: ") {
                    Some(p) => p.trim().to_string(),
                    None => continue,
                };
                if payload == "[DONE]" {
                    yield LlmEvent::Done;
                    return;
                }
                if payload.is_empty() {
                    continue;
                }
                let v: serde_json::Value = match serde_json::from_str(&payload) {
                    Ok(v) => v,
                    Err(_) => continue, // skip malformed
                };
                if let Some(text) = v
                    .get("choices")
                    .and_then(|c| c.get(0))
                    .and_then(|c| c.get("delta"))
                    .and_then(|d| d.get("content"))
                    .and_then(|c| c.as_str())
                {
                    if !text.is_empty() {
                        yield LlmEvent::Token { text: text.to_string() };
                    }
                }
            }
        }
        yield LlmEvent::Done;
    }
}

/// Convenience: collect the streamed response into a single string.
pub async fn complete_chat<'a>(
    http: reqwest::Client,
    api_key: String,
    opts: OpenAiOptions<'a>,
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
