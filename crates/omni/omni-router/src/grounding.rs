//! Kanit zorunlulugu kapisi — MASTER-PLAN 10.3 madde 1/3, invariant I8.
//!
//! `temperature=0` determinizm vermez; determinizm **surec katmanindadir**:
//! her olgusal iddia bir tool ciktisinin **belirli araligina** (byte araligi +
//! birebir alinti) referans verir. Referanssiz, referansi tutmayan ya da
//! aralikta yazmayani iddia eden her sey **reddedilir** — tolerans yok.
//!
//! Kapinin sorusu "dogru mu?" degil "**curutebilir miyim?**" (10.3 madde 3):
//! her iddia icin curutme denemeleri kosulur; **tek** basarili curutme iddiayi
//! dusurur. Bu dosya sadece kanit-duzeyi curutmeleri uygular; surec-duzeyi
//! curutmeler (yeniden calistirma farki, ureten==dogrulayan) `judge.rs`'de
//! uretilir ama ayni `Refutation` tipini kullanir.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;
use tracing::debug;

/// Bir tool cagrisinin kanonik ciktisi — kanit havuzunun **tek** kaynagi.
/// `tool_call_id` referanslarin cozuldugu anahtardir.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutput {
    pub tool_call_id: String,
    pub tool_name: String,
    pub output_text: String,
    pub success: bool,
}

impl ToolOutput {
    pub fn new(
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        output_text: impl Into<String>,
        success: bool,
    ) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            tool_name: tool_name.into(),
            output_text: output_text.into(),
            success,
        }
    }

    /// Ciktinin tamamini kaplayan referans (kapali-acik `[0, len)` araligi).
    pub fn whole_span(&self) -> EvidenceRef {
        EvidenceRef {
            tool_call_id: self.tool_call_id.clone(),
            start: 0,
            end: self.output_text.len(),
            quote: self.output_text.clone(),
        }
    }

    /// Belirtilen araligi kaplayan referans; aralik gecersizse `None`
    /// (I6: panik yok, `get` ile sinir/karakter kontrolu).
    pub fn span(&self, start: usize, end: usize) -> Option<EvidenceRef> {
        let slice = self.output_text.get(start..end)?;
        Some(EvidenceRef {
            tool_call_id: self.tool_call_id.clone(),
            start,
            end,
            quote: slice.to_string(),
        })
    }
}

/// Bir olgusal iddianin dayandigi **belirli aralik**: hangi tool cagrisinin
/// ciktisinda, hangi byte araliginda, birebir hangi metin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceRef {
    pub tool_call_id: String,
    /// Dahil (byte offset).
    pub start: usize,
    /// Haric (byte offset).
    pub end: usize,
    /// Aralikta yazdigi iddia edilen birebir metin.
    pub quote: String,
}

impl EvidenceRef {
    pub fn new(
        tool_call_id: impl Into<String>,
        start: usize,
        end: usize,
        quote: impl Into<String>,
    ) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            start,
            end,
            quote: quote.into(),
        }
    }
}

/// Kapiya sunulan olgusal iddia. `refs` bos ise iddia daha bakilmadan duser.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactualClaim {
    pub id: String,
    pub statement: String,
    pub refs: Vec<EvidenceRef>,
}

impl FactualClaim {
    /// Referansli iddia.
    pub fn new(id: impl Into<String>, statement: impl Into<String>, refs: Vec<EvidenceRef>) -> Self {
        Self {
            id: id.into(),
            statement: statement.into(),
            refs,
        }
    }

    /// Referanssiz iddia — kapinin **her zaman** reddettigi bicim (I8).
    pub fn unreferenced(id: impl Into<String>, statement: impl Into<String>) -> Self {
        Self::new(id, statement, Vec::new())
    }

    /// Tool ciktisinin belirli araligini birebir tekrarlayan iddia.
    /// Ureticinin metni degistirmeden aktardigi tek guvenli bicim.
    pub fn verbatim(id: impl Into<String>, output: &ToolOutput) -> Self {
        Self::new(id, output.output_text.clone(), vec![output.whole_span()])
    }

    pub fn cited_sources(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        for r in &self.refs {
            if !out.contains(&r.tool_call_id.as_str()) {
                out.push(r.tool_call_id.as_str());
            }
        }
        out
    }
}

