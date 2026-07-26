//! AS2 kapisi — **interrupt granulaligi** (MASTER-PLAN Bolum 18 AS2; 8.3, 10.3, 7.5).
//!
//! Kapinin sozu: **yarim kalan bir tool cagrisi diskte asla yarim gorunmez.**
//! Yan etkiden once `write_journal`'a dusen crash-only niyet (`applied = 0`)
//! cagrinin idempotanslik sinifindan turemis, onceden cozulmus bir kader tasir:
//!
//!   * idempotent  -> `completed_idempotent` (tamamlanmis sayilir),
//!   * idempotent degil -> `cancelled_incomplete` (iptal isaretli).
//!
//! Surec hangi anda olurse olsun — `SIGKILL` dahil — yeniden acilista
//! `EventWriter::recover()` niyeti idempotent olarak geri uygular ve
//! `tool_calls` satiri terminal olur. Kesme canliyken gelirse ayni karar
//! `InterruptCoordinator::handle_interrupt` icinde verilir; ayrica baglam CAS'a
//! korunur, ceza hem trust skoruna hem baglama (system-reminder) yazilir ve
//! iptal iki uctan yapilir: sampler abort + `SubagentBackend::cancel`.
//!
//! ## I1 — girdi / beklenen cikti / esik
//!
//! | Kapi | GIRDI | BEKLENEN CIKTI | ESIK |
//! |------|-------|----------------|------|
//! | [`niyet_satiri_daima_terminaldir`] | 2 ucusta cagri (idempotent + idempotent degil), hicbiri bitirilmemis | `tool_calls`'ta 2 satir; ikisi de terminal durumda | Yarim satir **= 0** |
//! | [`cokme_sonrasi_idempotent_olmayan_cagri_iptal_isaretlenir`] | Cagri baslatilir, yazici cokme benzetimiyle dusurulur, yeni surecte `recover()` | Cagrinin yetkili durumu `cancelled_incomplete` | Yarim satir **= 0** |
//! | [`cokme_sonrasi_idempotent_cagri_tamamlanmis_isaretlenir`] | Ayni akis, cagri idempotent | Cagrinin yetkili durumu `completed_idempotent` | Yarim satir **= 0** |
//! | [`yarim_journal_niyeti_acilista_terminal_satira_doner`] | Elle yazilmis `applied = 0` ToolCall niyeti (journal ile satir arasinda cokme) | `recover()` -> `reapplied = 1`, satir terminal | `applied = 0` niyet **= 0**, yarim satir **= 0** |
//! | [`kill9_sonrasi_hicbir_tool_yarim_kalmaz`] | Ayri bir cocuk surec ucusta cagrilar acar ve `SIGKILL` yer | Yeniden acilista her `tool_calls` satiri terminal | Yarim satir **= 0**, yazilmis cagri **>= `MIN_TOOL_ROWS`** |
//! | [`kesme_yarim_toolu_cozer_baglami_korur_ceza_uygular`] | Ucusta idempotent olmayan cagri + `TaskStop` kesmesi | Cozum `cancelled_incomplete`; baglam CAS'tan geri okunur ve system-reminder icerir; trust delta < 0; sampler abort cagrilir; alt-ajan iptal edilir | Ucusta kalan cagri **= 0**, yarim satir **= 0** |
//! | [`idempotent_cagri_kesmede_tamamlanmis_isaretlenir`] | Ucusta idempotent cagri + `ContextWarning` kesmesi | Cozum `completed_idempotent` | Ucusta kalan cagri **= 0** |

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use omni_router::sampler::SamplerLayer;
use omni_scheduler::interrupt::{
    CancelKind, HalfToolCall, HalfToolEnvelope, HalfToolResolution, InterruptBus,
    InterruptContext, InterruptCoordinator, InterruptLevel, SamplerAbort, ToolIdempotency,
    ToolOutcome, is_terminal_tool_status,
};
use omni_scheduler::subagent_backend::{
    OmniSubagentBackend, SubagentBackendConfig, SubagentRunContext, SubagentRunOutput,
    SubagentRunner,
};
use omni_storage::cas::CasBlobStore;
use omni_storage::events::EventWriter;
use omni_tests::{ENV_CHILD_CAS, ENV_CHILD_DB, ENV_CHILD_MAX_MS, SEED_AGENT_ID, TestDb};
use rusqlite::Connection;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use xai_grok_sampler::{RequestId, RetryPolicy, SamplerConfig};
use xai_grok_tools::implementations::grok_build::task::backend::SubagentBackend;
use xai_grok_tools::implementations::grok_build::task::types::{
    SubagentOwner, SubagentRequest, SubagentRuntimeOverrides,
};

