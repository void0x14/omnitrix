//! Baglam sikistirma — "3.1 uyku" yolu (MASTER-PLAN Bolum 2, 7.1).
//!
//! Bolum 2 sozlesmesi: `xai-grok-compaction` + `xai-token-estimation` →
//! *compaction politikasi + token tahmini* → `omni-scheduler`, `omni-agent`.
//! Bu modul o sozlesmenin **omni-agent** yakasidir.
//!
//! ## Ne yapar
//!
//! Ajan `Sleeping` tier'ina gecerken (7.1) baglami RAM'de tutmak yasaktir;
//! baglam **sikistirilir** ve CAS'a alinir, geriye yalnizca `context_ref`
//! kalir. Uyaninca geri yuklenir.
//!
//! **IS KAYBI YOK (K2).** Bunun mekanigi burada su sekilde garanti edilir:
//! [`CompactedContext`] tam anlik goruntuyu (`full`) **hicbir turu silmeden**
//! tasir; sikistirma yalnizca `split_idx` + `digest` alanlarinda gorunur.
//! Yani:
//!
//! - [`ContextCompactor::restore`] → **tam** geri yukleme (kayipsiz).
//! - [`ContextCompactor::restore_compacted`] → daraltilmis gorunum (ucuz
//!   devam; eski turlar ozete iner ama blob'ta durmaya devam eder).
//!
//! Sikistirmanin kendisi hicbir turu **atmaz** — atsaydi "is kaybi yok"
//! kurali blob seviyesinde ihlal edilirdi.
//!
//! ## Kimin isi ne
//!
//! - Bolme noktasi: `xai_grok_compaction::select_turns_to_compact` —
//!   tool-istegi/tool-sonucu ciftlerini asla ayirmaz (ayirsa saglayici 400
//!   doner).
//! - Token tahmini: `xai-token-estimation` (bytes/4 sezgiseli) ve ayni
//!   sezgisele oturan `xai_chat_state::estimate_item_tokens`.
//! - Ozet metni temizligi: `xai_grok_compaction::format_compact_summary`.
//! - CAS yazimi/okumasi: **burada degil.** Bu modul yalnizca bayt uretir
//!   ([`CompactedContext::to_blob`]) ve baytlardan geri okur
//!   ([`CompactedContext::from_blob`]); blob'u CAS'a koymak `omni-scheduler`
//!   tier makinesinin isidir (7.1/I7: yan etki oncesi niyet kaydi orada).
//!
//! ## `with_compaction_policy` baglantisi
//!
//! Faz 1'de vendored `AgentBuilder::with_compaction_policy` bilincli olarak
//! disarida birakilmisti (bkz. `session.rs` bas yorumu). Artik sirasi geldi:
//! [`CompactionPolicySpec`] omni tarafinin politikasidir ve
//! [`AgentBuilderCompactionExt::with_compaction_policy_spec`] onu vendored
//! `CompactionPolicy`'ye cevirip builder'a baglar.
//!
//! I5: bu dosyada literal model adi ya da fiyat YOKTUR —
//! [`CompactionPolicySpec::compact_model`] daima yapilandirmadan gelir,
//! `None` ise oturumun kendi modeli kullanilir.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use xai_chat_state::{ChatStateSnapshot, Credentials, estimate_item_tokens};
use xai_grok_agent::{AgentBuilder, CompactionPolicy};
use xai_grok_compaction::{
    CompactionItem, CompactionItemFactory, CompactionRole, DEFAULT_AUTO_COMPACT_THRESHOLD_PERCENT,
    SplitPlan, format_compact_summary, select_turns_to_compact,
};

use crate::error::AgentError;

/// Ajanin baglam durumu.
///
/// `xai-chat-state` aktoru durumu icerde `pub(crate) struct ChatState` olarak
/// tutar; disariya verdigi tasinabilir/serilestirilebilir bicim
/// [`ChatStateSnapshot`]'tir. Sikistirma her zaman bu tasinabilir bicim
/// uzerinde calisir (blob'a giden de odur), bu yuzden omni tarafinin adi
/// `ChatState`'tir.
pub type ChatState = ChatStateSnapshot;

/// [`CompactedContext`] sema surumu. Blob CAS'ta kalicidir; alan eklendiginde
/// artirilir ve eski surumler okunmaya devam eder.
pub const COMPACTED_CONTEXT_SCHEMA_VERSION: u32 = 1;

