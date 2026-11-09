//! OpenAI-compatible streaming client for llama-server (`/v1/chat/completions`).
//! Streaming lets us show text as it is generated and lets a superseded request
//! be cancelled simply by dropping the future (llama-server stops generating
//! when the connection closes).

use anyhow::{anyhow, Result};
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::time::Duration;
use wuxia_core::prompt::Message;

use crate::config::ModelPreset;

#[derive(Clone)]
pub struct LlmClient {
    http: reqwest::Client,
    base: String,
}

impl LlmClient {
    pub fn new(base: &str, timeout_s: u64) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(timeout_s.max(5)))
            .connect_timeout(Duration::from_secs(3))
            .no_proxy() // never route localhost traffic through a system proxy
            .build()
            .expect("reqwest client");
        Self { http, base: base.trim_end_matches('/').to_string() }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    pub async fn health(&self) -> Result<bool> {
        let r = self.http.get(format!("{}/health", self.base)).timeout(Duration::from_secs(2)).send().await?;
        Ok(r.status().is_success())
    }

    /// Stream a chat completion. `on_delta` receives the text accumulated so far.
    /// A stream that breaks off before the model finished is an error; a
    /// reply cut by the token limit comes back with `truncated` set (shown,
    /// but never cached).
    pub async fn chat(&self, messages: &[Message], p: &ModelPreset, mut on_delta: impl FnMut(&str)) -> Result<Reply> {
        let msgs: Vec<Value> = messages.iter().map(|m| json!({"role": m.role, "content": m.content})).collect();
        let mut body = json!({
            "messages": msgs,
            "stream": true,
            "temperature": p.temperature,
            "top_p": p.top_p,
            "top_k": p.top_k,
            "repeat_penalty": p.repeat_penalty,
            "max_tokens": p.max_tokens,
            "cache_prompt": true,
        });
        if p.no_think {
            body["chat_template_kwargs"] = json!({"enable_thinking": false});
        }
        let url = format!("{}/v1/chat/completions", self.base);
        let resp = match self.http.post(&url).json(&body).send().await {
            Ok(r) => r,
            // llama-server may close a kept-alive connection right as we reuse it
            // ("error sending request" in logs): one quick retry on a fresh one.
            Err(e) if e.is_request() || e.is_connect() => {
                tracing::debug!("llama-server: повтор запроса после «{e}»");
                tokio::time::sleep(Duration::from_millis(60)).await;
                self.http.post(&url).json(&body).send().await?
            }
            Err(e) => return Err(e.into()),
        };
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("llama-server {status}: {}", text.chars().take(300).collect::<String>()));
        }
        let mut stream = resp.bytes_stream();
        let mut buf: Vec<u8> = Vec::new();
        let mut acc = Accumulator::default();
        while let Some(chunk) = stream.next().await {
            buf.extend_from_slice(&chunk?);
            while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = buf.drain(..=pos).collect();
                match acc.feed(&String::from_utf8_lossy(&line))? {
                    Step::Delta => on_delta(&acc.text),
                    Step::Done => return acc.finish(),
                    Step::None => {}
                }
            }
        }
        acc.finish()
    }
}

/// A finished reply of the model.
#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub text: String,
    /// Stopped by the token limit (`finish_reason: length`), not by the model.
    pub truncated: bool,
}

/// What one line of the server-sent event stream did.
#[derive(Debug, PartialEq)]
enum Step {
    None,
    Delta,
    Done,
}

/// Collects a streamed reply line by line (`data: {...}` events of the
/// OpenAI-compatible API) and knows whether it ended properly.
#[derive(Default)]
struct Accumulator {
    text: String,
    finish: Option<String>,
    done: bool,
}

impl Accumulator {
    fn feed(&mut self, line: &str) -> Result<Step> {
        let Some(data) = line.trim().strip_prefix("data:") else { return Ok(Step::None) };
        let data = data.trim();
        if data == "[DONE]" {
            self.done = true;
            return Ok(Step::Done);
        }
        let Ok(v) = serde_json::from_str::<Value>(data) else { return Ok(Step::None) };
        if let Some(err) = v.get("error") {
            return Err(anyhow!("llama-server: {err}"));
        }
        if let Some(r) = v["choices"][0]["finish_reason"].as_str() {
            self.finish = Some(r.to_string());
        }
        match v["choices"][0]["delta"]["content"].as_str() {
            Some(d) if !d.is_empty() => {
                self.text.push_str(d);
                Ok(Step::Delta)
            }
            _ => Ok(Step::None),
        }
    }

    /// The stream is over: complete, cut by the limit, or broken off.
    fn finish(self) -> Result<Reply> {
        match self.finish.as_deref() {
            Some("length") => Ok(Reply { text: self.text, truncated: true }),
            Some(_) => Ok(Reply { text: self.text, truncated: false }),
            None if self.done => Ok(Reply { text: self.text, truncated: false }),
            None => Err(anyhow!("ответ модели оборвался на полуслове (соединение закрыто)")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(lines: &[&str]) -> Result<Reply> {
        let mut a = Accumulator::default();
        for l in lines {
            if a.feed(l)? == Step::Done {
                break;
            }
        }
        a.finish()
    }

    #[test]
    fn complete_reply() {
        let r = run(&[
            r#"data: {"choices":[{"delta":{"content":"При"}}]}"#,
            "",
            r#"data: {"choices":[{"delta":{"content":"вет"}}]}"#,
            r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
            "data: [DONE]",
        ])
        .unwrap();
        assert_eq!(r, Reply { text: "Привет".into(), truncated: false });
    }

    #[test]
    fn cut_by_token_limit() {
        let r = run(&[r#"data: {"choices":[{"delta":{"content":"Длинный"}}]}"#, r#"data: {"choices":[{"delta":{},"finish_reason":"length"}]}"#, "data: [DONE]"]).unwrap();
        assert!(r.truncated);
    }

    #[test]
    fn broken_stream_is_an_error() {
        // The server went away mid-answer: no finish_reason, no [DONE].
        assert!(run(&[r#"data: {"choices":[{"delta":{"content":"Обры"}}]}"#]).is_err());
        // [DONE] without a finish reason (some servers): the server said it is over.
        assert!(run(&[r#"data: {"choices":[{"delta":{"content":"Да"}}]}"#, "data: [DONE]"]).is_ok());
    }

    #[test]
    fn server_error_in_stream() {
        assert!(run(&[r#"data: {"error":{"message":"out of memory"}}"#]).is_err());
    }

    #[test]
    fn ignores_noise() {
        let r = run(&[": keep-alive", "event: ping", "data: not json", r#"data: {"choices":[{"delta":{"content":"Да"},"finish_reason":"stop"}]}"#]).unwrap();
        assert_eq!(r.text, "Да");
    }
}