/// Basarili bir curutme. Her varyant "iddia neden dustu"nun kanit-referansidir.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Error)]
pub enum Refutation {
    /// Hicbir kanit referansi yok — I8 ihlali.
    #[error("claim has no evidence reference")]
    MissingReference,
    /// Referans edilen tool cagrisi kanit havuzunda yok (uydurulmus kaynak).
    #[error("unknown evidence source '{tool_call_id}'")]
    UnknownSource { tool_call_id: String },
    /// `start >= end`: aralik bos.
    #[error("empty range on '{tool_call_id}' ({start}..{end})")]
    EmptyRange {
        tool_call_id: String,
        start: usize,
        end: usize,
    },
    /// Aralik cikti uzunlugunu asiyor.
    #[error("range {end} exceeds output length {len} on '{tool_call_id}'")]
    OutOfBounds {
        tool_call_id: String,
        end: usize,
        len: usize,
    },
    /// Aralik UTF-8 karakter sinirina denk gelmiyor.
    #[error("range is not a char boundary on '{tool_call_id}'")]
    NotCharBoundary { tool_call_id: String },
    /// Alinti, araliktaki gercek metinle birebir ayni degil (uydurulmus alinti).
    #[error("quote does not match range content on '{tool_call_id}'")]
    QuoteMismatch { tool_call_id: String },
    /// Aralik sadece bosluk — tasiyici kanit yok.
    #[error("cited range on '{tool_call_id}' is blank")]
    BlankSpan { tool_call_id: String },
    /// Basarisiz tool ciktisi kanit olarak gosterilemez.
    #[error("evidence '{tool_call_id}' comes from a failed tool call")]
    FailedToolEvidence { tool_call_id: String },
    /// Iddiadaki icerik terimi kanit araliginda gecmiyor (konu kaymasi).
    #[error("term '{term}' does not occur in the cited range")]
    TermNotInSpan { term: String },
    /// Iddiadaki sayisal deger kanit araliginda gecmiyor (uydurulmus sayi).
    #[error("literal '{literal}' does not occur in the cited range")]
    NumberNotInSpan { literal: String },
    /// Iddia, kanit araligini olumsuzluk yonunde ters cevirmis.
    #[error("claim polarity contradicts the cited range")]
    NegationFlip,
    /// Dogrulanabilir icerik yok (terim de sayi da yok) — bos iddia.
    #[error("claim carries no verifiable content")]
    NoVerifiableContent,
    /// Yargic tool'u yeniden calistirdi, cikti degisti (10.3 madde 2).
    #[error("replayed output of '{tool_call_id}' differs from cited evidence")]
    ReplayMismatch { tool_call_id: String },
    /// Ureten ajan ile dogrulayan ajan ayrilmamis (10.3 madde 2).
    #[error("verifier is not separated from producer: {detail}")]
    VerificationNotSeparated { detail: String },
}

/// Tek bir iddianin kapi karari.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimDecision {
    pub claim_id: String,
    pub accepted: bool,
    pub refutations: Vec<Refutation>,
    /// Yapisal denetimi gecen referanslarin kaynak kimlikleri.
    pub verified_refs: Vec<String>,
}

impl ClaimDecision {
    /// Disaridan (surec katmanindan) gelen curutmeyi ekler; karar duser.
    pub fn refute(&mut self, refutation: Refutation) {
        if !self.refutations.contains(&refutation) {
            self.refutations.push(refutation);
        }
        self.accepted = false;
    }

    pub fn reason(&self) -> String {
        if self.refutations.is_empty() {
            "grounded: every factual assertion resolves to a cited range".to_string()
        } else {
            self.refutations
                .iter()
                .map(|r| r.to_string())
                .collect::<Vec<_>>()
                .join("; ")
        }
    }
}

/// Bir iddia kumesinin toplu karari.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GroundingOutcome {
    pub decisions: Vec<ClaimDecision>,
}

impl GroundingOutcome {
    pub fn total(&self) -> usize {
        self.decisions.len()
    }

    pub fn accepted(&self) -> usize {
        self.decisions.iter().filter(|d| d.accepted).count()
    }

    pub fn rejected(&self) -> usize {
        self.decisions.iter().filter(|d| !d.accepted).count()
    }

    /// Kapi esigi: **tek** reddedilen iddia bile varsa kume gecmez (tolerans 0).
    pub fn all_accepted(&self) -> bool {
        self.decisions.iter().all(|d| d.accepted)
    }

