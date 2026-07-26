//! Saglayici katmani (MASTER-PLAN 3.2 / 19.2, K14).
//!
//! Cekirdek arastirma dongusu **yalnizca** [`ResearchProvider`] trait'ini bilir.
//! Bugunku uygulama hazir MCP tasimasidir ([`McpResearchProvider`], `xai-grok-mcp`
//! uzerinden Firecrawl/Exa + anti-detect); yarin native crawler geldiginde ayni
//! trait'in ikinci bir uygulamasi yazilir ve cekirdek dosyaya dokunulmaz.
//!
//! Saglayici secimi **config'ten** gelir: [`ProviderConfig`] saf JSON'dur
//! (`omni-config` `ConfigStore::get` ciktisi dogrudan buraya cozulebilir), yani
//! Firecrawl -> Exa gecisi bir konfig degisikligidir, kod degisikligi degil (AS8).

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value as JsonValue};

use xai_grok_mcp::rmcp::model::CallToolResult;
use xai_grok_mcp::servers::{HttpConfig, McpClient};

use crate::modes::{ModeParams, ResearchMode};
use crate::{Finding, ResearchError};

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Tek bir arama isteginin tum baglami. Saglayici mod butcesini burada gorur;
/// butceyi cekirdek belirler, saglayici yalnizca uyar.
#[derive(Debug, Clone)]
pub struct SearchRequest {
    /// Calistirilacak sorgu metni.
    pub query: String,
    /// Istegin uretildigi mod.
    pub mode: ResearchMode,
    /// Modun somut butcesi (kaynak sayisi, derinlik, tur sayisi).
    pub params: ModeParams,
    /// Kacinci genisletme turunda uretildi (0 tabanli).
    pub round: u8,
}

/// Degistirilebilir arastirma saglayicisi.
///
/// Cekirdegin gordugu **tek** yuzey budur. MCP uygulamasi bunun bir uygulamasi
/// olmaktan ibarettir; ters bagimlilik yoktur (I3'un arastirma karsiligi).
#[async_trait]
pub trait ResearchProvider: Send + Sync {
    /// Kayitlarda ve `Finding::source` alaninda gorunecek ad.
    fn name(&self) -> &str;

    /// Tek bir sorguyu calistirir. Donen liste sirasi onemsizdir; cekirdek
    /// tekilleyip skora gore siralar.
    async fn search(&self, req: &SearchRequest) -> Result<Vec<Finding>, ResearchError>;
}

// ---------------------------------------------------------------------------
// Konfigurasyon
// ---------------------------------------------------------------------------

/// Config agacindan cozulen saglayici tanimi. Etiketli birlik oldugu icin
/// yarinki native crawler yeni bir varyant olarak eklenir; cagiran kod ayni kalir.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProviderConfig {
    /// Hazir MCP sunucusu (Firecrawl, Exa, ...) uzerinden arama.
    Mcp(McpProviderConfig),
}

impl ProviderConfig {
    /// Config'ten calisir saglayici uretir. Cekirdek bu fonksiyonu cagirir ve
    /// hangi saglayicinin dondugunu bilmez.
    pub fn build(&self) -> Result<Arc<dyn ResearchProvider>, ResearchError> {
        match self {
            ProviderConfig::Mcp(cfg) => Ok(Arc::new(McpResearchProvider::new(cfg.clone())?)),
        }
    }
}

/// MCP saglayicisinin tum degiskenleri. Sunucu adi, URL, arac adi ve alan
/// eslemesi config'ten gelir; bu yuzden Firecrawl -> Exa gecisi kod degistirmez.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpProviderConfig {
    /// MCP sunucusunun mantiksal adi (log/telemetri anahtari).
    pub server_name: String,
    /// Streamable-HTTP uc noktasi.
    pub url: String,
    /// Tasima basliklari (yetkilendirme, anti-detect ust bilgileri).
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// Cagrilacak MCP aracinin adi (orn. `firecrawl_search`, `web_search_exa`).
    pub tool: String,
    /// Sorgu metninin gecirilecegi argüman adi.
    #[serde(default = "default_query_arg")]
    pub query_arg: String,
    /// Sonuc adedi argümani; `None` ise gonderilmez.
    #[serde(default = "default_limit_arg")]
    pub limit_arg: Option<String>,
    /// Link derinligi argümani; `None` ise gonderilmez.
    #[serde(default)]
    pub depth_arg: Option<String>,
    /// Her cagriya eklenen sabit argümanlar (orn. `{"scrapeOptions":{...}}`).
    #[serde(default)]
    pub extra_args: JsonMap<String, JsonValue>,
    /// Anti-detect ayarlari (19.2).
    #[serde(default)]
    pub anti_detect: AntiDetect,
    /// Anti-detect ayarlarinin sarilacagi argüman adi. `None` ise argümanlar
    /// duz olarak en uste serilir.
    #[serde(default)]
    pub anti_detect_arg: Option<String>,
    /// Saglayici yanitindaki alan adlari.
    #[serde(default)]
    pub mapping: FieldMapping,
}