/// Yarim satir toleransi. Bu esik bilerek sifirdir (AS2).
const MAX_HALF_TOOL_ROWS: usize = 0;
/// `SIGKILL` turunun anlamli sayilmasi icin gereken en az cagri satiri.
const MIN_TOOL_ROWS: usize = 3;
/// Cocuk surec bu sureden fazla yasamaz (oksuz surec emniyeti).
const CHILD_MAX_MS: u64 = 5_000;
/// Cocugu oldurmeden once beklenen sure.
const KILL_DELAY_MS: u64 = 600;
/// Cocuk surec giris noktasinin adi; [`interrupt_worker_entrypoint`] ile ayni olmali.
const WORKER_TEST: &str = "interrupt_worker_entrypoint";

// ---------------------------------------------------------------------------
// Yardimcilar
// ---------------------------------------------------------------------------

/// `tool_calls` satirlarini `(id, status, args_json)` olarak okur.
fn tool_rows(conn: &Connection) -> Vec<(i64, String, Option<String>)> {
    let mut stmt = conn
        .prepare("SELECT id, status, args_json FROM tool_calls ORDER BY id ASC")
        .expect("tool_calls sorgusu hazirlanamadi");
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        })
        .expect("tool_calls okunamadi");
    rows.filter_map(|r| r.ok()).collect()
}

/// Terminal olmayan — yani "yarim" — satirlarin durumlari.
fn half_tool_rows(conn: &Connection) -> Vec<String> {
    tool_rows(conn)
        .into_iter()
        .map(|(_, status, _)| status)
        .filter(|status| !is_terminal_tool_status(status))
        .collect()
}

/// Bir `op_id` icin **yetkili** durum: o cagriya ait en son satirin durumu.
fn authoritative_status(conn: &Connection, op_id: &str) -> Option<String> {
    tool_rows(conn)
        .into_iter()
        .rfind(|(_, _, args)| envelope_of(args.as_deref()).is_some_and(|e| e.op_id == op_id))
        .map(|(_, status, _)| status)
}

fn envelope_of(args_json: Option<&str>) -> Option<HalfToolEnvelope> {
    serde_json::from_str::<HalfToolEnvelope>(args_json?).ok()
}

/// Migrations kosulmus bos depoda bir koordinator kurar.
async fn coordinator_on(db: &TestDb) -> InterruptCoordinator {
    let writer = EventWriter::open(db.db_path(), db.cas_path()).expect("EventWriter::open");
    InterruptCoordinator::new(InterruptBus::new(32)).with_event_writer(Arc::new(writer))
}

fn call(tool: &str, idempotency: ToolIdempotency) -> HalfToolCall {
    HalfToolCall {
        agent_row_id: SEED_AGENT_ID,
        agent_id: Uuid::new_v4(),
        tool: tool.to_string(),
        args_json: Some("{\"path\":\"src/lib.rs\"}".to_string()),
        idempotency,
        capability_ok: Some(true),
        request_id: None,
        subagent_id: None,
    }
}

/// `omni-router::sampler::SamplerLayer::cancel` -> [`SamplerAbort`] koprusu.
/// AS2'nin "iptal: `SamplerHandle` abort" yarisi bu kopruden gecer.
struct SamplerBridge {
    layer: SamplerLayer,
    seen: std::sync::Mutex<Vec<String>>,
}