    pub fn rejected_ids(&self) -> Vec<&str> {
        self.decisions
            .iter()
            .filter(|d| !d.accepted)
            .map(|d| d.claim_id.as_str())
            .collect()
    }

    pub fn accepted_ids(&self) -> Vec<&str> {
        self.decisions
            .iter()
            .filter(|d| d.accepted)
            .map(|d| d.claim_id.as_str())
            .collect()
    }

    pub fn decision(&self, claim_id: &str) -> Option<&ClaimDecision> {
        self.decisions.iter().find(|d| d.claim_id == claim_id)
    }

    pub fn decision_mut(&mut self, claim_id: &str) -> Option<&mut ClaimDecision> {
        self.decisions.iter_mut().find(|d| d.claim_id == claim_id)
    }

    /// Eski rapor bicimine kopru (`JudgeVerdict.grounding_report`).
    pub fn to_report(&self) -> GroundingReport {
        let claims: Vec<ClaimResult> = self
            .decisions
            .iter()
            .map(|d| ClaimResult {
                claim_id: d.claim_id.clone(),
                grounded: d.accepted,
                evidence_refs: d.verified_refs.clone(),
                reason: d.reason(),
            })
            .collect();
        GroundingReport {
            total_claims: claims.len(),
            passed_claims: claims.iter().filter(|c| c.grounded).count(),
            failed_claims: claims.iter().filter(|c| !c.grounded).count(),
            claims,
        }
    }
}

/// Referanslarin cozuldugu havuz: `tool_call_id -> ToolOutput`.
#[derive(Debug, Clone, Default)]
pub struct EvidencePool {
    outputs: HashMap<String, ToolOutput>,
}

impl EvidencePool {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, output: ToolOutput) {
        self.outputs.insert(output.tool_call_id.clone(), output);
    }

    pub fn with(mut self, output: ToolOutput) -> Self {
        self.insert(output);
        self
    }

    pub fn from_outputs(outputs: impl IntoIterator<Item = ToolOutput>) -> Self {
        let mut pool = Self::new();
        for o in outputs {
            pool.insert(o);
        }
        pool
    }

    pub fn get(&self, tool_call_id: &str) -> Option<&ToolOutput> {
        self.outputs.get(tool_call_id)
    }

    pub fn len(&self) -> usize {
        self.outputs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.outputs.is_empty()
    }

    pub fn ids(&self) -> Vec<&str> {
        self.outputs.keys().map(|k| k.as_str()).collect()
    }
}

/// Yanlislamaci kanit kapisi. Durumsuzdur; ayni girdi -> ayni cikti.
#[derive(Debug, Clone, Copy, Default)]
pub struct GroundingGate;

impl GroundingGate {
    pub fn new() -> Self {
        Self
    }

    /// Tek iddiaya karsi tum curutme denemelerini kosar.
    /// Bos vektor = curutulemedi = kabul.
    pub fn refute(&self, claim: &FactualClaim, pool: &EvidencePool) -> Vec<Refutation> {
        let mut refutations: Vec<Refutation> = Vec::new();

        // Curutme 1: kanit referansi yok (I8).
        if claim.refs.is_empty() {
            refutations.push(Refutation::MissingReference);
            return refutations;
        }

        // Curutme 2-7: her referansin yapisal denetimi.
        let mut span = String::new();
        for r in &claim.refs {
            match self.check_ref(r, pool) {
                Ok(text) => {
                    span.push_str(&text);
                    span.push(' ');
                }
                Err(found) => {
                    for f in found {
                        if !refutations.contains(&f) {
                            refutations.push(f);
                        }
                    }
                }
            }
        }

        if span.trim().is_empty() {
            // Yapisal denetimi gecen tasiyici aralik kalmadi.
            if refutations.is_empty() {
                refutations.push(Refutation::NoVerifiableContent);
            }
            return refutations;
        }

        // Curutme 8-10: iddianin icerigi gercekten aralikta yaziyor mu?
        let span_lower = span.to_lowercase();
        let terms = content_terms(&claim.statement);
        let numbers = numeric_literals(&claim.statement);

        if terms.is_empty() && numbers.is_empty() {
            refutations.push(Refutation::NoVerifiableContent);
        }
        for term in terms {
            if !word_boundary_contains(&span_lower, &term) {
                refutations.push(Refutation::TermNotInSpan { term });
            }
        }
        for literal in numbers {
            if !word_boundary_contains(&span_lower, &literal) {
                refutations.push(Refutation::NumberNotInSpan { literal });
            }
        }
        if has_negation(&claim.statement.to_lowercase()) != has_negation(&span_lower) {
            refutations.push(Refutation::NegationFlip);
        }

        refutations
    }

