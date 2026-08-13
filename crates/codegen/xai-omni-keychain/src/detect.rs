//! API anahtarı prefix tablosu: saf ve deterministik provider tespiti.
//!
//! Ağ erişimi, dosya sistemi, env okuması veya secret loglama YOKTUR.
//! Uzun/özgül prefix önce eşleşir; genel `sk-` gibi adaylar yalnızca daha
//! özgül bir `sk-*` kuralı eşleşmediğinde düşük confidence ile döner.

/// Anahtar prefix'inden tespit edilen provider adayı.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectCandidate {
    pub provider_id: String,
    /// 0..=100 aralığında güven skoru (100 = kesin önek).
    pub confidence: u8,
    /// Eşleşen prefix'i `prefix:<prefix>` biçiminde belirtir.
    pub reason: String,
    pub suggested_regions: Vec<String>,
}

struct Rule {
    prefix: &'static str,
    provider_id: &'static str,
    confidence: u8,
}

/// Özgül (uzun) prefix kuralları — aynı aile içinde uzun prefix önce listelenir.
/// Bu kuralardan biri eşleşirse genel `sk-` kuralları devreye girmez.
const SPECIFIC_RULES: &[Rule] = &[
    Rule {
        prefix: "sk-ant-api03-",
        provider_id: "anthropic",
        confidence: 100,
    },
    Rule {
        prefix: "github_pat_",
        provider_id: "github",
        confidence: 100,
    },
    Rule {
        prefix: "sk-or-v1-",
        provider_id: "openrouter",
        confidence: 100,
    },
    Rule {
        prefix: "sk-svcacct-",
        provider_id: "openai",
        confidence: 100,
    },
    Rule {
        prefix: "sk-proj-",
        provider_id: "openai",
        confidence: 100,
    },
    Rule {
        prefix: "sk-realtime-",
        provider_id: "openai",
        confidence: 90,
    },
    Rule {
        prefix: "sk-admin-",
        provider_id: "openai",
        confidence: 95,
    },
    Rule {
        prefix: "sk-ant-",
        provider_id: "anthropic",
        confidence: 95,
    },
    Rule {
        prefix: "sk-or-",
        provider_id: "openrouter",
        confidence: 90,
    },
    Rule {
        prefix: "gsk_",
        provider_id: "groq",
        confidence: 100,
    },
    Rule {
        prefix: "xai-",
        provider_id: "xai",
        confidence: 100,
    },
    Rule {
        prefix: "AIza",
        provider_id: "google",
        confidence: 100,
    },
    Rule {
        prefix: "hf_",
        provider_id: "huggingface",
        confidence: 100,
    },
    Rule {
        prefix: "hf-",
        provider_id: "huggingface",
        confidence: 90,
    },
    Rule {
        prefix: "ghp_",
        provider_id: "github",
        confidence: 100,
    },
    Rule {
        prefix: "gho_",
        provider_id: "github",
        confidence: 100,
    },
    Rule {
        prefix: "ghu_",
        provider_id: "github",
        confidence: 90,
    },
    Rule {
        prefix: "ghs_",
        provider_id: "github",
        confidence: 90,
    },
    Rule {
        prefix: "glpat-",
        provider_id: "gitlab",
        confidence: 100,
    },
    Rule {
        prefix: "pplx-",
        provider_id: "perplexity",
        confidence: 100,
    },
    Rule {
        prefix: "pplx_",
        provider_id: "perplexity",
        confidence: 90,
    },
    Rule {
        prefix: "nvapi-",
        provider_id: "nvidia",
        confidence: 95,
    },
    Rule {
        prefix: "tgp_",
        provider_id: "together",
        confidence: 95,
    },
    Rule {
        prefix: "fw_",
        provider_id: "fireworks",
        confidence: 95,
    },
    Rule {
        prefix: "r8_",
        provider_id: "replicate",
        confidence: 95,
    },
    Rule {
        prefix: "pa-",
        provider_id: "voyage",
        confidence: 95,
    },
    Rule {
        prefix: "rpk_",
        provider_id: "runpod",
        confidence: 90,
    },
    Rule {
        prefix: "mt-",
        provider_id: "modal",
        confidence: 90,
    },
];

/// Genel `sk-` kuralları — yalnızca hiçbir özgül `sk-*` kuralı eşleşmediğinde.
/// `sk-` birden çok provider'da kullanıldığı için düşük confidence adaylardır.
const GENERIC_SK_RULES: &[Rule] = &[
    Rule {
        prefix: "sk-",
        provider_id: "openai",
        confidence: 40,
    },
    Rule {
        prefix: "sk-",
        provider_id: "deepseek",
        confidence: 35,
    },
];

/// Test'lerin kural sayısını doğrulayabildiği toplam.
#[cfg(test)]
const TOTAL_RULE_COUNT: usize = SPECIFIC_RULES.len() + GENERIC_SK_RULES.len();

fn to_candidate(rule: &Rule) -> DetectCandidate {
    DetectCandidate {
        provider_id: rule.provider_id.to_string(),
        confidence: rule.confidence,
        reason: format!("prefix:{}", rule.prefix),
        suggested_regions: Vec::new(),
    }
}

/// API anahtarının önekinden provider adaylarını tespit eder (saf, deterministik).
///
/// - Boş/bilinmeyen anahtar: boş liste.
/// - Uzun/özgül prefix önce eşleşir; çakışmada yüksek confidence aday baştadır.
/// - Sonuçlar confidence'a göre yüksekten düşüğe sıralanır; aynı provider
///   yalnızca en yüksek confidence'lı adayla tekilleşir.
pub fn detect_providers_from_key(api_key: &str) -> Vec<DetectCandidate> {
    if api_key.is_empty() {
        return Vec::new();
    }

    let mut matched_specific = false;
    let mut candidates: Vec<DetectCandidate> = Vec::new();

    for rule in SPECIFIC_RULES {
        if api_key.starts_with(rule.prefix) {
            matched_specific = true;
            candidates.push(to_candidate(rule));
        }
    }

    if !matched_specific {
        for rule in GENERIC_SK_RULES {
            if api_key.starts_with(rule.prefix) {
                candidates.push(to_candidate(rule));
            }
        }
    }

    // Confidence'a göre yüksekten düşüğe; eşitlikte tablo sırası korunur
    // (stable sort) → deterministik.
    candidates.sort_by_key(|c| std::cmp::Reverse(c.confidence));

    // Aynı provider yalnızca en yüksek confidence'lı adayla kalır.
    let mut deduped: Vec<DetectCandidate> = Vec::new();
    for candidate in candidates {
        if !deduped
            .iter()
            .any(|c| c.provider_id == candidate.provider_id)
        {
            deduped.push(candidate);
        }
    }
    deduped
}

#[cfg(test)]
#[path = "detect_tests.rs"]
mod tests; // modül ayrı dosyada (detect_tests.rs)