impl SamplerAbort for SamplerBridge {
    fn abort_request(&self, request_id: &str) {
        // Gercek bagi kurar: bilinmeyen kimlik sessizce yok sayilir (I6).
        self.layer.cancel(RequestId::from(request_id));
        if let Ok(mut seen) = self.seen.lock() {
            seen.push(request_id.to_string());
        }
    }
}

/// Iptal edilene kadar bekleyen alt-ajan kosucusu.
struct BlockingRunner;

#[async_trait::async_trait]
impl SubagentRunner for BlockingRunner {
    async fn run(
        &self,
        ctx: SubagentRunContext,
        _progress: omni_scheduler::subagent_backend::SubagentProgressHandle,
    ) -> Result<SubagentRunOutput, String> {
        ctx.cancel.cancelled().await;
        Err("cancelled".to_string())
    }
}

fn subagent_request(id: &str, cancel: CancellationToken) -> SubagentRequest {
    SubagentRequest {
        id: id.to_string(),
        prompt: "long running work".to_string(),
        description: "as2 gate".to_string(),
        subagent_type: "explorer".to_string(),
        parent_session_id: "parent-as2".to_string(),
        parent_prompt_id: Some("prompt-as2".to_string()),
        resume_from: None,
        cwd: None,
        runtime_overrides: SubagentRuntimeOverrides::default(),
        run_in_background: true,
        surface_completion: false,
        await_to_completion: false,
        fork_context: false,
        owner: SubagentOwner::Task,
        cancel_token: cancel,
    }
}

// ---------------------------------------------------------------------------
// Bolum 1 — niyet satiri hicbir zaman yarim degildir
// ---------------------------------------------------------------------------

#[tokio::test]
async fn niyet_satiri_daima_terminaldir() {
    let db = TestDb::new();
    let coordinator = coordinator_on(&db).await;

    // GIRDI: iki ucusta cagri; hicbiri bitirilmiyor.
    let idem = coordinator
        .begin_tool(call("read_file", ToolIdempotency::Idempotent))
        .await
        .expect("idempotent niyet");
    let mutating = coordinator
        .begin_tool(call("apply_patch", ToolIdempotency::NonIdempotent))
        .await
        .expect("idempotent olmayan niyet");

    let conn = db.connect();
    let rows = tool_rows(&conn);

    // BEKLENEN: iki satir, ikisi de terminal.
    assert_eq!(rows.len(), 2, "her niyet bir satir dusurmeli");
    assert_eq!(
        authoritative_status(&conn, &idem.op_id).as_deref(),
        Some(HalfToolResolution::CompletedIdempotent.status())
    );
    assert_eq!(
        authoritative_status(&conn, &mutating.op_id).as_deref(),
        Some(HalfToolResolution::CancelledIncomplete.status())
    );

    // ESIK: yarim satir = 0.
    assert_eq!(
        half_tool_rows(&conn).len(),
        MAX_HALF_TOOL_ROWS,
        "ucusta cagri varken bile diskte yarim satir olamaz"
    );

    // Zarf makine-okunur: op_id ile eslesme kurulabiliyor.
    let envelope = envelope_of(rows[1].2.as_deref()).expect("zarf cozulmeli");
    assert_eq!(envelope.op_id, mutating.op_id);
    assert_eq!(envelope.idempotency, ToolIdempotency::NonIdempotent);
}

#[tokio::test]
async fn cokme_sonrasi_idempotent_olmayan_cagri_iptal_isaretlenir() {
    let db = TestDb::new();

    // GIRDI: cagri baslatilir, sonrasinda surec "coker" (koordinator + yazici duser).
    let op_id = {
        let coordinator = coordinator_on(&db).await;
        let ticket = coordinator
            .begin_tool(call("write_file", ToolIdempotency::NonIdempotent))
            .await
            .expect("niyet");
        ticket.op_id
    };

    // Yeniden acilis: yeni yazici + acilis kurtarmasi.
    let reopened = coordinator_on(&db).await;
    let report = reopened.recover().expect("acilis kurtarmasi");
    assert_eq!(report.failed, 0, "kurtarma hatasiz bitmeli");

    let conn = db.connect();
    // BEKLENEN: yetkili durum "iptal isaretli".
    assert_eq!(
        authoritative_status(&conn, &op_id).as_deref(),
        Some(HalfToolResolution::CancelledIncomplete.status()),
        "idempotent olmayan yarim cagri iptal isaretlenmeli"
    );
    // ESIK: yarim satir = 0.
    assert_eq!(half_tool_rows(&conn).len(), MAX_HALF_TOOL_ROWS);
    assert_eq!(omni_tests::unapplied_intents(&conn), 0);
}

