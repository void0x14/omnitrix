//! `omnitrix run <gorev>` — Faz 1 uctan uca dikey dilim (MASTER-PLAN Faz 1 kapisi).
//!
//! Zincir tek yonlu ve tek parcadir:
//!
//! 1. keyring'den anahtar + saglayici tespiti (`omni-provider`)
//! 2. `SamplerConfig` kurulumu — model kimligi YAPILANDIRMADAN gelir (I5)
//! 3. FS-shim + Faz 1 tool seti (`omni-tools`: `DiffShimFs` + `faz1_toolset_with`)
//! 4. tek ajan oturumu (`omni-agent`: `AgentSpec` -> `AgentSession`)
//! 5. tur dongusu (`omni-router::turn::TurnLoop`) — dongu ROUTER'a aittir,
//!    vendored `Agent`'in `run`/`turn`/`step` metodu YOKTUR
//! 6. her adim olay-log'a, her dosya dokunusu diff akisina (`omni-storage`)
//! 7. sonuc + olculen RSS stdout'a
//!
//! I5: bu dosyada hicbir literal model adi/fiyati yoktur; model kimligi
//! `OMNITRIX_MODEL` ya da `config_kv('model.default')` uzerinden gelir.
//! I6: uretim yolunda `unwrap`/`expect`/`panic!` yoktur; anahtar yoksa, ag
//! yoksa ya da model tanimsizsa surec duzgun bir hata mesajiyla doner.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use omni_agent::{AgentSpec, PersonaSpec};
use omni_provider::detection::ProviderKind;
use omni_provider::keyring::KeyManager;
use omni_router::sampler::SamplerLayer;
use omni_router::turn::{EventSink, TurnLoop, TurnOutcome};
use omni_storage::cas::CasBlobStore;
use omni_storage::events::{AgentEventRecord, EventWriter, FileTouchRecord};
use omni_storage::sqlite_schema::SchemaManager;
use omni_storage::writer_actor::{WriteOp, WriterActor};
use omni_tools::broker::{AuditEvent, AuditGate, AuditStream, ToolBroker, audit_channel};
use omni_tools::fs_shim::{DiffShimFs, TouchStream, touch_channel};
use omni_tools::registry::{faz1_toolset_with, toolset_tool_names};
use omni_tools::{AsyncFileSystem, ToolNotificationHandle, local_fs, local_terminal};
use rusqlite::OptionalExtension;
use rusqlite::types::Value;
use xai_grok_sampler::{ApiBackend, AuthScheme, RetryPolicy, SamplerConfig};

/// Faz 1 dikey diliminin tek personasi. Katalog Faz 5'te gelir.
const PERSONA_NAME: &str = "omni-slice";
/// Persona aciklamasi — sistem promptuna girer.
const PERSONA_DESCRIPTION: &str = "Faz 1 uctan uca dikey dilim ajani";

/// Oturum kalintilarinin yazildigi calisma dizini alt klasoru.
const SESSION_DIR: &str = ".omni";
/// Vendored tool durumunun tasindigi dosya (ust dizini oturum klasoru olur).
const STATE_FILE: &str = "agent-state.json";

/// Model kimliginin okundugu `config_kv` anahtari (0008 semasi).
const MODEL_CONFIG_KEY: &str = "model.default";
/// Model kimligini ezen ortam degiskeni.
const MODEL_ENV: &str = "OMNITRIX_MODEL";
/// Kullanilacak saglayici kimligini secen ortam degiskeni.
const PROVIDER_ENV: &str = "OMNITRIX_PROVIDER";
/// Saglayicinin varsayilan taban URL'ini ezen ortam degiskeni.
const BASE_URL_ENV: &str = "OMNITRIX_BASE_URL";

/// Etkilesimli kosum icin tur ust siniri.
const MAX_TURNS: u32 = 12;
/// Tek turluk cikti ust siniri.
const MAX_COMPLETION_TOKENS: u32 = 4096;
/// Baglam penceresi tahmini; saglayici bunu asagi cekebilir.
const CONTEXT_WINDOW: u64 = 128_000;
/// Etkilesimli kosumda vendored varsayilan (15) cok uzun sürer; kisaltilir.
const SAMPLER_MAX_RETRIES: u32 = 2;
/// Akis bosta kalirsa istek bu sureden sonra dusurulur.
const IDLE_TIMEOUT_SECS: u64 = 300;

