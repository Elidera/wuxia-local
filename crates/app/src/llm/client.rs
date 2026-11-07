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
    pub async fn chat(&self, messages: &[Message], p: &ModelPreset, mut on_delta: impl FnMut(&str)) -> Result<String> {
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
        let resp = self.http.post(format!("{}/v1/chat/completions", self.base)).json(&body).send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("llama-server {status}: {}", text.chars().take(300).collect::<String>()));
        }
        let mut stream = resp.bytes_stream();
        let mut buf: Vec<u8> = Vec::new();
        let mut out = String::new();
        while let Some(chunk) = stream.next().await {
            buf.extend_from_slice(&chunk?);
            while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = buf.drain(..=pos).collect();
                let line = String::from_utf8_lossy(&line);
                let line = line.trim();
                let Some(data) = line.strip_prefix("data:") else { continue };
                let data = data.trim();
                if data == "[DONE]" {
                    return Ok(out);
                }
                let v: Value = match serde_json::from_str(data) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if let Some(err) = v.get("error") {
                    return Err(anyhow!("llama-server: {err}"));
                }
                if let Some(delta) = v["choices"][0]["delta"]["content"].as_str() {
                    if !delta.is_empty() {
                        out.push_str(delta);
                        on_delta(&out);
                    }
                }
            }
        }
        Ok(out)
    }
}
