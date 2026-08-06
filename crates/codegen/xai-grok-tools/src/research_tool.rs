//! `grok_research` tool — arastirma motorunun grok tool sistemine eritilmis hali.
//!
//! omni-research katmani (`crates/omni/omni-research`) ayri bir kapi degildir;
//! arastirma dongusu bu dosyaya tasindi ve kapi artik grok'un tool sistemidir:
//! `grok_research` tool'u (surface | deep | ocean modlari).
//!
//! ## Eritilen parcalar
//!
//! | omni-research kaynagi | Bu dosya |
//! |---|---|
//! | `modes.rs` — `ResearchMode` / `ModeParams` | [`ResearchMode`] / [`ModeParams`] |
//! | `lib.rs` — `ResearchEngine::collect` dongusu | [`run_research`] |
//! | `lib.rs` — `refine_queries` / `tokenize` / `is_stopword` | ayni isimli fonksiyonlar |
//! | `provider.rs` — `ResearchProvider` / `McpResearchProvider` | ayni isimli tipler |
//! | `provider.rs` — `FieldMapping` / `findings_from_json` | ayni isimli tipler |
//!
//! Mod butceleri (tur sayilari gorev geregi: surface=1, deep=3, ocean=7):
//!
//! | Mod | `max_sources` | `crawl_depth` | `refine_rounds` | `per_query_results` | `refine_fanout` | `max_queries` |
//! |-----|---------------|---------------|-----------------|---------------------|-----------------|---------------|
//! | surface | 8 | 1 | 1 | 8 | 0 | 1 |
//! | deep | 40 | 2 | 3 | 12 | 3 | 12 |
//! | ocean | 160 | 4 | 7 | 20 | 6 | 60 |
//!
//! Mod yukseldikce dongu daha cok kaynak tarar, daha cok tur calistirir ve her
//! tur sonunda yeni (genisletilmis) sorgular uretir.
//!
//! ## Saglayici ve MCP tasimasi
//!
//! Tool, `SharedResources` icinden `Arc<dyn ResearchProvider>` okur; host
//! katmani (oturum) bunu enjekte eder. Saglayici yapilandirilmamissa tool
//! **panik etmez**, duzgun bir hata metni doner (I6).
//!
//! `xai-grok-mcp`'nin `McpClient`'i dogrudan kullanilamaz: `xai-grok-mcp`,
//! `xai-grok-tools`'a bagimlidir (Cargo.toml: `xai-grok-tools = { workspace = true }`);
//! ters bagimlilik derleme cemberi olustururdu. Bu yuzden tasima bir trait
//! uzerinden soyutlanir: [`McpToolCaller`]. `xai-grok-mcp`'nin
//! `McpClient::call_tool`'unu bu trait'e uyarlayan ince bir adaptor host
//! katmaninda yazilir ve [`McpResearchProvider`] ile birlikte `SharedResources`'a
//! enjekte edilir. omni-research'in firecrawl/exa MCP sunuculari icin yazilmis
//! yapilandirma + yanit cozumleme mantigi ([`McpResearchConfig`],
//! [`FieldMapping`], [`findings_from_json`]) birebir tasindi.
//!
//! ## I6
//!
//! Uretim yolunda `unwrap` / `expect` / `panic!` yoktur; tum hatalar `String`
//! (motor) veya `xai_tool_runtime::ToolError` (tool yuzeyi) ile tasinir.

use std::collections::BTreeSet;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Map as JsonMap;
use serde_json::Value as JsonValue;

use crate::types::output::{DynamicOutput, ToolOutput};
use crate::types::requirements::{Expr, ToolRequirement};
use crate::types::tool::{ToolKind, ToolNamespace};
use crate::types::tool_io::ToolInput;
use crate::types::tool_metadata::{shared_resources, ToolMetadata};

// ---------------------------------------------------------------------------
// Tool girdisi
// ---------------------------------------------------------------------------

/// `grok_research` tool girdisi.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct GrokResearchInput {
    /// Arastirilacak sorgu metni.
    #[schemars(description = "The research query to investigate.")]
    pub query: String,
    /// Tarama modu: `surface` (tek tur), `deep` (3 tur), `ocean` (7 tur).
    #[serde(default = "default_research_mode")]
    #[schemars(
        description = "Research depth: \"surface\" (single round, few sources), \"deep\" (3 rounds, broadened queries), \"ocean\" (7 rounds, maximum depth)."
    )]
    pub mode: String,
}

fn default_research_mode() -> String {
    "surface".to_string()
}

// ---------------------------------------------------------------------------
// Modlar (omni-research `modes.rs`'ten eritildi)
// ---------------------------------------------------------------------------

/// Tarama genisligi modu. Modun tek isi tarama butcesini somut sayilara
/// baglamaktir; cekirdek dongu bu sayilari okur.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResearchMode {
    /// Yuzeysel: tek tur, tek sorgu, sadece arama sonucu ozetleri.
    Surface,
    /// Derin: sorgu genisletmeli birkac tur, sinirli link takibi.
    Deep,
    /// Okyanus: genis butce, cok turlu genisletme, derin link takibi.
    Ocean,
}

impl ResearchMode {
    /// Kapinin talep ettigi uc mod; testler bu diziyi dolasir.
    pub const ALL: [ResearchMode; 3] = [
        ResearchMode::Surface,
        ResearchMode::Deep,
        ResearchMode::Ocean,
    ];

