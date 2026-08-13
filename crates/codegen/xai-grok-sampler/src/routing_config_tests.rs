//! P1.4 config köprüsü testleri: canonical passthrough, legacy alias'lar,
//! bilinmeyen/boş hatalar ve I5-güvenli rol/config ayrıştırma.
//!
//! Tüm testler ağsız ve dosya sisteminden bağımsızdır: gerçek
//! `config/routing.toml` / `config/models.toml` kopyaları derleme anında
//! `include_str!` ile gömülür (P1.2 katalog testleriyle aynı desen).

use crate::router_engine::SUPPORTED_MODES;
use crate::routing_config::{
    ConfigError, GroundingMode, LegacyStrategy, StrategyError, StrategyResolution, VALID_ROLE_IDS,
    parse_models_config, parse_routing_config, resolve_strategy,
};

/// Testte kullanılan "katalog" dilimi: P1.2 kataloğundan alınan gerçek
/// canonical ID'ler (motorun `SUPPORTED_MODES`'unun ötesinde katalog
/// genişliğini de kapsar).
const KNOWN: &[&str] = &[
    "rr",
    "wrr",
    "random",
    "least-busy",
    "ewma-latency",
    "sticky-session",
    "hash-prompt",
    "fallback-strict",
    "fallback-soft",
    "backup-only-on-429",
    "default-fallback",
    "hedge-p95",
    "jep-classic",
    "jep-cheap-plan",
    "planner-only-chain",
    "balance-then-fallback",
    "cheapest-alive",
    "hedge",
    "canary-10",
];

/// Gerçek config dosyalarının derleme anındaki kopyaları (repo kökü).
const ROUTING_TOML: &str = include_str!("../../../../config/routing.toml");
const MODELS_TOML: &str = include_str!("../../../../config/models.toml");

// ---------------------------------------------------------------------------
// Canonical passthrough
// ---------------------------------------------------------------------------

/// Motorun desteklediği her canonical ID olduğu gibi geçmeli.
#[test]
fn supported_modes_pass_through_unchanged() {
    for id in SUPPORTED_MODES {
        assert_eq!(
            resolve_strategy(id, SUPPORTED_MODES).unwrap(),
            StrategyResolution::Canonical(id),
            "canonical ID passthrough: {id}"
        );
    }
}

/// Katalog genişliğinde canonical ID'ler değişmeden geçer ve `canonical_id`
/// birleştiricisi aynı değeri döner.
#[test]
fn catalog_ids_pass_through_unchanged() {
    for id in KNOWN {
        let resolved = resolve_strategy(id, KNOWN).unwrap();
        assert_eq!(resolved, StrategyResolution::Canonical(id));
        assert_eq!(resolved.canonical_id(), *id);
    }
}

/// `fallback-strict` gibi canonical ID'ler asla legacy alias'a dönüşmez
/// (çakışma güvencesi: alias adları katalogda canonical ID olmamalı).
#[test]
fn canonical_ids_are_never_aliased() {
    for canonical in ["fallback-strict", "rr", "wrr", "jep-classic"] {
        assert_eq!(
            resolve_strategy(canonical, KNOWN).unwrap(),
            StrategyResolution::Canonical(canonical)
        );
    }
}

// ---------------------------------------------------------------------------
// Legacy alias'lar
// ---------------------------------------------------------------------------

/// Zorunlu alias: `fallback` -> `fallback-strict`.
#[test]
fn fallback_alias_resolves_to_fallback_strict() {
    let resolved = resolve_strategy("fallback", KNOWN).unwrap();
    assert_eq!(
        resolved,
        StrategyResolution::Legacy(LegacyStrategy::Fallback)
    );
    assert_eq!(resolved.canonical_id(), "fallback-strict");
}

/// Korunan tüm legacy değerlerin açık eşleme tablosu.
#[test]
fn all_legacy_aliases_resolve_deterministically() {
    let table: &[(&str, LegacyStrategy, &str)] = &[
        ("fallback", LegacyStrategy::Fallback, "fallback-strict"),
        ("round_robin", LegacyStrategy::RoundRobin, "rr"),
        ("weighted", LegacyStrategy::Weighted, "wrr"),
        ("jep", LegacyStrategy::Jep, "jep-classic"),
    ];
    for (alias, variant, canonical) in table {
        assert_eq!(
            LegacyStrategy::parse(alias),
            Some(*variant),
            "parse {alias}"
        );
        assert_eq!(variant.as_str(), *alias, "as_str {variant:?}");
        assert_eq!(variant.canonical_id(), *canonical, "canonical {variant:?}");
        let resolved = resolve_strategy(alias, KNOWN).unwrap();
        assert_eq!(resolved, StrategyResolution::Legacy(*variant));
        assert_eq!(resolved.canonical_id(), *canonical);
    }
}

/// `ALL` sabiti tip dökümünün tamamını kapsar (yeni varyant eklenince bu
/// test unutulmayı yakalar).
#[test]
fn legacy_all_covers_every_variant() {
    for variant in LegacyStrategy::ALL {
        assert_eq!(LegacyStrategy::parse(variant.as_str()), Some(variant));
    }
}

