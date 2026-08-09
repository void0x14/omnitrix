//! Routing mod kataloğu: `RoutingModeDef` tanımı, TOML yükleme ve gömülü fallback.
//!
//! - `load_routing_modes`: verilen yoldaki katalog dosyasını ayrıştırır; dosya yoksa
//!   gömülü varsayılan kataloğa düşer (cwd'den bağımsız — test ve paketlenmiş
//!   ortamlarda da çalışır).
//! - `find_mode` / `builtin_modes`: derleme anında gömülen `config/routing_modes.toml`
//!   kopyası üzerinden kararlı built-in bakım yapar.
//!
//! Katalog veridir: mod id/başlıkları hariç model adı ve fiyat literal'i barındırmaz;
//! fiyat/limit/kalite gibi değerler çalışma zamanında endpoint meta verisinden gelir.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::Deserialize;
use thiserror::Error;

/// Modun ait olduğu aile (spec 3.2; TOML `family` anahtarı).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
pub enum ModeFamily {
    Balance,
    Failover,
    RoleSplit,
    Hybrid,
    Specialty,
    Cost,
    Privacy,
}

impl ModeFamily {
    /// Tüm aileler; TUI filtre ve testlerde sıralı dolaşım için.
    pub const ALL: [ModeFamily; 7] = [
        ModeFamily::Balance,
        ModeFamily::Failover,
        ModeFamily::RoleSplit,
        ModeFamily::Hybrid,
        ModeFamily::Specialty,
        ModeFamily::Cost,
        ModeFamily::Privacy,
    ];

    /// Kısa makine dostu ad (aile adının kebab-case hali).
    pub fn as_str(self) -> &'static str {
        match self {
            ModeFamily::Balance => "balance",
            ModeFamily::Failover => "failover",
            ModeFamily::RoleSplit => "role-split",
            ModeFamily::Hybrid => "hybrid",
            ModeFamily::Specialty => "specialty",
            ModeFamily::Cost => "cost",
            ModeFamily::Privacy => "privacy",
        }
    }
}

impl std::fmt::Display for ModeFamily {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Seçici sınıfı (P1.1 sınıflandırması; TOML `class` anahtarı).
///
/// - `Primitive`: tek seçim kuralı (örn. round-robin).
/// - `Policy`: koşullu tetikleyici-kısıt (örn. sınıflandırılmış fallback).
/// - `Composite`: seçici + policy veya öğrenilmiş/dış sistem (örn. hedge, cascade).
///
/// P1.3, `selector_kind` + `selector` ikilisiyle somut seçici implementasyonunu eşler.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
pub enum SelectorKind {
    #[serde(rename = "primitive")]
    Primitive,
    #[serde(rename = "policy")]
    Policy,
    #[serde(rename = "composition")]
    Composite,
}

impl SelectorKind {
    /// TOML'de kullanılan sınıf adı.
    pub fn as_str(self) -> &'static str {
        match self {
            SelectorKind::Primitive => "primitive",
            SelectorKind::Policy => "policy",
            SelectorKind::Composite => "composition",
        }
    }
}

impl std::fmt::Display for SelectorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Parametre tipi (TOML `type` anahtarı).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParamKind {
    Bool,
    Number,
    String,
    List,
    Map,
}

/// Tek parametre tanımı — P1.3'ün kullanıcı config'ini bu şemaya göre doğrulaması
/// ve TUI'nin form/yardım basması için yeterli bilgiyi taşır.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct ParamDef {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: ParamKind,
    #[serde(default = "default_optional")]
    pub optional: bool,
    #[serde(default)]
    pub default: Option<String>,
    #[serde(default)]
    pub help: Option<String>,
}

/// Varsayılan davranış: belirtilmezse parametre opsiyoneldir.
fn default_optional() -> bool {
    true
}

/// Parametre şeması: mod başına sıralı tanım listesi (TOML `[[modes.params]]`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(transparent)]
pub struct ModeParams {
    pub defs: Vec<ParamDef>,
}

impl ModeParams {
    /// Belirtilen ada sahip parametre tanımı.
    pub fn find(&self, name: &str) -> Option<&ParamDef> {
        self.defs.iter().find(|d| d.name == name)
    }
}

/// Bir routing modunun tam tanımı (spec 3.2).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct RoutingModeDef {
    /// Katalog içinde unique olmalıdır; boş olamaz.
    pub id: String,
    pub family: ModeFamily,
    /// TUI picker'da gösterilen kısa başlık.
    pub title: String,
    /// Tek cümlelik özet (TUI blurb paneli).
    pub blurb: String,
    /// `/routing help <id>` için uzun yardım metni.
    pub long_help: String,
    /// Parametre şeması; TOML `[[modes.params]]` anahtarı. Belirtilmemişse boş şema.
    #[serde(rename = "params", default)]
    pub params_schema: ModeParams,
    /// Sınıf (primitive/policy/composition); TOML `class` anahtarı.
    #[serde(rename = "class")]
    pub selector_kind: SelectorKind,
    /// Somut seçici tanımlayıcısı; P1.3'te `selector_kind` ile birlikte
    /// implementasyona eşlenir (örn. "round_robin", "ordered_fallback", "hedge").
    pub selector: String,
}