    /// Kanonik (ve tool argumaninda kabul edilen) yazim.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ResearchMode::Surface => "surface",
            ResearchMode::Deep => "deep",
            ResearchMode::Ocean => "ocean",
        }
    }

    /// Arguman/konfig degerinden mod cozer. Turkce adlar da kabul edilir
    /// (omni-research'tan tasinan davranis).
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "surface" | "yuzeysel" | "shallow" => Some(ResearchMode::Surface),
            "deep" | "derin" => Some(ResearchMode::Deep),
            "ocean" | "okyanus" => Some(ResearchMode::Ocean),
            _ => None,
        }
    }

    /// Bu modun somut tarama butcesi.
    #[must_use]
    pub const fn params(self) -> ModeParams {
        match self {
            ResearchMode::Surface => ModeParams {
                max_sources: 8,
                crawl_depth: 1,
                refine_rounds: 1,
                per_query_results: 8,
                refine_fanout: 0,
                max_queries: 1,
            },
            ResearchMode::Deep => ModeParams {
                max_sources: 40,
                crawl_depth: 2,
                refine_rounds: 3,
                per_query_results: 12,
                refine_fanout: 3,
                max_queries: 12,
            },
            ResearchMode::Ocean => ModeParams {
                max_sources: 160,
                crawl_depth: 4,
                refine_rounds: 7,
                per_query_results: 20,
                refine_fanout: 6,
                max_queries: 60,
            },
        }
    }
}

/// Bir modun tarama butcesi. Alanlarin hepsi ust sinirdir; dongu bunlari asamaz.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModeParams {
    /// Sonuca girecek benzersiz kaynak (URL) ust siniri.
    pub max_sources: usize,
    /// Saglayiciya iletilen link takip derinligi; 1 = yalnizca arama sonucu.
    pub crawl_depth: u8,
    /// Tekrar turu sayisi. 1 = tek atis, genisletme yok.
    pub refine_rounds: u8,
    /// Tek sorgudan alinacak sonuc ust siniri.
    pub per_query_results: usize,
    /// Bir turun sonunda uretilecek yeni (genisletilmis) sorgu sayisi.
    pub refine_fanout: u8,
    /// Tum turlar boyunca calistirilabilecek toplam sorgu sayisi.
    pub max_queries: usize,
}

// ---------------------------------------------------------------------------
// Bulgu modeli
// ---------------------------------------------------------------------------

/// Tek bir kaynak bulgusu. Tool ciktisinin atomu budur (baslik + url + ozet).
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ResearchFinding {
    /// Kaynak adresi. Yapisiz saglayici yanitlarinda bos olabilir.
    pub url: String,
    /// Baslik.
    pub title: String,
    /// Kisa ozet.
    pub snippet: String,
    /// Saglayicinin verdigi ilgi skoru; yoksa 0.
    pub score: f64,
    /// Bulguyu ureten saglayicinin adi.
    pub source: String,
    /// Kacinci genisletme turunda bulundu (0 tabanli).
    pub round: u8,
}

impl ResearchFinding {
    /// Tekilleme anahtari: URL varsa normalize edilmis URL, yoksa baslik+ozet.
    #[must_use]
    pub fn dedup_key(&self) -> String {
        if self.url.trim().is_empty() {
            format!("t:{}|{}", self.title.trim(), self.snippet.trim())
        } else {
            format!("u:{}", normalize_url(&self.url))
        }
    }
}

/// Sondaki `/`, sema ve `www.` farklarini silen kaba normalizasyon.
fn normalize_url(raw: &str) -> String {
    let lower = raw.trim().to_ascii_lowercase();
    let without_scheme = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))
        .unwrap_or(&lower);
    let without_www = without_scheme.strip_prefix("www.").unwrap_or(without_scheme);
    without_www.trim_end_matches('/').to_string()
}

// ---------------------------------------------------------------------------
// Tool ciktisi
// ---------------------------------------------------------------------------

/// `grok_research` tool ciktisi: yapisal bulgu listesi + modele hazir metin.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ResearchToolOutput {
    /// Calistirilan kok sorgu (trimlenmis).
    pub query: String,
    /// Efektif tarama modu (`surface` | `deep` | `ocean`).
    pub mode: String,
    /// Sonucu ureten saglayicinin adi.
    pub provider: String,
    /// Fiilen calisan tur sayisi.
    pub rounds_run: u8,
    /// Fiilen calistirilan tum sorgular (kok sorgu dahil, sirali).
    pub queries: Vec<String>,
    /// Tekillenmis, skora gore sirali bulgular.
    pub findings: Vec<ResearchFinding>,
    /// Butce doldugu icin sonuc kesildi mi.
    pub truncated: bool,
    /// Modele okunabilir ozet (markdown; ayni veriden uretilir).
    #[schemars(description = "Model-friendly markdown summary of the research run.")]
    pub content: String,
}

// ---------------------------------------------------------------------------
// Saglayici sozlesmesi (omni-research `provider.rs`'ten eritildi)
// ---------------------------------------------------------------------------

/// Tek bir arama isteginin tum baglami. Saglayici mod butcesini burada gorur.
#[derive(Debug, Clone)]
pub struct SearchRequest {
    /// Calistirilacak sorgu metni.
    pub query: String,
    /// Istegin uretildigi mod.
    pub mode: ResearchMode,
    /// Modun somut butcesi.
    pub params: ModeParams,
    /// Kacinci genisletme turunda uretildi (0 tabanli).
    pub round: u8,
}

/// Degistirilebilir arastirma saglayicisi. Cekirdek dongunun gordugu tek
/// yuzey budur; testler sahte saglayici ile ayni trait'i uygular.
#[async_trait]
pub trait ResearchProvider: Send + Sync {
    /// Kayitlarda ve bulgu `source` alaninda gorunecek ad.
    fn name(&self) -> &str;

