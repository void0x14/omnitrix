//! Persona kayit defteri (MASTER-PLAN 11.1 / 11.2).
//!
//! Acilista `config/personas/` taranir, her `*.toml` dosyasi `_schema.md`'ye
//! gore valide edilir ve `personas` cache tablosuna yazilir. **MASTER-PLAN 60
//! personayi doldurmaz** (11.2): burada yalnizca sema + yukleyici vardir,
//! katalogu kullanici doldurur.
//!
//! **Hot-reload (Faz 5):** [`SharedPersonas`] defteri kilit arkasinda tutar,
//! [`SharedPersonas::watch`] ise `xai-fsnotify` ile persona dizinini izler.
//! Yeni/degisen/silinen `*.toml` **derleme olmadan**, surec calisirken yuklenir
//! ve [`PersonaChange`] olarak yayinlanir. Yeniden tarama basarisiz olursa
//! (yarim yazilmis TOML, sema ihlali) onceki defter **korunur**: hot-reload
//! calisan sistemi bozamaz.
//!
//! I5: bu dosyada hicbir literal model adi/fiyati yoktur. `model` alani
//! diskteki persona dosyasindan gelir, koda gomulu degildir.
//! I2: bagimlilik tek yon — `omni-core` -> `xai-fsnotify`; vendored agaca
//! hicbir sey yazilmaz.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError, RwLock, RwLockReadGuard};

use omni_proto::{Timestamp, now};
use omni_storage::traits::StorageError;
use omni_storage::writer_actor::{WriteOp, WriterActor};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, oneshot};
use tokio::task::JoinHandle;
use xai_fsnotify::{FsConfig, FsEvent, FsEventSource, FsNotifyError};

/// Sema dosyasinin adi (11.2). Persona taramasindan haric tutulur.
pub const SCHEMA_FILE: &str = "_schema.md";

/// Persona dosyalarinin uzantisi.
const PERSONA_EXT: &str = "toml";

/// Yukleyicinin tanidigi ust duzey alanlar (11.1 semasi).
const KNOWN_KEYS: &[&str] = &[
    "name",
    "description",
    "system_prompt",
    "role",
    "temperature",
    "tools",
    "disallowed",
    "budget",
    "routing",
    "max_depth",
    "recording",
    "model",
];

/// Bulunmasi zorunlu alanlar.
const REQUIRED_KEYS: &[&str] = &["name", "description"];

/// Ornekleme sicakligi icin kabul edilen araligin ust siniri.
const MAX_TEMPERATURE: f32 = 2.0;

/// Hot-reload izleyicisinin varsayilan debounce penceresi (ms).
///
/// Editorler dosyayi genelde "yaz + yeniden adlandir" ile kaydeder; pencere
/// bu carpmalari tek bir yeniden taramaya indirger.
pub const PERSONA_DEBOUNCE_MS: u64 = 150;

/// Degisim yayin kanalinin kapasitesi. Tuketici geride kalirsa `Lagged` gelir;
/// izleyici bunu **tam yeniden tarama** ile karsilar, olay kaybetmez.
const CHANGE_CHANNEL_CAPACITY: usize = 64;

/// Persona katmani hatalari. Uretim yolunda panik yok (I6).
#[derive(Debug, thiserror::Error)]
pub enum PersonaError {
    /// Persona dizini okunamadi.
    #[error("persona dizini okunamadi ({path}): {source}")]
    Dir {
        /// Okunmaya calisilan dizin.
        path: PathBuf,
        /// Alt hata.
        source: std::io::Error,
    },

    /// Dosya okunamadi.
    #[error("persona dosyasi okunamadi ({path}): {source}")]
    Read {
        /// Okunmaya calisilan dosya.
        path: PathBuf,
        /// Alt hata.
        source: std::io::Error,
    },

    /// TOML cozulemedi.
    #[error("persona dosyasi cozulemedi ({path}): {source}")]
    Parse {
        /// Cozulemeyen dosya.
        path: PathBuf,
        /// Alt hata.
        source: toml::de::Error,
    },

    /// Sema disi ya da tutarsiz alan.
    #[error("persona '{name}' gecersiz: {reason}")]
    Invalid {
        /// Persona adi (ya da dosya kok adi).
        name: String,
        /// Gerekce.
        reason: String,
    },

    /// Ayni ad iki dosyada tanimlanmis.
    #[error("persona adi cakisiyor: '{name}' hem {first} hem {second} icinde")]
    Duplicate {
        /// Cakisan ad.
        name: String,
        /// Once yuklenen dosya.
        first: PathBuf,
        /// Sonra gelen dosya.
        second: PathBuf,
    },

    /// `_schema.md` yukleyicinin bilmedigi alan tanimliyor (sema kaymasi).
    #[error("{path} yukleyicinin bilmedigi alan(lar) iceriyor: {keys}")]
    SchemaDrift {
        /// Sema dosyasi.
        path: PathBuf,
        /// Bilinmeyen alanlar, virgulle ayrilmis.
        keys: String,
    },

    /// `_schema.md` icinde ```toml blogu yok.
    #[error("{path} icinde toml ornek blogu yok")]
    SchemaBlock {
        /// Sema dosyasi.
        path: PathBuf,
    },

    /// `personas` tablosuna yazilamadi.
    #[error("persona cache satiri yazilamadi: {0}")]
    Storage(#[from] StorageError),

    /// Dosya izleyicisi baslatilamadi (hot-reload, Faz 5).
    #[error("persona dizini izlenemedi ({path}): {source}")]
    Watch {
        /// Izlenmeye calisilan dizin.
        path: PathBuf,
        /// Alt hata.
        source: FsNotifyError,
    },
}

/// Persona butcesi (`budget = { mode = "...", max_cost = ... }`, AS4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersonaBudget {
    /// Butce modu adi; yapilandirmadan gelir, koda gomulu degildir.
    pub mode: String,
    /// Ust sinir; `None` ise mod kendi zarfini belirler.
    #[serde(default)]
    pub max_cost: Option<f64>,
}

