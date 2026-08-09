//! Deterministik görev sınıflandırıcı. AI YOK: akış seçimi burada, saf
//! kelime/eşik kurallarıyla yapılır. `config/flow/rules.toml` override
//! edilebilir (Task 3), varsayılan gömülüdür.

use super::definition::FlowDefinition;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserMode {
    UserOriented,
    Autonomous,
}

#[derive(Debug, Clone)]
pub struct FlowRules {
    /// commit akışını tetikleyen kelimeler (küçük harfe çevrilmiş prompt'ta aranır)
    pub commit_keywords: Vec<&'static str>,
    /// araştırma sinyali kelimeleri
    pub research_keywords: Vec<&'static str>,
    /// yazma sinyali kelimeleri (dosya/kod değişikliği isteyen görev)
    pub write_keywords: Vec<&'static str>,
    /// direct akış eşiği: bu değerden kısa ve araştırma/yazma sinyali yoksa direct
    pub direct_max_len: usize,
    /// commit akışı için minimal uzunluk üst sınırı (uzun metin = universal)
    pub commit_max_len: usize,
    /// mvp kararı için task uzunluk üst sınırı
    pub mvp_max_len: usize,
}

impl Default for FlowRules {
    fn default() -> Self {
        Self {
            commit_keywords: vec!["commit", "commit et", "commit at", "git commit", "değişiklikleri kaydet"],
            research_keywords: vec!["araştır", "research", "nedir", "nasıl çalışır", "kıyasla", "karşılaştır",
                "web", "kaynak", "güncel", "2026", "ne yapmalı", "en iyi"],
            write_keywords: vec!["yaz", "ekle", "oluştur", "düzelt", "fix", "implement", "kodla",
                "dosya", "fonksiyon", "modül", "hata", "bug", "refactor", "feature"],
            direct_max_len: 120,
            commit_max_len: 400,
            mvp_max_len: 800,
        }
    }
}

pub fn default_rules() -> FlowRules {
    FlowRules::default()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassifiedFlow {
    Universal,
    Commit,
    Direct,
}

impl ClassifiedFlow {
    pub fn flow(&self) -> &'static FlowDefinition {
        let name = match self {
            Self::Universal => "universal",
            Self::Commit => "commit",
            Self::Direct => "direct",
        };
        super::definition::find_flow(name).expect("varsayılan akış tanımlı olmalı")
    }
}

pub struct FlowClassifier;

impl FlowClassifier {
    pub fn classify(prompt: &str, _mode: UserMode, rules: &FlowRules) -> ClassifiedFlow {
        let lower = prompt.to_lowercase();
        let has = |words: &[&'static str]| words.iter().any(|w| lower.contains(w));

        // 1) commit sinyali
        if has(&rules.commit_keywords) && lower.len() <= rules.commit_max_len {
            return ClassifiedFlow::Commit;
        }
        // 2) direct: kısa ve araştırma/yazma sinyali yok
        let research = has(&rules.research_keywords);
        let write = has(&rules.write_keywords);
        if lower.len() <= rules.direct_max_len && !research && !write {
            return ClassifiedFlow::Direct;
        }
        // 3) aksi halde universal
        ClassifiedFlow::Universal
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_detected() {
        let r = default_rules();
        let f = FlowClassifier::classify("commit at bakalım değişiklikleri", UserMode::UserOriented, &r);
        assert_eq!(f, ClassifiedFlow::Commit);
    }

    #[test]
    fn short_trivial_goes_direct() {
        let r = default_rules();
        let f = FlowClassifier::classify("bu dosyayı sil", UserMode::UserOriented, &r);
        assert_eq!(f, ClassifiedFlow::Direct);
    }

    #[test]
    fn research_task_goes_universal() {
        let r = default_rules();
        let f = FlowClassifier::classify(
            "2026 yılında en iyi rust web frameworkü nedir araştır ve karşılaştır",
            UserMode::UserOriented, &r);
        assert_eq!(f, ClassifiedFlow::Universal);
    }
}