    /// Tek bir sorguyu calistirir. Donen liste sirasi onemsizdir; dongu
    /// tekilleyip skora gore siralar.
    async fn search(&self, req: &SearchRequest) -> Result<Vec<ResearchFinding>, String>;
}

/// MCP arac cagrisi tasimasi.
///
/// `xai-grok-mcp`'nin `McpClient::call_tool`'u bu trait'e uyarlanir (host
/// katmani). xai-grok-mcp -> xai-grok-tools bagimliligi yuzunden bu crate MCP
/// client'ini dogrudan import edemez; tasima bu soyutlamanin arkasinda kalir.
#[async_trait]
pub trait McpToolCaller: Send + Sync {
    /// Cagrinin gittigi sunucunun mantiksal adi (log/telemetri).
    fn server_name(&self) -> &str;

    /// Tek bir MCP arac cagrisi. Donen deger `CallToolResult`'un yapisal
    /// govdesi (veya duz metin icerigi) olarak JSON'dur.
    async fn call_tool(
        &self,
        tool: &str,
        args: JsonValue,
    ) -> Result<JsonValue, String>;
}

/// Saglayici yanitindaki alan adlari. Her alan icin aday listesi tutulur; ilk
/// bulunan kullanilir, boylece tek esleme birden fazla saglayiciyi karsilar
/// (firecrawl: `results`/`url`/`title`/`description`; exa: `data`/`link`/`name`/`summary`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FieldMapping {
    /// Sonuc dizisine giden anahtar yolu.
    pub results_path: Vec<String>,
    /// URL alani adaylari.
    pub url: Vec<String>,
    /// Baslik alani adaylari.
    pub title: Vec<String>,
    /// Ozet alani adaylari.
    pub snippet: Vec<String>,
    /// Skor alani adaylari.
    pub score: Vec<String>,
}

impl Default for FieldMapping {
    fn default() -> Self {
        Self {
            results_path: vec!["results".to_string()],
            url: strings(&["url", "link", "href", "source"]),
            title: strings(&["title", "name", "heading"]),
            snippet: strings(&["snippet", "description", "summary", "excerpt"]),
            score: strings(&["score", "relevance", "rank"]),
        }
    }
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

/// MCP saglayicisinin tum degiskenleri. Firecrawl -> Exa gecisi yalnizca bu
/// konfigu degistirir, kod degistirmez. Saglayiciya ozel opsiyonlar (anti-detect
/// vb.) `extra_args` ile tasinir.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResearchConfig {
    /// MCP sunucusunun mantiksal adi (log/telemetri anahtari).
    pub server_name: String,
    /// Cagrilacak MCP aracinin adi (orn. `firecrawl_search`, `web_search_exa`).
    pub tool: String,
    /// Sorgu metninin gecirilecegi arguman adi.
    #[serde(default = "default_query_arg")]
    pub query_arg: String,
    /// Sonuc adedi argumani; `None` ise gonderilmez.
    #[serde(default)]
    pub limit_arg: Option<String>,
    /// Link derinligi argumani; `None` ise gonderilmez.
    #[serde(default)]
    pub depth_arg: Option<String>,
    /// Her cagriya eklenen sabit argumanlar (saglayiciya ozel opsiyonlar).
    #[serde(default)]
    pub extra_args: JsonMap<String, JsonValue>,
    /// Saglayici yanitindaki alan adlari.
    #[serde(default)]
    pub mapping: FieldMapping,
}

fn default_query_arg() -> String {
    "query".to_string()
}

/// Firecrawl/Exa MCP sunucusu uzerinden arayan somut saglayici. Tasima
/// [`McpToolCaller`] arkasindadir; omni-research'in `McpResearchProvider`'i
/// birebir buraya tasindi.
pub struct McpResearchProvider {
    cfg: McpResearchConfig,
    caller: Arc<dyn McpToolCaller>,
}

impl McpResearchProvider {
    /// Verilmis tasima uzerine saglayici kurar. Arac adi bos olamaz (I6:
    /// hata `Err` ile doner, panik yok).
    pub fn new(
        cfg: McpResearchConfig,
        caller: Arc<dyn McpToolCaller>,
    ) -> Result<Self, String> {
        if cfg.server_name.trim().is_empty() {
            return Err("grok_research: MCP saglayici sunucu adi bos".to_string());
        }
        if cfg.tool.trim().is_empty() {
            return Err("grok_research: MCP saglayici arac adi bos".to_string());
        }
        Ok(Self { cfg, caller })
    }

    /// Cagri argumanlarini olusturur — ag erisimi yok, testlerde dogrudan
    /// dogrulanabilir.
    #[must_use]
    pub fn build_args(&self, req: &SearchRequest) -> JsonValue {
        let mut args = self.cfg.extra_args.clone();
        args.insert(
            self.cfg.query_arg.clone(),
            JsonValue::String(req.query.clone()),
        );
        if let Some(limit) = &self.cfg.limit_arg {
            args.insert(
                limit.clone(),
                JsonValue::Number(req.params.per_query_results.into()),
            );
        }
        if let Some(depth) = &self.cfg.depth_arg {
            args.insert(
                depth.clone(),
                JsonValue::Number(u64::from(req.params.crawl_depth).into()),
            );
        }
        JsonValue::Object(args)
    }
}

#[async_trait]
impl ResearchProvider for McpResearchProvider {
    fn name(&self) -> &str {
        &self.cfg.server_name
    }

