//! Katman mekanikleri: dosya (en dusuk), DB runtime (orta), env (en yuksek).
//!
//! Bu modul yalnizca katmanlari **uretir** ve nokta-ayrilmis anahtar yollari
//! uzerinde birlestirme yapar. Oncelik sirasi `ConfigStore` tarafindan uygulanir.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags, params};
use serde_json::Value as JsonValue;
use xai_grok_config::{deep_merge_toml, env_bool, expand_env_vars_in_string, load_toml_file};

use crate::ConfigError;

/// Env katmani icin zorunlu on ek.
pub const ENV_PREFIX: &str = "OMNITRIX_";

/// Env degisken adinda yol ayraci. Tek `_` segment icinde kalir, `__` nokta olur:
/// `OMNITRIX_RUNTIME__MAX_ACTIVE_AGENTS` -> `runtime.max_active_agents`.
pub const ENV_PATH_SEP: &str = "__";

/// Aktif profili secen env degiskeni.
pub const PROFILE_ENV: &str = "OMNITRIX_PROFILE";

/// Profil secilmediginde kullanilan varsayilan (mevcut `config/profiles/` semasi).
pub const DEFAULT_PROFILE: &str = "mid";

/// Birlesmis agacta aktif profil adinin yazildigi anahtar.
pub const ACTIVE_PROFILE_KEY: &str = "active_profile";

/// `config_kv.source` sutununa yazilan katman etiketi.
pub const RUNTIME_SOURCE: &str = "runtime";

/// `OMNITRIX_PROFILE` degerini okur; tanimsiz/bos ise varsayilana duser.
pub fn active_profile() -> String {
    match std::env::var(PROFILE_ENV) {
        Ok(raw) if !raw.trim().is_empty() => raw.trim().to_owned(),
        _ => DEFAULT_PROFILE.to_owned(),
    }
}

/// Dosya katmani (en dusuk oncelik).
///
/// Sira: once `config/*.toml` koku (ad sirasina gore), sonra
/// `config/profiles/<aktif>.toml` ustune bindirilir — profil, makine sinifina
/// gore bilincli bir secim oldugu icin genel varsayilanlari ezer. Mevcut profil
/// semasi (`[runtime]`/`[router]`/`[record]`/`[notify]`) oldugu gibi korunur.
///
/// Dizin yoksa hata degil, bos agac doner: config'siz calisma gecerli bir durum.
pub fn load_file_layer(config_dir: &Path, profile: &str) -> Result<JsonValue, ConfigError> {
    let root = dunce::canonicalize(config_dir).unwrap_or_else(|_| config_dir.to_path_buf());
    let mut merged = toml::Value::Table(toml::Table::new());

    if root.is_dir() {
        for path in toml_files_in(&root)? {
            let loaded = load_toml_file(&path).map_err(|source| ConfigError::ReadFile {
                path: path.clone(),
                source,
            })?;
            deep_merge_toml(&mut merged, &loaded);
        }

        let profile_path = root.join("profiles").join(format!("{profile}.toml"));
        if profile_path.is_file() {
            let loaded =
                load_toml_file(&profile_path).map_err(|source| ConfigError::ReadFile {
                    path: profile_path.clone(),
                    source,
                })?;
            deep_merge_toml(&mut merged, &loaded);
        } else {
            tracing::debug!(path = %profile_path.display(), "profil dosyasi yok, atlaniyor");
        }
    } else {
        tracing::debug!(path = %root.display(), "config dizini yok, dosya katmani bos");
    }

    let mut tree = serde_json::to_value(&merged)?;
    insert_at(&mut tree, ACTIVE_PROFILE_KEY, JsonValue::String(profile.to_owned()));
    Ok(tree)
}