/// Diskteki bir persona dosyasinin cozulmus hali (11.1 semasi).
///
/// `omni-agent` bu tipi `PersonaSpec`/`AgentDefinition`'a esler; bu crate
/// vendored ajan tiplerine bagimli degildir.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersonaDefinition {
    /// Persona adi; dosya kok adiyla ayni olmak zorundadir.
    pub name: String,
    /// Kisa aciklama.
    pub description: String,
    /// Sistem promptu dosya referansi (persona dizinine gore).
    #[serde(default)]
    pub system_prompt: Option<String>,
    /// JEP rolu (10.1); model esleme katalogdan gelir (AS7).
    #[serde(default)]
    pub role: Option<String>,
    /// Ornekleme sicakligi.
    #[serde(default)]
    pub temperature: Option<f32>,
    /// K3 allowlist — acilacak tool adlari.
    #[serde(default)]
    pub tools: Vec<String>,
    /// Kapatilacak tool adlari.
    #[serde(default)]
    pub disallowed: Vec<String>,
    /// Butce zarfi (AS4).
    #[serde(default)]
    pub budget: Option<PersonaBudget>,
    /// Yonlendirme politikasi adi (6.5).
    #[serde(default)]
    pub routing: Option<String>,
    /// Derinlik tavani override'i (AS3).
    #[serde(default)]
    pub max_depth: Option<u8>,
    /// Kayit modu (AS6).
    #[serde(default)]
    pub recording: Option<String>,
    /// Model kimligi; **yalnizca** yapilandirmadan gelir (I5).
    #[serde(default)]
    pub model: Option<String>,
}

impl PersonaDefinition {
    /// Alan tutarliligini dogrular.
    ///
    /// # Errors
    /// Zorunlu alan bos, ad dosya adiyla uyusmuyor, sicaklik arali disinda ya
    /// da tool listeleri celisiyorsa [`PersonaError::Invalid`] doner.
    pub fn validate(&self, stem: &str) -> Result<(), PersonaError> {
        let bad = |reason: String| PersonaError::Invalid {
            name: if self.name.trim().is_empty() {
                stem.to_owned()
            } else {
                self.name.clone()
            },
            reason,
        };

        if self.name.trim().is_empty() {
            return Err(bad("ad bos".to_owned()));
        }
        if self.name != stem {
            return Err(bad(format!(
                "ad dosya adiyla uyusmuyor (dosya '{stem}.{PERSONA_EXT}')"
            )));
        }
        if !self
            .name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
        {
            return Err(bad(
                "ad yalnizca kucuk harf, rakam, '_' ve '-' icerebilir".to_owned(),
            ));
        }
        if self.description.trim().is_empty() {
            return Err(bad("aciklama bos".to_owned()));
        }
        if let Some(t) = self.temperature
            && (!t.is_finite() || !(0.0..=MAX_TEMPERATURE).contains(&t))
        {
            return Err(bad(format!(
                "sicaklik 0.0..={MAX_TEMPERATURE} araliginda olmali"
            )));
        }
        if let Some(depth) = self.max_depth
            && depth == 0
        {
            return Err(bad("max_depth en az 1 olmali".to_owned()));
        }
        for tool in self.tools.iter().chain(self.disallowed.iter()) {
            if tool.trim().is_empty() {
                return Err(bad("tool listesinde bos girdi var".to_owned()));
            }
        }
        if let Some(cakisan) = self
            .tools
            .iter()
            .find(|t| self.disallowed.iter().any(|d| d == *t))
        {
            return Err(bad(format!(
                "'{cakisan}' hem tools hem disallowed icinde"
            )));
        }
        if let Some(budget) = &self.budget {
            if budget.mode.trim().is_empty() {
                return Err(bad("butce modu bos".to_owned()));
            }
            if let Some(max) = budget.max_cost
                && (!max.is_finite() || max < 0.0)
            {
                return Err(bad(
                    "max_cost sonlu ve negatif olmayan olmali".to_owned(),
                ));
            }
        }
        Ok(())
    }
}

/// Persona semasi.
///
/// Alan kumesi `_schema.md` icindeki ornek TOML blogundan okunur; dosya yoksa
/// yukleyicinin yerlesik kumesi kullanilir. Sema dosyasi yukleyicinin
/// bilmedigi bir alan tanimlarsa yukleme **hata verir** — sessiz kayma yok.
#[derive(Debug, Clone)]
pub struct PersonaSchema {
    keys: BTreeSet<String>,
    required: BTreeSet<String>,
    source: Option<PathBuf>,
    checksum: Option<String>,
}

impl Default for PersonaSchema {
    fn default() -> Self {
        Self::builtin()
    }
}

impl PersonaSchema {
    /// Yukleyiciye gomulu sema (11.1 referansi).
    #[must_use]
    pub fn builtin() -> Self {
        Self {
            keys: KNOWN_KEYS.iter().map(|k| (*k).to_owned()).collect(),
            required: REQUIRED_KEYS.iter().map(|k| (*k).to_owned()).collect(),
            source: None,
            checksum: None,
        }
    }

    /// Persona dizinindeki `_schema.md`'yi yukler; yoksa yerlesik semaya duser.
    ///
    /// # Errors
    /// Dosya okunamazsa, icinde TOML blogu yoksa, blok cozulemezse ya da
    /// yukleyicinin bilmedigi alan tanimliyorsa hata doner.
    pub fn load(dir: &Path) -> Result<Self, PersonaError> {
        let path = dir.join(SCHEMA_FILE);
        if !path.is_file() {
            return Ok(Self::builtin());
        }
        let raw = std::fs::read_to_string(&path).map_err(|source| PersonaError::Read {
            path: path.clone(),
            source,
        })?;
        Self::from_markdown(&raw, &path)
    }

    /// Markdown govdesindeki ilk TOML blogundan sema cikarir.
    ///
    /// # Errors
    /// Blok yoksa, cozulemezse ya da bilinmeyen alan iceriyorsa hata doner.
    pub fn from_markdown(doc: &str, path: &Path) -> Result<Self, PersonaError> {
        let block = toml_block(doc).ok_or_else(|| PersonaError::SchemaBlock {
            path: path.to_path_buf(),
        })?;
        let table: toml::Table =
            toml::from_str(&block).map_err(|source| PersonaError::Parse {
                path: path.to_path_buf(),
                source,
            })?;

        let keys: BTreeSet<String> = table.keys().cloned().collect();
        let bilinmeyen: Vec<&str> = keys
            .iter()
            .map(String::as_str)
            .filter(|k| !KNOWN_KEYS.contains(k))
            .collect();
        if !bilinmeyen.is_empty() {
            return Err(PersonaError::SchemaDrift {
                path: path.to_path_buf(),
                keys: bilinmeyen.join(", "),
            });
        }

        let required = REQUIRED_KEYS
            .iter()
            .map(|k| (*k).to_owned())
            .filter(|k| keys.contains(k))
            .collect();

        Ok(Self {
            keys,
            required,
            source: Some(path.to_path_buf()),
            checksum: Some(checksum(doc.as_bytes())),
        })
    }