fn default_query_arg() -> String {
    "query".to_string()
}

fn default_limit_arg() -> Option<String> {
    Some("limit".to_string())
}

/// Anti-detect (bot tespiti kacinma) ayarlari. Icerik saglayiciya gore degisir,
/// bu yuzden bilinen alanlar disinda serbest `extra` haritasi tasinir.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AntiDetect {
    /// Tarayici kimligi.
    pub user_agent: Option<String>,
    /// Cikis vekili (proxy) URL'si.
    pub proxy: Option<String>,
    /// Saglayicinin gizli mod bayragi.
    pub stealth: bool,
    /// Dil/bolge ipucu.
    pub locale: Option<String>,
    /// Saglayiciya ozel serbest alanlar.
    pub extra: JsonMap<String, JsonValue>,
}

impl AntiDetect {
    /// Hicbir ayar verilmemis mi?
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.user_agent.is_none()
            && self.proxy.is_none()
            && !self.stealth
            && self.locale.is_none()
            && self.extra.is_empty()
    }

    /// Argüman nesnesine cevirir.
    #[must_use]
    pub fn to_args(&self) -> JsonMap<String, JsonValue> {
        let mut out = JsonMap::new();
        if let Some(ua) = &self.user_agent {
            out.insert("userAgent".to_string(), JsonValue::String(ua.clone()));
        }
        if let Some(proxy) = &self.proxy {
            out.insert("proxy".to_string(), JsonValue::String(proxy.clone()));
        }
        if self.stealth {
            out.insert("stealth".to_string(), JsonValue::Bool(true));
        }
        if let Some(locale) = &self.locale {
            out.insert("locale".to_string(), JsonValue::String(locale.clone()));
        }
        for (k, v) in &self.extra {
            out.insert(k.clone(), v.clone());
        }
        out
    }
}

/// Saglayici yanitindaki alan adlari. Her alan icin aday listesi tutulur; ilk
/// bulunan kullanilir, boylece tek esleme birden fazla saglayiciyi karsilar.
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
    /// Tam icerik alani adaylari.
    pub content: Vec<String>,
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
            content: strings(&["content", "markdown", "text", "body"]),
            score: strings(&["score", "relevance", "rank"]),
        }
    }
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

// ---------------------------------------------------------------------------
// MCP uygulamasi
// ---------------------------------------------------------------------------

/// Hazir MCP sunucusu uzerinden arayan saglayici (I2b: `xai-grok-mcp` tuketimi
/// tek yonludur, o agaca yazilmaz).
pub struct McpResearchProvider {
    cfg: McpProviderConfig,
    client: Arc<McpClient>,
}

impl McpResearchProvider {
    /// Config'ten HTTP tasimali bir MCP istemcisi kurar. El sikisma ilk arama
    /// sirasinda tembel yapilir (`McpClient::ensure_initialized`).
    pub fn new(cfg: McpProviderConfig) -> Result<Self, ResearchError> {
        if cfg.url.trim().is_empty() {
            return Err(ResearchError::Config("MCP saglayici url'i bos".into()));
        }
        if cfg.tool.trim().is_empty() {
            return Err(ResearchError::Config("MCP saglayici arac adi bos".into()));
        }
        let http = HttpConfig {
            url: cfg.url.clone(),
            headers: cfg
                .headers
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        };
        let client = Arc::new(McpClient::new_http(
            cfg.server_name.clone(),
            http,
            None,
            None,
        ));
        Ok(Self { cfg, client })
    }

    /// Hazir bir istemci uzerine kurar (paylasilan havuzdan gelen baglanti).
    #[must_use]
    pub fn with_client(cfg: McpProviderConfig, client: Arc<McpClient>) -> Self {
        Self { cfg, client }
    }

    /// Cagri argümanlarini olusturur — ag erisimi yok, testlerde dogrudan
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

        if !self.cfg.anti_detect.is_empty() {
            let anti = self.cfg.anti_detect.to_args();
            match &self.cfg.anti_detect_arg {
                Some(name) => {
                    args.insert(name.clone(), JsonValue::Object(anti));
                }
                None => {
                    for (k, v) in anti {
                        args.insert(k, v);
                    }
                }
            }
        }