/// Bir dizinin dogrudan altindaki `*.toml` dosyalari, ada gore siralanmis.
fn toml_files_in(dir: &Path) -> Result<Vec<PathBuf>, ConfigError> {
    let entries = std::fs::read_dir(dir).map_err(|source| ConfigError::ReadDir {
        path: dir.to_path_buf(),
        source,
    })?;
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| ConfigError::ReadDir {
            path: dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        if path.is_file() && path.extension().is_some_and(|ext| ext == "toml") {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

/// Env katmani (en yuksek oncelik): surecteki `OMNITRIX_*` degiskenleri.
pub fn load_env_layer() -> BTreeMap<String, JsonValue> {
    env_layer_from_vars(std::env::vars())
}

/// Test edilebilir cekirdek: verilen (ad, deger) ciftlerinden env katmani kurar.
pub fn env_layer_from_vars<I>(vars: I) -> BTreeMap<String, JsonValue>
where
    I: IntoIterator<Item = (String, String)>,
{
    let mut layer = BTreeMap::new();
    for (name, raw) in vars {
        let Some(rest) = name.strip_prefix(ENV_PREFIX) else {
            continue;
        };
        // Profil secimi bir config anahtari degil, katman girdisi olarak sizmasin.
        if name == PROFILE_ENV {
            continue;
        }
        let segments: Vec<String> = rest
            .split(ENV_PATH_SEP)
            .map(|s| s.trim_matches('_').to_ascii_lowercase())
            .collect();
        if segments.iter().any(|s| s.is_empty()) {
            continue;
        }
        layer.insert(segments.join("."), parse_env_value(&name, &raw));
    }
    layer
}

/// Env degerini JSON'a cevirir: once `$VAR` genisletmesi, sonra JSON ayristirma,
/// olmazsa bool sozlugu (`on`/`yes`/`enabled`...), en sonda duz metin.
pub fn parse_env_value(var_name: &str, raw: &str) -> JsonValue {
    let expanded = expand_env_vars_in_string(raw);
    let trimmed = expanded.trim();
    if !trimmed.is_empty()
        && let Ok(parsed) = serde_json::from_str::<JsonValue>(trimmed)
        && !parsed.is_null()
    {
        return parsed;
    }
    match env_bool(var_name) {
        Some(flag) => JsonValue::Bool(flag),
        None => JsonValue::String(expanded),
    }
}

/// DB runtime katmani (orta oncelik): `config_kv` tablosunun tamami.
///
/// Veritabani ya da tablo yoksa hata degil, bos katman doner — sema henuz
/// gocurulmemis olabilir ve config yuklemesi bunun yuzunden dusmemeli.
pub fn load_runtime_layer(db: &Path) -> Result<BTreeMap<String, JsonValue>, ConfigError> {
    let mut layer = BTreeMap::new();
    if !db.is_file() {
        tracing::debug!(path = %db.display(), "veritabani yok, runtime katmani bos");
        return Ok(layer);
    }
    let conn = open_db(db)?;
    if !has_config_kv(&conn)? {
        tracing::warn!(path = %db.display(), "config_kv tablosu yok, runtime katmani bos");
        return Ok(layer);
    }
    let mut stmt = conn.prepare("SELECT key, value_json FROM config_kv")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (key, value_json) = row?;
        if key.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<JsonValue>(&value_json) {
            Ok(value) => {
                layer.insert(key, value);
            }
            Err(err) => {
                tracing::warn!(key = %key, error = %err, "config_kv degeri cozulemedi, atlandi");
            }
        }
    }
    Ok(layer)
}

/// `config_kv` icine tek anahtar yazar/gunceller.
pub fn upsert_runtime_value(db: &Path, key: &str, value: &JsonValue) -> Result<(), ConfigError> {
    if !db.is_file() {
        return Err(ConfigError::DatabaseMissing(db.to_path_buf()));
    }
    let conn = open_db(db)?;
    if !has_config_kv(&conn)? {
        return Err(ConfigError::MissingTable("config_kv"));
    }
    let value_json = serde_json::to_string(value)?;
    conn.execute(
        "INSERT INTO config_kv (key, value_json, source, updated_at) \
         VALUES (?1, ?2, ?3, datetime('now')) \
         ON CONFLICT(key) DO UPDATE SET \
             value_json = excluded.value_json, \
             source = excluded.source, \
             updated_at = excluded.updated_at",
        params![key, value_json, RUNTIME_SOURCE],
    )?;
    Ok(())
}

fn open_db(db: &Path) -> Result<Connection, ConfigError> {
    // CREATE bayragi yok: semasiz bos bir dosya uretmek yerine hata verilir.
    Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_WRITE).map_err(ConfigError::from)
}

fn has_config_kv(conn: &Connection) -> Result<bool, ConfigError> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'config_kv'",
        [],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

/// Nokta-ayrilmis yol ile agacta gezinir. Bos anahtar kokun kendisidir.
pub fn lookup<'a>(tree: &'a JsonValue, key: &str) -> Option<&'a JsonValue> {
    if key.is_empty() {
        return Some(tree);
    }
    let mut node = tree;
    for segment in key.split('.') {
        node = node.as_object()?.get(segment)?;
    }
    Some(node)
}