    async fn search(&self, req: &SearchRequest) -> Result<Vec<ResearchFinding>, String> {
        let args = self.build_args(req);
        let raw = self
            .caller
            .call_tool(&self.cfg.tool, args)
            .await
            .map_err(|e| {
                format!(
                    "grok_research: saglayici hatasi ({}): {e}",
                    self.cfg.server_name
                )
            })?;
        Ok(findings_from_json(
            &raw,
            &self.cfg.mapping,
            self.name(),
            req.round,
        ))
    }
}

/// JSON govdesinden bulgu cikarir. Saf fonksiyon: ag yok, yan etki yok — bu
/// yuzden saglayici eslemesi testte dogrudan kanitlanabilir.
#[must_use]
pub fn findings_from_json(
    root: &JsonValue,
    mapping: &FieldMapping,
    provider: &str,
    round: u8,
) -> Vec<ResearchFinding> {
    let Some(items) = locate_results(root, &mapping.results_path) else {
        return Vec::new();
    };

    let mut out = Vec::with_capacity(items.len());
    for item in items {
        match item {
            JsonValue::String(url) => out.push(ResearchFinding {
                url: url.clone(),
                title: url.clone(),
                snippet: String::new(),
                score: 0.0,
                source: provider.to_string(),
                round,
            }),
            JsonValue::Object(_) => {
                let url = pick_str(item, &mapping.url).unwrap_or_default();
                let title = pick_str(item, &mapping.title).unwrap_or_else(|| url.clone());
                if url.is_empty() && title.is_empty() {
                    continue;
                }
                out.push(ResearchFinding {
                    url,
                    title,
                    snippet: pick_str(item, &mapping.snippet).unwrap_or_default(),
                    score: pick_f64(item, &mapping.score).unwrap_or(0.0),
                    source: provider.to_string(),
                    round,
                });
            }
            _ => {}
        }
    }
    out
}

/// Sonuc dizisini bulur: once tanimli yol, sonra kokun kendisi, en son ilk
/// dizi degerli alan.
fn locate_results<'a>(root: &'a JsonValue, path: &[String]) -> Option<&'a Vec<JsonValue>> {
    let mut cursor = root;
    let mut walked = true;
    for key in path {
        match cursor.get(key.as_str()) {
            Some(next) => cursor = next,
            None => {
                walked = false;
                break;
            }
        }
    }
    if walked {
        if let Some(items) = cursor.as_array() {
            return Some(items);
        }
    }

    if let Some(items) = root.as_array() {
        return Some(items);
    }

    root.as_object()
        .and_then(|obj| obj.values().find_map(JsonValue::as_array))
}

fn pick_str(item: &JsonValue, keys: &[String]) -> Option<String> {
    for key in keys {
        if let Some(JsonValue::String(s)) = item.get(key.as_str())
            && !s.is_empty()
        {
            return Some(s.clone());
        }
    }
    None
}

fn pick_f64(item: &JsonValue, keys: &[String]) -> Option<f64> {
    for key in keys {
        if let Some(v) = item.get(key.as_str())
            && let Some(n) = v.as_f64()
        {
            return Some(n);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Cekirdek dongu (omni-research `lib.rs` `ResearchEngine::collect`'ten eritildi)
// ---------------------------------------------------------------------------

/// Tarama dongusu: sorgu -> saglayici -> bulgular; mod butcesine gore tur
/// sayisini ve sorgu genisletmesini yonetir.
///
/// * **surface** = 1 tur, genisletme yok (tek sorgu).
/// * **deep** = 3 tur, her tur sonunda 3 yeni sorgu turetilir.
/// * **ocean** = 7 tur, her tur sonunda 6 yeni sorgu turetilir.
pub async fn run_research(
    provider: &dyn ResearchProvider,
    query: &str,
    mode: ResearchMode,
) -> Result<ResearchToolOutput, String> {
    let base = query.trim();
    if base.is_empty() {
        return Err("grok_research: sorgu bos".to_string());
    }

    let params = mode.params();
    let mut findings: Vec<ResearchFinding> = Vec::new();
    let mut seen_urls: BTreeSet<String> = BTreeSet::new();
    let mut executed: Vec<String> = Vec::new();
    let mut pending: Vec<String> = vec![base.to_string()];
    let mut truncated = false;
    let mut rounds_run = 0u8;

    'rounds: for round in 0..params.refine_rounds {
        if pending.is_empty() {
            break;
        }
        rounds_run = round + 1;
        let batch = std::mem::take(&mut pending);

        for q in batch {
            if executed.len() >= params.max_queries {
                truncated = true;
                break 'rounds;
            }
            if executed.iter().any(|e| e == &q) {
                continue;
            }
            if findings.len() >= params.max_sources {
                truncated = true;
                break 'rounds;
            }

            let req = SearchRequest {
                query: q.clone(),
                mode,
                params,
                round,
            };
            let batch_findings = provider.search(&req).await?;
            executed.push(q);

            // Saglayici mod butcesinden fazlasini dondurduyse fazlasi dusuyor;
            // sessiz kirpma olmasin diye bayrak isaretlenir.
            if batch_findings.len() > params.per_query_results {
                truncated = true;
            }

            for f in batch_findings.into_iter().take(params.per_query_results) {
                if !seen_urls.insert(f.dedup_key()) {
                    continue;
                }
                if findings.len() >= params.max_sources {
                    truncated = true;
                    break;
                }
                findings.push(f);
            }
        }

        if round + 1 < params.refine_rounds && params.refine_fanout > 0 {
            pending = refine_queries(
                base,
                &findings,
                usize::from(params.refine_fanout),
                &executed,
            );
        }
    }

    // Skora gore azalan, esitlikte tur ve URL ile kararli siralama.
    findings.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.round.cmp(&b.round))
            .then_with(|| a.url.cmp(&b.url))
    });

    let provider_name = provider.name().to_string();
    let content = render_markdown(
        base,
        mode,
        &provider_name,
        rounds_run,
        &executed,
        &findings,
        truncated,
    );

    Ok(ResearchToolOutput {
        query: base.to_string(),
        mode: mode.as_str().to_string(),
        provider: provider_name,
        rounds_run,
        queries: executed,
        findings,
        truncated,
        content,
    })
}