/// Sezgisel ozette tur basina alinan azami karakter.
const HEURISTIC_TURN_CHARS: usize = 320;

/// Ozet blogunun basligi. Model adi ya da saglayici icermez (I5).
const DIGEST_HEADER: &str = "Onceki turlar sikistirildi ve tam haliyle CAS'ta saklandi.";

/// Hata yardimcisi.
///
/// `AgentError` bu modulun sahibi degildir (o `error.rs`'te durur ve bu is
/// paketinde bana ait degil), bu yuzden sikistirma hatalari
/// [`AgentError::InvalidSession`] uzerinden tasinir; ayirt edilebilsin diye
/// sabit bir onek konur.
fn compaction_error(detail: impl AsRef<str>) -> AgentError {
    AgentError::InvalidSession(format!("baglam sikistirma: {}", detail.as_ref()))
}

/// Epoch milisaniye. I6: panik yok — saat geriye giderse 0 doner.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| u64::try_from(d.as_millis()).ok())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Politika
// ---------------------------------------------------------------------------

/// Omni tarafinin sikistirma politikasi.
///
/// Vendored [`CompactionPolicy`] yalnizca "ne zaman + hangi model" sorusunu
/// yanitlar; bolme plani icin gereken tampon degerleri (kuyrukta tutulacak
/// token, sikistirmaya deger asgari token) orada yoktur. Bu tip ikisini bir
/// arada tasir ve [`Self::to_vendor_policy`] ile vendored bicime cevrilir.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionPolicySpec {
    /// Baglam penceresinin yuzde kaci dolunca otomatik sikistirma tetiklenir.
    pub auto_compact_threshold_percent: u8,
    /// Sikistirmadan sonra **aynen** korunacak kuyruk butcesi (token).
    pub keep_tail_tokens: u32,
    /// Bu kadar token birikmediyse sikistirma yapilmaz (LLM'e degmez).
    pub min_compactable_tokens: u32,
    /// Ozet uretimi icin kullanilacak model **adi yapilandirmadan gelir**;
    /// `None` = oturumun kendi modeli (I5: burada literal ad yok).
    pub compact_model: Option<String>,
    /// Sikistirmadan once bellek bosaltma turu kosulsun mu.
    pub memory_flush_enabled: bool,
    /// Sikistirma basina duvar saati butcesi (saniye).
    pub wall_clock_budget_secs: u64,
    /// Iki gecisli (prefire) sikistirma.
    pub two_pass_enabled: bool,
    /// Uretilen ozetin azami karakter uzunlugu.
    pub max_digest_chars: usize,
}

impl Default for CompactionPolicySpec {
    fn default() -> Self {
        Self {
            auto_compact_threshold_percent: DEFAULT_AUTO_COMPACT_THRESHOLD_PERCENT,
            keep_tail_tokens: 16_384,
            min_compactable_tokens: 4_096,
            compact_model: None,
            memory_flush_enabled: false,
            wall_clock_budget_secs: 300,
            two_pass_enabled: false,
            max_digest_chars: 8_192,
        }
    }
}

impl CompactionPolicySpec {
    /// Esik yuzdesini degistirir.
    #[must_use]
    pub fn with_threshold_percent(mut self, percent: u8) -> Self {
        self.auto_compact_threshold_percent = percent;
        self
    }

    /// Korunacak kuyruk butcesini degistirir.
    #[must_use]
    pub fn with_keep_tail_tokens(mut self, tokens: u32) -> Self {
        self.keep_tail_tokens = tokens;
        self
    }

    /// Sikistirmaya deger asgari token miktarini degistirir.
    #[must_use]
    pub fn with_min_compactable_tokens(mut self, tokens: u32) -> Self {
        self.min_compactable_tokens = tokens;
        self
    }

    /// Ozet modelini **yapilandirmadan gelen** adla baglar (I5).
    #[must_use]
    pub fn with_compact_model(mut self, model: Option<String>) -> Self {
        self.compact_model = model;
        self
    }

    /// Politikayi dogrular. I6: panik yok, hata `Result` ile.
    pub fn validate(&self) -> Result<(), AgentError> {
        if self.auto_compact_threshold_percent == 0 || self.auto_compact_threshold_percent > 100 {
            return Err(compaction_error(format!(
                "esik yuzdesi 1..=100 araliginda olmali: {}",
                self.auto_compact_threshold_percent
            )));
        }
        if self.keep_tail_tokens == 0 {
            return Err(compaction_error("korunacak kuyruk butcesi sifir olamaz"));
        }
        if self.wall_clock_budget_secs == 0 {
            return Err(compaction_error("duvar saati butcesi sifir olamaz"));
        }
        if self.max_digest_chars == 0 {
            return Err(compaction_error("ozet karakter tavani sifir olamaz"));
        }
        if let Some(model) = &self.compact_model
            && model.trim().is_empty()
        {
            return Err(compaction_error("ozet modeli adi bos"));
        }
        Ok(())
    }