    /// Tek iddianin karari.
    pub fn verify_claim(&self, claim: &FactualClaim, pool: &EvidencePool) -> ClaimDecision {
        let refutations = self.refute(claim, pool);
        let verified_refs = claim
            .refs
            .iter()
            .filter(|r| self.check_ref(r, pool).is_ok())
            .map(|r| format!("{}[{}..{}]", r.tool_call_id, r.start, r.end))
            .collect();
        let accepted = refutations.is_empty();

        debug!(
            "claim '{}': accepted={}, refutations={}",
            claim.id,
            accepted,
            refutations.len()
        );

        ClaimDecision {
            claim_id: claim.id.clone(),
            accepted,
            refutations,
            verified_refs,
        }
    }

    /// Iddia kumesinin karari.
    pub fn verify(&self, claims: &[FactualClaim], pool: &EvidencePool) -> GroundingOutcome {
        GroundingOutcome {
            decisions: claims
                .iter()
                .map(|c| self.verify_claim(c, pool))
                .collect(),
        }
    }

    /// Referansin yapisal denetimi; basarili ise aralikta yazan gercek metin.
    fn check_ref(&self, r: &EvidenceRef, pool: &EvidencePool) -> Result<String, Vec<Refutation>> {
        let Some(output) = pool.get(&r.tool_call_id) else {
            return Err(vec![Refutation::UnknownSource {
                tool_call_id: r.tool_call_id.clone(),
            }]);
        };

        if !output.success {
            return Err(vec![Refutation::FailedToolEvidence {
                tool_call_id: r.tool_call_id.clone(),
            }]);
        }

        if r.start >= r.end {
            return Err(vec![Refutation::EmptyRange {
                tool_call_id: r.tool_call_id.clone(),
                start: r.start,
                end: r.end,
            }]);
        }

        let len = output.output_text.len();
        if r.end > len {
            return Err(vec![Refutation::OutOfBounds {
                tool_call_id: r.tool_call_id.clone(),
                end: r.end,
                len,
            }]);
        }

        // `get` panik atmaz; karakter sinirini tutmayan aralikta `None` doner (I6).
        let Some(actual) = output.output_text.get(r.start..r.end) else {
            return Err(vec![Refutation::NotCharBoundary {
                tool_call_id: r.tool_call_id.clone(),
            }]);
        };

        if actual != r.quote {
            return Err(vec![Refutation::QuoteMismatch {
                tool_call_id: r.tool_call_id.clone(),
            }]);
        }

        if actual.trim().is_empty() {
            return Err(vec![Refutation::BlankSpan {
                tool_call_id: r.tool_call_id.clone(),
            }]);
        }

        Ok(actual.to_string())
    }
}

