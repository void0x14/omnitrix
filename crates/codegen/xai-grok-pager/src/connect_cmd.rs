//! `grok connect` — provider bağlama: models.dev kataloğundan provider
//! çözümleme, API key'i keychain'e şifreli saklama, config.toml'a
//! `[model_providers.<id>]` + `[model.<id>]` + `[models] default` yazma.
//!
//! Güvenlik kuralı: config.toml'a API key DÜZ METİN OLARAK ASLA YAZILMAZ.
//! Config'e yalnızca `base_url` + `api_backend` + model bağlantısı gider;
//! key yalnızca keychain'de şifreli durur (credential provider
//! entegrasyonu Task 6).

use std::io::IsTerminal;
use std::path::Path;

use anyhow::Context as _;
use indexmap::IndexMap;
use toml_edit::DocumentMut;
use xai_grok_shell::sampling::ApiBackend;
use xai_grok_shell::util::auto_connect::{
    AutoConnectError, AutoConnectOutcome, auto_connect_from_key,
};
use xai_grok_shell::util::models_dev::{
    CatalogCache, ModelInfo, ProviderCatalog, api_backend_for_provider, base_url_for_provider,
    fetch_catalog, provider_models,
};
use xai_omni_keychain::{KeyEntry, Keychain};

use crate::app::cli::ConnectArgs;

/// `grok connect` giriş noktası.
///
/// Flag verilmemişse (TTY'de) interaktif wizard'ın yerine geçici bir mesaj
/// basılır — wizard UI'ı TUI göreviyle (Task 7/8) birlikte gelecek; bu
/// task'ta bozuk bir TUI stub'ı kurulmadı.
///
/// Dönüş: `true` = oturum başlatma koşulları sağlandı (TTY + `--no-session`
/// yok) ve config/keychain yazımı tamamlandı — çağıran normal TUI
/// başlatma akışına devam etmeli (model `[models] default` olarak zaten
/// yazıldı; borrow edilen key process runtime store'da). `false` = işlem
/// burada bitti (wizard mesajı / `--no-session` / TTY değil).
pub async fn run(connect_args: ConnectArgs) -> anyhow::Result<bool> {
    if !has_flags(&connect_args) {
        if std::io::stdin().is_terminal() {
            println!("interaktif connect wizard, TUI göreviyle (Task 7/8) birlikte geliyor.");
            println!("Şimdilik flag'lerle kullanın, örnek:");
            println!("  grok connect --provider openai --api-key sk-... [--model gpt-4o]");
            println!("  grok connect --auto --api-key sk-... [--model gpt-4o]");
        } else {
            anyhow::bail!(
                "hiçbir flag verilmedi; en azından --provider + --api-key (veya --auto + --api-key, veya --keychain-id) gerekli"
            );
        }
        return Ok(false);
    }
    let grok_home = xai_grok_shell::util::grok_home::grok_home();
    if connect_args.auto {
        programmatic_connect_auto(&grok_home, &connect_args).await
    } else {
        programmatic_connect(&grok_home, &connect_args).await
    }
}

/// Oturum başlatma koşulları: `--no-session` yok VE stdin bir TTY.
/// Hem `connect_cmd` hem bin/main.rs aynı kararı tek yerden alır.
pub fn session_will_launch(no_session: bool) -> bool {
    !no_session && std::io::stdin().is_terminal()
}

/// Flag'lerden en az biri verilmiş mi? (wizard-dan mı programatik mi?)
fn has_flags(args: &ConnectArgs) -> bool {
    args.auto
        || args.provider.is_some()
        || args.api_key.is_some()
        || args.base_url.is_some()
        || args.model.is_some()
        || args.keychain_id.is_some()
        || args.category.is_some()
}

/// Auto-connect sonucunun config yazımına çevrilmiş girdileri (saf).
#[derive(Debug, PartialEq)]
pub(crate) struct AutoConnectPlan {
    pub provider_id: String,
    pub base_url: String,
    pub api_backend: ApiBackend,
    pub model: String,
    pub model_key: String,
}