    /// Semanin okundugu dosya (yerlesik semada `None`).
    #[must_use]
    pub fn source(&self) -> Option<&Path> {
        self.source.as_deref()
    }

    /// Sema dosyasinin ozeti; degisiklik tespiti icin.
    #[must_use]
    pub fn checksum(&self) -> Option<&str> {
        self.checksum.as_deref()
    }

    /// Semanin tanidigi alanlar.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.keys.iter().map(String::as_str)
    }

    /// Bir persona tablosunun alanlarini semaya gore denetler.
    ///
    /// # Errors
    /// Zorunlu alan eksikse ya da sema disi alan varsa hata doner.
    pub fn check_table(&self, table: &toml::Table, stem: &str) -> Result<(), PersonaError> {
        for zorunlu in &self.required {
            if !table.contains_key(zorunlu) {
                return Err(PersonaError::Invalid {
                    name: stem.to_owned(),
                    reason: format!("zorunlu alan eksik: {zorunlu}"),
                });
            }
        }
        let fazla: Vec<&str> = table
            .keys()
            .map(String::as_str)
            .filter(|k| !self.keys.contains(*k))
            .collect();
        if !fazla.is_empty() {
            return Err(PersonaError::Invalid {
                name: stem.to_owned(),
                reason: format!("sema disi alan(lar): {}", fazla.join(", ")),
            });
        }
        Ok(())
    }
}

/// Diskten yuklenmis tek persona.
#[derive(Debug, Clone)]
pub struct LoadedPersona {
    /// Cozulmus tanim.
    pub definition: PersonaDefinition,
    /// Dosya yolu (`personas.path`).
    pub path: PathBuf,
    /// Dosya icerik ozeti (`personas.checksum`).
    pub checksum: String,
    /// Yukleme ani (`personas.loaded_at`).
    pub loaded_at: Timestamp,
}

impl LoadedPersona {
    /// Persona adi.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.definition.name
    }

    /// `system_prompt` referansinin cozulmus yolu (persona dizinine gore).
    #[must_use]
    pub fn system_prompt_path(&self) -> Option<PathBuf> {
        let referans = self.definition.system_prompt.as_ref()?;
        let dizin = self.path.parent()?;
        Some(dizin.join(referans))
    }

    /// `system_prompt` dosyasini okur.
    ///
    /// # Errors
    /// Referans tanimliysa ama dosya okunamiyorsa hata doner. Referans yoksa
    /// `Ok(None)` doner.
    pub fn read_system_prompt(&self) -> Result<Option<String>, PersonaError> {
        let Some(path) = self.system_prompt_path() else {
            return Ok(None);
        };
        std::fs::read_to_string(&path)
            .map(Some)
            .map_err(|source| PersonaError::Read { path, source })
    }
}

/// Persona kayit defteri (11.2).
///
/// Tek seferlik yukleyici: `config/personas/` taranir, valide edilir, bellekte
/// tutulur ve [`PersonaRegistry::persist`] ile `personas` tablosuna yazilir.
/// Dosya izleme (hot-reload) FAZ 5'te eklenir.
#[derive(Debug, Clone)]
pub struct PersonaRegistry {
    dir: PathBuf,
    schema: PersonaSchema,
    personas: BTreeMap<String, LoadedPersona>,
}

impl PersonaRegistry {
    /// Yapilandirma kokune gore varsayilan persona dizini.
    #[must_use]
    pub fn default_dir(config_root: &Path) -> PathBuf {
        config_root.join("personas")
    }

    /// Verilen dizini bir kez tarar.
    ///
    /// Dizin yoksa **bos defter** doner (katalog kullanici tarafindan
    /// doldurulur, 11.2); bu bir hata degildir. `_schema.md` ve `_` ile
    /// baslayan dosyalar taramanin disindadir.
    ///
    /// # Errors
    /// Dizin okunamazsa, bir dosya cozulemezse, sema ihlali ya da ad cakismasi
    /// varsa hata doner.
    pub fn load_from(dir: &Path) -> Result<Self, PersonaError> {
        let mut registry = Self {
            dir: dir.to_path_buf(),
            schema: PersonaSchema::builtin(),
            personas: BTreeMap::new(),
        };
        if !dir.is_dir() {
            tracing::info!(dizin = %dir.display(), "persona dizini yok, defter bos");
            return Ok(registry);
        }
        registry.schema = PersonaSchema::load(dir)?;

        let girdiler = std::fs::read_dir(dir).map_err(|source| PersonaError::Dir {
            path: dir.to_path_buf(),
            source,
        })?;

        // Deterministik sira: dosya sistemi sirasi platforma gore degisir.
        let mut yollar: Vec<PathBuf> = Vec::new();
        for girdi in girdiler {
            let girdi = girdi.map_err(|source| PersonaError::Dir {
                path: dir.to_path_buf(),
                source,
            })?;
            let path = girdi.path();
            if !path.is_file() {
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some(PERSONA_EXT) {
                continue;
            }
            let gizli = path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with('_'));
            if gizli {
                continue;
            }
            yollar.push(path);
        }
        yollar.sort();

        for path in yollar {
            let persona = registry.load_file(&path)?;
            let ad = persona.name().to_owned();
            if let Some(onceki) = registry.personas.get(&ad) {
                return Err(PersonaError::Duplicate {
                    name: ad,
                    first: onceki.path.clone(),
                    second: path,
                });
            }
            registry.personas.insert(ad, persona);
        }
        tracing::info!(
            dizin = %dir.display(),
            adet = registry.personas.len(),
            "persona defteri yuklendi"
        );
        Ok(registry)
    }