    /// Vendored bicime cevirir — `AgentBuilder::with_compaction_policy` girdisi.
    #[must_use]
    pub fn to_vendor_policy(&self) -> CompactionPolicy {
        CompactionPolicy {
            auto_compact_threshold_percent: u32::from(self.auto_compact_threshold_percent),
            compact_model: self.compact_model.clone(),
            memory_flush_enabled: self.memory_flush_enabled,
            wall_clock_budget_secs: self.wall_clock_budget_secs,
            two_pass_enabled: self.two_pass_enabled,
        }
    }
}

/// `AgentBuilder`'a sikistirma politikasini baglayan uzanti.
///
/// Faz 1'de bilincli disarida birakilan `with_compaction_policy` bu uzanti
/// uzerinden devreye girer. `AgentSpec` kurulum akisinda kullanimi:
///
/// ```ignore
/// use omni_agent::compaction::{AgentBuilderCompactionExt, CompactionPolicySpec};
///
/// let policy = CompactionPolicySpec::default();
/// let builder = AgentBuilder::new(cwd, terminal, notify)
///     .from_definition(definition)
///     .with_compaction_policy_spec(&policy)?;
/// ```
pub trait AgentBuilderCompactionExt: Sized {
    /// Politikayi dogrular ve vendored `with_compaction_policy`'ye baglar.
    fn with_compaction_policy_spec(self, spec: &CompactionPolicySpec) -> Result<Self, AgentError>;
}

impl AgentBuilderCompactionExt for AgentBuilder {
    fn with_compaction_policy_spec(self, spec: &CompactionPolicySpec) -> Result<Self, AgentError> {
        spec.validate()?;
        Ok(self.with_compaction_policy(spec.to_vendor_policy()))
    }
}

// ---------------------------------------------------------------------------
// Ozet uretici seam
// ---------------------------------------------------------------------------

/// Ozet uretimi seam'i — LLM cagrisi buradan gecer.
///
/// Varsayilan uygulama yoktur: [`ContextCompactor`] ozet ureticisi
/// baglanmadigi surece **deterministik sezgisel** ozeti kullanir (LLM'siz).
/// Boylece uyku yolu saglayici erisimi olmadan da calisir; is kaybi zaten
/// ozetin kalitesine bagli degildir (tam kayit blob'ta durur).
#[async_trait]
pub trait ContextDigest: Send + Sync {
    /// Sikistirilan onekten ozet uretir. Donen ham metin
    /// [`format_compact_summary`] ile temizlenir.
    async fn digest(&self, transcript: &str) -> Result<String, AgentError>;
}

// ---------------------------------------------------------------------------
// Sikistirilmis baglam
// ---------------------------------------------------------------------------

/// Uyku sirasinda CAS'a giden kayit.
///
/// `full` **tam** anlik goruntudur: hicbir tur silinmez (K2). Sikistirma
/// yalnizca `split_idx` + `digest` alanlarinda gorunur.
///
/// Sir hijyeni: `full.credentials` blob'a yazilmadan once temizlenir. Geri
/// yukleyen taraf kimlik bilgilerini kendisi enjekte eder
/// ([`ContextCompactor::restore_with_credentials`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactedContext {
    /// Sema surumu ([`COMPACTED_CONTEXT_SCHEMA_VERSION`]).
    pub schema_version: u32,
    /// Tam anlik goruntu — kimlik bilgileri temizlenmis.
    pub full: ChatState,
    /// `full.conversation[..split_idx]` sikistirildi, `[split_idx..]` aynen
    /// korunuyor. `0` = bolmeye deger bir onek yoktu.
    pub split_idx: usize,
    /// Sikistirilan onegin ozeti. `split_idx == 0` iken bostur.
    pub digest: String,
    /// Sikistirma oncesi token tahmini.
    pub tokens_before: usize,
    /// Daraltilmis gorunumun token tahmini (ozet + korunan kuyruk).
    pub tokens_after: usize,
    /// Sikistirmanin yapildigi an (epoch ms).
    pub compacted_at_ms: u64,
}