#[tokio::test]
async fn cokme_sonrasi_idempotent_cagri_tamamlanmis_isaretlenir() {
    let db = TestDb::new();

    let op_id = {
        let coordinator = coordinator_on(&db).await;
        let ticket = coordinator
            .begin_tool(call("grep", ToolIdempotency::Idempotent))
            .await
            .expect("niyet");
        ticket.op_id
    };

    let reopened = coordinator_on(&db).await;
    reopened.recover().expect("acilis kurtarmasi");

    let conn = db.connect();
    // BEKLENEN: yetkili durum "idempotent tamamlandi".
    assert_eq!(
        authoritative_status(&conn, &op_id).as_deref(),
        Some(HalfToolResolution::CompletedIdempotent.status()),
        "idempotent yarim cagri tamamlanmis sayilmali"
    );
    assert_eq!(half_tool_rows(&conn).len(), MAX_HALF_TOOL_ROWS);
}

#[tokio::test]
async fn yarim_journal_niyeti_acilista_terminal_satira_doner() {
    let db = TestDb::new();
    // Sema hizalamasi ve sahne tablosu icin once yazici acilir.
    let coordinator = coordinator_on(&db).await;

    // GIRDI: journal ile satir arasinda cokmus bir ToolCall niyeti (applied = 0).
    let op_id = "op-as2-yarim";
    let payload = serde_json::json!({
        "op_id": op_id,
        "ts": "2026-01-01T00:00:00+00:00",
        "write": {
            "ToolCall": {
                "agent_id": SEED_AGENT_ID,
                "tool": "apply_patch",
                "args_json": serde_json::to_string(&serde_json::json!({
                    "op_id": op_id,
                    "phase": "intent",
                    "tool": "apply_patch",
                    "idempotency": "NonIdempotent",
                    "crash_resolution": "CancelledIncomplete",
                }))
                .expect("zarf"),
                "result_ref": null,
                "status": HalfToolResolution::CancelledIncomplete.status(),
                "capability_ok": null,
            }
        }
    })
    .to_string();

    {
        let conn = db.connect();
        omni_tests::insert_journal_entry(
            &conn,
            op_id,
            "omni_events",
            op_id,
            payload.as_bytes(),
            "SET",
            false,
        );
        assert_eq!(omni_tests::unapplied_intents(&conn), 1);
        assert_eq!(tool_rows(&conn).len(), 0, "cokme aninda satir yoktu");
    }

    // BEKLENEN: kurtarma niyeti tam olarak bir kez geri uygular.
    let report = coordinator.recover().expect("kurtarma");
    assert_eq!(report.replayed, 1, "niyet sahne tablosuna tasinmali");
    assert_eq!(report.reapplied, 1, "niyet asil tabloya yazilmali");
    assert_eq!(report.failed, 0);

    let conn = db.connect();
    let rows = tool_rows(&conn);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].1, HalfToolResolution::CancelledIncomplete.status());

    // ESIK: yarim satir = 0, bekleyen niyet = 0.
    assert_eq!(half_tool_rows(&conn).len(), MAX_HALF_TOOL_ROWS);
    assert_eq!(omni_tests::unapplied_intents(&conn), 0);

    // Idempotans: ikinci kurtarma yeni satir uretmez.
    let again = coordinator.recover().expect("ikinci kurtarma");
    assert_eq!(again.reapplied, 0);
    assert_eq!(tool_rows(&db.connect()).len(), 1);
}

// ---------------------------------------------------------------------------
// Bolum 2 — gercek SIGKILL
// ---------------------------------------------------------------------------

