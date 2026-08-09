//! `routing_catalog` modülü için ağ kullanmayan testler.
//!
//! Kapsam: geçerli dosya ayrıştırma (tüm alanlar), dosya yoksa gömülü fallback,
//! katalog bütünlüğü (>=40 unique id, 7 aile, alan dolu olma), parse/validation
//! hataları ve `find_mode` davranışı. Geçici dosyalar ve satır içi TOML kullanır.

use super::routing_catalog::*;
use std::path::Path;
use tempfile::TempDir;

/// İki modlu, tüm alanları (params dahil) dolu geçerli örnek katalog.
const VALID_SAMPLE: &str = r#"
[[modes]]
id = "rr"
family = "Balance"
class = "primitive"
selector = "round_robin"
title = "Round Robin"
blurb = "Sağlıklı endpoint'lere sırayla gönderir, listeyi döngüsel dolaşır."
long_help = "Sıradaki sağlıklı endpoint'e sırayla yönlendirir; liste sonuna gelince başa döner. Dağıtık sayıcı gerektirmez ve tekrarlanabilir sıralama sağlar."

[[modes.params]]
name = "weights"
type = "map"
optional = true
default = "{}"
help = "Endpoint → ağırlık haritası (yalnızca ağırlıklı davranış istenirse)."

[[modes]]
id = "fallback-strict"
family = "Failover"
class = "policy"
selector = "ordered_fallback"
title = "Katı Fallback"
blurb = "Öncelik sıralı zinciri sırayla dener, ilk başarıda durur."
long_help = "Öncelik sıralı endpoint listesini sırayla yürür ve ilk başarılı cevapta durur. Liste bir kez taranır; son aday da başarısızsa istek hata döner."

[[modes.params]]
name = "max_fallbacks"
type = "number"
optional = true
default = "5"
help = "En fazla kaç fallback adımı deneneceği."
"#;

fn write_sample(dir: &TempDir, name: &str, content: &str) -> std::path::PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, content).expect("geçici katalog dosyası yazılmalı");
    path
}

#[test]
fn parses_valid_file_with_all_fields() {
    let dir = TempDir::new().expect("tempdir oluşmalı");
    let path = write_sample(&dir, "valid.toml", VALID_SAMPLE);

    let modes = load_routing_modes(&path).expect("geçerli katalog yüklenmeli");

    assert_eq!(modes.len(), 2);

    let rr = &modes[0];
    assert_eq!(rr.id, "rr");
    assert_eq!(rr.family, ModeFamily::Balance);
    assert_eq!(rr.selector_kind, SelectorKind::Primitive);
    assert_eq!(rr.selector, "round_robin");
    assert_eq!(rr.title, "Round Robin");
    assert!(!rr.blurb.is_empty());
    assert!(!rr.long_help.is_empty());
    assert_eq!(rr.params_schema.defs.len(), 1);
    let weights = &rr.params_schema.defs[0];
    assert_eq!(weights.name, "weights");
    assert_eq!(weights.kind, ParamKind::Map);
    assert!(weights.optional);
    assert_eq!(weights.default.as_deref(), Some("{}"));

    let strict = &modes[1];
    assert_eq!(strict.id, "fallback-strict");
    assert_eq!(strict.family, ModeFamily::Failover);
    assert_eq!(strict.selector_kind, SelectorKind::Policy);
    assert_eq!(strict.selector, "ordered_fallback");
    assert_eq!(strict.params_schema.defs[0].kind, ParamKind::Number);
}

#[test]
fn missing_file_uses_builtin_fallback() {
    let dir = TempDir::new().expect("tempdir oluşmalı");
    let path = dir.path().join("yok-boyle-bir-dosya.toml");

    let modes = load_routing_modes(&path).expect("eksik dosya fallback ile yüklenmeli");

    assert!(modes.len() >= 40, "fallback katalog en az 40 mod içermeli, {} bulundu", modes.len());
    assert!(modes.iter().any(|m| m.id == "rr"), "fallback 'rr' modunu içermeli");
    assert!(modes.iter().any(|m| m.id == "fallback-strict"), "fallback 'fallback-strict' modunu içermeli");
}

#[test]
fn builtin_catalog_has_40_plus_unique_ids() {
    let modes = builtin_modes();

    assert!(modes.len() >= 40, "gömülü katalog en az 40 mod içermeli, {} bulundu", modes.len());

    let mut ids = std::collections::HashSet::new();
    for m in modes {
        assert!(ids.insert(m.id.as_str()), "yinelenen id: {}", m.id);
        assert!(!m.title.trim().is_empty(), "{}: title boş", m.id);
        assert!(!m.blurb.trim().is_empty(), "{}: blurb boş", m.id);
        assert!(!m.long_help.trim().is_empty(), "{}: long_help boş", m.id);
        assert!(!m.selector.trim().is_empty(), "{}: selector boş", m.id);
    }

    for family in ModeFamily::ALL {
        assert!(
            modes.iter().any(|m| m.family == family),
            "gömülü katalog {:?} ailesinden en az bir mod içermeli",
            family
        );
    }
}