/// Nokta-ayrilmis yola deger yazar; ara dugumler yoksa nesne olarak yaratilir,
/// nesne degilse nesneye donusturulur (ezme semantigi).
pub fn insert_at(tree: &mut JsonValue, key: &str, value: JsonValue) {
    if key.is_empty() {
        *tree = value;
        return;
    }
    if !tree.is_object() {
        *tree = JsonValue::Object(serde_json::Map::new());
    }
    let mut node = tree;
    let mut segments = key.split('.').peekable();
    while let Some(segment) = segments.next() {
        let is_last = segments.peek().is_none();
        // `node` bu noktada her zaman nesne (dongu basinda/ustte garanti edildi).
        let Some(map) = node.as_object_mut() else {
            return;
        };
        if is_last {
            map.insert(segment.to_owned(), value);
            return;
        }
        let child = map
            .entry(segment.to_owned())
            .or_insert_with(|| JsonValue::Object(serde_json::Map::new()));
        if !child.is_object() {
            *child = JsonValue::Object(serde_json::Map::new());
        }
        node = child;
    }
}

/// Anahtarin kendisi ya da bir atasi katmanda tanimli mi?
pub fn layer_covers(layer: &BTreeMap<String, JsonValue>, key: &str) -> bool {
    if layer.contains_key(key) {
        return true;
    }
    let mut prefix = String::new();
    for segment in key.split('.') {
        if !prefix.is_empty() {
            prefix.push('.');
        }
        prefix.push_str(segment);
        if layer.contains_key(&prefix) {
            return true;
        }
    }
    false
}