/// Alias çözümü katalog bilgisi olmadan da çalışır (saf, deterministik).
#[test]
fn aliases_resolve_without_catalog() {
    assert_eq!(
        resolve_strategy("fallback", &[]).unwrap().canonical_id(),
        "fallback-strict"
    );
    assert_eq!(
        resolve_strategy("jep", &[]).unwrap().canonical_id(),
        "jep-classic"
    );
}

// ---------------------------------------------------------------------------
// Bilinmeyen / boş hatalar (sessiz fallback YOK)
// ---------------------------------------------------------------------------

#[test]
fn unknown_id_returns_typed_error() {
    match resolve_strategy("quantum-bounce", KNOWN) {
        Err(StrategyError::Unknown(id)) => assert_eq!(id, "quantum-bounce"),
        other => panic!("beklenen StrategyError::Unknown, alınan: {other:?}"),
    }
}

/// Hata mesajı hem girdiyi hem de geçerli alias'ları içerir (yararlı hata).
#[test]
fn unknown_error_message_is_useful() {
    let err = resolve_strategy("quantum-bounce", KNOWN).unwrap_err();
    let message = err.to_string();
    assert!(
        message.contains("quantum-bounce"),
        "girdi mesajda: {message}"
    );
    assert!(
        message.contains("fallback"),
        "alias önerisi mesajda: {message}"
    );
    assert!(message.contains("jep"), "alias önerisi mesajda: {message}");
}

#[test]
fn empty_id_returns_typed_error() {
    assert_eq!(resolve_strategy("", KNOWN), Err(StrategyError::Empty));
    assert_eq!(resolve_strategy("   ", KNOWN), Err(StrategyError::Empty));
}

/// Bilinmeyen değer asla bir varsayılana sessizce çevrilmez.
#[test]
fn unknown_id_is_never_silently_fallback() {
    assert!(matches!(
        resolve_strategy("no-such-mode", KNOWN),
        Err(StrategyError::Unknown(_))
    ));
}

/// Boş `known_ids` ile canonical girişi bilinmeyen sayılır (hata).
#[test]
fn canonical_without_catalog_is_unknown() {
    assert!(matches!(
        resolve_strategy("rr", &[]),
        Err(StrategyError::Unknown(_))
    ));
}

// ---------------------------------------------------------------------------
// I5-güvenli config ayrıştırma (gerçek dosyalar)
// ---------------------------------------------------------------------------

/// Gerçek `config/routing.toml`: `fallback` legacy değeri köprüden geçer ve
/// `fallback-strict`'e çözülür.
#[test]
fn real_routing_toml_parses_and_resolves() {
    let config = parse_routing_config(ROUTING_TOML).unwrap();
    assert_eq!(config.strategy, "fallback");
    assert_eq!(config.grounding, Some(GroundingMode::Required));
    assert_eq!(
        resolve_strategy(&config.strategy, KNOWN)
            .unwrap()
            .canonical_id(),
        "fallback-strict"
    );
}

/// Gerçek `[jep]` değerleri rol kimliğidir (model adı değil — I5).
#[test]
fn real_routing_toml_jep_values_are_role_ids() {
    let config = parse_routing_config(ROUTING_TOML).unwrap();
    for value in [
        config.jep.planner.as_deref(),
        config.jep.executor.as_deref(),
        config.jep.judge.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        assert!(
            VALID_ROLE_IDS.contains(&value),
            "rol değeri rol kimliği olmalı: {value}"
        );
    }
}

/// Gerçek `config/models.toml`: rol anahtarları geçerli, değerler ayarlanmamış
/// (boş -> runtime katalogdan çözülür) — I5 güvenli iskelet.
#[test]
fn real_models_toml_is_i5_safe_skeleton() {
    let config = parse_models_config(MODELS_TOML).unwrap();
    for (key, value) in &config.roles {
        assert!(
            VALID_ROLE_IDS.contains(&key.as_str()),
            "rol anahtarı geçerli olmalı: {key}"
        );
        assert_eq!(value, "", "iskelet: '{key}' değeri ayarlanmamış olmalı");
    }
}

/// `models.toml` her rol için bir anahtar içerir.
#[test]
fn real_models_toml_has_all_role_keys() {
    let config = parse_models_config(MODELS_TOML).unwrap();
    for role in VALID_ROLE_IDS {
        assert!(
            config.roles.contains_key(*role),
            "eksik rol anahtarı: {role}"
        );
    }
}

// ---------------------------------------------------------------------------
// I5 sınırı: `[jep]` değeri model adı olamaz; bilinmeyen anahtar hata verir
// ---------------------------------------------------------------------------

/// `[jep]` içinde model adı literal'i reddedilir (I5 zorlaması).
#[test]
fn jep_value_rejects_model_literal() {
    let toml = "[jep]\nplanner = \"grok-4\"\n";
    match parse_routing_config(toml) {
        Err(ConfigError::InvalidJepRole { key, value, .. }) => {
            assert_eq!(key, "planner");
            assert_eq!(value, "grok-4");
        }
        other => panic!("beklenen InvalidJepRole, alınan: {other:?}"),
    }
}