        JsonValue::Object(args)
    }
}

#[async_trait]
impl ResearchProvider for McpResearchProvider {
    fn name(&self) -> &str {
        &self.cfg.server_name
    }

    async fn search(&self, req: &SearchRequest) -> Result<Vec<Finding>, ResearchError> {
        let args = self.build_args(req);
        let result = self
            .client
            .call_tool(&self.cfg.tool, args)
            .await
            .map_err(|e| ResearchError::Provider {
                provider: self.cfg.server_name.clone(),
                message: e.to_string(),
            })?;

        findings_from_call_result(&result, &self.cfg.mapping, self.name(), req.round)
    }
}

// ---------------------------------------------------------------------------
// Yanit cozumleme (saf fonksiyonlar)
// ---------------------------------------------------------------------------

/// MCP `CallToolResult` -> [`Finding`] listesi.
///
/// Sirasiyla `structured_content`, sonra metin bloklarinin JSON cozumu denenir;
/// hicbiri tutmazsa metin tek bir bulguya sarilir (bilgi kaybolmaz).
pub fn findings_from_call_result(
    result: &CallToolResult,
    mapping: &FieldMapping,
    provider: &str,
    round: u8,
) -> Result<Vec<Finding>, ResearchError> {
    if result.is_error.unwrap_or(false) {
        return Err(ResearchError::Provider {
            provider: provider.to_string(),
            message: joined_text(result),
        });
    }

    if let Some(structured) = &result.structured_content {
        let findings = findings_from_json(structured, mapping, provider, round);
        if !findings.is_empty() {
            return Ok(findings);
        }
    }

    let text = joined_text(result);
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }

    if let Ok(parsed) = serde_json::from_str::<JsonValue>(&text) {
        let findings = findings_from_json(&parsed, mapping, provider, round);
        if !findings.is_empty() {
            return Ok(findings);
        }
    }

    // Yapisiz metin: tek bulgu olarak saklanir; URL'siz bulgular cekirdekte
    // metin ozetiyle tekillenir.
    Ok(vec![Finding {
        url: String::new(),
        title: format!("{provider} duz metin yaniti"),
        snippet: truncate(&text, 400),
        content: Some(text),
        score: 0.0,
        source: provider.to_string(),
        round,
        fetched_at: omni_proto::now(),
    }])
}

