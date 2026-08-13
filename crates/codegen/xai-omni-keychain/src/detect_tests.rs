//! Prefix tablosu testleri: `detect_providers_from_key` davranışı.
//! Fonksiyon saf olmalı: ağ yok, dosya yok, env yok, secret log yok.

use super::*;

#[test]
fn anthropic_api03_prefix_detected() {
    let got = detect_providers_from_key("sk-ant-api03-abcdef123456");
    let first = got.first().expect("anthropic adayı olmalı");
    assert_eq!(first.provider_id, "anthropic");
    assert_eq!(first.confidence, 100);
    assert_eq!(first.reason, "prefix:sk-ant-api03-");
    assert!(first.suggested_regions.is_empty());
}

#[test]
fn anthropic_short_prefix_detected() {
    let got = detect_providers_from_key("sk-ant-xyz987");
    let first = got.first().expect("anthropic adayı olmalı");
    assert_eq!(first.provider_id, "anthropic");
    assert_eq!(first.confidence, 95);
    assert_eq!(first.reason, "prefix:sk-ant-");
}

#[test]
fn openai_project_prefix_detected_first() {
    let got = detect_providers_from_key("sk-proj-abc123");
    let first = got.first().expect("openai adayı olmalı");
    assert_eq!(first.provider_id, "openai");
    assert_eq!(first.confidence, 100);
    assert_eq!(first.reason, "prefix:sk-proj-");
    // Uzun/özgül prefix eşleşince genel `sk-` adayları girmemeli.
    assert_eq!(got.len(), 1, "genel sk- adayları bastırılmalı: {:?}", got);
}

#[test]
fn openai_service_prefix_detected() {
    let got = detect_providers_from_key("sk-svcacct-abc123");
    let first = got.first().expect("openai adayı olmalı");
    assert_eq!(first.provider_id, "openai");
    assert_eq!(first.confidence, 100);
    assert_eq!(first.reason, "prefix:sk-svcacct-");
}

#[test]
fn google_prefix_detected() {
    let got = detect_providers_from_key("AIzaSyD-abc123xyz");
    let first = got.first().expect("google adayı olmalı");
    assert_eq!(first.provider_id, "google");
    assert_eq!(first.confidence, 100);
    assert_eq!(first.reason, "prefix:AIza");
}

#[test]
fn groq_prefix_detected() {
    let got = detect_providers_from_key("gsk_AbC123xyz");
    let first = got.first().expect("groq adayı olmalı");
    assert_eq!(first.provider_id, "groq");
    assert_eq!(first.confidence, 100);
    assert_eq!(first.reason, "prefix:gsk_");
}

#[test]
fn xai_prefix_detected() {
    let got = detect_providers_from_key("xai-abc123def");
    let first = got.first().expect("xai adayı olmalı");
    assert_eq!(first.provider_id, "xai");
    assert_eq!(first.confidence, 100);
    assert_eq!(first.reason, "prefix:xai-");
}

#[test]
fn openrouter_longest_prefix_wins() {
    let got = detect_providers_from_key("sk-or-v1-abc123");
    let first = got.first().expect("openrouter adayı olmalı");
    assert_eq!(first.provider_id, "openrouter");
    assert_eq!(first.confidence, 100);
    assert_eq!(first.reason, "prefix:sk-or-v1-");
    assert_eq!(got.len(), 1, "sk-or- ile çakışmada özgül prefix kalmalı");
}

#[test]
fn huggingface_prefix_detected() {
    let got = detect_providers_from_key("hf_abc123def");
    let first = got.first().expect("huggingface adayı olmalı");
    assert_eq!(first.provider_id, "huggingface");
    assert_eq!(first.confidence, 100);
    assert_eq!(first.reason, "prefix:hf_");
}

#[test]
fn generic_sk_gives_multiple_candidates_sorted_desc() {
    let got = detect_providers_from_key("sk-unknown123xyz");
    assert_eq!(
        got.len(),
        2,
        "genel sk- birden fazla aday üretmeli: {:?}",
        got
    );
    let first = &got[0];
    let second = &got[1];
    assert_eq!(first.provider_id, "openai");
    assert_eq!(first.confidence, 40);
    assert_eq!(first.reason, "prefix:sk-");
    assert_eq!(second.provider_id, "deepseek");
    assert_eq!(second.confidence, 35);
    assert_eq!(second.reason, "prefix:sk-");
    assert!(
        first.confidence >= second.confidence,
        "confidence'a göre yüksekten düşüğe sıralı olmalı"
    );
}

#[test]
fn unknown_key_returns_no_candidates() {
    assert!(detect_providers_from_key("hello-world-123").is_empty());
}

#[test]
fn empty_key_returns_no_candidates() {
    assert!(detect_providers_from_key("").is_empty());
    assert!(detect_providers_from_key("   ").is_empty());
}

#[test]
fn prefix_matching_is_case_sensitive() {
    assert!(detect_providers_from_key("aiza-not-uppercase").is_empty());
    assert!(detect_providers_from_key("Gsk_-uppercase").is_empty());
    assert!(detect_providers_from_key("XAI-UPPER").is_empty());
}

#[test]
fn confidence_always_within_range() {
    let keys = [
        "sk-ant-api03-x",
        "sk-ant-x",
        "sk-proj-x",
        "sk-svcacct-x",
        "sk-admin-x",
        "sk-realtime-x",
        "sk-or-v1-x",
        "sk-or-x",
        "sk-unknown",
        "gsk_x",
        "xai-x",
        "AIzaX",
        "hf_x",
        "hf-x",
        "glpat-x",
        "nvapi-x",
        "tgp_x",
        "fw_x",
        "r8_x",
        "pa-x",
        "rpk_x",
        "mt-x",
    ];
    for key in keys {
        for c in detect_providers_from_key(key) {
            assert!(
                (0..=100).contains(&c.confidence),
                "confidence 0..=100 olmalı: {} -> {}",
                key,
                c.confidence
            );
        }
    }
}

#[test]
fn table_has_at_least_25_rules() {
    assert!(
        TOTAL_RULE_COUNT >= 25,
        "en az 25 provider kuralı olmalı, mevcut: {}",
        TOTAL_RULE_COUNT
    );
}

#[test]
fn multiple_calls_are_deterministic() {
    let a = detect_providers_from_key("sk-unknown123");
    let b = detect_providers_from_key("sk-unknown123");
    assert_eq!(a, b);
    let a2 = detect_providers_from_key("sk-ant-api03-x");
    let b2 = detect_providers_from_key("sk-ant-api03-x");
    assert_eq!(a2, b2);
}

#[test]
fn detect_key_type_still_works() {
    use crate::store::KeyType;
    assert_eq!(
        crate::detect_key_type("openai", "sk-proj-x"),
        KeyType::Project
    );
    assert_eq!(
        crate::detect_key_type("openai", "sk-svcacct-x"),
        KeyType::Service
    );
    assert_eq!(
        crate::detect_key_type("anthropic", "sk-ant-api03-x"),
        KeyType::Anthropic
    );
    assert_eq!(crate::detect_key_type("google", "AIzaX"), KeyType::Google);
    assert_eq!(
        crate::detect_key_type("deepseek", "sk-x"),
        KeyType::DeepSeek
    );
    assert_eq!(crate::detect_key_type("groq", "gsk_x"), KeyType::Groq);
    assert_eq!(crate::detect_key_type("xai", "xai-x"), KeyType::Xai);
    assert_eq!(
        crate::detect_key_type("nope", "zzz-not-a-key"),
        KeyType::Unknown
    );
}