/// Winner outcome'u plana çevirir: provider/base URL/backend winner'dan,
/// model `--model` ile doğrulanır (winner model listesine karşı); verilmezse
/// tek model, çokluysa ilki seçilir — manuel akıştaki `resolve_model_id`
/// davranışının aynısı. Key burada asla yer almaz (yalnızca katalog/outcome
/// metadata'sı).
pub(crate) fn plan_from_auto_outcome(
    outcome: &AutoConnectOutcome,
    catalog: &CatalogCache,
    explicit_model: Option<&str>,
) -> anyhow::Result<AutoConnectPlan> {
    let api_backend = catalog
        .providers
        .get(&outcome.provider_id)
        .map(api_backend_for_provider)
        .unwrap_or(ApiBackend::ChatCompletions);
    let models: IndexMap<String, ModelInfo> = outcome
        .models
        .iter()
        .map(|m| (m.id.clone(), m.clone()))
        .collect();
    let model = resolve_model_id(explicit_model, None, &models)?;
    Ok(AutoConnectPlan {
        provider_id: outcome.provider_id.clone(),
        base_url: outcome.base_url.clone(),
        api_backend,
        model: model.clone(),
        model_key: model_entry_key(&outcome.provider_id, &model),
    })
}

/// AutoConnectError'ün kullanıcıya giden Türkçe mesajı. Hiçbir variant API
/// key içermez (typed error sözleşmesi); Ambiguous yalnızca provider id
/// listesi taşır.
fn auto_connect_error_msg(e: &AutoConnectError) -> String {
    match e {
        AutoConnectError::NoDetectedProviders => {
            "auto-connect: API key'den provider tespit edilemedi (key formatı bilinmiyor)".into()
        }
        AutoConnectError::MissingCatalogEntry { provider_id } => {
            format!("auto-connect: '{provider_id}' katalogda bulunamadı veya base URL çözülemedi")
        }
        AutoConnectError::NoProbeWinner => {
            "auto-connect: aday provider'lara canlı bağlantı kurulamadı (ağ/timeout)".into()
        }
        AutoConnectError::Ambiguous { providers } => format!(
            "auto-connect: provider'lar arasında karar verilemedi (eşit güven: {})",
            providers.join(", ")
        ),
        AutoConnectError::EmptyModels { provider_id } => {
            format!("auto-connect: '{provider_id}' için uygulanabilir model yok")
        }
    }
}

/// `--auto` akışı: key → P0.3 orkestrasyonu (`auto_connect_from_key`) →
/// winner planı → ortak keychain/config/oturum yolu. `--provider` gerekmez;
/// `--auto` + manuel kaynak flag'leri clap'ta parse-time çakışır.
async fn programmatic_connect_auto(grok_home: &Path, args: &ConnectArgs) -> anyhow::Result<bool> {
    let mut kc = crate::keys_cmd::prompt_and_open_keychain(grok_home)?;
    let Some(api_key) = args.api_key.as_deref() else {
        anyhow::bail!("--auto için --api-key zorunlu");
    };
    let catalog = fetch_catalog(&reqwest::Client::new(), grok_home, false)
        .await
        .context("models.dev kataloğu çözümlenemedi")?;
    let outcome = auto_connect_from_key(api_key, &catalog)
        .await
        .map_err(|e| anyhow::anyhow!("{}", auto_connect_error_msg(&e)))?;
    let plan = plan_from_auto_outcome(&outcome, &catalog, args.model.as_deref())?;
    finalize_connect(grok_home, args, &mut kc, None, &plan).await
}