#[test]
fn kill9_sonrasi_hicbir_tool_yarim_kalmaz() {
    let db = TestDb::new();

    // GIRDI: ayri bir surec ucusta cagrilar acar ve SIGKILL yer.
    let child = omni_tests::spawn_self_test(
        WORKER_TEST,
        &[
            (ENV_CHILD_DB, db.db_path().display().to_string()),
            (ENV_CHILD_CAS, db.cas_path().display().to_string()),
            (ENV_CHILD_MAX_MS, CHILD_MAX_MS.to_string()),
        ],
    );
    std::thread::sleep(Duration::from_millis(KILL_DELAY_MS));
    let child_log = omni_tests::kill9_and_collect(child);

    // Yeniden acilis + kurtarma.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let report = runtime.block_on(async {
        let coordinator = coordinator_on(&db).await;
        coordinator.recover().expect("acilis kurtarmasi")
    });
    assert_eq!(report.failed, 0, "kurtarma hatasiz bitmeli: {child_log}");

    let conn = db.connect();
    let rows = tool_rows(&conn);

    // ESIK: yarim satir = 0; kaosun is uretmis olmasi icin en az MIN_TOOL_ROWS satir.
    assert_eq!(
        half_tool_rows(&conn).len(),
        MAX_HALF_TOOL_ROWS,
        "SIGKILL sonrasi yarim tool satiri kaldi; cocuk gunlugu:\n{child_log}"
    );
    assert!(
        rows.len() >= MIN_TOOL_ROWS,
        "cocuk yeterince cagri acmadi ({} satir); gunluk:\n{child_log}",
        rows.len()
    );
    assert_eq!(omni_tests::unapplied_intents(&conn), 0);

    // Her satirin kaderi iki cozumden biri olmali; ucuncu hal yok.
    for (_, status, _) in rows {
        assert!(
            status == HalfToolResolution::CompletedIdempotent.status()
                || status == HalfToolResolution::CancelledIncomplete.status(),
            "beklenmeyen durum: {status}"
        );
    }
}