/// JSON agacini TOML'a cevirir. `null` degerler TOML'da temsil edilemedigi icin
/// atilir; her tabloda once yapraklar, sonra alt tablolar yazilir ki uretilen
/// metin gecerli TOML sirasinda olsun.
pub fn json_to_toml(value: &JsonValue) -> Option<toml::Value> {
    match value {
        JsonValue::Null => None,
        JsonValue::Bool(b) => Some(toml::Value::Boolean(*b)),
        JsonValue::String(s) => Some(toml::Value::String(s.clone())),
        JsonValue::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(toml::Value::Integer(i))
            } else {
                n.as_f64().map(toml::Value::Float)
            }
        }
        JsonValue::Array(items) => Some(toml::Value::Array(
            items.iter().filter_map(json_to_toml).collect(),
        )),
        JsonValue::Object(map) => {
            let mut table = toml::Table::new();
            for (key, item) in map {
                if item.is_object() {
                    continue;
                }
                if let Some(converted) = json_to_toml(item) {
                    table.insert(key.clone(), converted);
                }
            }
            for (key, item) in map {
                if !item.is_object() {
                    continue;
                }
                if let Some(converted) = json_to_toml(item) {
                    table.insert(key.clone(), converted);
                }
            }
            Some(toml::Value::Table(table))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn env_layer_maps_double_underscore_to_dots() {
        let layer = env_layer_from_vars(vars(&[
            ("OMNITRIX_RUNTIME__MAX_ACTIVE_AGENTS", "4"),
            ("OMNITRIX_ROUTER__DEFAULT_STRATEGY", "weighted"),
            ("PATH", "/usr/bin"),
        ]));
        assert_eq!(layer.len(), 2);
        assert_eq!(layer["runtime.max_active_agents"], serde_json::json!(4));
        assert_eq!(
            layer["router.default_strategy"],
            serde_json::json!("weighted")
        );
    }

    #[test]
    fn env_layer_skips_profile_selector_and_empty_segments() {
        let layer = env_layer_from_vars(vars(&[
            ("OMNITRIX_PROFILE", "high"),
            ("OMNITRIX_", "x"),
            ("OMNITRIX_A____B", "1"),
        ]));
        assert!(layer.is_empty(), "beklenmeyen girdiler: {layer:?}");
    }

    #[test]
    fn parse_env_value_handles_json_and_plain_text() {
        assert_eq!(
            parse_env_value("OMNITRIX_X", "true"),
            serde_json::json!(true)
        );
        assert_eq!(parse_env_value("OMNITRIX_X", "1024"), serde_json::json!(1024));
        assert_eq!(
            parse_env_value("OMNITRIX_X", "[1, 2]"),
            serde_json::json!([1, 2])
        );
        assert_eq!(
            parse_env_value("OMNITRIX_X", "fallback"),
            serde_json::json!("fallback")
        );
    }

    #[test]
    fn insert_at_creates_and_overwrites_paths() {
        let mut tree = serde_json::json!({"runtime": {"max_depth": 2}});
        insert_at(&mut tree, "runtime.max_depth", serde_json::json!(7));
        insert_at(&mut tree, "router.grounding", serde_json::json!("required"));
        assert_eq!(tree["runtime"]["max_depth"], serde_json::json!(7));
        assert_eq!(tree["router"]["grounding"], serde_json::json!("required"));
    }

    #[test]
    fn insert_at_replaces_non_object_intermediate() {
        let mut tree = serde_json::json!({"record": 1});
        insert_at(&mut tree, "record.video", serde_json::json!(true));
        assert_eq!(tree["record"]["video"], serde_json::json!(true));
    }

    #[test]
    fn lookup_walks_dotted_paths() {
        let tree = serde_json::json!({"a": {"b": {"c": 3}}});
        assert_eq!(lookup(&tree, "a.b.c"), Some(&serde_json::json!(3)));
        assert_eq!(lookup(&tree, "a.b.z"), None);
        assert_eq!(lookup(&tree, ""), Some(&tree));
    }

    #[test]
    fn layer_covers_matches_ancestors() {
        let mut layer = BTreeMap::new();
        layer.insert("runtime".to_owned(), serde_json::json!({}));
        assert!(layer_covers(&layer, "runtime.max_depth"));
        assert!(!layer_covers(&layer, "router.grounding"));
    }

    #[test]
    fn json_to_toml_orders_leaves_before_tables() {
        let tree = serde_json::json!({
            "runtime": {"max_depth": 2},
            "active_profile": "mid",
            "dropped": null,
        });
        let Some(value) = json_to_toml(&tree) else {
            panic!("donusum basarisiz");
        };
        let rendered = match toml::to_string_pretty(&value) {
            Ok(s) => s,
            Err(err) => panic!("TOML uretilemedi: {err}"),
        };
        let profile_at = rendered.find("active_profile").expect("profil satiri");
        let table_at = rendered.find("[runtime]").expect("runtime tablosu");
        assert!(profile_at < table_at, "yaprak tablodan sonra geldi:\n{rendered}");
        assert!(!rendered.contains("dropped"));
    }
}