/// Programatik akış: key → provider/model çözümleme → config yazımı →
/// (TTY + `--no-session` yoksa) oturum key'ini runtime store'a itme.
async fn programmatic_connect(grok_home: &Path, args: &ConnectArgs) -> anyhow::Result<bool> {
    // 1) Keychain'i aç (key ekleme veya keychain_id okuma için şart).
    let mut kc = crate::keys_cmd::prompt_and_open_keychain(grok_home)?;

    // 2) --keychain-id varsa mevcut kaydın metadata'sını oku (provider/model/
    //    base_url için kaynak; ham key Task 6'nın credential provider'ına kalır).
    let entry = match &args.keychain_id {
        Some(kid) => {
            let entries = kc.list_keys()?;
            Some(
                entries
                    .iter()
                    .find(|e| e.id == *kid)
                    .cloned()
                    .ok_or_else(|| {
                        anyhow::anyhow!("keychain'de {kid} yok (grok keys list ile bakın)")
                    })?,
            )
        }
        None => None,
    };

    // 3) Provider id: flag > keychain kaydı.
    let provider_id = args
        .provider
        .clone()
        .or_else(|| entry.as_ref().map(|e| e.provider_id.clone()))
        .ok_or_else(|| {
            anyhow::anyhow!("--provider gerekli (ya da --keychain-id ile mevcut kayıt kullanın)")
        })?;

    // 4) Katalog çözümleme (custom hariç).
    let (catalog_entry, catalog_models) = resolve_catalog(grok_home, &provider_id, args).await?;

    // 5) Base URL: flag > keychain kaydı > katalog.
    let base_url = args
        .base_url
        .clone()
        .or_else(|| entry.as_ref().and_then(|e| e.base_url.clone()))
        .or_else(|| catalog_entry.as_ref().and_then(base_url_for_provider))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "base URL çözülemedi: --base-url verin (ör. https://api.example.com/v1)"
            )
        })?;

    // 6) Model: flag > keychain kaydı > katalog tek/ilk model.
    let model = resolve_model_id(
        args.model.as_deref(),
        entry.as_ref().and_then(|e| e.model_id.as_deref()),
        &catalog_models,
    )?;

    // 7) API backend (katalog bilgisi yoksa OpenAI-compatible varsayım).
    let api_backend = catalog_entry
        .as_ref()
        .map(api_backend_for_provider)
        .unwrap_or(ApiBackend::ChatCompletions);

    let plan = AutoConnectPlan {
        model_key: model_entry_key(&provider_id, &model),
        provider_id,
        base_url,
        api_backend,
        model,
    };
    finalize_connect(grok_home, args, &mut kc, entry.as_ref(), &plan).await
}

/// Ortak sonlandırma (manuel ve `--auto` akışları paylaşır): key'i keychain'e
/// yaz veya mevcut kaydı doğrula → config.toml'a provider/model yaz →
/// varsayılan modeli set et → (TTY + `--no-session` yoksa) oturum key'ini
/// process runtime store'a it. `entry` yalnızca manuel `--keychain-id`
/// akışında Some'dur; `--auto` akışında key her zaman `--api-key`'ten gelir.
async fn finalize_connect(
    grok_home: &Path,
    args: &ConnectArgs,
    kc: &mut Keychain,
    entry: Option<&KeyEntry>,
    plan: &AutoConnectPlan,
) -> anyhow::Result<bool> {
    let AutoConnectPlan {
        provider_id,
        base_url,
        api_backend,
        model,
        model_key,
    } = plan;

    // 8) Key'i keychain'e yaz veya mevcut kaydı doğrula.
    let category = match &args.category {
        Some(c) if !c.is_empty() => c.clone(),
        _ => kc.default_category(),
    };
    let key_id = if let Some(key) = &args.api_key {
        let id = kc.add_key(
            &category,
            provider_id,
            key,
            Some(model.clone()),
            Some(base_url.clone()),
        )?;
        kc.save()?;
        println!("keychain'e eklendi: {provider_id} ({category}) [{id}]");
        id
    } else if let Some(e) = entry {
        e.id.clone()
    } else {
        anyhow::bail!("--api-key veya --keychain-id gerekli (keychain'de key yok)");
    };

    // 9) Config yazımı: model_providers + model + models.default.
    write_provider_config(
        grok_home,
        provider_id,
        base_url,
        api_backend,
        model_key,
        model,
    )
    .await?;
    xai_grok_shell::util::config::set_default_model(model_key.clone())
        .await
        .context("varsayılan model yazılamadı")?;

    println!("bağlandı: {provider_id} ({base_url})");
    println!("model: {model} → [{model_key}] (varsayılan)");
    println!("keychain kaydı: {key_id} (kategori: {category})");

    // 10) Oturum başlatma (Task 6 ratified gap closure): TTY + `--no-session`
    //     yoksa borrow edilen key'i process runtime store'a it (config'e düz
    //     metin yazılmaz) ve normal TUI akışına devam et — shell açılışta
    //     `[models] default`'u çözer, `resolve_credentials` runtime key'i
    //     görür (config.toml'da api_key YOK).
    //
    //     NOT: client tipi Generic kalır (main.rs zaten `set_client_name`
    //     çağırdı — OnceLock ikinci set'te panic eder); yalnızca User-Agent
    //     origin'ini etkiler, oturum akışını bozmaz.
    if session_will_launch(args.no_session) {
        push_runtime_key(kc, &key_id, model)?;
        return Ok(true);
    }
    println!(
        "Ajan oturumu şöyle başlatılır: grok --model {model_key} (tam oturum entegrasyonu TUI göreviyle geliyor)"
    );
    Ok(false)
}