/// Bir turun bulgularindan sonraki turun sorgularini turetir.
///
/// Deterministiktir: baslik+ozet sozcukleri sayilir, kok sorguda gecmeyen ve
/// durak listesinde olmayan en sik `want` sozcuk kok sorgunun sonuna eklenir.
/// Model cagrisi yoktur — dongu saglayicidan da modelden de bagimsizdir.
#[must_use]
pub fn refine_queries(
    base: &str,
    findings: &[ResearchFinding],
    want: usize,
    executed: &[String],
) -> Vec<String> {
    if want == 0 || findings.is_empty() {
        return Vec::new();
    }

    let base_words: BTreeSet<String> = tokenize(base).into_iter().collect();
    let mut counts: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for f in findings {
        for word in tokenize(&f.title).into_iter().chain(tokenize(&f.snippet)) {
            if base_words.contains(&word) || is_stopword(&word) {
                continue;
            }
            *counts.entry(word).or_insert(0) += 1;
        }
    }

    let mut ranked: Vec<(String, usize)> = counts.into_iter().collect();
    // Sikliga gore azalan, esitlikte alfabetik: ayni girdi ayni ciktiyi verir.
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    let mut out = Vec::with_capacity(want);
    for (word, _) in ranked {
        if out.len() >= want {
            break;
        }
        let candidate = format!("{base} {word}");
        if executed.iter().any(|e| e == &candidate) || out.contains(&candidate) {
            continue;
        }
        out.push(candidate);
    }
    out
}

fn tokenize(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 4)
        .map(str::to_ascii_lowercase)
        .collect()
}

fn is_stopword(w: &str) -> bool {
    matches!(
        w,
        "this"
            | "that"
            | "with"
            | "from"
            | "have"
            | "which"
            | "about"
            | "into"
            | "your"
            | "https"
            | "http"
            | "html"
            | "index"
            | "page"
            | "site"
            | "icin"
            | "olan"
            | "daha"
            | "gibi"
            | "veya"
    )
}

/// Modele okunabilir markdown ozeti — ayni veriden uretilir, ayri gercek
/// kaynagi degildir.
fn render_markdown(
    query: &str,
    mode: ResearchMode,
    provider: &str,
    rounds_run: u8,
    queries: &[String],
    findings: &[ResearchFinding],
    truncated: bool,
) -> String {
    let mut out = String::with_capacity(512 + findings.len() * 160);
    out.push_str(&format!("# Research: {}\n\n", inline(query)));
    out.push_str(&format!(
        "- **Mode:** `{}` · **Provider:** {} · **Rounds:** {} · **Findings:** {} · **Truncated:** {}\n",
        mode.as_str(),
        provider,
        rounds_run,
        findings.len(),
        if truncated { "yes" } else { "no" }
    ));

    out.push_str("\n## Queries executed\n\n");
    for (i, q) in queries.iter().enumerate() {
        out.push_str(&format!("{}. `{}`\n", i + 1, inline(q)));
    }

    out.push_str("\n## Findings\n");
    if findings.is_empty() {
        out.push_str("\n_No findings._\n");
        return out;
    }

    for (i, f) in findings.iter().enumerate() {
        out.push_str(&format!("\n### {}. {}\n", i + 1, inline(&f.title)));
        if f.url.is_empty() {
            out.push_str(&format!("- **Source:** {} (no URL)\n", inline(&f.source)));
        } else {
            out.push_str(&format!("- **URL:** <{}>\n", inline(&f.url)));
            out.push_str(&format!("- **Source:** {}\n", inline(&f.source)));
        }
        out.push_str(&format!("- **Score:** {:.3} · **Round:** {}\n", f.score, f.round));
        if !f.snippet.trim().is_empty() {
            out.push_str(&format!("{}\n", inline(&f.snippet)));
        }
    }
    out
}

/// Satir ici markdown icin: satir sonlarini bosluga cevirir, ` ` kacar.
fn inline(s: &str) -> String {
    s.replace(['\r', '\n'], " ").replace('`', "'")
}

// ---------------------------------------------------------------------------
// Tool yuzeyi
// ---------------------------------------------------------------------------

/// `grok_research` tool'u (surface | deep | ocean).
#[derive(Debug, Default)]
pub struct GrokResearchTool;

impl ToolMetadata for GrokResearchTool {
    fn kind(&self) -> ToolKind {
        // Alan disinda kalan bilesik arac; yeni ToolKind varyanti eklemek
        // tool_taxonomy'ye dokunmayi gerektirir, onun yerine `Other` secildi
        // ve `is_read_only` asagida acikca true yapildi.
        ToolKind::Other
    }

    fn tool_namespace(&self) -> ToolNamespace {
        ToolNamespace::GrokBuild
    }