    fn load_file(&self, path: &Path) -> Result<LoadedPersona, PersonaError> {
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_owned();
        let raw = std::fs::read_to_string(path).map_err(|source| PersonaError::Read {
            path: path.to_path_buf(),
            source,
        })?;

        // Once sema alan denetimi, sonra tipli cozme: hata mesajlari boylece
        // "sema disi alan" ile "tip uyusmazligi"ni ayirt eder.
        let table: toml::Table = toml::from_str(&raw).map_err(|source| PersonaError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
        self.schema.check_table(&table, &stem)?;

        let definition: PersonaDefinition =
            toml::from_str(&raw).map_err(|source| PersonaError::Parse {
                path: path.to_path_buf(),
                source,
            })?;
        definition.validate(&stem)?;

        Ok(LoadedPersona {
            definition,
            path: path.to_path_buf(),
            checksum: checksum(raw.as_bytes()),
            loaded_at: now(),
        })
    }

    /// Tarama yapilan dizin.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Kullanilan sema.
    #[must_use]
    pub fn schema(&self) -> &PersonaSchema {
        &self.schema
    }

    /// Ada gore persona.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&LoadedPersona> {
        self.personas.get(name)
    }

    /// Yuklu persona adlari (alfabetik).
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.personas.keys().map(String::as_str)
    }

    /// Yuklu personalar (alfabetik).
    pub fn iter(&self) -> impl Iterator<Item = &LoadedPersona> {
        self.personas.values()
    }

    /// Yuklu persona sayisi.
    #[must_use]
    pub fn len(&self) -> usize {
        self.personas.len()
    }

    /// Defter bos mu?
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.personas.is_empty()
    }

    /// Defteri `personas` cache tablosuna yazar (11.2, migration 0008).
    ///
    /// Yazim tek yazici uzerinden gider (I7); ad birincil anahtardir, yeniden
    /// yukleme satiri gunceller. Etkilenen satir sayisi doner.
    ///
    /// # Errors
    /// Yazici kapaliysa ya da SQL basarisiz olursa [`PersonaError::Storage`]
    /// doner.
    pub async fn persist(&self, writer: &WriterActor) -> Result<usize, PersonaError> {
        const SQL: &str = "INSERT INTO personas (name, path, checksum) VALUES (?1, ?2, ?3) \
             ON CONFLICT(name) DO UPDATE SET path = excluded.path, \
             checksum = excluded.checksum, loaded_at = datetime('now')";

        let mut yazilan = 0_usize;
        for persona in self.personas.values() {
            let (reply, rx) = oneshot::channel();
            let op = WriteOp::Execute {
                sql: SQL.to_owned(),
                params: vec![
                    rusqlite::types::Value::Text(persona.name().to_owned()),
                    rusqlite::types::Value::Text(persona.path.display().to_string()),
                    rusqlite::types::Value::Text(persona.checksum.clone()),
                ],
                reply,
            };
            writer.write(op).await?;
            match rx.await {
                Ok(Ok(satir)) => yazilan += satir,
                Ok(Err(err)) => return Err(err.into()),
                Err(_) => {
                    return Err(PersonaError::Storage(StorageError::Internal(
                        "persona yaziminda yanit kanali kapandi".to_owned(),
                    )));
                }
            }
        }
        Ok(yazilan)
    }
}

/// Bir yeniden taramanin defterde yaptigi tek degisiklik (Faz 5 hot-reload).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PersonaChange {
    /// Dizine yeni bir persona dosyasi girdi — **derleme yok**.
    Added(String),
    /// Var olan personanin icerigi degisti (ozet farkli).
    Updated(String),
    /// Persona dosyasi dizinden kalkti.
    Removed(String),
}

impl PersonaChange {
    /// Degisimin ilgili oldugu persona adi.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Added(ad) | Self::Updated(ad) | Self::Removed(ad) => ad,
        }
    }
}

/// Iki tarama arasindaki farki uretir; sira deterministiktir (ad sirasi).
fn diff(
    onceki: &BTreeMap<String, LoadedPersona>,
    taze: &BTreeMap<String, LoadedPersona>,
) -> Vec<PersonaChange> {
    let mut degisim = Vec::new();
    for (ad, yeni) in taze {
        match onceki.get(ad) {
            None => degisim.push(PersonaChange::Added(ad.clone())),
            Some(eski) if eski.checksum != yeni.checksum => {
                degisim.push(PersonaChange::Updated(ad.clone()));
            }
            Some(_) => {}
        }
    }
    for ad in onceki.keys() {
        if !taze.contains_key(ad) {
            degisim.push(PersonaChange::Removed(ad.clone()));
        }
    }
    degisim
}

/// Bir persona dosya olayinin yeniden taramayi tetikleyip tetiklemedigi.
///
/// Yalnizca persona dosyalari (`*.toml`, `_` ile baslamayanlar) ve sema dosyasi
/// sayilir; prompt dizinleri ve editor gecici dosyalari gurultudur.
fn is_relevant(path: &Path) -> bool {
    let Some(ad) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    if ad == SCHEMA_FILE {
        return true;
    }
    let persona_uzantisi = path.extension().and_then(|e| e.to_str()) == Some(PERSONA_EXT);
    persona_uzantisi && !ad.starts_with('_')
}

/// Paylasilan, hot-reload'a acik persona defteri (Faz 5).
///
/// Klonlanabilir tutamak: tum klonlar ayni defteri gorur. Okuyucular
/// [`SharedPersonas::snapshot`] / [`SharedPersonas::get`] ile anlik goruntu
/// alir; [`SharedPersonas::watch`] arka planda dizini izleyip defteri
/// **derlemesiz** tazeler.
#[derive(Debug, Clone)]
pub struct SharedPersonas {
    inner: Arc<RwLock<PersonaRegistry>>,
    changes: broadcast::Sender<PersonaChange>,
}

impl SharedPersonas {
    /// Yuklenmis bir defteri paylasima acar.
    #[must_use]
    pub fn new(registry: PersonaRegistry) -> Self {
        let (changes, _) = broadcast::channel(CHANGE_CHANNEL_CAPACITY);
        Self {
            inner: Arc::new(RwLock::new(registry)),
            changes,
        }
    }

    /// Dizini bir kez tarayip paylasilan defter uretir.
    ///
    /// # Errors
    /// [`PersonaRegistry::load_from`] ile ayni kosullarda hata doner.
    pub fn load_from(dir: &Path) -> Result<Self, PersonaError> {
        Ok(Self::new(PersonaRegistry::load_from(dir)?))
    }

