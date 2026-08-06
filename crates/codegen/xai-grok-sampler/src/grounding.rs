//! Kanit zorunlulugu kapisi — sampler katmaninda "AI yalan soylemesin".
//!
//! Vizyon: her olgusal iddia bir tool ciktisinin **belirli araligina**
//! (span + birebir alinti) referans verir; referanssiz iddia **reddedilir**.
//! Kapinin sorusu "dogru mu?" degil "**curutebilir miyim?**": her iddia
//! icin curutme denemeleri kosulur (terim/sayi ortusmesi, negasyon uyumu,
//! span-alinti tutarliligi); tek tutarsizlik iddiayi dusurur.
//!
//! Bu modul omni-router'in `GroundingGate` mantiginin **basitlestirilmis**
//! halidir: tool ciktisi yerine yalnizca [`EvidenceRef`] kumesi uzerinden
//! calisir (aralik icerigi `quote` olarak tasinir), iddia kumesi metindeki
//! olgusal kalip taramasiyla cikarilir. I6: uretim yolunda panik yok.

use serde::{Deserialize, Serialize};

/// Varsayilan ihlal cezasi (trust skoru dusumu).
pub const DEFAULT_VIOLATION_PENALTY: f64 = 0.2;

/// Kanit kapisinin knoblari. `None` olarak tasinan yoklugu = kapi kapali.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GroundingConfig {
    /// `true`: olgusal iddia taranir ve referans zorunludur.
    /// `false`: tarama yapilmaz, karar `NoClaims` olur.
    #[serde(default = "default_true")]
    pub require_evidence: bool,
    /// `true`: siki curutme — tum terimler, tum sayilar ve negasyon uyumu.
    /// `false`: gevsek — tek ortusen terim referans sayilir.
    #[serde(default = "default_true")]
    pub falsification_mode: bool,
    /// Desteklenmeyen iddianin trust skoruna maliyeti.
    #[serde(default = "default_violation_penalty")]
    pub violation_penalty: f64,
}

const fn default_true() -> bool {
    true
}

const fn default_violation_penalty() -> f64 {
    DEFAULT_VIOLATION_PENALTY
}

impl Default for GroundingConfig {
    fn default() -> Self {
        Self {
            require_evidence: true,
            falsification_mode: true,
            violation_penalty: DEFAULT_VIOLATION_PENALTY,
        }
    }
}

impl GroundingConfig {
    /// Ihlal maliyeti: `Unsupported` ise `violation_penalty`, degilse 0.
    pub fn penalty_for(&self, verdict: &Verdict) -> f64 {
        match verdict {
            Verdict::Unsupported { .. } => self.violation_penalty,
            Verdict::Supported | Verdict::NoClaims => 0.0,
        }
    }
}

/// Bir olgusal iddianin dayandigi **belirli aralik**: hangi tool ciktisinda,
/// hangi aralikta, birebir hangi metin.
///
/// `span_start`/`span_end` karakter sayisi cinsindendir; `quote` karakter
/// sayisi aralik genisligiyle uyusmuyorsa referans **guvenilmez** sayilir
/// (curutulur).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceRef {
    pub tool_name: String,
    /// Dahil (karakter offset).
    pub span_start: usize,
    /// Haric (karakter offset).
    pub span_end: usize,
    /// Aralikta yazdigi iddia edilen birebir metin.
    pub quote: String,
}

impl EvidenceRef {
    pub fn new(
        tool_name: impl Into<String>,
        span_start: usize,
        span_end: usize,
        quote: impl Into<String>,
    ) -> Self {
        Self {
            tool_name: tool_name.into(),
            span_start,
            span_end,
            quote: quote.into(),
        }
    }

    /// Yapisal tutarlilik: bos olmayan, dogru yonlu, quote uzunlugu aralikla
    /// uyumlu. I6: panik yok, `None` donduren yol yok.
    pub fn is_consistent(&self) -> bool {
        if self.span_start >= self.span_end {
            return false;
        }
        if self.quote.trim().is_empty() {
            return false;
        }
        self.quote.chars().count() == self.span_end - self.span_start
    }
}

/// Kanit kapisinin tek iddia karari.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Verdict {
    /// Taranan her olgusal iddia referansli ve curutulemedi.
    Supported,
    /// En az bir iddia referanssiz/curutuldu.
    Unsupported {
        /// Ilk curutulen iddianin cumlesi.
        claim: String,
        /// Yaklasik eslesen referanslar (en yuksek ortusmeden).
        suggestions: Vec<String>,
    },
    /// Metinde olgusal iddia yok (ya da kapi kapali).
    NoClaims,
}