/// Geçerli rol kimlikleri `[jep]`'te kabul edilir.
#[test]
fn jep_valid_role_values_pass() {
    let config = parse_routing_config(
        "[jep]\njudge = \"judge\"\nexecutor = \"executor\"\nplanner = \"planner\"\n",
    )
    .unwrap();
    assert_eq!(config.jep.judge.as_deref(), Some("judge"));
    assert_eq!(config.jep.executor.as_deref(), Some("executor"));
    assert_eq!(config.jep.planner.as_deref(), Some("planner"));
}

/// `[roles]` içinde bilinmeyen anahtar (örn. model adı) reddedilir.
#[test]
fn models_config_rejects_unknown_role_key() {
    match parse_models_config("[roles]\ngrok-4 = \"x\"\n") {
        Err(ConfigError::InvalidRoleKey { key, .. }) => assert_eq!(key, "grok-4"),
        other => panic!("beklenen InvalidRoleKey, alınan: {other:?}"),
    }
}

/// P1.4 yalnızca düz rol -> string biçimini ayrıştırır; geniş tablo biçimi
/// sonraki config okuyucusuna bırakılır ve sessizce yutulmaz.
#[test]
fn models_config_wide_role_form_is_typed_error_until_supported() {
    assert!(matches!(
        parse_models_config("[roles.judge]\nmodel = \"grok-4\"\n"),
        Err(ConfigError::Toml { .. })
    ));
}

/// Bozuk TOML türlü hata döner (sessiz geçiş yok).
#[test]
fn malformed_toml_is_typed_error() {
    assert!(matches!(
        parse_routing_config("strategy = "),
        Err(ConfigError::Toml { .. })
    ));
    assert!(matches!(
        parse_models_config("nope"),
        Err(ConfigError::Toml { .. })
    ));
}

// ---------------------------------------------------------------------------
// Grounding / strateji alanları
// ---------------------------------------------------------------------------

#[test]
fn grounding_modes_parse() {
    assert_eq!(
        parse_routing_config("strategy = \"rr\"\ngrounding = \"required\"")
            .unwrap()
            .grounding,
        Some(GroundingMode::Required)
    );
    assert_eq!(
        parse_routing_config("strategy = \"rr\"\ngrounding = \"preferred\"")
            .unwrap()
            .grounding,
        Some(GroundingMode::Preferred)
    );
    assert_eq!(
        parse_routing_config("strategy = \"rr\"\ngrounding = \"off\"")
            .unwrap()
            .grounding,
        Some(GroundingMode::Off)
    );
    assert_eq!(
        parse_routing_config("strategy = \"rr\"").unwrap().grounding,
        None
    );
}

/// `strategy` eksikse boş hata çözümüne düşer (yok sayılmaz).
#[test]
fn config_without_strategy_resolves_to_empty_error() {
    let config = parse_routing_config("[jep]\njudge = \"judge\"\n").unwrap();
    assert_eq!(
        resolve_strategy(&config.strategy, KNOWN),
        Err(StrategyError::Empty)
    );
}

/// Canonical strateji değeri dosyada kullanılabilir (passthrough).
#[test]
fn canonical_strategy_in_file_resolves() {
    let config = parse_routing_config("strategy = \"fallback-strict\"\n").unwrap();
    assert_eq!(
        resolve_strategy(&config.strategy, KNOWN)
            .unwrap()
            .canonical_id(),
        "fallback-strict"
    );
}

/// P1.4'ün yorumlamadığı bölümler (circuit_breaker/budget) yok sayılır;
/// dosya bütünlüğü bozulmaz.
#[test]
fn unknown_sections_are_ignored() {
    let toml = "\
strategy = \"fallback\"
grounding = \"required\"

[jep]
judge = \"judge\"

[circuit_breaker]
enabled = true
window_secs = 60

[budget]
max_cost = 1.5
";
    let config = parse_routing_config(toml).unwrap();
    assert_eq!(config.strategy, "fallback");
    assert_eq!(config.jep.judge.as_deref(), Some("judge"));
}

// ---------------------------------------------------------------------------
// Model tipi / sızıntı koruması (I5)
// ---------------------------------------------------------------------------

/// Bridge üretim tipleri model adı/fiyat literal'i içermez: modüller arası
/// yüzeyde yalnızca rol kimlikleri ve strateji ID'leri taşınır. Bu test,
/// `ModelsConfig` değerlerinin yorumlanmadığını (string olarak taşındığını)
/// ve `[jep]` değerlerinin rol kümesiyle sınırlı olduğunu doğrular.
#[test]
fn i5_no_model_or_price_literals_in_roles() {
    let models = parse_models_config(MODELS_TOML).unwrap();
    for value in models.roles.values() {
        assert!(value.is_empty(), "skeleton değeri boş olmalı: {value:?}");
    }
    let routing = parse_routing_config(ROUTING_TOML).unwrap();
    for value in [
        routing.jep.planner.as_deref(),
        routing.jep.executor.as_deref(),
        routing.jep.judge.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        assert!(
            !value.contains("grok") && !value.contains("$"),
            "rol değeri model/fiyat iması taşımamalı: {value}"
        );
    }
}