impl CompactedContext {
    /// CAS'a yazilacak baytlar.
    pub fn to_blob(&self) -> Result<Vec<u8>, AgentError> {
        serde_json::to_vec(self).map_err(|e| compaction_error(format!("blob serilestirme: {e}")))
    }

    /// CAS'tan okunan baytlardan geri kurar.
    pub fn from_blob(bytes: &[u8]) -> Result<Self, AgentError> {
        let parsed: Self = serde_json::from_slice(bytes)
            .map_err(|e| compaction_error(format!("blob cozumleme: {e}")))?;
        if parsed.schema_version > COMPACTED_CONTEXT_SCHEMA_VERSION {
            return Err(compaction_error(format!(
                "bilinmeyen sema surumu: {} (destek: {})",
                parsed.schema_version, COMPACTED_CONTEXT_SCHEMA_VERSION
            )));
        }
        Ok(parsed)
    }

    /// Sikistirma gercekten bir sey kazandirdi mi.
    #[must_use]
    pub fn is_effective(&self) -> bool {
        self.split_idx > 0 && self.tokens_after < self.tokens_before
    }

    /// Kazanilan token (daraltilmis gorunume gecildiginde).
    #[must_use]
    pub fn saved_tokens(&self) -> usize {
        self.tokens_before.saturating_sub(self.tokens_after)
    }
}

// ---------------------------------------------------------------------------
// Sikistirici
// ---------------------------------------------------------------------------

/// Uyku sikistirmasini yuruten tip (MASTER-PLAN Bolum 2, 7.1).
pub struct ContextCompactor {
    policy: CompactionPolicySpec,
    digest: Option<Arc<dyn ContextDigest>>,
}

impl std::fmt::Debug for ContextCompactor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ContextCompactor")
            .field("policy", &self.policy)
            .field("digest", &self.digest.is_some())
            .finish()
    }
}

impl Default for ContextCompactor {
    fn default() -> Self {
        Self::new(CompactionPolicySpec::default())
    }
}

impl ContextCompactor {
    /// Politikayla kurar; ozet uretici baglanmadigi icin sezgisel ozet kullanilir.
    #[must_use]
    pub fn new(policy: CompactionPolicySpec) -> Self {
        Self {
            policy,
            digest: None,
        }
    }

    /// LLM ozet ureticisini baglar.
    #[must_use]
    pub fn with_digest(mut self, digest: Arc<dyn ContextDigest>) -> Self {
        self.digest = Some(digest);
        self
    }

    /// Yururlukteki politika.
    #[must_use]
    pub fn policy(&self) -> &CompactionPolicySpec {
        &self.policy
    }

    /// Baglamin token tahmini (`xai-token-estimation` bytes/4 sezgiseli).
    ///
    /// `xai_chat_state::estimate_item_tokens` her `ConversationItem` turunu
    /// ayni sezgisele gore sayar (goruntuler icin
    /// `xai_token_estimation::IMAGE_TOKEN_ESTIMATE`), boylece burada uretilen
    /// sayi vendored otomatik-sikistirma kapisinin gordugu sayiyla ayni olur.
    #[must_use]
    pub fn estimate_tokens(&self, ctx: &ChatState) -> usize {
        let total: u64 = ctx
            .conversation
            .iter()
            .map(estimate_item_tokens)
            .fold(0u64, u64::saturating_add);
        usize::try_from(total).unwrap_or(usize::MAX)
    }

    /// Baglam penceresinin yuzde kaci dolu.
    #[must_use]
    pub fn usage_percent(&self, ctx: &ChatState) -> u8 {
        let used = u64::try_from(self.estimate_tokens(ctx)).unwrap_or(u64::MAX);
        xai_token_estimation::usage_percentage_truncated_u8(
            used,
            ctx.sampling_config.context_window.get(),
        )
    }

    /// Politikanin esigi asildi mi — otomatik sikistirma kapisi.
    #[must_use]
    pub fn should_compact(&self, ctx: &ChatState) -> bool {
        let used = u64::try_from(self.estimate_tokens(ctx)).unwrap_or(u64::MAX);
        xai_token_estimation::exceeds_threshold(
            used,
            ctx.sampling_config.context_window.get(),
            self.policy.auto_compact_threshold_percent,
        )
    }

