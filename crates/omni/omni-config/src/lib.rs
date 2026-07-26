//! Omnitrix katmanli config sistemi (MASTER-PLAN Bolum 15 / AS8).
//!
//! Uc katman, oncelik **yuksekten dusuge**:
//!
//! 1. **Env override** (en yuksek) — `OMNITRIX_*`. Yol ayraci `__`:
//!    `OMNITRIX_RUNTIME__MAX_ACTIVE_AGENTS` -> `runtime.max_active_agents`.
//! 2. **DB runtime** — `config_kv` tablosu. WebUI'dan degisen ayarlar buraya
//!    yazilir ve [`ConfigStore::set_runtime`] ile **calisirken** etkili olur.
//! 3. **Dosya varsayilan** (en dusuk) — `config/*.toml` + aktif
//!    `config/profiles/<profil>.toml`. Git-dostu; persona/prompt kaynagi.
//!
//! Yukleyici olarak `xai-grok-config` (TOML okuma, `$VAR` genisletme, derin
//! birlestirme, atomik yazma) ve `xai-grok-config-types` (`ConfigSource`,
//! `Resolved`) kullanilir — Bolum 2 sozlesmesi.
//!
//! ```no_run
//! use std::path::Path;
//! use omni_config::ConfigStore;
//!
//! # fn main() -> Result<(), omni_config::ConfigError> {
//! let store = ConfigStore::load(Path::new("config"), Some(Path::new("omnitrix.db")))?;
//! let max_depth: Option<u32> = store.get("runtime.max_depth");
//! store.set_runtime("runtime.max_depth", serde_json::json!(4))?;
//! # Ok(())
//! # }
//! ```

pub mod layers;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use parking_lot::RwLock;
use serde::de::DeserializeOwned;
use serde_json::Value as JsonValue;
use xai_grok_config::fs_atomic::write_atomically;
use xai_grok_config_types::{ConfigSource, Resolved};

pub use layers::{ACTIVE_PROFILE_KEY, DEFAULT_PROFILE, ENV_PATH_SEP, ENV_PREFIX, PROFILE_ENV};

/// Config yukleme/yazma hatalari.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("config dizini okunamadi ({}): {source}", path.display())]
    ReadDir {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("config dosyasi okunamadi ({}): {source}", path.display())]
    ReadFile {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("config dosyasi yazilamadi ({}): {source}", path.display())]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("JSON donusumu basarisiz: {0}")]
    Json(#[from] serde_json::Error),
    #[error("TOML uretilemedi: {0}")]
    TomlEncode(#[from] toml::ser::Error),
    #[error("veritabani hatasi: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("calisma zamani katmani icin veritabani yolu tanimli degil")]
    NoDatabase,
    #[error("veritabani bulunamadi: {0}")]
    DatabaseMissing(PathBuf),
    #[error("gerekli tablo yok: {0} (goc calistirilmamis olabilir)")]
    MissingTable(&'static str),
    #[error("config anahtari bos olamaz")]
    EmptyKey,
    #[error("deger TOML'a cevrilemiyor: {0}")]
    Unrepresentable(String),
}

/// Uc katmanli config deposu. `Send + Sync`; paylasimli okuma icin `Arc` ile sarin.
pub struct ConfigStore {
    config_dir: PathBuf,
    db_path: Option<PathBuf>,
    profile: String,
    /// 3. katman: `config/*.toml` + aktif profil, birlesmis.
    file_layer: JsonValue,
    /// 1. katman: `OMNITRIX_*`. Surec omru boyunca sabit.
    env_layer: BTreeMap<String, JsonValue>,
    /// 2. katman: `config_kv`. Calisirken degisir.
    runtime_layer: RwLock<BTreeMap<String, JsonValue>>,
    /// Uc katmanin birlesmis hali; her yazmada yeniden hesaplanir.
    effective: RwLock<JsonValue>,
}

impl std::fmt::Debug for ConfigStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfigStore")
            .field("config_dir", &self.config_dir)
            .field("db_path", &self.db_path)
            .field("profile", &self.profile)
            .field("env_keys", &self.env_layer.len())
            .field("runtime_keys", &self.runtime_layer.read().len())
            .finish()
    }
}