/// Keychain'den borrow edilen key'i process runtime store'a iter. Key RAM'de
/// TTL'li yaşar (`BorrowedKey` drop'ta sıfırlanır; store kopyası tıpkı
/// `set_process_static_api_key` gibi `String`'dir); config.toml'a asla
/// yazılmaz. Chat seam: `resolve_credentials` → `own_credential` fallback;
/// tools/voice seam: shell `sync_process_static_api_key` aynı fallback'ten
/// beslenir.
fn push_runtime_key(kc: &mut Keychain, key_id: &str, model: &str) -> anyhow::Result<()> {
    let borrowed = kc.borrow(key_id.to_string())?;
    xai_grok_shell::auth::runtime_key::set_runtime_model_key(
        model,
        Some(borrowed.get().to_string()),
    );
    Ok(())
}

/// Katalogu getirir ve provider kaydını arar. `custom` provider (veya
/// --base-url ile bilinmeyen id) katalog aramaz: models boş, backend
/// ChatCompletions varsayılır. `custom` id'si config'e de o şekilde yazılır.
async fn resolve_catalog(
    grok_home: &Path,
    provider_id: &str,
    args: &ConnectArgs,
) -> anyhow::Result<(Option<ProviderCatalog>, IndexMap<String, ModelInfo>)> {
    resolve_catalog_for(grok_home, provider_id, args.base_url.is_some()).await
}

/// `resolve_catalog`'un `ConnectArgs`-bağımsız çekirdeği — headless
/// (`grok -p --provider ... --api-key ...`) akışı da aynı çözümlemeyi kullanır.
pub(crate) async fn resolve_catalog_for(
    grok_home: &Path,
    provider_id: &str,
    has_base_url_flag: bool,
) -> anyhow::Result<(Option<ProviderCatalog>, IndexMap<String, ModelInfo>)> {
    if provider_id == "custom" {
        return Ok((None, IndexMap::new()));
    }
    let cache = fetch_catalog(&reqwest::Client::new(), grok_home, false)
        .await
        .context("models.dev kataloğu çözümlenemedi")?;
    match (cache.providers.get(provider_id), has_base_url_flag) {
        (Some(entry), _) => Ok((
            Some(entry.clone()),
            provider_models(&cache, provider_id)
                .cloned()
                .unwrap_or_default(),
        )),
        // Katalogda yok ama --base-url verildi → custom endpoint (kendi id'siyle).
        (None, true) => Ok((None, IndexMap::new())),
        (None, false) => anyhow::bail!(
            "'{provider_id}' models.dev kataloğunda yok; --base-url ile custom endpoint kullanın"
        ),
    }
}

/// `resolve_catalog`'un `ConnectArgs`-bağımsız çekirdeği — TUI
/// `Effect::ConnectProviderWrite` akışı (`persist_provider_connect`) aynı
/// katalog çözümlemesini `--base-url` bayrağı olmadan kullanır.
pub(crate) async fn catalog_for_provider(
    grok_home: &Path,
    provider_id: &str,
) -> anyhow::Result<(Option<ProviderCatalog>, IndexMap<String, ModelInfo>)> {
    if provider_id == "custom" {
        return Ok((None, IndexMap::new()));
    }
    let cache = fetch_catalog(&reqwest::Client::new(), grok_home, false)
        .await
        .context("models.dev kataloğu çözümlenemedi")?;
    match cache.providers.get(provider_id) {
        Some(entry) => Ok((
            Some(entry.clone()),
            provider_models(&cache, provider_id)
                .cloned()
                .unwrap_or_default(),
        )),
        None => Ok((None, IndexMap::new())),
    }
}

/// Model id çözümleme (saf): flag/keychain kaydı varsa doğrula (katalog
/// doluysa); yoksa tek modeli seç, çokluysa ilkini seç ve stderr'e not düş.
/// Headless (`grok -p`) akışı da aynı çözümlemeyi kullanır (pub(crate)).
pub(crate) fn resolve_model_id(
    explicit: Option<&str>,
    entry_model: Option<&str>,
    models: &IndexMap<String, ModelInfo>,
) -> anyhow::Result<String> {
    match explicit.or(entry_model) {
        Some(m) => {
            if models.is_empty() || models.contains_key(m) {
                Ok(m.to_string())
            } else {
                anyhow::bail!("'{m}' bu provider'ın kataloğunda yok; --model ile doğru id verin")
            }
        }
        None => {
            if models.is_empty() {
                anyhow::bail!("model seçilemedi: --model <ID> verin");
            }
            if models.len() == 1 {
                Ok(models.keys().next().expect("len==1").clone())
            } else {
                let first = models.keys().next().expect("non-empty").clone();
                eprintln!(
                    "model belirtilmedi; ilk model seçildi: {first} (--model ile değiştirin)"
                );
                Ok(first)
            }
        }
    }
}

