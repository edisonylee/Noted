//! Opt-in provider contract checks. These consume provider quota and are
//! ignored by default. Run one explicitly, for example:
//! `OPENAI_API_KEY=... cargo test --test byok_live openai_live -- --ignored`

use tauri_app_lib::provider::{self, ByokConfig, CapabilityChoice, ProviderId};

fn choice(provider: ProviderId, model: &str, base_url: &str) -> CapabilityChoice {
    CapabilityChoice {
        provider,
        model: model.into(),
        base_url: base_url.into(),
    }
}

fn passed(results: &serde_json::Value, capability: &str) {
    let status = results
        .get(capability)
        .and_then(|v| v.as_str())
        .unwrap_or("missing");
    assert!(status.starts_with("passed"), "{capability}: {status}");
}

#[tokio::test]
#[ignore = "requires OPENAI_API_KEY and consumes quota"]
async fn openai_live() {
    let key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY");
    let results =
        provider::test_byok_candidate(ByokConfig::default(), Some(key), None, None, None, None)
            .await;
    for capability in ["intelligence", "vision", "embeddings", "transcription"] {
        passed(&results, capability);
    }
}

#[tokio::test]
#[ignore = "requires GEMINI_API_KEY and consumes quota"]
async fn gemini_live() {
    let key = std::env::var("GEMINI_API_KEY").expect("GEMINI_API_KEY");
    let mut config = ByokConfig::default();
    config.intelligence = choice(ProviderId::Gemini, "gemini-2.5-flash", "");
    config.vision = config.intelligence.clone();
    config.embeddings = choice(ProviderId::Gemini, "gemini-embedding-2", "");
    config.transcription = config.intelligence.clone();
    let results = provider::test_byok_candidate(config, None, Some(key), None, None, None).await;
    for capability in ["intelligence", "vision", "embeddings", "transcription"] {
        passed(&results, capability);
    }
}

#[tokio::test]
#[ignore = "requires ANTHROPIC_API_KEY plus OPENAI_API_KEY for unsupported slots"]
async fn anthropic_live() {
    let anthropic = std::env::var("ANTHROPIC_API_KEY").expect("ANTHROPIC_API_KEY");
    let openai = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY");
    let mut config = ByokConfig::default();
    config.intelligence = choice(ProviderId::Anthropic, "claude-sonnet-4-5", "");
    config.vision = config.intelligence.clone();
    let results =
        provider::test_byok_candidate(config, Some(openai), None, Some(anthropic), None, None)
            .await;
    passed(&results, "intelligence");
    passed(&results, "vision");
}

#[tokio::test]
#[ignore = "requires GROQ_API_KEY plus OPENAI_API_KEY for other slots"]
async fn groq_transcription_live() {
    let groq = std::env::var("GROQ_API_KEY").expect("GROQ_API_KEY");
    let openai = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY");
    let mut config = ByokConfig::default();
    config.transcription = choice(ProviderId::Groq, "whisper-large-v3-turbo", "");
    let results =
        provider::test_byok_candidate(config, Some(openai), None, None, Some(groq), None).await;
    passed(&results, "transcription");
}

#[tokio::test]
#[ignore = "requires COMPAT_BASE_URL and optionally COMPAT_API_KEY"]
async fn openai_compatible_live() {
    let base = std::env::var("COMPAT_BASE_URL").expect("COMPAT_BASE_URL");
    let key = std::env::var("COMPAT_API_KEY").unwrap_or_default();
    let text = std::env::var("COMPAT_TEXT_MODEL").expect("COMPAT_TEXT_MODEL");
    let embedding = std::env::var("COMPAT_EMBED_MODEL").expect("COMPAT_EMBED_MODEL");
    let transcription = std::env::var("COMPAT_TRANSCRIBE_MODEL").expect("COMPAT_TRANSCRIBE_MODEL");
    let mut config = ByokConfig::default();
    config.intelligence = choice(ProviderId::OpenaiCompatible, &text, &base);
    config.vision = config.intelligence.clone();
    config.embeddings = choice(ProviderId::OpenaiCompatible, &embedding, &base);
    config.transcription = choice(ProviderId::OpenaiCompatible, &transcription, &base);
    let results = provider::test_byok_candidate(config, None, None, None, None, Some(key)).await;
    for capability in ["intelligence", "vision", "embeddings", "transcription"] {
        passed(&results, capability);
    }
}