/// Tur dongusu bittikten sonra diff akisinin bosalmasi icin beklenen sure.
const DRAIN_GRACE: Duration = Duration::from_secs(2);

// ---------------------------------------------------------------------------
// Giris noktasi
// ---------------------------------------------------------------------------

/// `omnitrix run <gorev metni>` alt-komutu.
pub async fn cmd_run(rest: Vec<String>) -> anyhow::Result<()> {
    let task = rest.join(" ").trim().to_string();
    if task.is_empty() {
        anyhow::bail!("gorev metni bos — kullanim: omnitrix run <gorev>");
    }
    execute(&task).await
}

/// Zincirin tamami. Her adim sirayla kurulur; hicbiri panige donusmez (I6).
async fn execute(task: &str) -> anyhow::Result<()> {
    let workspace = resolve_workspace()?;
    let data_dir = crate::bootstrap::data_dir()?;
    // Ag dosya sisteminde gercek dosya adi degisebilir; butun bilesenler AYNI
    // yolu gormeli, yoksa yazici ve okuyucu farkli dosyalara duser.
    let db = EventWriter::effective_db_path(&crate::bootstrap::db_path()?);

    prepare_schema(&db)?;

    // 1. Anahtar + saglayici tespiti.
    let provider = resolve_provider().await?;
    // 2. Model kimligi — yapilandirmadan gelir, burada literal yoktur (I5).
    let model = resolve_model(&db)?;

    println!("saglayici: {} ({})", provider.label, provider.slug);
    println!("model: {model}");
    println!("calisma dizini: {}", workspace.display());

    let sampler = SamplerLayer::spawn(
        sampler_config(&provider, &model),
        RetryPolicy {
            max_retries: SAMPLER_MAX_RETRIES,
            ..Default::default()
        },
    );

    // 3. FS-shim + Faz 1 tool seti.
    let cas = CasBlobStore::new(&data_dir.join("cas"))
        .map_err(|e| anyhow::anyhow!("CAS acilamadi: {e}"))?;
    let (sink, touches) = touch_channel();
    let fs: Arc<dyn AsyncFileSystem> =
        Arc::new(DiffShimFs::new(local_fs(), workspace.clone(), sink).with_cas(cas.clone()));
    let terminal = local_terminal();
    let toolset = faz1_toolset_with(
        &workspace,
        Arc::clone(&fs),
        Arc::clone(&terminal),
        ToolNotificationHandle::noop(),
    )
    .map_err(|e| anyhow::anyhow!("tool seti kurulamadi: {e}"))?;

    // K3: izin listesi tool setinin KENDISINDEN turer; broker karari model
    // tarafindan gorulen adlara gore verilir.
    let broker = ToolBroker::new(toolset_tool_names(&toolset), Vec::new());
    let allowed = broker.allowed_tools().to_vec();
    println!("tool seti: {}", allowed.join(", "));

    // 4. Tek ajan oturumu.
    let session_dir = workspace.join(SESSION_DIR);
    std::fs::create_dir_all(&session_dir)
        .map_err(|e| anyhow::anyhow!("oturum klasoru olusturulamadi: {e}"))?;

    let definition = PersonaSpec::new(PERSONA_NAME, PERSONA_DESCRIPTION)
        .with_tools(allowed.clone())
        .with_max_turns(MAX_TURNS)
        .with_model_id(model.clone())
        .to_definition_checked()?;

    let session = AgentSpec::new(definition, workspace.clone(), session_dir.join(STATE_FILE))
        .with_tools(allowed)
        .with_fs(Arc::clone(&fs))
        .build(Arc::clone(&terminal), ToolNotificationHandle::noop())
        .await?;

    // 5/6. Olay-log. Tek `EventWriter`, tek `WriterActor`: butun yazimlar
    // (dongu, diff akisi, denetim) ayni sirada ilerler.
    let writer = Arc::new(WriterActor::new(&db));
    let journal = Arc::new(
        EventWriter::with_writer(&db, cas, Arc::clone(&writer))
            .map_err(|e| anyhow::anyhow!("olay yazici acilamadi: {e}"))?,
    );

    // Yarim kalmis niyetler once geri uygulanir (8.1 adim 4).
    match journal.recover() {
        Ok(report) if report.replayed > 0 || report.reapplied > 0 || report.failed > 0 => {
            tracing::info!(
                replayed = report.replayed,
                reapplied = report.reapplied,
                failed = report.failed,
                "olay-log kurtarmasi tamamlandi"
            );
        }
        Ok(_) => {}
        Err(e) => tracing::warn!(%e, "olay-log kurtarmasi basarisiz"),
    }

    let agent_id = ensure_agent_row(&writer, &db, task).await?;

    // Diff akisi ve K3 denetim akisi UNBOUNDED'dir; ikisi de drene edilmezse
    // bellek suresiz buyur.
    let touch_drain = tokio::spawn(drain_touches(Arc::clone(&journal), agent_id, touches));
    let (audit_sink, audit_stream) = audit_channel();
    let audit_drain = tokio::spawn(drain_audit(Arc::clone(&journal), agent_id, audit_stream));

    record_event(
        &journal,
        agent_id,
        "task.started",
        serde_json::json!({ "task": task, "model": &model, "provider": &provider.slug }),
    )
    .await;

    // 7. Tur dongusu. Dongu ROUTER'a aittir; burada yalnizca parcalar baglanir.
    let sink = EventSink::new(Arc::clone(&journal), agent_id);
    let mut turn_loop = TurnLoop::new(sampler, session, sink)
        .with_max_turns(MAX_TURNS)
        .with_broker(broker)
        .with_audit_gate(AuditGate::new(audit_sink));
    let outcome = turn_loop.run_task(task).await;

    // Akislarin sonlanmasi icin gonderici uclarinin dusmesi gerekir.
    drop(turn_loop);
    drop(toolset);
    drop(fs);
    drop(terminal);
    if tokio::time::timeout(DRAIN_GRACE, touch_drain)
        .await
        .is_err()
    {
        tracing::warn!("diff akisi zamaninda bosalmadi");
    }
    if tokio::time::timeout(DRAIN_GRACE, audit_drain)
        .await
        .is_err()
    {
        tracing::warn!("denetim akisi zamaninda bosalmadi");
    }

    let outcome = match outcome {
        Ok(value) => {
            record_event(
                &journal,
                agent_id,
                "task.completed",
                serde_json::json!({ "task": task }),
            )
            .await;
            set_agent_state(&writer, agent_id, "done").await;
            value
        }
        Err(err) => {
            let reason = err.to_string();
            record_event(
                &journal,
                agent_id,
                "task.failed",
                serde_json::json!({ "task": task, "error": reason }),
            )
            .await;
            set_agent_state(&writer, agent_id, "failed").await;
            flush_writer(&writer).await;
            anyhow::bail!("gorev tamamlanamadi: {reason}");
        }
    };

    // Tek seferlik CLI yolu: surec hemen bitecegi icin yazicinin diskle isini
    // bitirmesi beklenir. Bu, TUI'nin SIGINT yolu DEGILDIR (8.1 orada gecerli).
    flush_writer(&writer).await;

    report(agent_id, &outcome);
    Ok(())
}