/// `[model.<key>]` giriş anahtarı: `omni-<provider>-<model>`, TOML için
/// güvenli karakterlere indirgenir (nokta/çizgi/alt çizgi korunur).
pub fn model_entry_key(provider_id: &str, model: &str) -> String {
    let slug = |s: &str| {
        s.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                    c
                } else {
                    '-'
                }
            })
            .collect::<String>()
    };
    format!("omni-{}-{}", slug(provider_id), slug(model))
}

/// ApiBackend'in config.toml'da kullanılacak snake_case karşılığı
/// (`xai_grok_sampling_types::ApiBackend` serde rename_all ile aynı).
fn api_backend_str(b: &ApiBackend) -> &'static str {
    match b {
        ApiBackend::ChatCompletions => "chat_completions",
        ApiBackend::Responses => "responses",
        ApiBackend::Messages => "messages",
    }
}

/// Provider + model bölümlerini TOML dokümanına işler (saf; disk yok).
/// `model_providers.<id>` yalnızca `base_url` + `api_backend` taşır —
/// API key burada ASLA bulunmaz (keychain'de şifreli durur).
pub fn apply_provider_config(
    doc: &mut DocumentMut,
    provider_id: &str,
    base_url: &str,
    api_backend: &ApiBackend,
    model_key: &str,
    model: &str,
) {
    doc["model_providers"][provider_id]["base_url"] = toml_edit::value(base_url);
    doc["model_providers"][provider_id]["api_backend"] =
        toml_edit::value(api_backend_str(api_backend));
    doc["model"][model_key]["model"] = toml_edit::value(model);
    doc["model"][model_key]["model_provider"] = toml_edit::value(provider_id);
}

/// Bölümleri `~/.grok/config.toml`'a atomik yazar (temp + rename; unix'te
/// mevcut dosya modunu korur). Bozuk TOML üzerine asla yazmaz.
/// `models.default` ayrıca [`set_default_model`] ile yazılır (campaign kanalı).
/// TUI `Effect::ConnectProviderWrite` akışı da aynı helper'ı kullanır.
pub(crate) async fn write_provider_config(
    grok_home: &Path,
    provider_id: &str,
    base_url: &str,
    api_backend: &ApiBackend,
    model_key: &str,
    model: &str,
) -> anyhow::Result<()> {
    let path = grok_home.join("config.toml");
    let mut doc =
        crate::config_toml_edit::read_config_document_for_edit(&path).ok_or_else(|| {
            anyhow::anyhow!(
                "config.toml geçerli TOML değil; düzeltmeden yazılamaz: {}",
                path.display()
            )
        })?;
    apply_provider_config(
        &mut doc,
        provider_id,
        base_url,
        api_backend,
        model_key,
        model,
    );
    atomic_write_config(&path, &doc.to_string())
        .map_err(|e| anyhow::anyhow!("config.toml yazılamadı: {e}"))
}

/// Temp dosya + rename ile atomik yazım (shell'in `atomic_write_string`
/// karşılığı — o fonksiyon `pub(crate)` olduğu için burada yerel kopya).
fn atomic_write_config(path: &Path, content: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    #[cfg(unix)]
    let prior_mode: Option<u32> = match std::fs::metadata(path) {
        Ok(m) => {
            use std::os::unix::fs::PermissionsExt;
            Some(m.permissions().mode())
        }
        Err(_) => None,
    };
    #[cfg(not(unix))]
    let prior_mode: Option<u32> = None;
    let suffix = {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        format!("toml.tmp.{}.{}", std::process::id(), nanos)
    };
    let tmp = path.with_extension(suffix);
    std::fs::write(&tmp, content)?;
    #[cfg(unix)]
    if let Some(mode) = prior_mode {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode));
    }
    let _ = prior_mode;
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

#[cfg(test)]
#[path = "connect_cmd_tests.rs"]
mod tests; // ayrı test dosyası (connect_cmd_tests.rs)