    /// Kilidi zehirlenmis olsa bile okuma erisimi verir.
    ///
    /// Zehirlenme yalnizca **baska** bir is parcaciginin kilit altinda panik
    /// atmasiyla olusur; defter degismez veri tuttugu icin ic tutarlilik
    /// bozulmaz, dolayisiyla panige donusturmek yerine icerik kullanilir (I6).
    fn read(&self) -> RwLockReadGuard<'_, PersonaRegistry> {
        self.inner.read().unwrap_or_else(PoisonError::into_inner)
    }

    /// Izlenen persona dizini.
    #[must_use]
    pub fn dir(&self) -> PathBuf {
        self.read().dir.clone()
    }

    /// Defterin o andaki tam kopyasi.
    #[must_use]
    pub fn snapshot(&self) -> PersonaRegistry {
        self.read().clone()
    }

    /// Ada gore personanin o andaki kopyasi.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<LoadedPersona> {
        self.read().get(name).cloned()
    }

    /// Yuklu persona adlari (alfabetik).
    #[must_use]
    pub fn names(&self) -> Vec<String> {
        self.read().names().map(str::to_owned).collect()
    }

    /// Yuklu persona sayisi.
    #[must_use]
    pub fn len(&self) -> usize {
        self.read().len()
    }

    /// Defter bos mu?
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.read().is_empty()
    }

    /// Degisim yayinina abone olur. Abone geride kalirsa `Lagged` gorur;
    /// dogru davranis [`SharedPersonas::snapshot`] ile tam durumu okumaktir.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<PersonaChange> {
        self.changes.subscribe()
    }

    /// Dizini yeniden tarar, farki yayinlar ve defteri degistirir.
    ///
    /// **Ya hep ya hic:** tarama basarisiz olursa defter **hic** dokunulmadan
    /// kalir ve hata doner; yarim yazilmis bir dosya calisan sistemi bozmaz.
    ///
    /// # Errors
    /// [`PersonaRegistry::load_from`] ile ayni kosullarda hata doner.
    pub fn reload(&self) -> Result<Vec<PersonaChange>, PersonaError> {
        let dir = self.dir();
        // Tarama kilit DISINDA: dosya G/C suresince okuyuculari bloklamayiz.
        let taze = PersonaRegistry::load_from(&dir)?;

        let degisim = {
            let mut guard = self.inner.write().unwrap_or_else(PoisonError::into_inner);
            let degisim = diff(&guard.personas, &taze.personas);
            if degisim.is_empty() {
                // Yalnizca alakasiz dosya dokunulmus; defteri degistirme.
                return Ok(degisim);
            }
            *guard = taze;
            degisim
        };

        for olay in &degisim {
            // Abone yoksa hata doner; yayin en-fazla-bir-kez, kayip tolere edilir.
            let _ = self.changes.send(olay.clone());
        }
        Ok(degisim)
    }

    /// Defteri `personas` cache tablosuna yazar (11.2).
    ///
    /// # Errors
    /// [`PersonaRegistry::persist`] ile ayni kosullarda hata doner.
    pub async fn persist(&self, writer: &WriterActor) -> Result<usize, PersonaError> {
        // Anlik goruntu alinir: `await` boyunca kilit tutulmaz.
        let anlik = self.snapshot();
        anlik.persist(writer).await
    }

    /// Persona dizinini izlemeye baslar (Faz 5 kapisi: yeni persona dosyasi
    /// **derlemesiz** yuklenir).
    ///
    /// Dizin yoksa olusturulur — aksi halde OS izleyicisi baglanamaz ve dizin
    /// sonradan yaratilinca hicbir olay gelmez.
    ///
    /// Bir tokio calisma zamani icinde cagrilmalidir.
    ///
    /// # Errors
    /// Dizin olusturulamazsa [`PersonaError::Dir`], OS izleyicisi
    /// baslatilamazsa [`PersonaError::Watch`] doner.
    pub fn watch(&self) -> Result<PersonaWatcher, PersonaError> {
        self.watch_with_debounce_ms(PERSONA_DEBOUNCE_MS)
    }

    /// [`SharedPersonas::watch`] ile ayni, debounce penceresi cagrandan gelir.
    ///
    /// # Errors
    /// [`SharedPersonas::watch`] ile ayni kosullarda hata doner.
    pub fn watch_with_debounce_ms(&self, debounce_ms: u64) -> Result<PersonaWatcher, PersonaError> {
        let dir = self.dir();
        if !dir.is_dir() {
            std::fs::create_dir_all(&dir).map_err(|source| PersonaError::Dir {
                path: dir.clone(),
                source,
            })?;
        }

        let config = FsConfig::default().with_debounce_ms(debounce_ms);
        // `shared()` degil `start()`: defterin izleyicisi bize aittir, boylece
        // `PersonaWatcher` dusunce OS izleyicisi de kapanir.
        let source = FsEventSource::start(dir.clone(), config)
            .map_err(|source| PersonaError::Watch { path: dir, source })?;
        let events = source.subscribe();
        let personas = self.clone();
        // `tokio::spawn` calisma zamani disinda panikler; buraya ancak
        // `FsEventSource::start` BASARILI olunca gelinir ve o da calisma zamani
        // yoksa `NoRuntime` doner. Sira paniki disarida birakir (I6).
        let task = tokio::spawn(watch_loop(personas, events));

        Ok(PersonaWatcher {
            personas: self.clone(),
            _source: source,
            task,
        })
    }
}

/// Persona dizinini izleyen arka plan gorevinin sahibi (Faz 5).
///
/// Dusurulunce OS izleyicisi ve yeniden yukleme gorevi **birlikte** kapanir;
/// sizan is parcacigi kalmaz.
pub struct PersonaWatcher {
    personas: SharedPersonas,
    _source: FsEventSource,
    task: JoinHandle<()>,
}

impl PersonaWatcher {
    /// Izlenen paylasilan defter.
    #[must_use]
    pub fn personas(&self) -> &SharedPersonas {
        &self.personas
    }

    /// Degisim yayinina abone olur.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<PersonaChange> {
        self.personas.subscribe()
    }

    /// Izlemeyi durdurur. `Drop` de ayni isi yapar; bu cagri niyeti aciklar.
    pub fn shutdown(self) {
        drop(self);
    }
}

impl Drop for PersonaWatcher {
    fn drop(&mut self) {
        self.task.abort();
    }
}