/// Sonuc ozeti + Faz 1 kapisinin olcumu: RSS.
fn report(agent_id: i64, outcome: &TurnOutcome) {
    println!("--- sonuc ---");
    if !outcome.final_text.trim().is_empty() {
        println!("{}", outcome.final_text.trim());
    }
    println!("--- ozet ---");
    println!("agent_id: {agent_id}");
    println!("durma nedeni: {}", outcome.stop.as_str());
    println!("tur: {}", outcome.turns);
    println!(
        "tool cagrisi: {} (red {}, hata {})",
        outcome.tool_calls, outcome.denied_tool_calls, outcome.failed_tool_calls
    );
    println!(
        "token: giris {} / cikis {}",
        outcome.tokens_in, outcome.tokens_out
    );
    if outcome.dropped_events > 0 {
        println!("yazilamayan olay: {}", outcome.dropped_events);
    }
    match crate::bootstrap::rss_kb() {
        Some(kb) => println!("rss_kb={kb}"),
        None => println!("rss_kb=bilinmiyor"),
    }
}

// ---------------------------------------------------------------------------
// 1. Saglayici + anahtar
// ---------------------------------------------------------------------------

/// Cozulmus saglayici baglantisi. Anahtar burada duz metindir; surec omru
/// boyunca yalnizca `SamplerConfig`'e tasinir.
struct ProviderCreds {
    /// Keyring dosya adi (`omnitrix key add` bunu uretir).
    slug: String,
    /// Kullaniciya gosterilen saglayici adi.
    label: String,
    base_url: String,
    api_key: String,
    api_backend: ApiBackend,
    auth_scheme: AuthScheme,
}