/// Katalog dosyası üst düzey şeması.
#[derive(Deserialize)]
struct CatalogFile {
    modes: Vec<RoutingModeDef>,
}

/// `load_routing_modes` / `builtin_modes` hata tipleri.
#[derive(Debug, Error)]
pub enum RoutingCatalogError {
    /// Dosya okunamadı (yok olması HARİÇ — yoksa fallback devreye girer).
    #[error("routing katalog dosyası okunamadı: {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// TOML ayrıştırma hatası (bozuk sözdizimi, bilinmeyen family/class vb.).
    #[error("routing katalog TOML ayrıştırılamadı: {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    /// Şema doğrulama hatası (yinelenen/boş id, boş zorunlu alan vb.).
    #[error("routing katalog doğrulaması başarısız: {path}: {details}")]
    Validation { path: PathBuf, details: String },
}

/// `path`'teki katalog dosyasını ayrıştırır ve doğrular.
///
/// Dosya **yoksa** hata dönmez; gömülü varsayılan katalog döner. Bu davranış
/// cwd'den bağımsızdır (gömülü kopya derleme anında kodun içine gömülür), bu
/// yüzden test ve paketlenmiş ortamlarda da aynı şekilde çalışır. Diğer okuma
/// hataları, TOML hataları ve doğrulama ihlalleri `RoutingCatalogError` döner.
pub fn load_routing_modes(path: &Path) -> Result<Vec<RoutingModeDef>, RoutingCatalogError> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            tracing::debug!(
                path = %path.display(),
                "routing katalog dosyası bulunamadı; gömülü fallback kullanılıyor",
            );
            return Ok(builtin_modes().to_vec());
        }
        Err(source) => {
            return Err(RoutingCatalogError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    parse_catalog(&content, path)
}

/// Derleme anında gömülen kataloğun bir kopyası üzerinde kararlı built-in bakım.
///
/// Ayrıştırma yalnızca ilk çağrıda yapılır; gömülü veri bozulursa (geliştirme
/// hatası) panik yerine boş katalogla degrade olur ve hata loglanır.
pub fn builtin_modes() -> &'static [RoutingModeDef] {
    BUILTIN.get_or_init(|| match parse_embedded_catalog() {
        Ok(modes) => modes,
        Err(err) => {
            tracing::error!(error = %err, "gömülü routing katalog ayrıştırılamadı; boş katalogla devam ediliyor");
            Vec::new()
        }
    })
}

/// Gömülü varsayılan katalogda `id`'ye sahip modu arar; yoksa `None` döner.
pub fn find_mode(id: &str) -> Option<&'static RoutingModeDef> {
    find_in(builtin_modes(), id)
}

/// Verilen katalog diliminde `id`'ye sahip modu arar; yoksa `None` döner.
///
/// `load_routing_modes` ile yüklenen (dosya tabanlı) kataloglarda arama için
/// kullanılır; `find_mode` ise her zaman gömülü katalogda arar.
pub fn find_in<'a>(catalog: &'a [RoutingModeDef], id: &str) -> Option<&'a RoutingModeDef> {
    catalog.iter().find(|mode| mode.id == id)
}

static BUILTIN: OnceLock<Vec<RoutingModeDef>> = OnceLock::new();

/// Gömülü katalog kaynağı: repo kökündeki `config/routing_modes.toml`'un
/// derleme anındaki kopyası. Paketlenmiş kurulumda dosya sistemi kopyası
/// mevcut olmasa bile çalışır.
const EMBEDDED_CATALOG: &str = include_str!("../../../../../config/routing_modes.toml");

fn parse_embedded_catalog() -> Result<Vec<RoutingModeDef>, RoutingCatalogError> {
    parse_catalog(EMBEDDED_CATALOG, Path::new("config/routing_modes.toml"))
}

fn parse_catalog(
    content: &str,
    path: &Path,
) -> Result<Vec<RoutingModeDef>, RoutingCatalogError> {
    let file: CatalogFile = toml::from_str(content).map_err(|source| RoutingCatalogError::Parse {
        path: path.to_path_buf(),
        source,
    })?;
    validate_catalog(&file.modes, path)?;
    Ok(file.modes)
}

/// Unique id ve zorunlu alan kontrolü; tüm ihlaller tek mesajda toplanır.
fn validate_catalog(
    modes: &[RoutingModeDef],
    path: &Path,
) -> Result<(), RoutingCatalogError> {
    let mut errors: Vec<String> = Vec::new();
    let mut seen: HashSet<&str> = HashSet::with_capacity(modes.len());
    for mode in modes {
        if mode.id.trim().is_empty() {
            errors.push("boş mod id".to_string());
        } else if !seen.insert(mode.id.as_str()) {
            errors.push(format!("yinelenen mod id: {}", mode.id));
        }
        if mode.title.trim().is_empty() {
            errors.push(format!("{}: title boş", mode.id));
        }
        if mode.blurb.trim().is_empty() {
            errors.push(format!("{}: blurb boş", mode.id));
        }
        if mode.long_help.trim().is_empty() {
            errors.push(format!("{}: long_help boş", mode.id));
        }
        if mode.selector.trim().is_empty() {
            errors.push(format!("{}: selector boş", mode.id));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(RoutingCatalogError::Validation {
            path: path.to_path_buf(),
            details: errors.join("; "),
        })
    }
}