// `FsEventSource` Debug turetmez; izleyiciyi yine de yapilarin icine gomulebilir
// tutmak icin elle yazildi.
impl std::fmt::Debug for PersonaWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PersonaWatcher")
            .field("dir", &self.personas.dir())
            .field("len", &self.personas.len())
            .finish_non_exhaustive()
    }
}

/// Dosya olaylarini dinleyip ilgili her degisiklikte defteri tazeler.
async fn watch_loop(personas: SharedPersonas, mut events: broadcast::Receiver<FsEvent>) {
    loop {
        let tetikle = match events.recv().await {
            Ok(FsEvent::FilesChanged { paths, .. }) => paths.iter().any(|p| is_relevant(p)),
            // Git/VCS olaylari personayi ilgilendirmez.
            Ok(_) => false,
            Err(broadcast::error::RecvError::Lagged(atlanan)) => {
                tracing::warn!(atlanan, "persona dosya olaylari atlandi; tam tarama yapiliyor");
                true
            }
            Err(broadcast::error::RecvError::Closed) => break,
        };
        if !tetikle {
            continue;
        }

        // Yeniden tarama senkron dosya G/C'sidir; calisma zamanini bloklamamak
        // icin blocking havuzuna verilir.
        let tutamak = personas.clone();
        match tokio::task::spawn_blocking(move || tutamak.reload()).await {
            Ok(Ok(degisim)) if !degisim.is_empty() => {
                tracing::info!(adet = degisim.len(), "persona defteri tazelendi");
            }
            Ok(Ok(_)) => {}
            Ok(Err(err)) => {
                // Onceki defter korunur: bozuk dosya calisan sistemi durdurmaz.
                tracing::warn!(hata = %err, "persona yeniden yuklenemedi; onceki defter korunuyor");
            }
            Err(err) => {
                tracing::warn!(hata = %err, "persona yeniden yukleme gorevi tamamlanamadi");
            }
        }
    }
}

/// `blake3:<hex>` bicimli icerik ozeti (CAS atiflariyla ayni bicim).
fn checksum(bytes: &[u8]) -> String {
    format!("blake3:{}", blake3::hash(bytes).to_hex())
}