/// Keyring'den anahtari alir ve saglayici bicimini cozer.
///
/// Anahtar yoksa surec panige degil, ne yapilmasi gerektigini soyleyen bir
/// hataya duser (I6).
async fn resolve_provider() -> anyhow::Result<ProviderCreds> {
    let keyring = KeyManager::new();
    let providers = keyring.list_providers().await;
    if providers.is_empty() {
        anyhow::bail!("kayitli API anahtari yok — once `omnitrix key add <ANAHTAR>` calistirin");
    }

    let slug = match std::env::var(PROVIDER_ENV) {
        Ok(requested) if !requested.trim().is_empty() => {
            let requested = requested.trim().to_lowercase();
            if !providers.contains(&requested) {
                anyhow::bail!(
                    "{PROVIDER_ENV}={requested} kayitli degil — mevcut saglayicilar: {}",
                    providers.join(", ")
                );
            }
            requested
        }
        _ => match providers.first() {
            Some(first) => first.clone(),
            None => anyhow::bail!("saglayici listesi bos"),
        },
    };

    let key = keyring
        .get_key(&slug)
        .await
        .map_err(|e| anyhow::anyhow!("'{slug}' anahtari okunamadi: {e}"))?;
    let api_key = key.trim().to_string();
    if api_key.is_empty() {
        anyhow::bail!("'{slug}' icin saklanan anahtar bos");
    }

    // Once anahtar onekine bak; tanimsizsa keyring kimliginden turet.
    let kind = ProviderKind::detect_from_key(&api_key).or_else(|| kind_from_slug(&slug));

    let base_url = match std::env::var(BASE_URL_ENV) {
        Ok(url) if !url.trim().is_empty() => url.trim().to_string(),
        _ => match &kind {
            Some(kind) if !kind.default_base_url().is_empty() => {
                kind.default_base_url().to_string()
            }
            _ => anyhow::bail!(
                "'{slug}' icin taban URL bilinmiyor — {BASE_URL_ENV} ortam degiskenini ayarlayin"
            ),
        },
    };

    let label = match &kind {
        Some(kind) => kind.name().to_string(),
        None => slug.clone(),
    };
    let (api_backend, auth_scheme) = wire_format(kind.as_ref());

    Ok(ProviderCreds {
        slug,
        label,
        base_url,
        api_key,
        api_backend,
        auth_scheme,
    })
}

/// `omnitrix key add` tarafindan uretilen keyring kimligini saglayici bicimine
/// geri cevirir. Bilinmeyen kimlik `None` doner; cagiran taban URL'i ortamdan
/// ister.
fn kind_from_slug(slug: &str) -> Option<ProviderKind> {
    match slug {
        "openai" => Some(ProviderKind::OpenAI),
        "anthropic" => Some(ProviderKind::Anthropic),
        "google" => Some(ProviderKind::Google),
        "xai" => Some(ProviderKind::Xai),
        "openrouter" => Some(ProviderKind::OpenRouter),
        "deepseek" => Some(ProviderKind::DeepSeek),
        _ => None,
    }
}

/// Saglayiciya gore HTTP protokolu ve kimlik dogrulama semasi.
///
/// Anthropic `POST {base}/messages` + `x-api-key` konusur; kalan saglayicilar
/// OpenAI uyumlu `POST {base}/chat/completions` + `Authorization: Bearer`.
fn wire_format(kind: Option<&ProviderKind>) -> (ApiBackend, AuthScheme) {
    match kind {
        Some(ProviderKind::Anthropic) => (ApiBackend::Messages, AuthScheme::XApiKey),
        _ => (ApiBackend::ChatCompletions, AuthScheme::Bearer),
    }
}

// ---------------------------------------------------------------------------
// 2. Sampler yapilandirmasi
// ---------------------------------------------------------------------------