impl ConfigStore {
    /// Uc katmani da yukler.
    ///
    /// `config_dir` yoksa dosya katmani bos kalir; `db` `None` ise ya da isaret
    /// ettigi dosya/tablo yoksa runtime katmani bos kalir. Ikisi de hata degildir
    /// — config sistemi acilisi bloke etmemelidir.
    pub fn load(config_dir: &Path, db: Option<&Path>) -> Result<Self, ConfigError> {
        let profile = layers::active_profile();
        let file_layer = layers::load_file_layer(config_dir, &profile)?;
        let env_layer = layers::load_env_layer();
        let runtime_layer = match db {
            Some(path) => layers::load_runtime_layer(path)?,
            None => BTreeMap::new(),
        };
        let effective = merge_layers(&file_layer, &runtime_layer, &env_layer);
        Ok(Self {
            config_dir: config_dir.to_path_buf(),
            db_path: db.map(Path::to_path_buf),
            profile,
            file_layer,
            env_layer,
            runtime_layer: RwLock::new(runtime_layer),
            effective: RwLock::new(effective),
        })
    }

    /// Nokta-ayrilmis anahtari cozer (`"runtime.max_depth"`). Anahtar bir tabloya
    /// isaret ediyorsa tum alt agac istenen tipe cozulebilir. Bos anahtar tum
    /// birlesmis agaci verir.
    pub fn get<T: DeserializeOwned>(&self, key: &str) -> Option<T> {
        let effective = self.effective.read();
        let value = layers::lookup(&effective, key)?;
        match serde_json::from_value::<T>(value.clone()) {
            Ok(parsed) => Some(parsed),
            Err(err) => {
                tracing::warn!(key = %key, error = %err, "config degeri istenen tipe cozulemedi");
                None
            }
        }
    }

    /// [`ConfigStore::get`] ile ayni, ama degerin hangi katmandan geldigini de
    /// dondurur (teshis / WebUI rozeti icin).
    pub fn get_resolved<T: DeserializeOwned>(&self, key: &str) -> Option<Resolved<T>> {
        let value = self.get::<T>(key)?;
        Some(Resolved::new(value, self.source_of(key)))
    }

    /// Anahtari saglayan en yuksek oncelikli katman.
    ///
    /// Esleme: env -> [`ConfigSource::Env`], DB runtime -> [`ConfigSource::Remote`],
    /// dosya -> [`ConfigSource::Config`], hicbiri -> [`ConfigSource::Default`].
    pub fn source_of(&self, key: &str) -> ConfigSource {
        if layers::layer_covers(&self.env_layer, key) {
            return ConfigSource::Env;
        }
        if layers::layer_covers(&self.runtime_layer.read(), key) {
            return ConfigSource::Remote;
        }
        if layers::lookup(&self.file_layer, key).is_some() {
            return ConfigSource::Config;
        }
        ConfigSource::Default
    }

    /// 2. katmana (DB `config_kv`) yazar ve birlesmis agaci **aninda** tazeler.
    ///
    /// Env katmani daha yuksek oncelikli oldugu icin ayni anahtar `OMNITRIX_*`
    /// ile ezilmisse okuma sonucu degismez — yazma yine de kalici olur.
    pub fn set_runtime(&self, key: &str, value: JsonValue) -> Result<(), ConfigError> {
        let key = key.trim();
        if key.is_empty() || key.split('.').any(str::is_empty) {
            return Err(ConfigError::EmptyKey);
        }
        let Some(db) = self.db_path.as_deref() else {
            return Err(ConfigError::NoDatabase);
        };
        layers::upsert_runtime_value(db, key, &value)?;

        let mut runtime = self.runtime_layer.write();
        runtime.insert(key.to_owned(), value);
        let refreshed = merge_layers(&self.file_layer, &runtime, &self.env_layer);
        drop(runtime);
        *self.effective.write() = refreshed;
        Ok(())
    }