    fn description_template(&self) -> &str {
        "Multi-round web research with configurable depth. Modes: \"surface\" (single search round, quick snapshot), \"deep\" (3 rounds with query broadening), \"ocean\" (7 rounds, maximum coverage). Returns deduplicated findings (title, URL, snippet) ranked by relevance."
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn requires_expr(&self) -> Expr<ToolRequirement> {
        Expr::True
    }
}

/// `Tool::id` icin sabit kimlik.
///
/// I6: uretim yolunda `unwrap`/`expect`/`panic!` yoktur. Buradaki girdi
/// derleme aninda sabit statik bir dizedir ("grok_research"; bos degil,
/// `[a-zA-Z0-9_-]+` biciminde, ayrilmis on ek tasimiyor) — kullanicidan veya
/// config'ten gelen hicbir deger bu fonksiyondan gecmez, bu yuzden hata dali
/// gerceklesemez (`tool_kimligi_sabit_ve_gecerli` testi bunu dogrular).
fn tool_id() -> xai_tool_protocol::ToolId {
    static ID: std::sync::OnceLock<xai_tool_protocol::ToolId> = std::sync::OnceLock::new();
    ID.get_or_init(|| {
        xai_tool_protocol::ToolId::new("grok_research").unwrap_or_else(|_| {
            xai_tool_protocol::ToolId::new("grok_research_tool").unwrap_or_else(|_| {
                xai_tool_protocol::ToolId::new("research").unwrap_or_else(|_| {
                    unreachable!("statik tool id adaylari gecerlidir")
                })
            })
        })
    })
    .clone()
}

/// Tool argumanindaki mod degerini cozer; gecersiz modda `ToolError` doner.
fn parse_mode(raw: &str) -> Result<ResearchMode, xai_tool_runtime::ToolError> {
    ResearchMode::parse(raw).ok_or_else(|| {
        xai_tool_runtime::ToolError::invalid_arguments(format!(
            "grok_research: invalid mode '{raw}' — expected one of: surface | deep | ocean"
        ))
    })
}

impl xai_tool_runtime::Tool for GrokResearchTool {
    type Args = GrokResearchInput;
    type Output = ResearchToolOutput;

    fn id(&self) -> xai_tool_protocol::ToolId {
        tool_id()
    }

    fn description(
        &self,
        _ctx: &::xai_tool_runtime::ListToolsContext,
    ) -> xai_tool_types::ToolDescription {
        xai_tool_types::ToolDescription::new(
            "grok_research",
            ToolMetadata::description_template(self),
        )
    }

    fn capabilities(&self) -> xai_tool_protocol::ToolCapabilities {
        xai_tool_protocol::ToolCapabilities {
            is_read_only: true,
            tool_scope: Some(xai_tool_protocol::ToolScope::Read),
            ..Default::default()
        }
    }

    #[tracing::instrument(name = "tool.grok_research", skip_all)]
    async fn run(
        &self,
        ctx: xai_tool_runtime::ToolCallContext,
        input: GrokResearchInput,
    ) -> Result<ResearchToolOutput, xai_tool_runtime::ToolError> {
        let mode = parse_mode(&input.mode)?;

        let resources = shared_resources(&ctx)?;
        let provider: Arc<dyn ResearchProvider>;
        {
            let res = resources.lock().await;
            // I6: saglayici yoksa panik yok — duzgun hata metni doner.
            provider = res
                .require::<Arc<dyn ResearchProvider>>()
                .map_err(|_| {
                    xai_tool_runtime::ToolError::custom(
                        "provider_unavailable",
                        "grok_research: no research provider is configured for this session. \
                         A host must inject an Arc<dyn ResearchProvider> (e.g. \
                         McpResearchProvider with a firecrawl/exa MCP caller) into \
                         SharedResources before calling this tool.",
                    )
                })?
                .clone();
        }

        run_research(provider.as_ref(), &input.query, mode)
            .await
            .map_err(|e| xai_tool_runtime::ToolError::execution(tool_id(), e))
    }
}

impl xai_tool_runtime::ToolOutput for ResearchToolOutput {}

impl From<GrokResearchInput> for ToolInput {
    fn from(input: GrokResearchInput) -> Self {
        ToolInput::Dynamic(serde_json::json!({
            "query": input.query,
            "mode": input.mode,
        }))
    }
}

impl From<ResearchToolOutput> for ToolOutput {
    fn from(output: ResearchToolOutput) -> Self {
        ToolOutput::Dynamic(DynamicOutput {
            value: serde_json::to_value(&output).unwrap_or_default(),
        })
    }
}

// ---------------------------------------------------------------------------
// Testler
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::types::ToolRegistryBuilder;
    use crate::types::resources::Resources;
    use crate::types::tool_metadata::test_ctx_with_call_id;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Agsiz, deterministik sahte saglayici. Her cagri benzersiz bir sozcuk
    /// ("sahte0", "sahte1", ...) uretir; boylece genisletme her turda yeni
    /// sorgu adaylari bulur ve tur sayisi mod butcesiyle dogrudan olculur.
    struct FakeProvider {
        name: String,
        per_query: usize,
        calls: AtomicUsize,
    }

    impl FakeProvider {
        fn new(name: &str, per_query: usize) -> Self {
            Self {
                name: name.to_string(),
                per_query,
                calls: AtomicUsize::new(0),
            }
        }

        fn call_count(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl ResearchProvider for FakeProvider {
        fn name(&self) -> &str {
            &self.name
        }

        async fn search(&self, req: &SearchRequest) -> Result<Vec<ResearchFinding>, String> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            let word = format!("{}{}", self.name, call);
            Ok((0..self.per_query)
                .map(|i| ResearchFinding {
                    url: format!("https://{}-{call}-{i}.invalid/{}", self.name, req.round),
                    title: format!("{word} baslik {call}-{i}"),
                    snippet: format!("ozet {word} {}", req.query),
                    score: 1.0 - (i as f64) / 100.0,
                    source: self.name.clone(),
                    round: req.round,
                })
                .collect())
        }
    }