#[test]
fn builtin_family_counts_match_research() {
    let modes = builtin_modes();
    let mut counts = std::collections::HashMap::new();
    for m in modes {
        *counts.entry(m.family).or_insert(0usize) += 1;
    }

    assert_eq!(counts.get(&ModeFamily::Balance), Some(&13));
    assert_eq!(counts.get(&ModeFamily::Failover), Some(&12));
    assert_eq!(counts.get(&ModeFamily::RoleSplit), Some(&7));
    assert_eq!(counts.get(&ModeFamily::Hybrid), Some(&8));
    assert_eq!(counts.get(&ModeFamily::Specialty), Some(&10));
    assert_eq!(counts.get(&ModeFamily::Cost), Some(&7));
    assert_eq!(counts.get(&ModeFamily::Privacy), Some(&8));
}

#[test]
fn duplicate_ids_fail_validation() {
    let dir = TempDir::new().expect("tempdir oluşmalı");
    let content = r#"
[[modes]]
id = "rr"
family = "Balance"
class = "primitive"
selector = "round_robin"
title = "Round Robin"
blurb = "Blurb."
long_help = "Uzun yardım."

[[modes]]
id = "rr"
family = "Failover"
class = "policy"
selector = "ordered_fallback"
title = "Round Robin Kopyası"
blurb = "Blurb."
long_help = "Uzun yardım."
"#;
    let path = write_sample(&dir, "dup.toml", content);

    let err = load_routing_modes(&path).expect_err("yinelenen id hata dönmeli");
    let msg = err.to_string();
    assert!(msg.contains("rr"), "hata mesajı yinelenen id'yi içermeli: {msg}");
    assert!(matches!(err, RoutingCatalogError::Validation { .. }));
}

#[test]
fn empty_id_fails_validation() {
    let dir = TempDir::new().expect("tempdir oluşmalı");
    let content = r#"
[[modes]]
id = ""
family = "Balance"
class = "primitive"
selector = "round_robin"
title = "Boş İd"
blurb = "Blurb."
long_help = "Uzun yardım."
"#;
    let path = write_sample(&dir, "empty-id.toml", content);

    let err = load_routing_modes(&path).expect_err("boş id hata dönmeli");
    assert!(matches!(err, RoutingCatalogError::Validation { .. }));
}

#[test]
fn malformed_toml_returns_parse_error() {
    let dir = TempDir::new().expect("tempdir oluşmalı");
    let path = write_sample(&dir, "broken.toml", "[[modes\nid = 'rr'");

    let err = load_routing_modes(&path).expect_err("bozuk TOML hata dönmeli");
    assert!(matches!(err, RoutingCatalogError::Parse { .. }));
}

#[test]
fn unknown_family_fails_parse() {
    let dir = TempDir::new().expect("tempdir oluşmalı");
    let content = r#"
[[modes]]
id = "rr"
family = "BilimKurgu"
class = "primitive"
selector = "round_robin"
title = "Round Robin"
blurb = "Blurb."
long_help = "Uzun yardım."
"#;
    let path = write_sample(&dir, "bad-family.toml", content);

    let err = load_routing_modes(&path).expect_err("bilinmeyen aile hata dönmeli");
    assert!(matches!(err, RoutingCatalogError::Parse { .. }));
}

#[test]
fn unknown_class_fails_parse() {
    let dir = TempDir::new().expect("tempdir oluşmalı");
    let content = r#"
[[modes]]
id = "rr"
family = "Balance"
class = "telepatik"
selector = "round_robin"
title = "Round Robin"
blurb = "Blurb."
long_help = "Uzun yardım."
"#;
    let path = write_sample(&dir, "bad-class.toml", content);

    let err = load_routing_modes(&path).expect_err("bilinmeyen sınıf hata dönmeli");
    assert!(matches!(err, RoutingCatalogError::Parse { .. }));
}

#[test]
fn find_mode_returns_known_record() {
    let mode = find_mode("rr").expect("gömülü katalog 'rr' modunu içermeli");
    assert_eq!(mode.family, ModeFamily::Balance);
    assert_eq!(mode.selector_kind, SelectorKind::Primitive);
}

#[test]
fn find_mode_unknown_id_returns_none() {
    assert!(find_mode("yok-boyle-bir-mod").is_none());
    assert!(find_mode("").is_none());
}

#[test]
fn find_in_searches_loaded_catalog() {
    let dir = TempDir::new().expect("tempdir oluşmalı");
    let path = write_sample(&dir, "valid.toml", VALID_SAMPLE);
    let modes = load_routing_modes(&path).expect("geçerli katalog yüklenmeli");

    let found = find_in(&modes, "fallback-strict").expect("yüklenen katalogda bulunmalı");
    assert_eq!(found.id, "fallback-strict");
    assert!(find_in(&modes, "rr-2").is_none());
}

#[test]
fn load_returns_io_error_for_unreadable_path() {
    // Çalışan kullanıcı için okunamaz olmayı garanti edemeyiz; en azından
    // dizin olan bir yolu dosya gibi açmayı deneyelim (Unix'te okuma hatası).
    let dir = TempDir::new().expect("tempdir oluşmalı");
    let path = dir.path().to_path_buf();
    match load_routing_modes(Path::new(&path)) {
        Err(RoutingCatalogError::Io { .. }) => {}
        other => panic!("dizin yolu Io hatası dönmeli, alınan: {other:?}"),
    }
}