    /// DB katmanini diskten yeniden okur (baska bir surec `config_kv`'yi
    /// degistirmisse). Dosya ve env katmanlarina dokunmaz.
    pub fn reload_runtime(&self) -> Result<(), ConfigError> {
        let Some(db) = self.db_path.as_deref() else {
            return Err(ConfigError::NoDatabase);
        };
        let fresh = layers::load_runtime_layer(db)?;
        let refreshed = merge_layers(&self.file_layer, &fresh, &self.env_layer);
        *self.runtime_layer.write() = fresh;
        *self.effective.write() = refreshed;
        Ok(())
    }

    /// Bolum 15 "opsiyonel dosyaya disa aktar": dosya + DB runtime katmanlarinin
    /// birlesimini TOML olarak yazar, boylece calisirken yapilan degisiklikler
    /// 3. katmana (git-izlenen dosya) kalicilasir.
    ///
    /// Env katmani **bilerek** disarida birakilir: gecici surec ortami git'e
    /// sizmamalidir. Yazma atomiktir (gecici dosya + rename).
    pub fn export_to_file(&self, path: &Path) -> Result<(), ConfigError> {
        let exportable = {
            let runtime = self.runtime_layer.read();
            merge_layers(&self.file_layer, &runtime, &BTreeMap::new())
        };
        let Some(value) = layers::json_to_toml(&exportable) else {
            return Err(ConfigError::Unrepresentable(
                "birlesmis config bos ya da TOML'da temsil edilemez".to_owned(),
            ));
        };
        let rendered = toml::to_string_pretty(&value)?;
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).map_err(|source| ConfigError::Write {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        write_atomically(path, &rendered, None).map_err(|source| ConfigError::Write {
            path: path.to_path_buf(),
            source,
        })
    }

    /// Birlesmis agacin kopyasi (WebUI'ye tek parca gondermek icin).
    pub fn snapshot(&self) -> JsonValue {
        self.effective.read().clone()
    }

    /// Aktif profil adi (`OMNITRIX_PROFILE`, varsayilan [`DEFAULT_PROFILE`]).
    pub fn profile(&self) -> &str {
        &self.profile
    }

    /// Yuklemede kullanilan config dizini.
    pub fn config_dir(&self) -> &Path {
        &self.config_dir
    }

    /// Runtime katmaninin baglandigi veritabani, varsa.
    pub fn db_path(&self) -> Option<&Path> {
        self.db_path.as_deref()
    }
}

/// Katmanlari oncelik sirasina gore bindirir: dosya taban, ustune DB runtime,
/// en uste env.
fn merge_layers(
    file_layer: &JsonValue,
    runtime_layer: &BTreeMap<String, JsonValue>,
    env_layer: &BTreeMap<String, JsonValue>,
) -> JsonValue {
    let mut tree = file_layer.clone();
    for (key, value) in runtime_layer {
        layers::insert_at(&mut tree, key, value.clone());
    }
    for (key, value) in env_layer {
        layers::insert_at(&mut tree, key, value.clone());
    }
    tree
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    const PROFILE_TOML: &str = r#"
[runtime]
max_active_agents = 2
mem_high_watermark_mb = 1024
swap_out_idle_ms = 15000
max_depth = 2

[router]
default_strategy = "fallback"
grounding = "required"

[record]
video = false
dom = false
events = true

[notify]
escalation = false
"#;

    const ROOT_TOML: &str = r#"
[runtime]
max_depth = 9
warm_pool = 1

[router]
default_strategy = "weighted"
"#;

    fn config_dir(base: &Path) -> PathBuf {
        let dir = base.join("config");
        let profiles = dir.join("profiles");
        std::fs::create_dir_all(&profiles).expect("config dizini");
        std::fs::write(dir.join("omnitrix.toml"), ROOT_TOML).expect("kok config");
        // Testler surec ortamindaki OMNITRIX_PROFILE'a bagimli olmasin.
        let active = layers::active_profile();
        std::fs::write(profiles.join(format!("{active}.toml")), PROFILE_TOML).expect("profil");
        dir
    }

    fn seeded_db(base: &Path) -> PathBuf {
        let path = base.join("omnitrix.db");
        let conn = Connection::open(&path).expect("db");
        conn.execute_batch(
            "CREATE TABLE config_kv (
                 key TEXT PRIMARY KEY,
                 value_json TEXT NOT NULL,
                 source TEXT NOT NULL,
                 updated_at TEXT NOT NULL DEFAULT (datetime('now'))
             );",
        )
        .expect("sema");
        path
    }