    /// Tur basina token tahminleri — [`select_turns_to_compact`] girdisi.
    fn item_token_counts(ctx: &ChatState) -> Vec<u32> {
        ctx.conversation
            .iter()
            .map(|item| u32::try_from(estimate_item_tokens(item)).unwrap_or(u32::MAX))
            .collect()
    }

    /// Bolme plani. `None` = sikistirmaya degmez.
    #[must_use]
    pub fn plan(&self, ctx: &ChatState) -> Option<SplitPlan> {
        let counts = Self::item_token_counts(ctx);
        select_turns_to_compact(
            &counts,
            &ctx.conversation,
            self.policy.keep_tail_tokens,
            self.policy.min_compactable_tokens,
        )
    }

    /// Sikistirilacak onegi ozet ureticiye verilecek metne cevirir.
    fn transcript(ctx: &ChatState, split_idx: usize) -> String {
        let mut out = String::new();
        for item in ctx.conversation.iter().take(split_idx) {
            let Some(text) = CompactionItem::text(item) else {
                continue;
            };
            let label = match CompactionItem::role(item) {
                CompactionRole::System => "system",
                CompactionRole::Developer => "developer",
                CompactionRole::User => "user",
                CompactionRole::Assistant => "assistant",
                CompactionRole::Tool => "tool",
            };
            out.push_str(label);
            out.push_str(": ");
            out.push_str(&text);
            out.push('\n');
        }
        out
    }

    /// LLM'siz, deterministik ozet: her turun bas kismi kirpilarak siralanir.
    ///
    /// Kalitesi LLM ozetinin altindadir ama **is kaybi uretmez** — tam kayit
    /// blob'ta durur ve [`ContextCompactor::restore`] onu aynen geri verir.
    fn heuristic_digest(&self, transcript: &str) -> String {
        let mut out = String::new();
        for line in transcript.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let clipped: String = trimmed.chars().take(HEURISTIC_TURN_CHARS).collect();
            out.push_str("- ");
            out.push_str(&clipped);
            if clipped.len() < trimmed.len() {
                out.push('…');
            }
            out.push('\n');
            if out.chars().count() >= self.policy.max_digest_chars {
                break;
            }
        }
        out
    }

    /// Ozet metnini uretir ve politika tavanina kirpar.
    async fn build_digest(&self, transcript: &str) -> Result<String, AgentError> {
        let raw = match &self.digest {
            Some(digest) => digest.digest(transcript).await?,
            None => self.heuristic_digest(transcript),
        };
        let cleaned = format_compact_summary(&raw);
        Ok(clip_chars(&cleaned, self.policy.max_digest_chars))
    }

    /// Baglami sikistirir (uyku yolu, 7.1).
    ///
    /// Sonuc CAS'a yazilmaya hazirdir ([`CompactedContext::to_blob`]).
    /// Bolmeye deger onek yoksa `split_idx == 0` olan, ozetsiz ama yine de
    /// **tam** bir kayit doner — uyku yolu her durumda ilerleyebilsin diye
    /// hata uretilmez.
    pub async fn compact(&self, ctx: &ChatState) -> Result<CompactedContext, AgentError> {
        self.policy.validate()?;

        let counts = Self::item_token_counts(ctx);
        let tokens_before = counts
            .iter()
            .map(|c| usize::try_from(*c).unwrap_or(usize::MAX))
            .fold(0usize, usize::saturating_add);

        let split_idx = select_turns_to_compact(
            &counts,
            &ctx.conversation,
            self.policy.keep_tail_tokens,
            self.policy.min_compactable_tokens,
        )
        .map_or(0, |plan| plan.split_idx)
        .min(ctx.conversation.len());

        let digest = if split_idx == 0 {
            String::new()
        } else {
            let transcript = Self::transcript(ctx, split_idx);
            self.build_digest(&transcript).await?
        };

        let kept_tokens = counts
            .iter()
            .skip(split_idx)
            .map(|c| usize::try_from(*c).unwrap_or(usize::MAX))
            .fold(0usize, usize::saturating_add);
        let digest_tokens =
            usize::try_from(xai_token_estimation::estimate_tokens(&digest)).unwrap_or(usize::MAX);

        // Sirlar blob'a gitmez; geri yukleyen taraf yeniden enjekte eder.
        let mut full = ctx.clone();
        full.credentials = Credentials::default();

        Ok(CompactedContext {
            schema_version: COMPACTED_CONTEXT_SCHEMA_VERSION,
            full,
            split_idx,
            digest,
            tokens_before,
            tokens_after: kept_tokens.saturating_add(digest_tokens),
            compacted_at_ms: now_ms(),
        })
    }

    /// Uyanma: **tam** geri yukleme (K2 — is kaybi yok).
    ///
    /// Kimlik bilgileri bostur; saglayici anahtarlari
    /// [`Self::restore_with_credentials`] ile geri konur.
    pub async fn restore(&self, c: &CompactedContext) -> Result<ChatState, AgentError> {
        if c.schema_version > COMPACTED_CONTEXT_SCHEMA_VERSION {
            return Err(compaction_error(format!(
                "bilinmeyen sema surumu: {}",
                c.schema_version
            )));
        }
        if c.split_idx > c.full.conversation.len() {
            return Err(compaction_error(format!(
                "bolme noktasi tur sayisini asiyor: {} > {}",
                c.split_idx,
                c.full.conversation.len()
            )));
        }
        Ok(c.full.clone())
    }

    /// Tam geri yukleme + kimlik bilgisi enjeksiyonu.
    pub async fn restore_with_credentials(
        &self,
        c: &CompactedContext,
        credentials: Credentials,
    ) -> Result<ChatState, AgentError> {
        let mut state = self.restore(c).await?;
        state.credentials = credentials;
        Ok(state)
    }

    /// Uyanma: **daraltilmis** gorunum (ucuz devam).
    ///
    /// Onekteki `System` turlari aynen korunur (sistem promptu kaybolmaz),
    /// ardindan ozet tasiyici bir sentetik tur gelir, sonra korunan kuyruk.
    /// Eski turlar yalnizca bu *gorunumden* dusar; blob'ta durmaya devam
    /// eder ve [`Self::restore`] onlari aynen geri verir.
    pub async fn restore_compacted(&self, c: &CompactedContext) -> Result<ChatState, AgentError> {
        let mut state = self.restore(c).await?;
        if c.split_idx == 0 || c.digest.is_empty() {
            return Ok(state);
        }

        let mut conversation = Vec::with_capacity(state.conversation.len() + 2);
        for item in state.conversation.iter().take(c.split_idx) {
            if matches!(CompactionItem::role(item), CompactionRole::System) {
                conversation.push(item.clone());
            }
        }
        conversation.push(CompactionItemFactory::new_user_meta(format!(
            "{DIGEST_HEADER}\n\n{}",
            c.digest
        )));
        conversation.extend_from_slice(&state.conversation[c.split_idx..]);

        state.conversation = conversation;
        state.last_compaction_prompt_index = Some(state.prompt_index);
        Ok(state)
    }
}