/// Aktorun varsayilan istek yapilandirmasi.
///
/// `model` disaridan gelir; `base_url` surum onekini ZATEN icermelidir, cunku
/// istemci sonuna `/chat/completions` ya da `/messages` ekler.
fn sampler_config(provider: &ProviderCreds, model: &str) -> SamplerConfig {
    SamplerConfig {
        api_key: Some(provider.api_key.clone()),
        base_url: provider.base_url.clone(),
        model: model.to_string(),
        max_completion_tokens: Some(MAX_COMPLETION_TOKENS),
        api_backend: provider.api_backend.clone(),
        auth_scheme: provider.auth_scheme,
        context_window: CONTEXT_WINDOW,
        max_retries: Some(SAMPLER_MAX_RETRIES),
        idle_timeout_secs: Some(IDLE_TIMEOUT_SECS),
        stream_tool_calls: true,
        ..Default::default()
    }
}

/// Model kimligini cozer: once ortam degiskeni, sonra `config_kv` satiri.
///
/// I5: burada hicbir varsayilan model adi YOKTUR. Ikisi de yoksa surec ne
/// yapilmasi gerektigini soyleyerek durur.
fn resolve_model(db: &Path) -> anyhow::Result<String> {
    if let Ok(model) = std::env::var(MODEL_ENV)
        && !model.trim().is_empty()
    {
        return Ok(model.trim().to_string());
    }

    let stored = read_config_kv(db, MODEL_CONFIG_KEY)?;
    match stored {
        Some(model) if !model.trim().is_empty() => Ok(model.trim().to_string()),
        _ => anyhow::bail!(
            "model kimligi tanimsiz — {MODEL_ENV} ortam degiskenini ayarlayin ya da \
             config_kv tablosuna '{MODEL_CONFIG_KEY}' satirini yazin"
        ),
    }
}

/// `config_kv` tablosundan tek deger okur.
///
/// Sutun JSON tuttugu icin once JSON dize olarak cozulur; cozulemezse ham
/// degerin kendisi kullanilir (elle yazilmis satirlara tolerans).
fn read_config_kv(db: &Path, key: &str) -> anyhow::Result<Option<String>> {
    let conn = rusqlite::Connection::open(db)
        .map_err(|e| anyhow::anyhow!("yapilandirma veritabani acilamadi: {e}"))?;
    let raw: Option<String> = conn
        .query_row(
            "SELECT value_json FROM config_kv WHERE key = ?1",
            rusqlite::params![key],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| anyhow::anyhow!("config_kv okunamadi: {e}"))?;

    Ok(raw.map(|value| {
        serde_json::from_str::<String>(&value).unwrap_or_else(|_| value.trim().to_string())
    }))
}

// ---------------------------------------------------------------------------
// 3. Calisma dizini ve sema
// ---------------------------------------------------------------------------

/// Ajanin calisma dizini. `AgentSpec` mutlak yol sart kosar.
///
/// `Path::canonicalize` yerine `dunce::canonicalize` kullanilir (clippy.toml).
fn resolve_workspace() -> anyhow::Result<PathBuf> {
    let cwd =
        std::env::current_dir().map_err(|e| anyhow::anyhow!("calisma dizini cozulemedi: {e}"))?;
    Ok(dunce::canonicalize(&cwd).unwrap_or(cwd))
}