// ---------------------------------------------------------------------------
// Sozcuk/terim yardimcilari — regex derlemesi yok, hepsi deterministik tarama.
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
pub fn content_terms(statement: &str) -> Vec<String> {
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

/// Iddiadaki sayisal degerler (surum, port, adres, oran...). Uydurma sayi
/// halusinasyonun en sik bicimi oldugu icin ayri ve tam eslesmeli kontrol.
pub fn numeric_literals(statement: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in tokens(statement) {
        if t.chars().any(|c| c.is_ascii_digit()) {
            push_unique(&mut out, t);
        }
    }
    out
}

/// Metinde olumsuzluk isaretcisi var mi (girdi kucuk harf olmali).
pub fn has_negation(text_lower: &str) -> bool {
    NEGATION_MARKERS
        .iter()
        .any(|m| word_boundary_contains(text_lower, m))
}

// ---------------------------------------------------------------------------
// Eski (gevsek) yuzey — `executor.rs` kanit havuzunu bu tiplerle tasiyor.
// Yeni kod `GroundingGate` + `FactualClaim` kullanir; buradaki eslesme artik
// **birebir kaynak kimligi** uzerinden yapilir, serbest metin aramasi yok.
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum GroundingError {
    #[error("claim not grounded: {0}")]
    NotGrounded(String),
    #[error("evidence not found: {0}")]
    EvidenceNotFound(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claim {
    pub id: String,
    pub statement: String,
    /// Kabul edilen kanit kaynagi kimlikleri (birebir eslesme).
    pub expected_evidence: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evidence {
    pub source: String,
    pub content: String,
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Verdict {
    Supported,
    Unsupported,
    Ambiguous,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroundingReport {
    pub total_claims: usize,
    pub passed_claims: usize,
    pub failed_claims: usize,
    pub claims: Vec<ClaimResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimResult {
    pub claim_id: String,
    pub grounded: bool,
    pub evidence_refs: Vec<String>,
    pub reason: String,
}

#[derive(Default)]
pub struct GroundingVerifier;

impl GroundingVerifier {
    pub fn new() -> Self {
        Self
    }

    /// Kaynak kimligi **birebir** eslesmeli; benzer metin kanit sayilmaz.
    pub fn verify_claim(&self, claim: &Claim, evidence_pool: &[Evidence]) -> ClaimResult {
        let matching: Vec<&Evidence> = evidence_pool
            .iter()
            .filter(|e| {
                claim.expected_evidence.iter().any(|want| {
                    want == &e.source || e.tool_call_id.as_deref() == Some(want.as_str())
                })
            })
            .collect();

        let evidence_refs: Vec<String> = matching.iter().map(|e| e.source.clone()).collect();
        let grounded = !evidence_refs.is_empty();
        let reason = if grounded {
            format!("{} evidence source(s) matched by id", evidence_refs.len())
        } else {
            "no evidence source matched the claim references".to_string()
        };

        ClaimResult {
            claim_id: claim.id.clone(),
            grounded,
            evidence_refs,
            reason,
        }
    }

    /// Serbest metin iddiasi icin kaba on-eleme. **Kapi degildir**: kanit
    /// zorunlulugu `GroundingGate` ile uygulanir.
    pub fn verify_claim_str(&self, claim: &str, evidence: &[ToolOutput]) -> Verdict {
        let terms = content_terms(claim);
        if terms.is_empty() {
            return Verdict::Ambiguous;
        }

        let mut matched = 0usize;
        for term in &terms {
            let hit = evidence.iter().any(|o| {
                o.success
                    && (word_boundary_contains(&o.output_text.to_lowercase(), term)
                        || word_boundary_contains(&o.tool_name.to_lowercase(), term))
            });
            if hit {
                matched += 1;
            }
        }

        if matched == terms.len() {
            Verdict::Supported
        } else if matched > 0 {
            Verdict::Ambiguous
        } else {
            Verdict::Unsupported
        }
    }

    pub fn verify_claims(&self, claims: &[Claim], evidence_pool: &[Evidence]) -> GroundingReport {
        let results: Vec<ClaimResult> = claims
            .iter()
            .map(|c| self.verify_claim(c, evidence_pool))
            .collect();

        let total_claims = results.len();
        let passed_claims = results.iter().filter(|r| r.grounded).count();

        GroundingReport {
            total_claims,
            passed_claims,
            failed_claims: total_claims - passed_claims,
            claims: results,
        }
    }

    pub fn verify_iterative(
        &self,
        claims: &[Claim],
        evidence_pool: &[Evidence],
        require_all: bool,
    ) -> Result<GroundingReport, GroundingError> {
        let report = self.verify_claims(claims, evidence_pool);
        if require_all && report.failed_claims > 0 {
            let first = report
                .claims
                .iter()
                .find(|c| !c.grounded)
                .map(|c| c.claim_id.clone())
                .unwrap_or_default();
            return Err(GroundingError::NotGrounded(first));
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool() -> EvidencePool {
        EvidencePool::from_outputs([
            ToolOutput::new(
                "call-1",
                "read_file",
                "listen address is 192.168.1.10 and port is 8080",
                true,
            ),
            ToolOutput::new("call-2", "run_tests", "build failed: 3 tests broken", true),
            ToolOutput::new("call-3", "http_get", "connection refused", false),
        ])
    }

    fn span_of(pool: &EvidencePool, id: &str, needle: &str) -> EvidenceRef {
        // Testte aralik hesaplamasi: kaynakta gercekten var olan alt-dizi.
        let out = pool.get(id).expect("test fixture output must exist");
        let start = out
            .output_text
            .find(needle)
            .expect("test fixture needle must exist");
        EvidenceRef::new(id, start, start + needle.len(), needle)
    }

    #[test]
    fn accepts_claim_backed_by_exact_range() {
        let pool = pool();
        let claim = FactualClaim::new(
            "c1",
            "listen address is 192.168.1.10",
            vec![span_of(&pool, "call-1", "listen address is 192.168.1.10")],
        );
        let d = GroundingGate::new().verify_claim(&claim, &pool);
        assert!(d.accepted, "refutations: {:?}", d.refutations);
    }

    #[test]
    fn rejects_claim_without_reference() {
        let pool = pool();
        let claim = FactualClaim::unreferenced("c2", "the deployment is healthy");
        let d = GroundingGate::new().verify_claim(&claim, &pool);
        assert!(!d.accepted);
        assert_eq!(d.refutations, vec![Refutation::MissingReference]);
    }

    #[test]
    fn rejects_unknown_source() {
        let pool = pool();
        let claim = FactualClaim::new(
            "c3",
            "port is 8080",
            vec![EvidenceRef::new("call-999", 0, 12, "port is 8080")],
        );
        let d = GroundingGate::new().verify_claim(&claim, &pool);
        assert!(!d.accepted);
        assert!(matches!(
            d.refutations.first(),
            Some(Refutation::UnknownSource { .. })
        ));
    }

    #[test]
    fn rejects_out_of_bounds_range() {
        let pool = pool();
        let claim = FactualClaim::new(
            "c4",
            "port is 8080",
            vec![EvidenceRef::new("call-1", 0, 9_999, "port is 8080")],
        );
        let d = GroundingGate::new().verify_claim(&claim, &pool);
        assert!(!d.accepted);
        assert!(matches!(
            d.refutations.first(),
            Some(Refutation::OutOfBounds { .. })
        ));
    }

    #[test]
    fn rejects_fabricated_quote() {
        let pool = pool();
        let claim = FactualClaim::new(
            "c5",
            "port is 9090",
            vec![EvidenceRef::new("call-1", 0, 12, "port is 9090")],
        );
        let d = GroundingGate::new().verify_claim(&claim, &pool);
        assert!(!d.accepted);
        assert!(matches!(
            d.refutations.first(),
            Some(Refutation::QuoteMismatch { .. })
        ));
    }

    #[test]
    fn rejects_hallucinated_number_within_valid_span() {
        let pool = pool();
        let claim = FactualClaim::new(
            "c6",
            "listen address is 10.0.0.1",
            vec![span_of(&pool, "call-1", "listen address is 192.168.1.10")],
        );
        let d = GroundingGate::new().verify_claim(&claim, &pool);
        assert!(!d.accepted);
        assert!(d
            .refutations
            .iter()
            .any(|r| matches!(r, Refutation::NumberNotInSpan { .. })));
    }

    #[test]
    fn rejects_negation_flip() {
        let pool = pool();
        let claim = FactualClaim::new(
            "c7",
            "build did not fail",
            vec![span_of(&pool, "call-2", "build failed")],
        );
        let d = GroundingGate::new().verify_claim(&claim, &pool);
        assert!(!d.accepted);
    }

    #[test]
    fn rejects_evidence_from_failed_tool() {
        let pool = pool();
        let claim = FactualClaim::new(
            "c8",
            "connection refused",
            vec![EvidenceRef::new("call-3", 0, 18, "connection refused")],
        );
        let d = GroundingGate::new().verify_claim(&claim, &pool);
        assert!(!d.accepted);
        assert!(matches!(
            d.refutations.first(),
            Some(Refutation::FailedToolEvidence { .. })
        ));
    }

    #[test]
    fn rejects_blank_span() {
        let mut pool = EvidencePool::new();
        pool.insert(ToolOutput::new("call-b", "noop", "a    b", true));
        let claim = FactualClaim::new(
            "c9",
            "service healthy",
            vec![EvidenceRef::new("call-b", 1, 5, "    ")],
        );
        let d = GroundingGate::new().verify_claim(&claim, &pool);
        assert!(!d.accepted);
    }

    #[test]
    fn rejects_non_char_boundary_range() {
        let mut pool = EvidencePool::new();
        pool.insert(ToolOutput::new("call-u", "read_file", "ölçüm tamam", true));
        let claim = FactualClaim::new(
            "c10",
            "ölçüm tamam",
            vec![EvidenceRef::new("call-u", 1, 6, "lçüm")],
        );
        let d = GroundingGate::new().verify_claim(&claim, &pool);
        assert!(!d.accepted);
        assert!(matches!(
            d.refutations.first(),
            Some(Refutation::NotCharBoundary { .. })
        ));
    }

    #[test]
    fn rejects_claim_with_no_verifiable_content() {
        let pool = pool();
        let claim = FactualClaim::new(
            "c11",
            "it is ok",
            vec![span_of(&pool, "call-1", "port is 8080")],
        );
        let d = GroundingGate::new().verify_claim(&claim, &pool);
        assert!(!d.accepted);
        assert!(d
            .refutations
            .contains(&Refutation::NoVerifiableContent));
    }

    #[test]
    fn one_bad_reference_sinks_the_whole_claim() {
        let pool = pool();
        let claim = FactualClaim::new(
            "c12",
            "port is 8080",
            vec![
                span_of(&pool, "call-1", "port is 8080"),
                EvidenceRef::new("call-nope", 0, 4, "port"),
            ],
        );
        let d = GroundingGate::new().verify_claim(&claim, &pool);
        assert!(!d.accepted, "tolerance must be zero");
    }

    #[test]
    fn outcome_aggregates_and_bridges_to_report() {
        let pool = pool();
        let claims = vec![
            FactualClaim::new(
                "ok",
                "port is 8080",
                vec![span_of(&pool, "call-1", "port is 8080")],
            ),
            FactualClaim::unreferenced("bad", "everything is fine"),
        ];
        let outcome = GroundingGate::new().verify(&claims, &pool);
        assert_eq!(outcome.total(), 2);
        assert_eq!(outcome.accepted(), 1);
        assert_eq!(outcome.rejected(), 1);
        assert!(!outcome.all_accepted());
        assert_eq!(outcome.rejected_ids(), vec!["bad"]);

        let report = outcome.to_report();
        assert_eq!(report.total_claims, 2);
        assert_eq!(report.passed_claims, 1);
        assert_eq!(report.failed_claims, 1);
    }

    #[test]
    fn verbatim_claim_is_self_grounding() {
        let out = ToolOutput::new("call-v", "read_file", "revision 0008 applied", true);
        let pool = EvidencePool::from_outputs([out.clone()]);
        let claim = FactualClaim::verbatim("v1", &out);
        assert!(GroundingGate::new().verify_claim(&claim, &pool).accepted);
    }

    #[test]
    fn legacy_verifier_matches_by_id_only() {
        let verifier = GroundingVerifier::new();
        let evidence = vec![Evidence {
            source: "tool:step-1".into(),
            content: "the server is running".into(),
            tool_call_id: Some("step-1".into()),
        }];

        let by_id = Claim {
            id: "l1".into(),
            statement: "server is running".into(),
            expected_evidence: vec!["tool:step-1".into()],
        };
        assert!(verifier.verify_claim(&by_id, &evidence).grounded);

        // Serbest metin benzerligi artik kanit sayilmaz.
        let by_text = Claim {
            id: "l2".into(),
            statement: "server is running".into(),
            expected_evidence: vec!["running".into()],
        };
        assert!(!verifier.verify_claim(&by_text, &evidence).grounded);
    }

    #[test]
    fn legacy_iterative_requires_all() {
        let verifier = GroundingVerifier::new();
        let claims = vec![Claim {
            id: "l3".into(),
            statement: "nothing matches".into(),
            expected_evidence: vec!["absent-source".into()],
        }];
        let evidence = vec![Evidence {
            source: "tool:step-1".into(),
            content: "content".into(),
            tool_call_id: None,
        }];
        assert!(verifier.verify_iterative(&claims, &evidence, true).is_err());
        assert!(verifier.verify_iterative(&claims, &evidence, false).is_ok());
    }

    #[test]
    fn word_boundary_rejects_substring_hits() {
        assert!(!word_boundary_contains("the assembly failed", "assemble"));
        assert!(word_boundary_contains("the assembly failed", "assembly"));
    }
}