/// Cocuk surec giris noktasi. Env yoksa hicbir sey yapmaz.
#[test]
#[ignore = "cocuk surec giris noktasi; ebeveyn SIGKILL ile oldurur"]
fn interrupt_worker_entrypoint() {
    let Ok(db_path) = std::env::var(ENV_CHILD_DB) else {
        return;
    };
    let cas_path = std::env::var(ENV_CHILD_CAS).unwrap_or_default();
    let max_ms: u64 = std::env::var(ENV_CHILD_MAX_MS)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(CHILD_MAX_MS);

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    runtime.block_on(async move {
        let writer = EventWriter::open(Path::new(&db_path), Path::new(&cas_path))
            .expect("EventWriter::open");
        let coordinator =
            InterruptCoordinator::new(InterruptBus::new(16)).with_event_writer(Arc::new(writer));
        if let Ok(report) = coordinator.recover() {
            eprintln!("cocuk: kurtarma reapplied={}", report.reapplied);
        }

        let deadline = std::time::Instant::now() + Duration::from_millis(max_ms);
        let mut n = 0u32;
        while std::time::Instant::now() < deadline {
            // Cagrilar bilerek bitirilmez: her biri ucusta kalir.
            let idempotency = if n.is_multiple_of(2) {
                ToolIdempotency::Idempotent
            } else {
                ToolIdempotency::NonIdempotent
            };
            match coordinator.begin_tool(call("chaos_tool", idempotency)).await {
                Ok(ticket) => eprintln!("cocuk: niyet {}", ticket.op_id),
                Err(e) => eprintln!("cocuk: niyet basarisiz {e}"),
            }
            n += 1;
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    });
}

// ---------------------------------------------------------------------------
// Bolum 3 — canli kesme: cozum + baglam + ceza + iptal
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kesme_yarim_toolu_cozer_baglami_korur_ceza_uygular() {
    let db = TestDb::new();
    let writer = EventWriter::open(db.db_path(), db.cas_path()).expect("EventWriter::open");

    let bridge = Arc::new(SamplerBridge {
        layer: SamplerLayer::spawn(SamplerConfig::default(), RetryPolicy::default()),
        seen: std::sync::Mutex::new(Vec::new()),
    });
    let backend = Arc::new(OmniSubagentBackend::with_runner(
        SubagentBackendConfig::default(),
        Arc::new(BlockingRunner),
    ));

    // Arka planda gercek bir alt-ajan; iptal edilene kadar bekler.
    let cancel = CancellationToken::new();
    let spawned = {
        let backend = Arc::clone(&backend);
        let request = subagent_request("child-as2", cancel.clone());
        tokio::spawn(async move { backend.spawn(request).await })
    };
    // Kaydin olusmasini bekle.
    for _ in 0..100 {
        if backend.live_children_of("parent-as2").is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        } else {
            break;
        }
    }

    let coordinator = InterruptCoordinator::new(InterruptBus::new(32))
        .with_event_writer(Arc::new(writer))
        .with_sampler(Arc::clone(&bridge) as Arc<dyn SamplerAbort>)
        .with_subagent_backend(Arc::clone(&backend) as Arc<dyn SubagentBackend>);

    // GIRDI: ucusta idempotent olmayan cagri + TaskStop kesmesi.
    let mut request = call("apply_patch", ToolIdempotency::NonIdempotent);
    request.request_id = Some("req-as2".to_string());
    request.subagent_id = Some("child-as2".to_string());
    let agent_id = request.agent_id;
    let ticket = coordinator.begin_tool(request).await.expect("niyet");
    assert_eq!(coordinator.inflight_count(), 1);

    let mut rx = coordinator.bus().subscribe();
    let interrupt = omni_scheduler::interrupt::Interrupt {
        id: Uuid::new_v4(),
        target_agent_id: agent_id,
        level: InterruptLevel::TaskStop,
        reason: "operator stopped the task".to_string(),
        issued_at: chrono::Utc::now(),
        issued_by: None,
        is_broadcast: false,
    };

    let outcome = coordinator
        .handle_interrupt(
            interrupt,
            InterruptContext {
                agent_row_id: SEED_AGENT_ID,
                context: b"turn 1: read src/lib.rs\nturn 2: patch pending".to_vec(),
                ticket: Some(ticket.clone()),
                request_id: None,
                subagent_id: None,
            },
        )
        .await
        .expect("kesme islenmeli");

    // BEKLENEN 1 — yarim tool cozuldu, iptal isaretli.
    assert_eq!(
        outcome.resolution,
        Some(HalfToolResolution::CancelledIncomplete)
    );
    // ESIK: ucusta kalan cagri = 0.
    assert_eq!(coordinator.inflight_count(), 0);

    // BEKLENEN 2 — iptal iki uctan: sampler abort + SubagentBackend::cancel.
    assert!(outcome.sampler_aborted, "sampler abort cagrilmali");
    assert_eq!(
        bridge.seen.lock().map(|s| s.len()).unwrap_or(0),
        1,
        "abort tam olarak bir kez, dogru kimlikle"
    );
    assert_eq!(outcome.subagent_cancel, CancelKind::Cancelled);
    assert!(cancel.is_cancelled() || spawned.await.is_ok());

    // BEKLENEN 3 — baglam CAS'a korunmus ve reminder iceriyor.
    let context_ref = outcome.context_ref.clone().expect("baglam atfi");
    let cas = CasBlobStore::new(db.cas_path()).expect("CAS acilmali");
    let blob = cas
        .load(&context_ref)
        .expect("CAS okunmali")
        .expect("blob bulunmali");
    let text = String::from_utf8_lossy(&blob);
    assert!(text.contains("turn 1: read src/lib.rs"), "baglam korunmali");
    assert!(text.contains("<system-reminder>"), "reminder eklenmeli");
    assert!(text.contains("behavior: "), "davranis satiri");
    assert!(text.contains("guidance: "), "yonlendirme satiri");
    assert!(text.contains("cancelled_incomplete"), "cozum bildirilmeli");

    // BEKLENEN 4 — ceza hem trust skoruna hem baglama.
    assert!(outcome.penalty.trust_delta < 0.0);
    assert_eq!(
        coordinator.penalties().for_agent(&agent_id).len(),
        1,
        "penalty_log kaydi dusmeli"
    );
    assert!(outcome.penalty.system_reminder.contains("guidance: "));

    // Kesme otobuse de dusmus olmali (UI/hiyerarsi bu akisi dinler).
    let seen = rx.try_recv().expect("kesme yayinlanmali");
    assert_eq!(seen.level, InterruptLevel::TaskStop);

    // ESIK: diskte yarim satir = 0; yetkili durum iptal.
    let conn = db.connect();
    assert_eq!(half_tool_rows(&conn).len(), MAX_HALF_TOOL_ROWS);
    assert_eq!(
        authoritative_status(&conn, &ticket.op_id).as_deref(),
        Some(HalfToolResolution::CancelledIncomplete.status())
    );

    // Baglam atfi olay-loga da dusmeli.
    let preserved: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM agent_events WHERE kind = 'interrupt.context_preserved'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    assert_eq!(preserved, 1);
}