/// Bir metin taramasinin toplu sonucu.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroundingVerdict {
    pub verdict: Verdict,
    /// Taranan olgusal iddia sayisi.
    pub claims_scanned: usize,
    /// Desteklenen iddia sayisi.
    pub supported_claims: usize,
}

impl GroundingVerdict {
    /// Kapi gecti mi? `Supported` ve `NoClaims` gecer.
    pub fn passed(&self) -> bool {
        matches!(self.verdict, Verdict::Supported | Verdict::NoClaims)
    }

    /// Varsayilan ayarla ihlal maliyeti (trust skoru dusumu).
    pub fn penalty(&self) -> f64 {
        penalty_for(&self.verdict)
    }
}

/// `Unsupported` iddianin varsayilan ayarla maliyeti (trust skoru dusumu).
pub fn penalty_for(verdict: &Verdict) -> f64 {
    match verdict {
        Verdict::Unsupported { .. } => DEFAULT_VIOLATION_PENALTY,
        Verdict::Supported | Verdict::NoClaims => 0.0,
    }
}

/// Varsayilan [`GroundingConfig`] ile metni tara ve hukmet.
pub fn check_evidence_claims(text: &str, refs: &[EvidenceRef]) -> GroundingVerdict {
    check_evidence_claims_with(text, refs, &GroundingConfig::default())
}

/// Metindeki olgusal iddialari tarar; referanssiz/curutulebilen iddia varsa
/// `Unsupported` doner. I6: panik yok.
pub fn check_evidence_claims_with(
    text: &str,
    refs: &[EvidenceRef],
    config: &GroundingConfig,
) -> GroundingVerdict {
    if !config.require_evidence {
        return GroundingVerdict {
            verdict: Verdict::NoClaims,
            claims_scanned: 0,
            supported_claims: 0,
        };
    }

    let claims = detect_claims(text);
    if claims.is_empty() {
        return GroundingVerdict {
            verdict: Verdict::NoClaims,
            claims_scanned: 0,
            supported_claims: 0,
        };
    }

    let mut supported = 0usize;
    for claim in &claims {
        if claim_supported(claim, refs, config) {
            supported += 1;
        }
    }
    if supported == claims.len() {
        return GroundingVerdict {
            verdict: Verdict::Supported,
            claims_scanned: claims.len(),
            supported_claims: supported,
        };
    }

    let first_unsupported = claims.iter().find(|c| !claim_supported(c, refs, config));
    let Some(first) = first_unsupported else {
        return GroundingVerdict {
            verdict: Verdict::Supported,
            claims_scanned: claims.len(),
            supported_claims: supported,
        };
    };

    GroundingVerdict {
        verdict: Verdict::Unsupported {
            claim: first.clone(),
            suggestions: suggestions_for(first, refs),
        },
        claims_scanned: claims.len(),
        supported_claims: supported,
    }
}

// ---------------------------------------------------------------------------
// Iddia tespiti — regex derlemesi yok, hepsi deterministik substring tarama.
// ---------------------------------------------------------------------------

/// Olgusal yuklem kalplari. Cikti dili Ingilizce agirlikli; Turkce kalplar da
/// taninir (kullanici vizyonundaki "oldugunu / edildi / -dir" ornekleri).
const FACTUAL_PATTERNS: &[&str] = &[
    // Copular (Ingilizce).
    " is ", " are ", " was ", " were ", " has been ", " have been ", " had been ",
    " will be ", " would be ", " is now ", " are now ",
    // Dogrulama/raporlama fillerleri.
    "confirmed", "verified", "shows", "showed", "reports", "reported", "found",
    "indicates", "indicated", "resulted", "failed", "succeeded", "completed",
    "contains", "contained", "returned", "states", "stated", "says", "said",
    // Turkce olgusal yuklemler.
    "oldugunu", "oldugu", "edildi", "edildigini", "edilmistir", "tamamlandi",
    "basarisiz", "basariyla", "bulundu", "belirtiyor", "belirtilmektedir",
    "gosteriyor", "gosteriyor ki", "raporlandi", "dogrulandi",
];

/// Turkce copula ekleri ("dogrudur", "tamamlanmistir" ...).
const TURKISH_COPULA_SUFFIXES: &[&str] = &[
    "dır", "dir", "dur", "dür", "tır", "tir", "tur", "tür",
];

