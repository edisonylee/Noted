// Thin client for the local Ollama server (http://localhost:11434).
// All inference is local + free. We use /api/chat with a JSON-schema `format`
// for structured extraction, and /api/embed for semantic search (M3).

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

const BASE: &str = "http://localhost:11434";

pub const TEXT_MODEL: &str = "qwen2.5:7b-instruct";
pub const VISION_MODEL: &str = "qwen2.5vl:7b";
pub const EMBED_MODEL: &str = "nomic-embed-text";

/// Which models are pulled — used for the M0 health check / setup screen.
pub async fn tags() -> Result<Value> {
    let resp = reqwest::get(format!("{BASE}/api/tags")).await?;
    Ok(resp.json::<Value>().await?)
}

/// Single structured chat turn. `images` are base64 strings (vision path).
/// `format` is a JSON Schema that constrains decoding; the returned content is
/// parsed as JSON. temperature 0 for determinism.
pub async fn chat_json(
    model: &str,
    system: &str,
    user: &str,
    images: Option<Vec<String>>,
    format: Option<Value>,
) -> Result<Value> {
    // Balanced mode: route the latency-sensitive structured calls (note extract
    // + photo OCR) to Gemini. On any failure (bad key, offline, rate limit) we
    // fall through to the local model so capture never hard-fails.
    if crate::provider::use_cloud_for_extract() {
        let vision = images.as_ref().map(|v| !v.is_empty()).unwrap_or(false);
        match crate::provider::gemini_chat_json(system, user, images.clone(), format.clone(), vision).await {
            Ok(v) => return Ok(v),
            Err(e) => eprintln!("[noted] gemini extract failed, falling back to local: {e}"),
        }
    }

    let has_images = images.as_ref().map(|v| !v.is_empty()).unwrap_or(false);
    let mut user_msg = json!({ "role": "user", "content": user });
    if let Some(imgs) = images {
        user_msg["images"] = json!(imgs);
    }

    // Ollama defaults num_ctx to 2048, which a single photo's image tokens blow
    // straight past (llama.cpp then 400s with "exceeds the available context
    // size"). Give vision OCR generous headroom; text extraction needs less.
    let num_ctx = if has_images { 16384 } else { 8192 };

    let mut body = json!({
        "model": model,
        "stream": false,
        "keep_alive": "5m",
        // A small non-zero temperature avoids two temp-0 failure modes: degenerate
        // JSON loops and anchoring every note onto the first listed category.
        "options": { "temperature": 0.3, "num_ctx": num_ctx },
        "messages": [
            { "role": "system", "content": system },
            user_msg,
        ],
    });
    // When a schema is supplied, constrain decoding; otherwise just ask for JSON
    // (used for free-form emergent `data` extraction).
    body["format"] = match format {
        Some(schema) => schema,
        None => json!("json"),
    };

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{BASE}/api/chat"))
        .json(&body)
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow!("ollama /api/chat {status}: {text}"));
    }

    let v: Value = resp.json().await?;
    let content = v
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .ok_or_else(|| anyhow!("no message.content in ollama response"))?;

    serde_json::from_str(content)
        .map_err(|e| anyhow!("model returned non-JSON content: {e}\n---\n{content}"))
}

/// Plain prose chat turn (no JSON constraint) — used for grounded RAG answers.
pub async fn chat_text(model: &str, system: &str, user: &str) -> Result<String> {
    let body = json!({
        "model": model,
        "stream": false,
        "keep_alive": "5m",
        "options": { "temperature": 0.2, "num_ctx": 8192 },
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user },
        ],
    });
    let client = reqwest::Client::new();
    let resp = client.post(format!("{BASE}/api/chat")).json(&body).send().await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow!("ollama /api/chat {status}: {text}"));
    }
    let v: Value = resp.json().await?;
    Ok(v.get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string())
}

/// Multi-message chat turn — used by the conversational Q&A assistant so prior
/// turns are remembered. `messages` is a full Ollama messages array.
pub async fn chat_messages(model: &str, messages: Vec<Value>, temperature: f32) -> Result<String> {
    let body = json!({
        "model": model,
        "stream": false,
        "keep_alive": "5m",
        "options": { "temperature": temperature, "num_ctx": 8192 },
        "messages": messages,
    });
    let client = reqwest::Client::new();
    let resp = client.post(format!("{BASE}/api/chat")).json(&body).send().await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow!("ollama /api/chat {status}: {text}"));
    }
    let v: Value = resp.json().await?;
    Ok(v.get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string())
}

/// Embed text with nomic-embed-text (768-dim). Used by semantic search (M3).
pub async fn embed(input: &str) -> Result<Vec<f32>> {
    // keep_alive -1 (a NUMBER, not "-1") keeps the tiny embed model resident.
    let body = json!({ "model": EMBED_MODEL, "input": input, "keep_alive": -1 });
    let client = reqwest::Client::new();
    let v: Value = client
        .post(format!("{BASE}/api/embed"))
        .json(&body)
        .send()
        .await?
        .json()
        .await?;
    let arr = v
        .get("embeddings")
        .and_then(|e| e.get(0))
        .and_then(|e| e.as_array())
        .ok_or_else(|| anyhow!("no embeddings in response"))?;
    Ok(arr.iter().filter_map(|x| x.as_f64().map(|f| f as f32)).collect())
}