/// Tablolar ilk kosumda burada olusur.
fn prepare_schema(db: &Path) -> anyhow::Result<()> {
    let schema = SchemaManager::new(db).map_err(|e| anyhow::anyhow!("SchemaManager: {e}"))?;
    schema
        .run_migrations()
        .map_err(|e| anyhow::anyhow!("migration: {e}"))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// 6. Olay-log ve diff akisi
// ---------------------------------------------------------------------------

/// Tek yazar aktoru uzerinden SQL yurutur; yanit beklenerek sira garanti edilir.
async fn writer_execute(
    writer: &WriterActor,
    sql: &str,
    params: Vec<Value>,
) -> anyhow::Result<usize> {
    let (reply, rx) = tokio::sync::oneshot::channel();
    writer
        .write(WriteOp::Execute {
            sql: sql.to_string(),
            params,
            reply,
        })
        .await
        .map_err(|e| anyhow::anyhow!("yazici kanali kapali: {e}"))?;
    rx.await
        .map_err(|_| anyhow::anyhow!("yazici yaniti kayboldu"))?
        .map_err(|e| anyhow::anyhow!("SQL yurutulemedi: {e}"))
}

/// Gorev + ajan satirlarini yazar ve olay-log'un ihtiyac duydugu `agent_id`'yi
/// dondurur. `agent_events` / `file_touches` bu kimlige yabanci anahtarla bagli.
/// TUI event sink'i (Task 1.3) de ayni satir desenini kullanir; bu yuzden
/// `pub(crate)` yapildi.
pub(crate) async fn ensure_agent_row(writer: &WriterActor, db: &Path, title: &str) -> anyhow::Result<i64> {
    // `root_id` kendine referans verdiginden yeni id onceden hesaplanir:
    // AUTOINCREMENT'in bir sonraki degeri = max(sqlite_sequence.seq, max(id)) + 1.
    writer_execute(
        writer,
        "INSERT INTO tasks (id, parent_id, root_id, title, mode, status, depth) \
         SELECT nid, NULL, nid, ?1, 'interactive', 'running', 0 FROM ( \
             SELECT MAX( \
                 COALESCE((SELECT MAX(id) FROM tasks), 0), \
                 COALESCE((SELECT seq FROM sqlite_sequence WHERE name = 'tasks'), 0) \
             ) + 1 AS nid \
         )",
        vec![Value::Text(title.to_string())],
    )
    .await?;

    let task_id = last_row_id(db, "tasks")?;

    writer_execute(
        writer,
        "INSERT INTO agents (task_id, persona, state) VALUES (?1, ?2, 'running')",
        vec![
            Value::Integer(task_id),
            Value::Text(PERSONA_NAME.to_string()),
        ],
    )
    .await?;

    last_row_id(db, "agents")
}

/// Tablodaki en buyuk `id`. Tek surecli CLI yolunda bu, az once yazilan satirdir.
fn last_row_id(db: &Path, table: &str) -> anyhow::Result<i64> {
    // `table` cagiran tarafindan verilen sabit bir addir; disaridan gelmez.
    let sql = format!("SELECT COALESCE(MAX(id), 0) FROM {table}");
    let conn =
        rusqlite::Connection::open(db).map_err(|e| anyhow::anyhow!("veritabani acilamadi: {e}"))?;
    let id: i64 = conn
        .query_row(&sql, [], |row| row.get(0))
        .map_err(|e| anyhow::anyhow!("{table} kimligi okunamadi: {e}"))?;
    if id == 0 {
        anyhow::bail!("{table} satiri yazilamadi");
    }
    Ok(id)
}

/// Ajan durumunu gunceller. Muhasebe hatasi gorev sonucunu degistirmez.
async fn set_agent_state(writer: &WriterActor, agent_id: i64, state: &str) {
    let result = writer_execute(
        writer,
        "UPDATE agents SET state = ?1, ended_at = datetime('now') WHERE id = ?2",
        vec![Value::Text(state.to_string()), Value::Integer(agent_id)],
    )
    .await;
    if let Err(e) = result {
        tracing::warn!(%e, agent_id, "ajan durumu guncellenemedi");
    }
}

/// Olay-log'a tek satir dusurur. Yazim hatasi gorevi dusurmez, yalniz kayda gecer.
async fn record_event(events: &EventWriter, agent_id: i64, kind: &str, payload: serde_json::Value) {
    let record = AgentEventRecord {
        agent_id,
        kind: kind.to_string(),
        payload_json: Some(payload.to_string()),
    };
    if let Err(e) = events.record_event(record).await {
        tracing::warn!(%e, kind, "olay yazilamadi");
    }
}

/// Diff akisini olay-log'a dokerek bosaltir.
///
/// Akis UNBOUNDED'dir; drene edilmezse bellek suresiz buyur. Dongu, butun
/// gonderici uclari dustugunde kendiliginden sonlanir.
async fn drain_touches(events: Arc<EventWriter>, agent_id: i64, mut stream: TouchStream) {
    while let Some(touch) = stream.recv().await {
        let (added, removed) = touch.counts_u32();
        let path = touch.path.display().to_string();

        if touch.outside_workspace {
            // K5: kisitlama yok, gorunurluk var.
            tracing::warn!(path = %path, "calisma dizini disinda dosya dokunusu");
        }

        let record = FileTouchRecord {
            agent_id,
            path: path.clone(),
            outside_workspace: touch.outside_workspace,
            added: i64::from(added),
            removed: i64::from(removed),
            // Icerik CAS'a shim tarafinda ZATEN yazildi; ikinci kez
            // gondermeyiz. Atiflar asagidaki olay govdesinde tasinir.
            pre: None,
            post: None,
        };
        if let Err(e) = events.record_file_touch(record).await {
            tracing::warn!(%e, path = %path, "dosya dokunusu yazilamadi");
        }

        record_event(
            &events,
            agent_id,
            "file.touched",
            serde_json::json!({
                "path": path,
                "outside_workspace": touch.outside_workspace,
                "added": added,
                "removed": removed,
                "pre_ref": touch.pre_ref,
                "post_ref": touch.post_ref,
            }),
        )
        .await;

        println!("dokunus: {path} (+{added}/-{removed})");
    }
}

/// K3 denetim akisini olay-log'a dokerek bosaltir.
///
/// `tool_calls` satirlarini tur dongusu ZATEN yaziyor; burada yalnizca yetki
/// kararlari kayda gecer, boylece ayni cagri iki kez yazilmaz.
async fn drain_audit(events: Arc<EventWriter>, agent_id: i64, mut stream: AuditStream) {
    while let Some(event) = stream.recv().await {
        let AuditEvent::Capability(decision) = event else {
            continue;
        };
        if !decision.is_allowed() {
            println!("red: {} ({})", decision.target, decision.decision.label());
        }
        record_event(
            &events,
            agent_id,
            "capability.decision",
            serde_json::json!({
                "capability": decision.capability,
                "target": decision.target,
                "decision": decision.decision.label(),
                "reason": decision.decision.reason(),
                "approver": decision.approver,
                "ts": decision.ts,
            }),
        )
        .await;
    }
}

/// CLI yolunda surec bitmeden once yazicinin diskle isini bitirmesini bekler.
async fn flush_writer(writer: &WriterActor) {
    writer.flush().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bilinen_keyring_kimlikleri_cozulur() {
        assert!(matches!(
            kind_from_slug("openai"),
            Some(ProviderKind::OpenAI)
        ));
        assert!(matches!(
            kind_from_slug("anthropic"),
            Some(ProviderKind::Anthropic)
        ));
        assert!(kind_from_slug("bilinmeyen").is_none());
    }

    #[test]
    fn anthropic_messages_protokolu_secer() {
        let (backend, scheme) = wire_format(Some(&ProviderKind::Anthropic));
        assert_eq!(backend, ApiBackend::Messages);
        assert_eq!(scheme, AuthScheme::XApiKey);
    }

    #[test]
    fn diger_saglayicilar_openai_uyumlu() {
        let (backend, scheme) = wire_format(Some(&ProviderKind::OpenRouter));
        assert_eq!(backend, ApiBackend::ChatCompletions);
        assert_eq!(scheme, AuthScheme::Bearer);
        let (backend, scheme) = wire_format(None);
        assert_eq!(backend, ApiBackend::ChatCompletions);
        assert_eq!(scheme, AuthScheme::Bearer);
    }

    #[test]
    fn sampler_yapilandirmasi_model_ve_anahtari_tasir() {
        // Kimlik testte bile literal yazilmaz; cagiran uretir (I5).
        let model = format!("{}-{}", "model", 1);
        let provider = ProviderCreds {
            slug: "test".into(),
            label: "Test".into(),
            base_url: "http://127.0.0.1:1/v1".into(),
            api_key: "anahtar".into(),
            api_backend: ApiBackend::ChatCompletions,
            auth_scheme: AuthScheme::Bearer,
        };
        let config = sampler_config(&provider, &model);
        assert_eq!(config.model, model);
        assert_eq!(config.api_key.as_deref(), Some("anahtar"));
        assert_eq!(config.max_retries, Some(SAMPLER_MAX_RETRIES));
    }

    #[test]
    fn calisma_dizini_mutlaktir() {
        let workspace = resolve_workspace().expect("calisma dizini");
        assert!(workspace.is_absolute());
    }

    #[test]
    fn tanimsiz_model_hata_verir() {
        // Var olmayan veritabani + ayarlanmamis ortam degiskeni -> hata, panik degil.
        let db = std::env::temp_dir().join("omnitrix-run-testi-olmayan.sqlite");
        let _ = std::fs::remove_file(&db);
        unsafe { std::env::remove_var(MODEL_ENV) };
        assert!(resolve_model(&db).is_err());
    }
}