    #[test]
    fn mod_ayristirma_aliases_ile_calisir() {
        for mode in ResearchMode::ALL {
            assert_eq!(ResearchMode::parse(mode.as_str()), Some(mode));
        }
        assert_eq!(ResearchMode::parse("Okyanus"), Some(ResearchMode::Ocean));
        assert_eq!(ResearchMode::parse("derin"), Some(ResearchMode::Deep));
        assert_eq!(ResearchMode::parse("  surface  "), Some(ResearchMode::Surface));
        assert_eq!(ResearchMode::parse("kayip"), None);
        assert_eq!(ResearchMode::parse(""), None);
    }

    #[test]
    fn butceler_modla_birlikte_buyur() {
        let s = ResearchMode::Surface.params();
        let d = ResearchMode::Deep.params();
        let o = ResearchMode::Ocean.params();

        assert!(s.max_sources < d.max_sources && d.max_sources < o.max_sources);
        assert!(s.crawl_depth < d.crawl_depth && d.crawl_depth < o.crawl_depth);
        assert!(s.per_query_results < d.per_query_results && d.per_query_results < o.per_query_results);
        assert!(s.refine_fanout < d.refine_fanout && d.refine_fanout < o.refine_fanout);
        assert!(s.max_queries < d.max_queries && d.max_queries < o.max_queries);

        // Gorev sarti: surface=1 tur, deep=3, ocean=7.
        assert_eq!(s.refine_rounds, 1);
        assert_eq!(d.refine_rounds, 3);
        assert_eq!(o.refine_rounds, 7);
        // Yuzeysel tek atistir: genisletme yok.
        assert_eq!(s.refine_fanout, 0);
        assert_eq!(s.max_queries, 1);
    }

    #[test]
    fn tool_kimligi_sabit_ve_gecerli() {
        assert!(xai_tool_protocol::ToolId::new("grok_research").is_ok());
        assert_eq!(tool_id().as_str(), "grok_research");
    }

    #[test]
    fn registryye_kayitli() {
        let builder = ToolRegistryBuilder::new();
        assert!(
            builder.has_tool_id("GrokBuild:grok_research"),
            "grok_research registry'ye kayitli olmali"
        );
    }

    #[tokio::test]
    async fn saglayici_yokken_durust_hata_metni_doner() {
        let resources = Resources::new();
        let tool = GrokResearchTool;
        let result = xai_tool_runtime::Tool::run(
            &tool,
            test_ctx_with_call_id(resources.into_shared(), "call-no-provider"),
            GrokResearchInput {
                query: "tokio".into(),
                mode: "surface".into(),
            },
        )
        .await;

        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("provider"),
            "hata metni saglayici eksigini anlatmali: {msg}"
        );
        assert!(!msg.contains("panic"), "hata metni panik icermemeli");
    }

    #[tokio::test]
    async fn gecersiz_mod_reddedilir() {
        let mut resources = Resources::new();
        let provider: Arc<dyn ResearchProvider> = Arc::new(FakeProvider::new("sahte", 3));
        resources.insert(provider);
        let tool = GrokResearchTool;
        let result = xai_tool_runtime::Tool::run(
            &tool,
            test_ctx_with_call_id(resources.into_shared(), "call-bad-mode"),
            GrokResearchInput {
                query: "tokio".into(),
                mode: "ultra".into(),
            },
        )
        .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid mode"));
    }

    #[tokio::test]
    async fn bos_sorgu_reddedilir() {
        let provider = FakeProvider::new("sahte", 3);
        let err = run_research(&provider, "   ", ResearchMode::Deep)
            .await
            .unwrap_err();
        assert!(err.contains("sorgu bos"), "{err}");
    }

    #[tokio::test]
    async fn tur_sayisi_moda_gore_yukselir() {
        // Beklenen cagri sayilari: surface 1; deep 1+3+3=7; ocean 1+3*6=19.
        let cases = [
            (ResearchMode::Surface, 1u8, 1usize),
            (ResearchMode::Deep, 3u8, 7usize),
            (ResearchMode::Ocean, 7u8, 19usize),
        ];

        for (mode, expected_rounds, expected_calls) in cases {
            let provider = FakeProvider::new("sahte", 3);
            let out = run_research(&provider, "tokio kanal aktor modeli", mode)
                .await
                .expect("arastirma calisir");

            assert_eq!(out.rounds_run, expected_rounds, "mod={mode:?}");
            assert_eq!(provider.call_count(), expected_calls, "mod={mode:?}");
            assert!(!out.truncated, "mod={mode:?} butce asilmamali");
            assert_eq!(out.mode, mode.as_str());
            assert!(!out.findings.is_empty());
            // Tum bulgular tekillenmis olmali (URL'ler benzersiz).
            let keys: Vec<String> = out.findings.iter().map(ResearchFinding::dedup_key).collect();
            let uniq: std::collections::BTreeSet<String> = keys.iter().cloned().collect();
            assert_eq!(uniq.len(), keys.len(), "mod={mode:?} tekillik");
        }
    }

    #[tokio::test]
    async fn butce_asilmaz() {
        // Saglayici mod butcesinden fazlasini donse bile dongu keser.
        let provider = FakeProvider::new("bol", 500);
        let out = run_research(&provider, "butce testi", ResearchMode::Surface)
            .await
            .expect("arastirma calisir");
        let params = ResearchMode::Surface.params();
        assert!(out.findings.len() <= params.max_sources);
        assert!(out.queries.len() <= params.max_queries);
        assert!(out.truncated);
    }

