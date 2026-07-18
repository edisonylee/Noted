use std::process::Command;
use std::time::Duration;

use anyhow::{anyhow, Result};
use serde_json::Value;

pub const BASE_URL: &str = "https://api.entersymphony.com";
pub const TEXT_MODEL: &str = "gemma3:4b";
pub const VISION_MODEL: &str = "gemma3:4b";
pub const EMBED_MODEL: &str = "nomic-embed-text";
const KEYCHAIN_SERVICE: &str = "com.noted.app";
const KEYCHAIN_ACCOUNT: &str = "hosted_api_key";

pub fn key() -> Option<String> {
    let out = Command::new("security")
        .args(["find-generic-password", "-s", KEYCHAIN_SERVICE, "-a", KEYCHAIN_ACCOUNT, "-w"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!value.is_empty()).then_some(value)
}

pub fn has_key() -> bool {
    key().is_some()
}

pub fn write_key(value: &str) -> Result<()> {
    let status = Command::new("security")
        .args([
            "add-generic-password", "-U", "-s", KEYCHAIN_SERVICE, "-a", KEYCHAIN_ACCOUNT,
            "-w", value,
        ])
        .status()?;
    if status.success() { Ok(()) } else { Err(anyhow!("failed to store hosted API key")) }
}

pub fn delete_key() {
    let _ = Command::new("security")
        .args(["delete-generic-password", "-s", KEYCHAIN_SERVICE, "-a", KEYCHAIN_ACCOUNT])
        .status();
}

pub fn wav_bytes(samples: &[f32]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&16_000u32.to_le_bytes());
    out.extend_from_slice(&32_000u32.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        out.extend_from_slice(&((sample.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    out
}

fn mime_for_image(b64: &str) -> &'static str {
    if b64.starts_with("iVBOR") { "image/png" }
    else if b64.starts_with("R0lGOD") { "image/gif" }
    else if b64.starts_with("UklGR") { "image/webp" }
    else { "image/jpeg" }
}

fn parse_json_content(content: &str) -> Result<Value> {
    let trimmed = content.trim();
    let trimmed = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .map(|s| s.strip_suffix("```").unwrap_or(s))
        .unwrap_or(trimmed)
        .trim();
    serde_json::from_str(trimmed).map_err(|e| anyhow!("hosted model returned invalid JSON: {e}"))
}

fn async_client(timeout_secs: u64) -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder().timeout(Duration::from_secs(timeout_secs)).build()?)
}

pub async fn models() -> Result<Value> {
    let key = key().ok_or_else(|| anyhow!("hosted API key is not configured"))?;
    let response = async_client(30)?.get(format!("{BASE_URL}/v1/models"))
        .bearer_auth(key).send().await?;
    let status = response.status();
    let body: Value = response.json().await?;
    if !status.is_success() {
        return Err(anyhow!("hosted models failed ({status}): {}", error_message(&body)));
    }
    Ok(body)
}

pub async fn chat_json(
    system: &str,
    user: &str,
    images: Option<Vec<String>>,
    format: Option<Value>,
) -> Result<Value> {
    let user_content = match images {
        Some(images) if !images.is_empty() => {
            let mut parts = vec![serde_json::json!({"type":"text", "text":user})];
            for image in images {
                parts.push(serde_json::json!({
                    "type":"image_url",
                    "image_url":{"url":format!("data:{};base64,{image}", mime_for_image(&image))}
                }));
            }
            Value::Array(parts)
        }
        _ => Value::String(user.to_string()),
    };
    let response_format = match format {
        Some(schema) => serde_json::json!({"type":"json_schema", "json_schema":{"name":"noted_result", "schema":schema}}),
        None => serde_json::json!({"type":"json_object"}),
    };
    let body = serde_json::json!({
        "model": if user_content.is_array() { VISION_MODEL } else { TEXT_MODEL },
        "temperature": 0.3,
        "messages":[{"role":"system","content":system},{"role":"user","content":user_content}],
        "response_format":response_format
    });
    parse_chat_response(send_chat(body, 300).await?, true)
}

pub async fn chat_text(system: &str, user: &str) -> Result<String> {
    let body = serde_json::json!({
        "model":TEXT_MODEL, "temperature":0.2,
        "messages":[{"role":"system","content":system},{"role":"user","content":user}]
    });
    parse_chat_response(send_chat(body, 300).await?, false)?.as_str()
        .map(str::to_string).ok_or_else(|| anyhow!("hosted chat returned no content"))
}

pub async fn chat_messages(messages: Vec<Value>, temperature: f32) -> Result<String> {
    let body = serde_json::json!({"model":TEXT_MODEL,"temperature":temperature,"messages":messages});
    parse_chat_response(send_chat(body, 300).await?, false)?.as_str()
        .map(str::to_string).ok_or_else(|| anyhow!("hosted chat returned no content"))
}

async fn send_chat(body: Value, timeout_secs: u64) -> Result<Value> {
    let key = key().ok_or_else(|| anyhow!("hosted API key is not configured"))?;
    let response = async_client(timeout_secs)?.post(format!("{BASE_URL}/v1/chat/completions"))
        .bearer_auth(key).json(&body).send().await?;
    let status = response.status();
    let body: Value = response.json().await?;
    if !status.is_success() {
        return Err(anyhow!("hosted chat failed ({status}): {}", error_message(&body)));
    }
    Ok(body)
}

fn parse_chat_response(body: Value, json_mode: bool) -> Result<Value> {
    let content = body.pointer("/choices/0/message/content").and_then(Value::as_str)
        .ok_or_else(|| anyhow!("hosted chat response missing content"))?;
    if json_mode { parse_json_content(content) } else { Ok(Value::String(content.to_string())) }
}

pub async fn embed(input: &str) -> Result<Vec<f32>> {
    let key = key().ok_or_else(|| anyhow!("hosted API key is not configured"))?;
    let response = async_client(60)?.post(format!("{BASE_URL}/v1/embeddings"))
        .bearer_auth(key).json(&serde_json::json!({"model":EMBED_MODEL,"input":input})).send().await?;
    let status = response.status();
    let body: Value = response.json().await?;
    if !status.is_success() {
        return Err(anyhow!("hosted embedding failed ({status}): {}", error_message(&body)));
    }
    let values = body.pointer("/data/0/embedding").and_then(Value::as_array)
        .ok_or_else(|| anyhow!("hosted embedding response missing vector"))?;
    let vector: Vec<f32> = values.iter().filter_map(|v| v.as_f64().map(|n| n as f32)).collect();
    if vector.len() != 768 { return Err(anyhow!("hosted embedding dimension was {}, expected 768", vector.len())); }
    Ok(vector)
}

pub async fn test_connection() -> Result<String> {
    let catalog = models().await?;
    let ids: Vec<&str> = catalog["data"].as_array().into_iter().flatten()
        .filter_map(|m| m["id"].as_str()).collect();
    for required in [TEXT_MODEL, VISION_MODEL, EMBED_MODEL] {
        if !ids.contains(&required) { return Err(anyhow!("hosted API is missing required model {required}")); }
    }
    Ok("Connected to Noted Hosted (chat, vision, embeddings, and transcription).".into())
}

pub async fn transcribe_batch(samples: &[f32], vocabulary: &[String]) -> Result<String> {
    let key = key().ok_or_else(|| anyhow!("hosted API key is not configured"))?;
    let file = reqwest::multipart::Part::bytes(wav_bytes(samples))
        .file_name("speech.wav")
        .mime_str("audio/wav")?;
    let mut form = reqwest::multipart::Form::new().part("file", file).text("language", "en");
    if !vocabulary.is_empty() {
        form = form.text("vocabulary", vocabulary.join(","));
    }
    let response = reqwest::Client::builder()
        .timeout(Duration::from_secs(300))
        .build()?
        .post(format!("{BASE_URL}/v1/noted/transcribe"))
        .bearer_auth(key)
        .multipart(form)
        .send()
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    if !status.is_success() {
        return Err(anyhow!("hosted transcription failed ({status}): {}", error_message(&body)));
    }
    body["text"].as_str().map(str::to_string).ok_or_else(|| anyhow!("hosted transcription returned no text"))
}

fn error_message(body: &Value) -> &str {
    body.pointer("/error/message").and_then(Value::as_str).unwrap_or("unknown error")
}

pub struct Session {
    client: reqwest::blocking::Client,
    key: String,
    id: String,
    next_seq: u64,
    vocabulary: Vec<String>,
}

impl Session {
    pub fn open(vocabulary: Vec<String>) -> Result<Self> {
        let key = key().ok_or_else(|| anyhow!("hosted API key is not configured"))?;
        let client = reqwest::blocking::Client::builder().timeout(Duration::from_secs(60)).build()?;
        let response = client.post(format!("{BASE_URL}/v1/noted/transcribe/sessions"))
            .bearer_auth(&key).send()?;
        let status = response.status();
        let body: Value = response.json()?;
        if !status.is_success() {
            return Err(anyhow!("could not open hosted transcription session ({status}): {}", error_message(&body)));
        }
        let id = body["session_id"].as_str().ok_or_else(|| anyhow!("session response missing id"))?.to_string();
        Ok(Self { client, key, id, next_seq: 0, vocabulary })
    }

    pub fn transcribe(&mut self, samples: &[f32]) -> Result<String> {
        let seq = self.next_seq;
        let bytes = wav_bytes(samples);
        let mut last = None;
        for delay in [0, 2, 4, 8] {
            if delay > 0 { std::thread::sleep(Duration::from_secs(delay)); }
            let file = reqwest::blocking::multipart::Part::bytes(bytes.clone())
                .file_name(format!("chunk-{seq}.wav")).mime_str("audio/wav")?;
            let mut form = reqwest::blocking::multipart::Form::new()
                .part("file", file).text("seq", seq.to_string());
            if !self.vocabulary.is_empty() {
                form = form.text("vocabulary", self.vocabulary.join(","));
            }
            match self.client.post(format!("{BASE_URL}/v1/noted/transcribe/sessions/{}/chunks", self.id))
                .bearer_auth(&self.key).multipart(form).send() {
                Ok(response) => {
                    let status = response.status();
                    let retryable = status.as_u16() == 429 || status.as_u16() == 503;
                    let body: Value = response.json()?;
                    if status.is_success() {
                        self.next_seq += 1;
                        return Ok(body["text"].as_str().unwrap_or("").to_string());
                    }
                    last = Some(anyhow!("hosted chunk failed ({status}): {}", error_message(&body)));
                    if !retryable { break; }
                }
                Err(e) => last = Some(e.into()),
            }
        }
        Err(last.unwrap_or_else(|| anyhow!("hosted chunk failed")))
    }

    pub fn finalize(&self) {
        let _ = self.client.post(format!("{BASE_URL}/v1/noted/transcribe/sessions/{}/finalize", self.id))
            .bearer_auth(&self.key).send();
    }
}

#[cfg(test)]
mod tests {
    use super::wav_bytes;

    #[test]
    fn writes_pcm16_wav_header() {
        let wav = wav_bytes(&[0.0, 1.0, -1.0]);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 16_000);
        assert_eq!(wav.len(), 50);
    }
}