    #[test]
    fn profile_overrides_root_defaults() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = config_dir(tmp.path());
        let store = ConfigStore::load(&dir, None).expect("yukleme");

        // Profil, kok varsayilanlarin ustune biner.
        assert_eq!(store.get::<u32>("runtime.max_depth"), Some(2));
        assert_eq!(
            store.get::<String>("router.default_strategy").as_deref(),
            Some("fallback")
        );
        // Profilde olmayan kok anahtar korunur.
        assert_eq!(store.get::<u32>("runtime.warm_pool"), Some(1));
        assert_eq!(store.source_of("runtime.max_depth"), ConfigSource::Config);
        assert_eq!(store.source_of("yok.olan"), ConfigSource::Default);
    }

    #[test]
    fn missing_config_dir_is_not_an_error() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let store = ConfigStore::load(&tmp.path().join("hic-yok"), None).expect("yukleme");
        assert_eq!(store.get::<u32>("runtime.max_depth"), None);
        assert_eq!(
            store.get::<String>(ACTIVE_PROFILE_KEY).as_deref(),
            Some(store.profile())
        );
    }

    #[test]
    fn subtree_deserializes_as_a_whole() {
        #[derive(serde::Deserialize)]
        struct Record {
            video: bool,
            dom: bool,
            events: bool,
        }
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = config_dir(tmp.path());
        let store = ConfigStore::load(&dir, None).expect("yukleme");
        let record: Record = store.get("record").expect("record tablosu");
        assert!(!record.video);
        assert!(!record.dom);
        assert!(record.events);
    }

    #[test]
    fn runtime_layer_overrides_file_and_persists() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = config_dir(tmp.path());
        let db = seeded_db(tmp.path());

        let store = ConfigStore::load(&dir, Some(&db)).expect("yukleme");
        assert_eq!(store.get::<u32>("runtime.max_depth"), Some(2));

        store
            .set_runtime("runtime.max_depth", serde_json::json!(6))
            .expect("yazma");
        // Calisirken aninda etkili.
        assert_eq!(store.get::<u32>("runtime.max_depth"), Some(6));
        assert_eq!(store.source_of("runtime.max_depth"), ConfigSource::Remote);

        // Kalici: yeni bir depo ayni degeri gorur.
        let reopened = ConfigStore::load(&dir, Some(&db)).expect("yeniden yukleme");
        assert_eq!(reopened.get::<u32>("runtime.max_depth"), Some(6));

        // source sutunu katman etiketini tasir.
        let conn = Connection::open(&db).expect("db");
        let source: String = conn
            .query_row(
                "SELECT source FROM config_kv WHERE key = ?1",
                ["runtime.max_depth"],
                |row| row.get(0),
            )
            .expect("satir");
        assert_eq!(source, layers::RUNTIME_SOURCE);
    }

    #[test]
    fn set_runtime_without_db_is_rejected() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = config_dir(tmp.path());
        let store = ConfigStore::load(&dir, None).expect("yukleme");
        assert!(matches!(
            store.set_runtime("runtime.max_depth", serde_json::json!(3)),
            Err(ConfigError::NoDatabase)
        ));
        assert!(matches!(
            store.set_runtime("  ", serde_json::json!(3)),
            Err(ConfigError::EmptyKey)
        ));
    }

    #[test]
    fn missing_config_kv_table_yields_empty_runtime_layer() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = config_dir(tmp.path());
        let db = tmp.path().join("bos.db");
        Connection::open(&db).expect("db").execute_batch(
            "CREATE TABLE baska (id INTEGER PRIMARY KEY);",
        ).expect("sema");

        let store = ConfigStore::load(&dir, Some(&db)).expect("yukleme");
        assert_eq!(store.get::<u32>("runtime.max_depth"), Some(2));
        assert!(matches!(
            store.set_runtime("runtime.max_depth", serde_json::json!(3)),
            Err(ConfigError::MissingTable("config_kv"))
        ));
    }

    #[test]
    fn reload_runtime_picks_up_external_writes() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = config_dir(tmp.path());
        let db = seeded_db(tmp.path());
        let store = ConfigStore::load(&dir, Some(&db)).expect("yukleme");

        layers::upsert_runtime_value(&db, "router.grounding", &serde_json::json!("optional"))
            .expect("dis yazma");
        assert_eq!(
            store.get::<String>("router.grounding").as_deref(),
            Some("required")
        );
        store.reload_runtime().expect("tazeleme");
        assert_eq!(
            store.get::<String>("router.grounding").as_deref(),
            Some("optional")
        );
    }

    #[test]
    fn env_layer_beats_runtime_layer() {
        // Surec ortamina dokunmadan katman onceligi dogrulanir.
        let file_layer = serde_json::json!({"runtime": {"max_depth": 2}});
        let mut runtime = BTreeMap::new();
        runtime.insert("runtime.max_depth".to_owned(), serde_json::json!(6));
        let mut env = BTreeMap::new();
        env.insert("runtime.max_depth".to_owned(), serde_json::json!(11));

        let merged = merge_layers(&file_layer, &runtime, &env);
        assert_eq!(merged["runtime"]["max_depth"], serde_json::json!(11));

        let without_env = merge_layers(&file_layer, &runtime, &BTreeMap::new());
        assert_eq!(without_env["runtime"]["max_depth"], serde_json::json!(6));
    }

    #[test]
    fn export_writes_file_plus_runtime_and_reloads() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = config_dir(tmp.path());
        let db = seeded_db(tmp.path());
        let store = ConfigStore::load(&dir, Some(&db)).expect("yukleme");
        store
            .set_runtime("runtime.max_depth", serde_json::json!(6))
            .expect("yazma");

        let out = tmp.path().join("disa/aktarim.toml");
        store.export_to_file(&out).expect("disa aktarim");

        let text = std::fs::read_to_string(&out).expect("okuma");
        let parsed: toml::Value = toml::from_str(&text).expect("gecerli TOML");
        assert_eq!(
            parsed
                .get("runtime")
                .and_then(|r| r.get("max_depth"))
                .and_then(toml::Value::as_integer),
            Some(6)
        );
        assert_eq!(
            parsed.get(ACTIVE_PROFILE_KEY).and_then(toml::Value::as_str),
            Some(store.profile())
        );
    }

    #[test]
    fn resolved_carries_layer_provenance() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = config_dir(tmp.path());
        let store = ConfigStore::load(&dir, None).expect("yukleme");
        let resolved: Resolved<u32> = store.get_resolved("runtime.max_depth").expect("deger");
        assert_eq!(resolved.value, 2);
        assert_eq!(resolved.source, ConfigSource::Config);
        assert_eq!(resolved.to_string(), "2 (config)");
    }
}