#[tokio::test]
async fn idempotent_cagri_kesmede_tamamlanmis_isaretlenir() {
    let db = TestDb::new();
    let coordinator = coordinator_on(&db).await;

    // GIRDI: ucusta idempotent cagri + ContextWarning kesmesi.
    let ticket = coordinator
        .begin_tool(call("read_file", ToolIdempotency::Idempotent))
        .await
        .expect("niyet");

    let interrupt = omni_scheduler::interrupt::Interrupt {
        id: Uuid::new_v4(),
        target_agent_id: ticket.agent_id,
        level: InterruptLevel::ContextWarning,
        reason: "context window pressure".to_string(),
        issued_at: chrono::Utc::now(),
        issued_by: None,
        is_broadcast: false,
    };

    let outcome = coordinator
        .handle_interrupt(
            interrupt,
            InterruptContext {
                agent_row_id: SEED_AGENT_ID,
                context: b"transcript".to_vec(),
                ticket: Some(ticket.clone()),
                ..InterruptContext::default()
            },
        )
        .await
        .expect("kesme islenmeli");

    // BEKLENEN: idempotent cagri tamamlanmis isaretlenir; sampler/backend yok,
    // bu yuzden iptal istekleri "baglanmadi" ile doner (I6: panik yok).
    assert_eq!(
        outcome.resolution,
        Some(HalfToolResolution::CompletedIdempotent)
    );
    assert!(!outcome.sampler_aborted);
    assert_eq!(outcome.subagent_cancel, CancelKind::NotRequested);

    // ESIK: ucusta kalan cagri = 0, yarim satir = 0.
    assert_eq!(coordinator.inflight_count(), 0);
    let conn = db.connect();
    assert_eq!(half_tool_rows(&conn).len(), MAX_HALF_TOOL_ROWS);
    assert_eq!(
        authoritative_status(&conn, &ticket.op_id).as_deref(),
        Some(HalfToolResolution::CompletedIdempotent.status())
    );
}

#[tokio::test]
async fn kesintisiz_biten_cagri_bileti_dusurur() {
    let db = TestDb::new();
    let coordinator = coordinator_on(&db).await;

    let ticket = coordinator
        .begin_tool(call("read_file", ToolIdempotency::Idempotent))
        .await
        .expect("niyet");
    coordinator
        .finish_tool(&ticket, ToolOutcome::Ok, Some(b"file body".to_vec()))
        .await
        .expect("sonuc satiri");

    // BEKLENEN: yetkili durum "ok"; ucusta kayit kalmaz.
    assert_eq!(coordinator.inflight_count(), 0);
    let conn = db.connect();
    assert_eq!(
        authoritative_status(&conn, &ticket.op_id).as_deref(),
        Some("ok")
    );
    assert_eq!(half_tool_rows(&conn).len(), MAX_HALF_TOOL_ROWS);
}