/// Bir token Turkce copula ekiyle mi bitiyor ("doğrudur" -> "dur").
fn has_turkish_copula_suffix(token: &str) -> bool {
    if token.chars().count() <= 3 {
        return false;
    }
    let lower = token.to_lowercase();
    TURKISH_COPULA_SUFFIXES.iter().any(|s| lower.ends_with(s))
}

fn is_factual_sentence(sentence: &str) -> bool {
    let lower = sentence.to_lowercase();
    for pattern in FACTUAL_PATTERNS {
        if lower.contains(pattern) {
            return true;
        }
    }
    for token in tokens(sentence) {
        if has_turkish_copula_suffix(&token) {
            return true;
        }
    }
    if !numeric_literals(sentence).is_empty() {
        return true;
    }
    false
}

/// Metni cumlelere bolup olgusal olanlari toplar.
fn detect_claims(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for sentence in text.split(['.', '!', '?', ';', '\n']) {
        let trimmed = sentence.trim();
        if trimmed.is_empty() {
            continue;
        }
        if is_factual_sentence(trimmed) {
            out.push(trimmed.to_string());
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Curutme denemeleri — omni-router `GroundingGate` mantiginin sadik ozu.
// ---------------------------------------------------------------------------

/// Iddia, elindeki referans kumesine karsi curutulebildi mi?
fn claim_supported(claim: &str, refs: &[EvidenceRef], config: &GroundingConfig) -> bool {
    let terms = content_terms(claim);
    let numbers = numeric_literals(claim);
    if terms.is_empty() && numbers.is_empty() {
        // Dogrulanabilir icerik yok — curutme denemesi bos, iddia duser.
        return false;
    }

    // Yapisal denetimi gecen referanslar: aralik tutarli ve alinti uyumlu.
    let mut span = String::new();
    for r in refs {
        if !r.is_consistent() {
            continue;
        }
        span.push_str(&r.quote);
        span.push(' ');
    }
    if span.trim().is_empty() {
        return false;
    }

    let span_lower = span.to_lowercase();
    if config.falsification_mode {
        // Siki curutme: tum terimler + tum sayilar kelime siniriyla gecmeli,
        // negasyon yonu span ile ayni olmali.
        for term in &terms {
            if !word_boundary_contains(&span_lower, term) {
                return false;
            }
        }
        for literal in &numbers {
            if !word_boundary_contains(&span_lower, literal) {
                return false;
            }
        }
        if has_negation(&claim.to_lowercase()) != has_negation(&span_lower) {
            return false;
        }
        true
    } else {
        // Gevsek: tek ortusen terim ya da sayi referansi dogrular.
        terms
            .iter()
            .any(|t| word_boundary_contains(&span_lower, t))
            || numbers
                .iter()
                .any(|n| word_boundary_contains(&span_lower, n))
    }
}

/// Curutulen iddiaya en yakin gelen referanslar (ortusme skoru azalan).
fn suggestions_for(claim: &str, refs: &[EvidenceRef]) -> Vec<String> {
    let terms = content_terms(claim);
    let numbers = numeric_literals(claim);

    let mut scored: Vec<(usize, String)> = refs
        .iter()
        .filter(|r| r.is_consistent())
        .map(|r| {
            let lower = r.quote.to_lowercase();
            let mut hits = 0usize;
            for t in &terms {
                if word_boundary_contains(&lower, t) {
                    hits += 1;
                }
            }
            for n in &numbers {
                if word_boundary_contains(&lower, n) {
                    hits += 1;
                }
            }
            (hits, format!("{}: {}", r.tool_name, excerpt(&r.quote)))
        })
        .collect();

    scored.sort_by(|a, b| b.0.cmp(&a.0));
    scored
        .into_iter()
        .filter(|(hits, _)| *hits > 0)
        .take(3)
        .map(|(_, label)| label)
        .collect()
}

/// Alintiyi kisa ozetler (80 karakter cekirdegi + elipsis).
fn excerpt(quote: &str) -> String {
    let chars: Vec<char> = quote.chars().collect();
    if chars.len() <= 80 {
        return quote.to_string();
    }
    let start = (chars.len() - 80) / 2;
    let mut out: String = chars[start..start + 80].iter().collect();
    out.insert_str(0, "\u{2026}");
    out.push('\u{2026}');
    out
}

// ---------------------------------------------------------------------------
// Terim/sayi/negasyon yardimcilari — omni-router'dan tasindi (deterministik).
// ---------------------------------------------------------------------------

/// Icerik tasimayan sik sozcukler; terim ortusmesinde sayilmaz.
const STOPWORDS: &[&str] = &[
    "that", "this", "with", "from", "have", "has", "been", "were", "was", "will", "into", "then",
    "than", "they", "them", "there", "their", "when", "what", "which", "while", "also", "only",
    "just", "such", "does", "did", "done", "the", "and", "for", "are", "its", "it's", "some",
    "very", "each", "both", "over", "after", "before", "about", "still", "shall", "should",
    "would", "could", "here",
];

/// Olumsuzluk isaretcileri — kutupsallik esitligi bunlarla olculur.
const NEGATION_MARKERS: &[&str] = &[
    "not", "no", "never", "without", "cannot", "can't", "isn't", "aren't", "doesn't", "didn't",
    "wasn't", "weren't", "won't", "fail", "fails", "failed", "failure", "missing", "absent",
    "unable", "none", "nor", "denied", "rejected", "error",
];

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Kelime sinirlarina saygili, kucuk harfe indirgenmis alt-dizi aramasi.
/// Girdilerin ikisi de **kucuk harf** olmali.
fn word_boundary_contains(haystack_lower: &str, needle_lower: &str) -> bool {
    if needle_lower.is_empty() || haystack_lower.is_empty() {
        return false;
    }
    for (idx, _) in haystack_lower.match_indices(needle_lower) {
        let before_ok = haystack_lower[..idx]
            .chars()
            .next_back()
            .is_none_or(|c| !is_word_char(c));
        let after = idx + needle_lower.len();
        let after_ok = haystack_lower[after..]
            .chars()
            .next()
            .is_none_or(|c| !is_word_char(c));
        if before_ok && after_ok {
            return true;
        }
    }
    false
}

/// Bosluga gore ayir, uclardaki noktalamayi at, kucuk harfe indir.
fn tokens(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|w| {
            w.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .filter(|w| !w.is_empty())
        .collect()
}

fn push_unique(out: &mut Vec<String>, value: String) {
    if !out.contains(&value) {
        out.push(value);
    }
}

/// Iddianin kanit araliginda aranacak icerik terimleri.
fn content_terms(statement: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in tokens(statement) {
        if t.chars().count() <= 3 {
            continue;
        }
        if !t.chars().any(|c| c.is_alphabetic()) {
            continue;
        }
        if STOPWORDS.contains(&t.as_str()) {
            continue;
        }
        push_unique(&mut out, t);
    }
    out
}

/// Iddiadaki sayisal degerler. Uydurma sayi halusinasyonun en sik bicimi
/// oldugu icin ayri ve tam eslesmeli kontrol.
fn numeric_literals(statement: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in tokens(statement) {
        if t.chars().any(|c| c.is_ascii_digit()) {
            push_unique(&mut out, t);
        }
    }
    out
}

/// Metinde olumsuzluk isaretcisi var mi (girdi kucuk harf olmali).
fn has_negation(text_lower: &str) -> bool {
    NEGATION_MARKERS
        .iter()
        .any(|m| word_boundary_contains(text_lower, m))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refs_for(quotes: &[(&str, &str)]) -> Vec<EvidenceRef> {
        quotes
            .iter()
            .map(|(tool, quote)| EvidenceRef::new(*tool, 0, quote.chars().count(), *quote))
            .collect()
    }

    #[test]
    fn supported_when_every_claim_matches_the_cited_span() {
        let text = "The listen address is 192.168.1.10 and the port is 8080.";
        let refs = refs_for(&[("read_file", "listen address is 192.168.1.10 and the port is 8080")]);
        let verdict = check_evidence_claims(text, &refs);
        assert!(verdict.passed(), "verdict: {:?}", verdict);
        assert_eq!(verdict.verdict, Verdict::Supported);
        assert_eq!(verdict.claims_scanned, 2);
        assert_eq!(verdict.supported_claims, 2);
    }

    #[test]
    fn unsupported_claim_without_matching_span() {
        let text = "The deployment is healthy.";
        let refs = refs_for(&[("read_file", "deployment shows 5 pods pending")]);
        let verdict = check_evidence_claims(text, &refs);
        assert!(!verdict.passed());
        let Verdict::Unsupported { claim, .. } = &verdict.verdict else {
            panic!("must be unsupported");
        };
        assert!(claim.contains("deployment is healthy"));
    }

    #[test]
    fn hallucinated_number_refuted_in_strict_mode() {
        let text = "The port is 9090.";
        let refs = refs_for(&[("read_file", "the port is 8080")]);
        let verdict = check_evidence_claims(text, &refs);
        assert!(!verdict.passed());
    }

    #[test]
    fn no_claims_when_text_is_pure_opinion() {
        let text = "Thanks for the summary, that looks quite good overall.";
        let verdict = check_evidence_claims(text, &[]);
        assert_eq!(verdict.verdict, Verdict::NoClaims);
        assert!(verdict.passed());
        assert_eq!(verdict.penalty(), 0.0);
    }

    #[test]
    fn negation_flip_is_refuted_in_falsification_mode() {
        let text = "The build did not fail.";
        let refs = refs_for(&[("run_tests", "build failed: 3 tests broken")]);
        let strict = GroundingConfig::default();
        assert!(!check_evidence_claims_with(text, &refs, &strict).passed());
        let lenient = GroundingConfig {
            falsification_mode: false,
            ..GroundingConfig::default()
        };
        assert!(check_evidence_claims_with(text, &refs, &lenient).passed());
    }

    #[test]
    fn lenient_mode_accepts_single_term_overlap() {
        let text = "The service was redeployed.";
        let refs = refs_for(&[("kubectl", "service redeployed at 12:00")]);
        let lenient = GroundingConfig {
            falsification_mode: false,
            ..GroundingConfig::default()
        };
        assert!(check_evidence_claims_with(text, &refs, &lenient).passed());
    }

    #[test]
    fn require_evidence_false_skips_the_gate() {
        let text = "The port is 9090.";
        let refs = refs_for(&[("read_file", "the port is 8080")]);
        let off = GroundingConfig {
            require_evidence: false,
            ..GroundingConfig::default()
        };
        let verdict = check_evidence_claims_with(text, &refs, &off);
        assert_eq!(verdict.verdict, Verdict::NoClaims);
        assert_eq!(verdict.claims_scanned, 0);
    }

    #[test]
    fn inconsistent_span_is_distrusted() {
        let text = "The port is 8080.";
        // quote karakter sayisi span genisligiyle uyusmuyor.
        let refs = vec![EvidenceRef::new("read_file", 0, 3, "the port is 8080")];
        let verdict = check_evidence_claims(text, &refs);
        assert!(!verdict.passed());
    }

    #[test]
    fn suggestions_rank_closest_references() {
        let text = "The cluster has 12 nodes running.";
        let refs = refs_for(&[
            ("kubectl", "cluster has 12 nodes running"),
            ("aws", "account quota is 20 nodes"),
            ("terraform", "plan shows 12 resources"),
        ]);
        let verdict = check_evidence_claims(text, &refs);
        let Verdict::Unsupported { suggestions, .. } = &verdict.verdict else {
            panic!("must be unsupported");
        };
        assert!(!suggestions.is_empty());
        assert!(suggestions[0].starts_with("kubectl:"), "{suggestions:?}");
    }

    #[test]
    fn penalty_reflects_verdict() {
        assert_eq!(penalty_for(&Verdict::Supported), 0.0);
        assert_eq!(penalty_for(&Verdict::NoClaims), 0.0);
        let unsupported = Verdict::Unsupported {
            claim: "x".into(),
            suggestions: Vec::new(),
        };
        assert_eq!(penalty_for(&unsupported), DEFAULT_VIOLATION_PENALTY);
        let config = GroundingConfig {
            violation_penalty: 0.5,
            ..GroundingConfig::default()
        };
        assert_eq!(config.penalty_for(&unsupported), 0.5);
    }

    #[test]
    fn config_round_trips_through_serde() {
        let config = GroundingConfig::default();
        let json = serde_json::to_string(&config).expect("serialize");
        let back: GroundingConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, config);

        // Eski JSON (alanlar yok) -> varsayilanlar.
        let stripped: GroundingConfig =
            serde_json::from_str("{}").expect("missing fields must default");
        assert_eq!(stripped, config);
    }

    #[test]
    fn turkish_copula_sentence_is_detected() {
        let text = "Sema uygulamasi tamamlanmistir, port 8080 olarak ayarlandi.";
        let refs = refs_for(&[("dogrulama", "sema uygulamasi tamamlanmistir, port 8080 olarak ayarlandi")]);
        assert!(check_evidence_claims(text, &refs).passed());
    }
}