/// Metin bloklarini birlestirir.
fn joined_text(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|block| block.as_text().map(|t| t.text.as_str()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// JSON govdesinden bulgu cikarir. Saf fonksiyon: ag yok, saat yok disinda
/// yan etki yok — bu yuzden saglayici eslemesi testte dogrudan kanitlanabilir.
#[must_use]
pub fn findings_from_json(
    root: &JsonValue,
    mapping: &FieldMapping,
    provider: &str,
    round: u8,
) -> Vec<Finding> {
    let Some(items) = locate_results(root, &mapping.results_path) else {
        return Vec::new();
    };

    let now = omni_proto::now();
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        match item {
            JsonValue::String(url) => out.push(Finding {
                url: url.clone(),
                title: url.clone(),
                snippet: String::new(),
                content: None,
                score: 0.0,
                source: provider.to_string(),
                round,
                fetched_at: now,
            }),
            JsonValue::Object(_) => {
                let url = pick_str(item, &mapping.url).unwrap_or_default();
                let title = pick_str(item, &mapping.title).unwrap_or_else(|| url.clone());
                if url.is_empty() && title.is_empty() {
                    continue;
                }
                out.push(Finding {
                    url,
                    title,
                    snippet: pick_str(item, &mapping.snippet).unwrap_or_default(),
                    content: pick_str(item, &mapping.content),
                    score: pick_f64(item, &mapping.score).unwrap_or(0.0),
                    source: provider.to_string(),
                    round,
                    fetched_at: now,
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
    if walked && let Some(items) = cursor.as_array() {
        return Some(items);
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

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    s.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use xai_grok_mcp::rmcp::model::ContentBlock;

    fn cfg() -> McpProviderConfig {
        McpProviderConfig {
            server_name: "arama-mcp".into(),
            url: "https://ornek.invalid/mcp".into(),
            headers: BTreeMap::new(),
            tool: "search".into(),
            query_arg: default_query_arg(),
            limit_arg: default_limit_arg(),
            depth_arg: Some("maxDepth".into()),
            extra_args: JsonMap::new(),
            anti_detect: AntiDetect::default(),
            anti_detect_arg: None,
            mapping: FieldMapping::default(),
        }
    }

    fn req(mode: ResearchMode) -> SearchRequest {
        SearchRequest {
            query: "rust async".into(),
            mode,
            params: mode.params(),
            round: 0,
        }
    }

    #[test]
    fn argumanlar_mod_butcesinden_turer() {
        let provider = McpResearchProvider::new(cfg()).expect("konfig gecerli");
        let args = provider.build_args(&req(ResearchMode::Ocean));
        assert_eq!(args["query"], JsonValue::String("rust async".into()));
        assert_eq!(args["limit"], JsonValue::from(20));
        assert_eq!(args["maxDepth"], JsonValue::from(4));
    }

    #[test]
    fn anti_detect_argumanlara_serilir() {
        let mut c = cfg();
        c.anti_detect.stealth = true;
        c.anti_detect.user_agent = Some("Mozilla/5.0".into());
        let provider = McpResearchProvider::new(c).expect("konfig gecerli");
        let args = provider.build_args(&req(ResearchMode::Surface));
        assert_eq!(args["stealth"], JsonValue::Bool(true));
        assert_eq!(args["userAgent"], JsonValue::String("Mozilla/5.0".into()));
    }

    #[test]
    fn anti_detect_sarmalanabilir() {
        let mut c = cfg();
        c.anti_detect.proxy = Some("http://vekil.invalid:8080".into());
        c.anti_detect_arg = Some("browser".into());
        let provider = McpResearchProvider::new(c).expect("konfig gecerli");
        let args = provider.build_args(&req(ResearchMode::Deep));
        assert_eq!(
            args["browser"]["proxy"],
            JsonValue::String("http://vekil.invalid:8080".into())
        );
    }

    #[test]
    fn bos_url_reddedilir() {
        let mut c = cfg();
        c.url = "  ".into();
        assert!(matches!(
            McpResearchProvider::new(c),
            Err(ResearchError::Config(_))
        ));
    }

    #[test]
    fn firecrawl_bicimi_cozulur() {
        let body = serde_json::json!({
            "results": [
                {"url": "https://a.invalid", "title": "A", "description": "a ozeti"},
                {"url": "https://b.invalid", "title": "B", "markdown": "# B"}
            ]
        });
        let out = findings_from_json(&body, &FieldMapping::default(), "firecrawl", 1);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].url, "https://a.invalid");
        assert_eq!(out[0].snippet, "a ozeti");
        assert_eq!(out[1].content.as_deref(), Some("# B"));
        assert_eq!(out[1].round, 1);
    }

    #[test]
    fn exa_bicimi_ayni_esleme_ile_cozulur() {
        // Farkli anahtar adlari, ayni varsayilan esleme: cekirdek degismez.
        let body = serde_json::json!({
            "data": [
                {"link": "https://c.invalid", "name": "C", "summary": "c ozeti", "relevance": 0.9}
            ]
        });
        let out = findings_from_json(&body, &FieldMapping::default(), "exa", 0);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].url, "https://c.invalid");
        assert_eq!(out[0].title, "C");
        assert!((out[0].score - 0.9).abs() < f64::EPSILON);
    }

    #[test]
    fn metin_blogundaki_json_cozulur() {
        let raw = serde_json::json!({"results": [{"url": "https://d.invalid"}]}).to_string();
        let result = CallToolResult::success(vec![ContentBlock::text(raw)]);
        let out = findings_from_call_result(&result, &FieldMapping::default(), "mcp", 0)
            .expect("cozulmeli");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].url, "https://d.invalid");
    }

    #[test]
    fn yapisiz_metin_tek_bulguya_sarilir() {
        let result = CallToolResult::success(vec![ContentBlock::text("duz cevap".to_string())]);
        let out = findings_from_call_result(&result, &FieldMapping::default(), "mcp", 0)
            .expect("cozulmeli");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].content.as_deref(), Some("duz cevap"));
    }

    #[test]
    fn hata_bayragi_saglayici_hatasina_donusur() {
        let result = CallToolResult::error(vec![ContentBlock::text("kota doldu".to_string())]);
        let err = findings_from_call_result(&result, &FieldMapping::default(), "mcp", 0)
            .expect_err("hata bekleniyor");
        assert!(matches!(err, ResearchError::Provider { .. }));
    }

    #[test]
    fn config_agacindan_saglayici_kurulur() {
        let raw = serde_json::json!({
            "kind": "mcp",
            "server_name": "firecrawl",
            "url": "https://ornek.invalid/mcp",
            "tool": "firecrawl_search"
        });
        let cfg: ProviderConfig = serde_json::from_value(raw).expect("config cozulmeli");
        let provider = cfg.build().expect("saglayici kurulmali");
        assert_eq!(provider.name(), "firecrawl");
    }
}