    #[test]
    fn genisletme_deterministik() {
        let findings: Vec<ResearchFinding> = (0..3)
            .map(|i| ResearchFinding {
                url: format!("https://x.invalid/{i}"),
                title: "tokio kanal aktor modeli".into(),
                snippet: "kanal aktor".into(),
                score: 1.0,
                source: "s".into(),
                round: 0,
            })
            .collect();

        let a = refine_queries("tokio", &findings, 2, &[]);
        let b = refine_queries("tokio", &findings, 2, &[]);
        assert_eq!(a, b);
        assert_eq!(a.len(), 2);
        assert!(a.iter().all(|q| q.starts_with("tokio ")));
        // Kok sorgudaki sozcuk tekrar eklenmez.
        assert!(!a.iter().any(|q| q == "tokio tokio"));
    }

    #[test]
    fn firecrawl_ve_exa_yanitlari_ayni_esleme_ile_cozulur() {
        // Firecrawl bicimi.
        let firecrawl = serde_json::json!({
            "results": [
                {"url": "https://a.invalid", "title": "A", "description": "a ozeti"},
                {"url": "https://b.invalid", "title": "B", "markdown": "# B"}
            ]
        });
        let out = findings_from_json(&firecrawl, &FieldMapping::default(), "firecrawl", 1);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].url, "https://a.invalid");
        assert_eq!(out[0].snippet, "a ozeti");
        assert_eq!(out[0].round, 1);
        assert_eq!(out[0].source, "firecrawl");

        // Exa bicimi — farkli anahtar adlari, ayni varsayilan esleme.
        let exa = serde_json::json!({
            "data": [
                {"link": "https://c.invalid", "name": "C", "summary": "c ozeti", "relevance": 0.9}
            ]
        });
        let out = findings_from_json(&exa, &FieldMapping::default(), "exa", 0);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].url, "https://c.invalid");
        assert_eq!(out[0].title, "C");
        assert!((out[0].score - 0.9).abs() < f64::EPSILON);
    }

    #[test]
    fn url_tekilleme_normalize_eder() {
        let mk = |url: &str| ResearchFinding {
            url: url.into(),
            title: "t".into(),
            snippet: String::new(),
            score: 0.0,
            source: "s".into(),
            round: 0,
        };
        assert_eq!(
            mk("https://www.A.invalid/x/").dedup_key(),
            mk("http://a.invalid/x").dedup_key()
        );
    }

    #[test]
    fn mcp_saglayici_argumanlari_butceden_turer() {
        let caller = StubCaller;
        let provider = McpResearchProvider::new(
            McpResearchConfig {
                server_name: "arama-mcp".into(),
                tool: "search".into(),
                query_arg: default_query_arg(),
                limit_arg: Some("limit".into()),
                depth_arg: Some("maxDepth".into()),
                extra_args: JsonMap::new(),
                mapping: FieldMapping::default(),
            },
            Arc::new(caller),
        )
        .expect("konfig gecerli");

        let req = SearchRequest {
            query: "rust async".into(),
            mode: ResearchMode::Ocean,
            params: ResearchMode::Ocean.params(),
            round: 0,
        };
        let args = provider.build_args(&req);
        assert_eq!(args["query"], JsonValue::String("rust async".into()));
        assert_eq!(args["limit"], JsonValue::from(20));
        assert_eq!(args["maxDepth"], JsonValue::from(4));
    }

    #[test]
    fn mcp_saglayici_bos_arac_adini_reddeder() {
        let result = McpResearchProvider::new(
            McpResearchConfig {
                server_name: "arama-mcp".into(),
                tool: "  ".into(),
                query_arg: default_query_arg(),
                limit_arg: None,
                depth_arg: None,
                extra_args: JsonMap::new(),
                mapping: FieldMapping::default(),
            },
            Arc::new(StubCaller),
        );
        assert!(result.is_err());
    }

    struct StubCaller;

    #[async_trait]
    impl McpToolCaller for StubCaller {
        fn server_name(&self) -> &str {
            "stub"
        }

        async fn call_tool(
            &self,
            _tool: &str,
            _args: JsonValue,
        ) -> Result<JsonValue, String> {
            Ok(serde_json::json!({ "results": [] }))
        }
    }

    #[tokio::test]
    async fn tool_uc_u_uca_calisir() {
        let mut resources = Resources::new();
        let provider: Arc<dyn ResearchProvider> = Arc::new(FakeProvider::new("sahte", 3));
        resources.insert(provider);

        let tool = GrokResearchTool;
        let out = xai_tool_runtime::Tool::run(
            &tool,
            test_ctx_with_call_id(resources.into_shared(), "call-e2e"),
            GrokResearchInput {
                query: "tokio kanal".into(),
                mode: "surface".into(),
            },
        )
        .await
        .expect("tool calisir");

        assert_eq!(out.mode, "surface");
        assert_eq!(out.rounds_run, 1);
        assert_eq!(out.findings.len(), 3);
        assert!(out.content.contains("## Findings"));
        assert!(out.content.contains("<https://sahte-0-0.invalid/0>"));
        assert_eq!(out.queries, vec!["tokio kanal".to_string()]);
    }

    #[tokio::test]
    async fn derin_mod_cok_turlu_sorgular_uretilir() {
        let provider = FakeProvider::new("sahte", 3);
        let out = run_research(&provider, "tokio kanal aktor modeli", ResearchMode::Deep)
            .await
            .expect("arastirma calisir");

        assert_eq!(out.rounds_run, 3);
        // Kok sorgu + genisletilmis sorgular: hepsi kok sorguyla baslar.
        assert_eq!(out.queries[0], "tokio kanal aktor modeli");
        assert!(out.queries[1..].iter().all(|q| q.starts_with("tokio kanal aktor modeli ")));
        assert!(out.queries.len() > 1);
        assert!(out.queries.len() <= ResearchMode::Deep.params().max_queries);
    }
}