/// Metni karakter tavanina kirpar (bayt sinirinda bolme yok).
fn clip_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    text.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Testlerde `ChatState` serde uzerinden kurulur: `SamplingConfig`'in
    /// `Default`'u yoktur ve alanlari omni-agent'in bagimlisi olmayan
    /// tiplerdir. I5: model adi bos birakilir — literal ad yazilmaz.
    fn state_with(items: &str, context_window: u64) -> ChatState {
        let json = format!(
            r#"{{
                "conversation": [{items}],
                "sampling_config": {{
                    "base_url": "",
                    "model": "",
                    "max_completion_tokens": null,
                    "temperature": null,
                    "top_p": null,
                    "context_window": {context_window}
                }},
                "prompt_index": 0,
                "total_tokens": 0,
                "agent_edited_paths": [],
                "prompt_texts": [],
                "stream_start_ms": null,
                "turn_start_ms": null,
                "last_compaction_prompt_index": null
            }}"#
        );
        serde_json::from_str(&json).expect("test baglami cozumlenmeli")
    }

    fn user(text: &str) -> String {
        format!(r#"{{"type":"user","content":[{{"type":"text","text":"{text}"}}]}}"#)
    }

    fn assistant(text: &str) -> String {
        format!(r#"{{"type":"assistant","content":"{text}"}}"#)
    }

    fn system(text: &str) -> String {
        format!(r#"{{"type":"system","content":"{text}"}}"#)
    }

    /// Sikistirmayi kesin tetikleyecek uzunlukta bir konusma.
    fn long_conversation() -> String {
        let mut parts = vec![system(&"s".repeat(400))];
        for _ in 0..24 {
            parts.push(user(&"u".repeat(4_000)));
            parts.push(assistant(&"a".repeat(4_000)));
        }
        parts.join(",")
    }

    fn small_policy() -> CompactionPolicySpec {
        CompactionPolicySpec::default()
            .with_keep_tail_tokens(2_000)
            .with_min_compactable_tokens(500)
    }

    #[test]
    fn varsayilan_politika_gecerli() {
        assert!(CompactionPolicySpec::default().validate().is_ok());
    }

    #[test]
    fn gecersiz_esik_reddedilir() {
        let spec = CompactionPolicySpec::default().with_threshold_percent(0);
        assert!(matches!(
            spec.validate(),
            Err(AgentError::InvalidSession(_))
        ));
        let spec = CompactionPolicySpec::default().with_threshold_percent(101);
        assert!(spec.validate().is_err());
    }

    #[test]
    fn bos_ozet_modeli_reddedilir() {
        let spec = CompactionPolicySpec::default().with_compact_model(Some("  ".to_owned()));
        assert!(spec.validate().is_err());
    }

    #[test]
    fn vendored_politikaya_cevrilir() {
        let spec = CompactionPolicySpec::default()
            .with_threshold_percent(70)
            .with_compact_model(Some("configured-summarizer".to_owned()));
        let vendored = spec.to_vendor_policy();
        assert_eq!(vendored.auto_compact_threshold_percent, 70);
        assert_eq!(
            vendored.compact_model.as_deref(),
            Some("configured-summarizer")
        );
        assert_eq!(vendored.wall_clock_budget_secs, spec.wall_clock_budget_secs);
    }

    #[test]
    fn token_tahmini_metinle_buyur() {
        let compactor = ContextCompactor::default();
        let bos = state_with("", 100_000);
        let dolu = state_with(&user(&"x".repeat(4_000)), 100_000);
        assert_eq!(compactor.estimate_tokens(&bos), 0);
        assert_eq!(compactor.estimate_tokens(&dolu), 1_000);
    }

    #[test]
    fn esik_asilinca_sikistirma_istenir() {
        let compactor = ContextCompactor::new(CompactionPolicySpec::default());
        // 1000 token / 2000 pencere = %50 -> esik (%85) asilmaz.
        let dusuk = state_with(&user(&"x".repeat(4_000)), 2_000);
        assert!(!compactor.should_compact(&dusuk));
        assert_eq!(compactor.usage_percent(&dusuk), 50);

        // 1000 token / 1000 pencere = %100 -> asilir.
        let yuksek = state_with(&user(&"x".repeat(4_000)), 1_000);
        assert!(compactor.should_compact(&yuksek));
    }

    #[tokio::test]
    async fn sikistirma_tam_kaydi_korur() {
        let compactor = ContextCompactor::new(small_policy());
        let state = state_with(&long_conversation(), 200_000);
        let before = state.conversation.len();

        let compacted = compactor.compact(&state).await.expect("sikistirma");
        assert!(compacted.split_idx > 0, "onek sikistirilmis olmali");
        assert!(!compacted.digest.is_empty());
        // K2: hicbir tur silinmez.
        assert_eq!(compacted.full.conversation.len(), before);
        assert!(compacted.is_effective());
        assert!(compacted.saved_tokens() > 0);
    }

    #[tokio::test]
    async fn geri_yukleme_kayipsiz() {
        let compactor = ContextCompactor::new(small_policy());
        let state = state_with(&long_conversation(), 200_000);

        let compacted = compactor.compact(&state).await.expect("sikistirma");
        let restored = compactor.restore(&compacted).await.expect("geri yukleme");

        assert_eq!(restored.conversation.len(), state.conversation.len());
        assert_eq!(
            compactor.estimate_tokens(&restored),
            compactor.estimate_tokens(&state)
        );
    }

    #[tokio::test]
    async fn daraltilmis_gorunum_kucultur_ve_sistem_turunu_korur() {
        let compactor = ContextCompactor::new(small_policy());
        let state = state_with(&long_conversation(), 200_000);

        let compacted = compactor.compact(&state).await.expect("sikistirma");
        let view = compactor
            .restore_compacted(&compacted)
            .await
            .expect("daraltilmis gorunum");

        assert!(view.conversation.len() < state.conversation.len());
        assert!(matches!(
            CompactionItem::role(&view.conversation[0]),
            CompactionRole::System
        ));
        assert!(compactor.estimate_tokens(&view) < compactor.estimate_tokens(&state));
    }

    #[tokio::test]
    async fn kisa_baglam_sikistirilmaz() {
        let compactor = ContextCompactor::new(small_policy());
        let state = state_with(&user("merhaba"), 200_000);

        let compacted = compactor.compact(&state).await.expect("sikistirma");
        assert_eq!(compacted.split_idx, 0);
        assert!(compacted.digest.is_empty());
        assert!(!compacted.is_effective());
    }

    #[tokio::test]
    async fn sirlar_bloba_yazilmaz() {
        let compactor = ContextCompactor::new(small_policy());
        let mut state = state_with(&long_conversation(), 200_000);
        state.credentials.api_key = Some("gizli-anahtar".to_owned());

        let compacted = compactor.compact(&state).await.expect("sikistirma");
        assert!(compacted.full.credentials.api_key.is_none());

        let blob = compacted.to_blob().expect("blob");
        let text = String::from_utf8(blob).expect("utf8");
        assert!(!text.contains("gizli-anahtar"));
    }

    #[tokio::test]
    async fn blob_gidis_donusu() {
        let compactor = ContextCompactor::new(small_policy());
        let state = state_with(&long_conversation(), 200_000);

        let compacted = compactor.compact(&state).await.expect("sikistirma");
        let blob = compacted.to_blob().expect("blob");
        let parsed = CompactedContext::from_blob(&blob).expect("blob cozumleme");

        assert_eq!(parsed.split_idx, compacted.split_idx);
        assert_eq!(parsed.digest, compacted.digest);
        assert_eq!(
            parsed.full.conversation.len(),
            compacted.full.conversation.len()
        );
    }

    #[tokio::test]
    async fn ileri_sema_surumu_reddedilir() {
        let compactor = ContextCompactor::new(small_policy());
        let state = state_with(&long_conversation(), 200_000);
        let mut compacted = compactor.compact(&state).await.expect("sikistirma");
        compacted.schema_version = COMPACTED_CONTEXT_SCHEMA_VERSION + 1;

        assert!(compactor.restore(&compacted).await.is_err());

        let blob = serde_json::to_vec(&compacted).expect("blob");
        assert!(CompactedContext::from_blob(&blob).is_err());
    }

    struct StubDigest;

    #[async_trait]
    impl ContextDigest for StubDigest {
        async fn digest(&self, transcript: &str) -> Result<String, AgentError> {
            Ok(format!(
                "<summary>ozet: {} karakter</summary>",
                transcript.len()
            ))
        }
    }

    #[tokio::test]
    async fn ozet_uretici_baglanabilir() {
        let compactor = ContextCompactor::new(small_policy()).with_digest(Arc::new(StubDigest));
        let state = state_with(&long_conversation(), 200_000);

        let compacted = compactor.compact(&state).await.expect("sikistirma");
        assert!(compacted.digest.contains("ozet:"));
        // `format_compact_summary` <summary> etiketlerini soyar.
        assert!(!compacted.digest.contains("<summary>"));
    }

    struct FailingDigest;

    #[async_trait]
    impl ContextDigest for FailingDigest {
        async fn digest(&self, _transcript: &str) -> Result<String, AgentError> {
            Err(compaction_error("ozet uretici dustu"))
        }
    }

    #[tokio::test]
    async fn ozet_hatasi_yukselir() {
        let compactor = ContextCompactor::new(small_policy()).with_digest(Arc::new(FailingDigest));
        let state = state_with(&long_conversation(), 200_000);
        assert!(compactor.compact(&state).await.is_err());
    }

    #[tokio::test]
    async fn kimlik_bilgisi_geri_konabilir() {
        let compactor = ContextCompactor::new(small_policy());
        let state = state_with(&long_conversation(), 200_000);
        let compacted = compactor.compact(&state).await.expect("sikistirma");

        let creds = Credentials {
            api_key: Some("yeniden-enjekte".to_owned()),
            ..Credentials::default()
        };
        let restored = compactor
            .restore_with_credentials(&compacted, creds)
            .await
            .expect("geri yukleme");
        assert_eq!(
            restored.credentials.api_key.as_deref(),
            Some("yeniden-enjekte")
        );
    }

    #[test]
    fn bolme_plani_tool_ciftini_ayirmaz() {
        let compactor = ContextCompactor::new(small_policy());
        let state = state_with(&long_conversation(), 200_000);
        let plan = compactor.plan(&state).expect("plan");
        assert!(plan.split_idx > 0);
        assert!(plan.split_idx <= state.conversation.len());
        assert!(plan.tokens_to_compact > 0);
    }
}