/// Markdown govdesindeki ilk ```toml blogunu cikarir.
fn toml_block(doc: &str) -> Option<String> {
    let mut icerde = false;
    let mut satirlar: Vec<&str> = Vec::new();
    for satir in doc.lines() {
        let kirpik = satir.trim_start();
        if icerde {
            if kirpik.starts_with("```") {
                return Some(satirlar.join("\n"));
            }
            satirlar.push(satir);
        } else if kirpik.starts_with("```toml") {
            icerde = true;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn yaz(dir: &Path, ad: &str, govde: &str) {
        std::fs::write(dir.join(ad), govde).expect("dosya yazilmali");
    }

    const PLANNER: &str = r#"
name = "planner"
description = "Gorevi yapi taslarina boler"
system_prompt = "prompts/planner.md"
role = "planner"
temperature = 0.2
tools = ["read", "search"]
disallowed = ["shell"]
routing = "jep"
max_depth = 3
recording = "event_log"
budget = { mode = "user_focused", max_cost = 5.0 }
"#;

    #[test]
    fn olmayan_dizin_bos_defter_verir() {
        let kok = tempfile::tempdir().expect("gecici dizin");
        let defter =
            PersonaRegistry::load_from(&PersonaRegistry::default_dir(kok.path())).expect("defter");
        assert!(defter.is_empty());
        assert_eq!(defter.len(), 0);
        assert!(defter.get("planner").is_none());
    }

    #[test]
    fn gecerli_persona_yuklenir() {
        let dir = tempfile::tempdir().expect("gecici dizin");
        yaz(dir.path(), "planner.toml", PLANNER);
        let defter = PersonaRegistry::load_from(dir.path()).expect("defter");

        assert_eq!(defter.len(), 1);
        let persona = defter.get("planner").expect("persona");
        assert_eq!(persona.name(), "planner");
        assert_eq!(persona.definition.role.as_deref(), Some("planner"));
        assert_eq!(persona.definition.tools, vec!["read", "search"]);
        assert!(persona.checksum.starts_with("blake3:"));
        assert_eq!(defter.names().collect::<Vec<_>>(), vec!["planner"]);
        assert_eq!(defter.iter().count(), 1);
        assert!(persona.definition.model.is_none());
    }

    #[test]
    fn alt_cizgili_dosyalar_atlanir() {
        let dir = tempfile::tempdir().expect("gecici dizin");
        yaz(dir.path(), "planner.toml", PLANNER);
        yaz(dir.path(), "_ornek.toml", "bozuk = [");
        yaz(dir.path(), "notlar.md", "persona degil");
        let defter = PersonaRegistry::load_from(dir.path()).expect("defter");
        assert_eq!(defter.len(), 1);
    }

    #[test]
    fn ad_dosya_adiyla_uyusmali() {
        let dir = tempfile::tempdir().expect("gecici dizin");
        yaz(dir.path(), "judge.toml", PLANNER);
        let hata = PersonaRegistry::load_from(dir.path()).expect_err("uyusmazlik");
        assert!(matches!(hata, PersonaError::Invalid { .. }));
    }

    #[test]
    fn zorunlu_alan_eksikse_reddedilir() {
        let dir = tempfile::tempdir().expect("gecici dizin");
        yaz(dir.path(), "bos.toml", "name = \"bos\"\n");
        let hata = PersonaRegistry::load_from(dir.path()).expect_err("eksik alan");
        assert!(matches!(hata, PersonaError::Invalid { .. }));
    }

    #[test]
    fn sema_disi_alan_reddedilir() {
        let dir = tempfile::tempdir().expect("gecici dizin");
        yaz(
            dir.path(),
            "ek.toml",
            "name = \"ek\"\ndescription = \"x\"\nuydurma = 1\n",
        );
        let hata = PersonaRegistry::load_from(dir.path()).expect_err("sema disi");
        match hata {
            PersonaError::Invalid { reason, .. } => assert!(reason.contains("uydurma")),
            other => panic!("beklenmeyen hata: {other}"),
        }
    }

    #[test]
    fn sicaklik_araligi_zorlanir() {
        let dir = tempfile::tempdir().expect("gecici dizin");
        yaz(
            dir.path(),
            "sicak.toml",
            "name = \"sicak\"\ndescription = \"x\"\ntemperature = 9.0\n",
        );
        assert!(matches!(
            PersonaRegistry::load_from(dir.path()),
            Err(PersonaError::Invalid { .. })
        ));
    }

    #[test]
    fn celisen_tool_listesi_reddedilir() {
        let dir = tempfile::tempdir().expect("gecici dizin");
        yaz(
            dir.path(),
            "celiski.toml",
            "name = \"celiski\"\ndescription = \"x\"\ntools = [\"shell\"]\ndisallowed = [\"shell\"]\n",
        );
        assert!(matches!(
            PersonaRegistry::load_from(dir.path()),
            Err(PersonaError::Invalid { .. })
        ));
    }

    #[test]
    fn bozuk_toml_parse_hatasi_verir() {
        let dir = tempfile::tempdir().expect("gecici dizin");
        yaz(dir.path(), "bozuk.toml", "name = [");
        assert!(matches!(
            PersonaRegistry::load_from(dir.path()),
            Err(PersonaError::Parse { .. })
        ));
    }

    #[test]
    fn sema_dosyasi_alan_kumesini_daraltir() {
        let dir = tempfile::tempdir().expect("gecici dizin");
        yaz(
            dir.path(),
            SCHEMA_FILE,
            "# sema\n\n```toml\nname = \"ornek\"\ndescription = \"ornek\"\n```\n",
        );
        yaz(dir.path(), "planner.toml", PLANNER);
        // Sema yalnizca name+description tanimliyor; planner fazla alan tasiyor.
        let hata = PersonaRegistry::load_from(dir.path()).expect_err("daraltilmis sema");
        assert!(matches!(hata, PersonaError::Invalid { .. }));
    }

    #[test]
    fn sema_dosyasi_tam_kume_ile_gecer() {
        let dir = tempfile::tempdir().expect("gecici dizin");
        let blok = format!("# sema\n\n```toml\n{PLANNER}\n```\n");
        yaz(dir.path(), SCHEMA_FILE, &blok);
        yaz(dir.path(), "planner.toml", PLANNER);
        let defter = PersonaRegistry::load_from(dir.path()).expect("defter");
        assert_eq!(defter.len(), 1);
        assert!(defter.schema().source().is_some());
        assert!(
            defter
                .schema()
                .checksum()
                .is_some_and(|c| c.starts_with("blake3:"))
        );
        assert!(defter.schema().keys().any(|k| k == "temperature"));
    }

    #[test]
    fn sema_kaymasi_yakalanir() {
        let dir = tempfile::tempdir().expect("gecici dizin");
        yaz(
            dir.path(),
            SCHEMA_FILE,
            "```toml\nname = \"x\"\ndescription = \"y\"\nbilinmeyen = true\n```\n",
        );
        assert!(matches!(
            PersonaRegistry::load_from(dir.path()),
            Err(PersonaError::SchemaDrift { .. })
        ));
    }

    #[test]
    fn semada_toml_blogu_yoksa_hata() {
        let dir = tempfile::tempdir().expect("gecici dizin");
        yaz(dir.path(), SCHEMA_FILE, "# yalnizca metin\n");
        assert!(matches!(
            PersonaRegistry::load_from(dir.path()),
            Err(PersonaError::SchemaBlock { .. })
        ));
    }

    #[test]
    fn sistem_promptu_cozulur() {
        let dir = tempfile::tempdir().expect("gecici dizin");
        std::fs::create_dir_all(dir.path().join("prompts")).expect("alt dizin");
        std::fs::write(dir.path().join("prompts/planner.md"), "plan yap").expect("prompt");
        yaz(dir.path(), "planner.toml", PLANNER);

        let defter = PersonaRegistry::load_from(dir.path()).expect("defter");
        let persona = defter.get("planner").expect("persona");
        assert_eq!(
            persona.read_system_prompt().expect("okuma"),
            Some("plan yap".to_owned())
        );
    }

    #[test]
    fn prompt_referansi_yoksa_none() {
        let dir = tempfile::tempdir().expect("gecici dizin");
        yaz(
            dir.path(),
            "sade.toml",
            "name = \"sade\"\ndescription = \"x\"\n",
        );
        let defter = PersonaRegistry::load_from(dir.path()).expect("defter");
        let persona = defter.get("sade").expect("persona");
        assert!(persona.system_prompt_path().is_none());
        assert_eq!(persona.read_system_prompt().expect("okuma"), None);
    }

    #[test]
    fn ozet_icerige_duyarli() {
        assert_ne!(checksum(b"a"), checksum(b"b"));
        assert_eq!(checksum(b"a"), checksum(b"a"));
    }

    #[test]
    fn toml_blogu_cikarilir() {
        assert_eq!(
            toml_block("metin\n```toml\nx = 1\n```\nsonrasi"),
            Some("x = 1".to_owned())
        );
        assert_eq!(toml_block("```rust\nfn main() {}\n```"), None);
    }

    // --- Faz 5: hot-reload -------------------------------------------------

    const EXECUTOR: &str = r#"
name = "executor"
description = "Plani uygular"
tools = ["write"]
"#;

    /// Depodaki gercek `config/personas/` dizini (I8: sema iddiasinin kaniti).
    fn depo_persona_dizini() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../config/personas")
    }

    #[test]
    fn depodaki_ornek_personalar_semaya_uyar() {
        let dir = depo_persona_dizini();
        assert!(dir.is_dir(), "config/personas bulunamadi: {}", dir.display());

        let defter = PersonaRegistry::load_from(&dir).expect("depo personalari yuklenmeli");
        // `_schema.md` gercekten okundu mu (yerlesik semaya dusmedi mi)?
        assert!(defter.schema().source().is_some());
        assert!(defter.schema().keys().any(|k| k == "budget"));

        let planner = defter.get("planner").expect("planner personasi");
        assert_eq!(planner.definition.role.as_deref(), Some("planner"));
        // I5: ornek personalar model adi gommez, rolden katalogla cozer.
        assert!(planner.definition.model.is_none());
        // Planlayici yazma yetkisi tasimaz (K3).
        assert!(!planner.definition.tools.iter().any(|t| t == "write"));

        let executor = defter.get("executor").expect("executor personasi");
        assert!(executor.definition.tools.iter().any(|t| t == "write"));
        assert!(executor.definition.model.is_none());

        // 11.2: MASTER-PLAN katalogu DOLDURMAZ; drop iki referans persona icerir.
        assert!(defter.len() >= 2, "beklenen en az iki referans persona");
    }

    fn sahte(ad: &str, ozet: &str) -> LoadedPersona {
        LoadedPersona {
            definition: PersonaDefinition {
                name: ad.to_owned(),
                description: "x".to_owned(),
                system_prompt: None,
                role: None,
                temperature: None,
                tools: Vec::new(),
                disallowed: Vec::new(),
                budget: None,
                routing: None,
                max_depth: None,
                recording: None,
                model: None,
            },
            path: PathBuf::from(format!("{ad}.toml")),
            checksum: ozet.to_owned(),
            loaded_at: now(),
        }
    }

    #[test]
    fn fark_ekleme_guncelleme_silmeyi_ayirir() {
        let onceki: BTreeMap<String, LoadedPersona> = [
            ("kalan".to_owned(), sahte("kalan", "a")),
            ("degisen".to_owned(), sahte("degisen", "a")),
            ("silinen".to_owned(), sahte("silinen", "a")),
        ]
        .into_iter()
        .collect();
        let taze: BTreeMap<String, LoadedPersona> = [
            ("kalan".to_owned(), sahte("kalan", "a")),
            ("degisen".to_owned(), sahte("degisen", "b")),
            ("yeni".to_owned(), sahte("yeni", "a")),
        ]
        .into_iter()
        .collect();

        let degisim = diff(&onceki, &taze);
        assert_eq!(
            degisim,
            vec![
                PersonaChange::Updated("degisen".to_owned()),
                PersonaChange::Added("yeni".to_owned()),
                PersonaChange::Removed("silinen".to_owned()),
            ]
        );
        assert_eq!(degisim[1].name(), "yeni");
        assert!(diff(&taze, &taze).is_empty());
    }

    #[test]
    fn ilgili_dosya_filtresi_gurultuyu_eler() {
        assert!(is_relevant(Path::new("/p/planner.toml")));
        assert!(is_relevant(Path::new("/p").join(SCHEMA_FILE).as_path()));
        assert!(!is_relevant(Path::new("/p/_taslak.toml")));
        assert!(!is_relevant(Path::new("/p/prompts/planner.md")));
        assert!(!is_relevant(Path::new("/p/planner.toml.swp")));
        assert!(!is_relevant(Path::new("/p")));
    }

    #[test]
    fn yeniden_yukleme_farki_bildirir() {
        let dir = tempfile::tempdir().expect("gecici dizin");
        yaz(dir.path(), "planner.toml", PLANNER);
        let paylasilan = SharedPersonas::load_from(dir.path()).expect("defter");
        assert_eq!(paylasilan.len(), 1);
        assert_eq!(paylasilan.names(), vec!["planner".to_owned()]);
        assert!(!paylasilan.is_empty());
        assert_eq!(paylasilan.dir().as_path(), dir.path());

        // Degisiklik yoksa fark bostur ve defter yerinde kalir.
        assert!(paylasilan.reload().expect("bos tarama").is_empty());

        yaz(dir.path(), "executor.toml", EXECUTOR);
        assert_eq!(
            paylasilan.reload().expect("ekleme"),
            vec![PersonaChange::Added("executor".to_owned())]
        );
        assert_eq!(paylasilan.len(), 2);
        assert!(paylasilan.get("executor").is_some());

        std::fs::remove_file(dir.path().join("executor.toml")).expect("silme");
        assert_eq!(
            paylasilan.reload().expect("silme"),
            vec![PersonaChange::Removed("executor".to_owned())]
        );
        assert!(paylasilan.get("executor").is_none());
        assert_eq!(paylasilan.snapshot().len(), 1);
    }

    #[test]
    fn bozuk_dosya_onceki_defteri_bozmaz() {
        let dir = tempfile::tempdir().expect("gecici dizin");
        yaz(dir.path(), "planner.toml", PLANNER);
        let paylasilan = SharedPersonas::load_from(dir.path()).expect("defter");

        // Yarim yazilmis kaydi taklit et.
        yaz(dir.path(), "executor.toml", "name = \"executor\"\ndesc");
        assert!(paylasilan.reload().is_err());

        // Defter dokunulmadan duruyor: calisan sistem bozulmadi.
        assert_eq!(paylasilan.len(), 1);
        assert!(paylasilan.get("planner").is_some());
    }

    /// FAZ 5 KAPISI: yeni persona dosyasi **derlemesiz** yuklenir.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn yeni_persona_dosyasi_derlemesiz_yuklenir() {
        let dir = tempfile::tempdir().expect("gecici dizin");
        yaz(dir.path(), "planner.toml", PLANNER);

        let paylasilan = SharedPersonas::load_from(dir.path()).expect("defter");
        let izleyici = paylasilan
            .watch_with_debounce_ms(20)
            .expect("izleyici baslamali");
        let mut abone = izleyici.subscribe();
        assert_eq!(paylasilan.len(), 1);

        // Surec calisirken diske YENI bir persona dusuyor.
        yaz(dir.path(), "executor.toml", EXECUTOR);

        let olay = tokio::time::timeout(std::time::Duration::from_secs(20), abone.recv())
            .await
            .expect("hot-reload olayi zaman asimina ugradi")
            .expect("yayin kanali acik kalmali");
        assert_eq!(olay, PersonaChange::Added("executor".to_owned()));

        let yeni = paylasilan.get("executor").expect("yeni persona defterde");
        assert_eq!(yeni.definition.tools, vec!["write"]);
        assert_eq!(paylasilan.len(), 2);

        // Izleyici dusunce OS izleyicisi ve gorev birlikte kapanir.
        assert!(format!("{izleyici:?}").contains("PersonaWatcher"));
        assert_eq!(izleyici.personas().len(), 2);
        izleyici.shutdown();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn izleme_olmayan_dizini_olusturur() {
        let kok = tempfile::tempdir().expect("gecici dizin");
        let dir = PersonaRegistry::default_dir(kok.path());
        assert!(!dir.is_dir());

        let paylasilan = SharedPersonas::load_from(&dir).expect("bos defter");
        assert!(paylasilan.is_empty());
        let izleyici = paylasilan.watch().expect("izleyici baslamali");
        assert!(dir.is_dir(), "izleme dizini olusturmali");
        drop(izleyici);
    }
}
