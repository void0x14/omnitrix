//! K7 kapisi (MASTER-PLAN Bolum 6.2 / Faz 2): **iki yuz tek akis**.
//!
//! "TUI komutu gonderir, WebUI akisinda AYNI `StateEvent` gorunur;
//! snapshot'lar bit-es."
//!
//! **Girdi:** `omni-control` router'i gercek bir TCP portunda (127.0.0.1:0) ayakta;
//! iki istemci baglanir — WebSocket ucundaki (`/v1/ws`) **TUI**, SSE ucundaki
//! (`/v1/events`) **WebUI**. Cekirdek yerine deterministik bir `ControlPlane`
//! uygulamasi durur: sabit zaman damgali `SystemSnapshot` ve komut basina sabit
//! olay dizisi uretir (tel formati disinda hicbir kaynak degiskeni yok).
//!
//! **Beklenen cikti:**
//! 1. Kimliksiz / gecersiz tokenli istek `401` alir ve cekirdege **hic** ulasmaz
//!    (`dispatch` cagrilmaz) — Faz 2 kapisi, Bolum 13 / K9.
//! 2. Iki istemcinin ilk yuklemede aldigi `SystemSnapshot` **bit-es**tir.
//! 3. TUI'nin WS uzerinden gonderdigi komutun urettigi olaylar WebUI akisinda da
//!    gorunur; ters yon (WebUI'nin `POST /v1/command`'i) TUI akisinda gorunur.
//! 4. Iki akistaki olay dizisi **ayni sirada ve bit-es**tir.
//!
//! **Esikler (I1) — hepsi acik:**
//! - Komut basina uretilen olay sayisi: **tam 2** (`EVENTS_PER_COMMAND`).
//! - Toplam beklenen olay: 2 komut x 2 = **4** (`EXPECTED_EVENTS`).
//! - Her tekil okuma icin zaman asimi: **5 sn** (`READ_TIMEOUT`).
//! - Karsilastirma toleransi: **yok** — kanonik serde serilestirmesinin
//!   byte dizileri tam esit olmalidir.
//!
//! Byte karsilastirmasi ham tel govdesi uzerinden degil, **kanonik** serde
//! serilestirmesi uzerinden yapilir: SSE tarafi `Event::json_data` ile dogrudan
//! tipi serilestirirken WS tarafi once `serde_json::Value`'ya cevirir; iki
//! tasimanin anahtar siralamasi ayni olmak zorunda degildir, tasidiklari **deger**
//! ayni olmak zorundadir. Bu yuzden her iki yuk once `omni-proto` tipine cozulur,
//! sonra ayni serilestirici ile byte'a dokulur ve byte'lar karsilastirilir.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use eventsource_stream::{Event as SseEvent, EventStreamError, Eventsource};
use futures::future::BoxFuture;
use futures::{SinkExt, Stream, StreamExt};
use omni_control::api::{PATH_COMMAND, PATH_EVENTS, PATH_SNAPSHOT, PATH_WS, router};
use omni_control::auth::{AuthState, TokenVerifier, hash_token};
use omni_control::stream::{Broadcaster, FRAME_SNAPSHOT, FRAME_STATE};
use omni_control::{ControlError, ControlPlane};
use omni_proto::{
    AgentState, AgentTier, AgentView, Command, NoticeLevel, NoticeView, ProviderHealthState,
    ProviderView, ResourceGauge, StateEvent, SystemSnapshot, TaskView, Timestamp,
};
use serde_json::Value;
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

// ---------------------------------------------------------------------------
// Esikler ve sabitler (I1)
// ---------------------------------------------------------------------------

/// Testin kullandigi ham token. Sunucu tarafinda yalnizca argon2 PHC'si durur.
const TOKEN: &str = "ui-parity-token";

/// Kayitli token'in mantiksal kimligi.
const IDENTITY: &str = "operator";

/// Kabul edilmemesi gereken token.
const WRONG_TOKEN: &str = "yanlis-token";

/// Tek bir cerceve okumasi icin ust sinir. Asilirsa test **basarisiz** olur.
const READ_TIMEOUT: Duration = Duration::from_secs(5);

/// Deterministik cekirdegin komut basina urettigi olay sayisi.
const EVENTS_PER_COMMAND: usize = 2;

/// Testte gonderilen komut sayisi (biri WS'ten, biri HTTP'den).
const COMMAND_COUNT: usize = 2;

/// Her iki istemcinin de gormesi gereken toplam olay sayisi.
const EXPECTED_EVENTS: usize = EVENTS_PER_COMMAND * COMMAND_COUNT;

/// Anlik goruntudeki sabit gorev/ajan kimlikleri.
const SEED_TASK_ID: i64 = 1;
const SEED_AGENT_ID: i64 = 7;
const SPAWNED_TASK_ID: i64 = 2;

// ---------------------------------------------------------------------------
// Deterministik cekirdek (ControlPlane fixture)
// ---------------------------------------------------------------------------

/// Sabit zaman damgasi: bit-es karsilastirmanin anlamli olmasi icin akista
/// **hicbir** degisken deger bulunmamalidir.
fn fixed_ts() -> Timestamp {
    chrono::DateTime::from_timestamp(1_700_000_000, 0).expect("sabit zaman damgasi gecerli olmali")
}

/// Anlik goruntudeki ornek ajan.
fn seed_agent() -> AgentView {
    AgentView {
        id: SEED_AGENT_ID,
        persona: "planner".to_string(),
        tier: AgentTier::Active,
        task_id: SEED_TASK_ID,
        parent_id: None,
        state: AgentState::Planning,
        rss_kb: 4096,
        tokens_in: 120,
        tokens_out: 45,
        cost: 0.0,
        trust: 1.0,
        depth: 0,
        last_event_seq: 3,
    }
}

/// Anlik goruntudeki ornek gorev.
fn seed_task() -> TaskView {
    TaskView {
        id: SEED_TASK_ID,
        parent_id: None,
        root_id: SEED_TASK_ID,
        title: "kok gorev".to_string(),
        mode: "user_driven".to_string(),
        status: "running".to_string(),
        depth: 0,
        budget_allocated: None,
        budget_spent: None,
        duration_target: None,
        created_at: fixed_ts(),
        closed_at: None,
    }
}

/// Anlik goruntudeki ornek saglayici. Model adi/fiyati koda gomulmez; burada
/// yalnizca config'ten gelmis gibi davranan bir yer tutucu tasinir (I5).
fn seed_provider() -> ProviderView {
    ProviderView {
        id: 1,
        name: "primary".to_string(),
        kind: "test_kind".to_string(),
        base_url: "http://127.0.0.1:1".to_string(),
        health: ProviderHealthState::Healthy,
        health_model: None,
        latency_ms: Some(12),
        checked_at: Some(fixed_ts()),
        detail: None,
        active_keys: 1,
        models: Vec::new(),
    }
}

/// Iki yuzun de ilk yuklemede alacagi kanonik goruntu.
fn seed_snapshot() -> SystemSnapshot {
    let ts = fixed_ts();
    let mut resource = ResourceGauge::at(ts);
    resource.rss_kb = 8192;
    resource.active = 1;
    resource.existing = 1;
    SystemSnapshot {
        agents: vec![seed_agent()],
        tasks: vec![seed_task()],
        providers: vec![seed_provider()],
        resource,
        ts,
    }
}

/// Komut -> olay esleme. Deterministiktir ve komut basina daima
/// [`EVENTS_PER_COMMAND`] olay uretir.
fn events_for(command: &Command) -> Vec<StateEvent> {
    let ts = fixed_ts();
    let kind = command.kind();
    match command {
        Command::SpawnTask {
            title,
            mode,
            parent_id,
            ..
        } => {
            let task = TaskView {
                id: SPAWNED_TASK_ID,
                parent_id: *parent_id,
                root_id: parent_id.unwrap_or(SPAWNED_TASK_ID),
                title: title.clone(),
                mode: mode.clone(),
                status: "queued".to_string(),
                depth: 0,
                budget_allocated: None,
                budget_spent: None,
                duration_target: None,
                created_at: ts,
                closed_at: None,
            };
            vec![
                StateEvent::TaskUpserted(task),
                StateEvent::Notice(
                    NoticeView::new(NoticeLevel::Info, "command_applied", kind, ts)
                        .with_task(SPAWNED_TASK_ID),
                ),
            ]
        }
        Command::WriteToAgent { agent_id, content } => {
            let mut agent = seed_agent();
            agent.id = *agent_id;
            agent.state = AgentState::RunningTool;
            agent.last_event_seq += 1;
            vec![
                StateEvent::AgentUpserted(agent),
                StateEvent::Notice(
                    NoticeView::new(NoticeLevel::Info, "command_applied", content.clone(), ts)
                        .with_agent(*agent_id),
                ),
            ]
        }
        other => vec![
            StateEvent::Notice(NoticeView::new(
                NoticeLevel::Warn,
                "command_ignored",
                other.kind(),
                ts,
            )),
            StateEvent::ResourceTick(ResourceGauge::at(ts)),
        ],
    }
}

/// Cekirdek yerine gecen kontrol duzlemi durumu.
#[derive(Clone)]
struct ParityCore {
    verifier: Arc<TokenVerifier>,
    events: Broadcaster,
    snapshot: Arc<SystemSnapshot>,
    /// Cekirdege **ulasan** komutlarin adlari. 401 testi bunun bos kalmasini bekler.
    dispatched: Arc<Mutex<Vec<String>>>,
}

impl ParityCore {
    fn new() -> Self {
        let phc = hash_token(TOKEN).expect("argon2 hash uretilmeli");
        Self {
            verifier: Arc::new(TokenVerifier::from_pairs([(IDENTITY, phc)])),
            events: Broadcaster::new(64),
            snapshot: Arc::new(seed_snapshot()),
            dispatched: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Cekirdege ulasmis komut adlari.
    fn dispatched(&self) -> Vec<String> {
        self.dispatched
            .lock()
            .expect("komut kaydi kilidi alinmali")
            .clone()
    }
}

impl AuthState for ParityCore {
    fn verifier(&self) -> &TokenVerifier {
        &self.verifier
    }
}

impl ControlPlane for ParityCore {
    fn events(&self) -> &Broadcaster {
        &self.events
    }

    fn snapshot(&self) -> BoxFuture<'_, Result<SystemSnapshot, ControlError>> {
        let snapshot = self.snapshot.as_ref().clone();
        Box::pin(async move { Ok(snapshot) })
    }

    fn dispatch(&self, command: Command) -> BoxFuture<'_, Result<(), ControlError>> {
        Box::pin(async move {
            let mut log = self
                .dispatched
                .lock()
                .map_err(|err| ControlError::Command(err.to_string()))?;
            log.push(command.kind().to_string());
            drop(log);

            for event in events_for(&command) {
                self.events.publish(event);
            }
            Ok(())
        })
    }
}

// ---------------------------------------------------------------------------
// Sunucu ve istemci yardimcilari
// ---------------------------------------------------------------------------

/// Router'i gercek bir portta ayaga kaldirir ve adresini dondurur.
async fn serve(core: ParityCore) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("gecici port baglanmali");
    let addr = listener.local_addr().expect("yerel adres okunmali");
    let app = router(core);
    tokio::spawn(async move {
        if let Err(err) = axum::serve(listener, app).await {
            eprintln!("kontrol duzlemi sunucusu durdu: {err}");
        }
    });
    addr
}

/// `http://<addr><path>` uretir.
fn url(addr: SocketAddr, path: &str) -> String {
    format!("http://{addr}{path}")
}

/// SSE (WebUI) akisi acar. Token verilmezse kimliksiz istek gonderilir.
async fn open_sse(addr: SocketAddr, token: Option<&str>) -> reqwest::Response {
    let client = reqwest::Client::new();
    let mut request = client.get(url(addr, PATH_EVENTS));
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    request.send().await.expect("SSE istegi gonderilmeli")
}

/// Kimlik dogrulanmis WebSocket (TUI) baglantisi acar.
type WsClient = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// WS baglantisi kurar; token verilmezse kimliksiz el sikismasi denenir.
async fn connect_ws(
    addr: SocketAddr,
    token: Option<&str>,
) -> Result<WsClient, tokio_tungstenite::tungstenite::Error> {
    let mut request = format!("ws://{addr}{PATH_WS}")
        .into_client_request()
        .expect("WS istegi kurulmali");
    if let Some(token) = token {
        let value = HeaderValue::from_str(&format!("Bearer {token}"))
            .expect("basslik degeri gecerli olmali");
        request.headers_mut().insert(AUTHORIZATION, value);
    }
    connect_async(request).await.map(|(socket, _resp)| socket)
}

/// `{ "kind": ..., "payload": ... }` cercevesini parcalar.
fn frame_parts(raw: &str) -> (String, Value) {
    let value: Value = serde_json::from_str(raw).expect("WS cercevesi JSON olmali");
    let kind = value
        .get("kind")
        .and_then(Value::as_str)
        .expect("cerceve 'kind' tasimali")
        .to_string();
    let payload = value
        .get("payload")
        .cloned()
        .expect("cerceve 'payload' tasimali");
    (kind, payload)
}

/// WS'ten bir sonraki metin cercevesini okur (ping/pong atlanir).
async fn next_ws_frame(ws: &mut WsClient) -> (String, Value) {
    loop {
        let message = timeout(READ_TIMEOUT, ws.next())
            .await
            .expect("WS cercevesi zaman asimina ugramamali")
            .expect("WS akisi kapanmamali")
            .expect("WS cercevesi hatasiz olmali");
        match message {
            Message::Text(text) => return frame_parts(text.as_str()),
            Message::Close(_) => panic!("WS oturumu beklenmedik sekilde kapandi"),
            _ => continue,
        }
    }
}

/// SSE akisindan bir sonraki olayi okur.
async fn next_sse_frame<S>(sse: &mut S) -> (String, Value)
where
    S: Stream<Item = Result<SseEvent, EventStreamError<reqwest::Error>>> + Unpin,
{
    let event = timeout(READ_TIMEOUT, sse.next())
        .await
        .expect("SSE olayi zaman asimina ugramamali")
        .expect("SSE akisi kapanmamali")
        .expect("SSE olayi hatasiz olmali");
    let payload: Value = serde_json::from_str(&event.data).expect("SSE yuku JSON olmali");
    (event.event, payload)
}

// ---------------------------------------------------------------------------
// Kanonik byte karsilastirmasi
// ---------------------------------------------------------------------------

/// Tel uzerindeki anlik goruntu yukunu kanonik byte'lara dokerr.
fn snapshot_bytes(payload: &Value) -> Vec<u8> {
    let snapshot: SystemSnapshot =
        serde_json::from_value(payload.clone()).expect("yuk SystemSnapshot'a cozulmeli");
    serde_json::to_vec(&snapshot).expect("SystemSnapshot serilestirilmeli")
}

/// Tel uzerindeki olay yukunu kanonik byte'lara doker.
fn event_bytes(payload: &Value) -> Vec<u8> {
    let event: StateEvent =
        serde_json::from_value(payload.clone()).expect("yuk StateEvent'e cozulmeli");
    serde_json::to_vec(&event).expect("StateEvent serilestirilmeli")
}

/// Hata mesajlarinda okunabilirlik icin byte dizisini metne cevirir.
fn readable(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Byte dizilerinin okunabilir izdusumu. Once bunun uzerinden karsilastirmak
/// hata ciktisini insan tarafindan okunur kilar; asil kapi yine byte esitligidir.
fn readable_all(frames: &[Vec<u8>]) -> Vec<String> {
    frames.iter().map(|bytes| readable(bytes)).collect()
}

/// Iki akisin ayni olaylari tasidigini once metin, sonra byte duzeyinde dogrular.
fn assert_streams_identical(tui: &[Vec<u8>], webui: &[Vec<u8>], context: &str) {
    assert_eq!(
        readable_all(tui),
        readable_all(webui),
        "{context} (okunabilir izdusum)"
    );
    assert_eq!(tui, webui, "{context} (byte esitligi)");
}

// ---------------------------------------------------------------------------
// 1. KAPI — kimliksiz istek 401
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unauthenticated_requests_are_rejected_with_401() {
    let core = ParityCore::new();
    let addr = serve(core.clone()).await;
    let client = reqwest::Client::new();

    // Okuma ucu: kimliksiz.
    let snapshot = client
        .get(url(addr, PATH_SNAPSHOT))
        .send()
        .await
        .expect("snapshot istegi gonderilmeli");
    assert_eq!(
        snapshot.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "kimliksiz snapshot istegi 401 almali"
    );
    let body: Value = snapshot.json().await.expect("401 govdesi JSON olmali");
    assert_eq!(
        body["error"], "unauthorized",
        "401 govdesi ayrinti sizdirmaz"
    );

    // Akis ucu: kimliksiz.
    let events = open_sse(addr, None).await;
    assert_eq!(
        events.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "kimliksiz SSE istegi 401 almali"
    );

    // Yazma ucu: kimliksiz.
    let command = Command::SpawnTask {
        title: "kimliksiz komut".to_string(),
        mode: "user_driven".to_string(),
        parent_id: None,
        persona: None,
        duration_target: None,
        budget: None,
    };
    let posted = client
        .post(url(addr, PATH_COMMAND))
        .json(&command)
        .send()
        .await
        .expect("komut istegi gonderilmeli");
    assert_eq!(
        posted.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "kimliksiz komut 401 almali"
    );

    // WS el sikismasi: kimliksiz.
    let ws = connect_ws(addr, None).await;
    match ws {
        Ok(_) => panic!("kimliksiz WS el sikismasi kabul edilmemeliydi"),
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
            // `http` surumleri arasinda tip esitligine bel baglamamak icin ham kod.
            assert_eq!(
                response.status().as_u16(),
                401,
                "kimliksiz WS el sikismasi 401 almali"
            );
        }
        Err(err) => panic!("beklenmedik WS hatasi: {err}"),
    }

    // Esik: cekirdege ulasan komut sayisi TAM 0.
    assert!(
        core.dispatched().is_empty(),
        "kimliksiz istekler cekirdege ulasmamali, ulasan: {:?}",
        core.dispatched()
    );
}

#[tokio::test]
async fn wrong_token_is_rejected_with_401() {
    let core = ParityCore::new();
    let addr = serve(core.clone()).await;
    let client = reqwest::Client::new();

    let snapshot = client
        .get(url(addr, PATH_SNAPSHOT))
        .bearer_auth(WRONG_TOKEN)
        .send()
        .await
        .expect("snapshot istegi gonderilmeli");
    assert_eq!(snapshot.status(), reqwest::StatusCode::UNAUTHORIZED);

    let events = open_sse(addr, Some(WRONG_TOKEN)).await;
    assert_eq!(events.status(), reqwest::StatusCode::UNAUTHORIZED);

    assert!(
        core.dispatched().is_empty(),
        "gecersiz token cekirdege ulasmamali"
    );
}

// ---------------------------------------------------------------------------
// 2. KAPI — ilk yukleme snapshot'lari bit-es
// ---------------------------------------------------------------------------

#[tokio::test]
async fn both_faces_receive_byte_identical_snapshots() {
    let core = ParityCore::new();
    let addr = serve(core).await;

    // WebUI: SSE akisi.
    let response = open_sse(addr, Some(TOKEN)).await;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let mut webui = Box::pin(response.bytes_stream().eventsource());
    let (sse_kind, sse_payload) = next_sse_frame(&mut webui).await;
    assert_eq!(
        sse_kind, FRAME_SNAPSHOT,
        "SSE ilk cercevesi snapshot olmali"
    );

    // TUI: WS akisi.
    let mut tui = connect_ws(addr, Some(TOKEN))
        .await
        .expect("gecerli token ile WS baglanmali");
    let (ws_kind, ws_payload) = next_ws_frame(&mut tui).await;
    assert_eq!(ws_kind, FRAME_SNAPSHOT, "WS ilk cercevesi snapshot olmali");

    // REST: tek seferlik okuma ucu.
    let rest_payload: Value = reqwest::Client::new()
        .get(url(addr, PATH_SNAPSHOT))
        .bearer_auth(TOKEN)
        .send()
        .await
        .expect("snapshot istegi gonderilmeli")
        .json()
        .await
        .expect("snapshot govdesi JSON olmali");

    let from_sse = snapshot_bytes(&sse_payload);
    let from_ws = snapshot_bytes(&ws_payload);
    let from_rest = snapshot_bytes(&rest_payload);

    assert_eq!(
        from_sse,
        from_ws,
        "TUI ve WebUI snapshot'lari bit-es olmali\nWebUI: {}\nTUI  : {}",
        readable(&from_sse),
        readable(&from_ws)
    );
    assert_eq!(
        from_sse,
        from_rest,
        "SSE ve REST snapshot'lari bit-es olmali\nSSE : {}\nREST: {}",
        readable(&from_sse),
        readable(&from_rest)
    );

    // Icerik gercekten dolu mu (bos govde ile "es" olmak sayilmaz)?
    let decoded: SystemSnapshot =
        serde_json::from_slice(&from_sse).expect("snapshot geri cozulmeli");
    assert_eq!(decoded.agents.len(), 1, "snapshot ajan tasimali");
    assert_eq!(decoded.tasks.len(), 1, "snapshot gorev tasimali");
    assert_eq!(decoded.active_count(), 1, "aktif ajan sayisi korunmali");
}

// ---------------------------------------------------------------------------
// 3 + 4. KAPI — komut bir yuzden gider, olay HER IKI akista gorunur
// ---------------------------------------------------------------------------

#[tokio::test]
async fn command_from_one_face_appears_in_both_streams() {
    let core = ParityCore::new();
    let addr = serve(core.clone()).await;

    // WebUI (SSE) once baglanir; abonelik snapshot'tan ONCE acildigi icin
    // snapshot'i okuduktan sonra hicbir olay kacmaz.
    let response = open_sse(addr, Some(TOKEN)).await;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let mut webui = Box::pin(response.bytes_stream().eventsource());
    let (kind, webui_snapshot) = next_sse_frame(&mut webui).await;
    assert_eq!(kind, FRAME_SNAPSHOT);

    // TUI (WS) baglanir.
    let mut tui = connect_ws(addr, Some(TOKEN))
        .await
        .expect("gecerli token ile WS baglanmali");
    let (kind, tui_snapshot) = next_ws_frame(&mut tui).await;
    assert_eq!(kind, FRAME_SNAPSHOT);
    assert_eq!(
        snapshot_bytes(&webui_snapshot),
        snapshot_bytes(&tui_snapshot),
        "iki yuz ayni ilk yuklemeyi almali"
    );

    // --- Yon 1: komut TUI'den (WS yukari yonu) gider. ---
    let from_tui = Command::SpawnTask {
        title: "dikey dilim".to_string(),
        mode: "user_driven".to_string(),
        parent_id: None,
        persona: Some("planner".to_string()),
        duration_target: None,
        budget: None,
    };
    tui.send(Message::Text(
        serde_json::to_string(&from_tui)
            .expect("komut serilestirilmeli")
            .into(),
    ))
    .await
    .expect("komut WS uzerinden gonderilmeli");

    let tui_first = read_events(&mut tui, EVENTS_PER_COMMAND).await;
    let webui_first = read_sse_events(&mut webui, EVENTS_PER_COMMAND).await;
    assert_streams_identical(
        &tui_first,
        &webui_first,
        "TUI komutunun olaylari WebUI akisinda da ayni sirada gorunmeli",
    );

    // --- Yon 2: komut WebUI'den (POST /v1/command) gider. ---
    let from_webui = Command::WriteToAgent {
        agent_id: SEED_AGENT_ID,
        content: "devam et".to_string(),
    };
    let accepted = reqwest::Client::new()
        .post(url(addr, PATH_COMMAND))
        .bearer_auth(TOKEN)
        .json(&from_webui)
        .send()
        .await
        .expect("komut istegi gonderilmeli");
    assert_eq!(
        accepted.status(),
        reqwest::StatusCode::ACCEPTED,
        "gecerli komut 202 almali (sonuc akistan okunur)"
    );

    let tui_second = read_events(&mut tui, EVENTS_PER_COMMAND).await;
    let webui_second = read_sse_events(&mut webui, EVENTS_PER_COMMAND).await;
    assert_streams_identical(
        &tui_second,
        &webui_second,
        "WebUI komutunun olaylari TUI akisinda da ayni sirada gorunmeli",
    );

    // --- Toplam akis esitligi ve esik kontrolu. ---
    let tui_all: Vec<Vec<u8>> = tui_first.iter().chain(tui_second.iter()).cloned().collect();
    let webui_all: Vec<Vec<u8>> = webui_first
        .iter()
        .chain(webui_second.iter())
        .cloned()
        .collect();

    assert_eq!(
        tui_all.len(),
        EXPECTED_EVENTS,
        "TUI tam {EXPECTED_EVENTS} olay gormeli"
    );
    assert_eq!(
        webui_all.len(),
        EXPECTED_EVENTS,
        "WebUI tam {EXPECTED_EVENTS} olay gormeli"
    );
    assert_streams_identical(&tui_all, &webui_all, "iki akisin tamami bit-es olmali");

    // Olaylarin gercekten komut sonucu oldugunu dogrula (bos akis esitligi sayilmaz).
    let kinds: Vec<String> = tui_all
        .iter()
        .map(|bytes| {
            let event: StateEvent = serde_json::from_slice(bytes).expect("olay geri cozulmeli");
            event.kind().to_string()
        })
        .collect();
    assert_eq!(
        kinds,
        vec![
            "task_upserted".to_string(),
            "notice".to_string(),
            "agent_upserted".to_string(),
            "notice".to_string(),
        ],
        "olay siralamasi komut sirasini izlemeli"
    );

    // Her iki komut da cekirdege ulasmis olmali.
    assert_eq!(
        core.dispatched(),
        vec!["spawn_task".to_string(), "write_to_agent".to_string()],
        "her iki yuzden gelen komut da ayni cekirdek ucuna dusmeli"
    );
}

/// WS akisindan `count` adet `state` cercevesi okur ve kanonik byte'lara doker.
async fn read_events(ws: &mut WsClient, count: usize) -> Vec<Vec<u8>> {
    let mut out = Vec::with_capacity(count);
    while out.len() < count {
        let (kind, payload) = next_ws_frame(ws).await;
        assert_eq!(
            kind, FRAME_STATE,
            "beklenen durum cercevesi yerine '{kind}' geldi (akis boslugu?)"
        );
        out.push(event_bytes(&payload));
    }
    out
}

/// SSE akisindan `count` adet `state` olayi okur ve kanonik byte'lara doker.
async fn read_sse_events<S>(sse: &mut S, count: usize) -> Vec<Vec<u8>>
where
    S: Stream<Item = Result<SseEvent, EventStreamError<reqwest::Error>>> + Unpin,
{
    let mut out = Vec::with_capacity(count);
    while out.len() < count {
        let (kind, payload) = next_sse_frame(sse).await;
        assert_eq!(
            kind, FRAME_STATE,
            "beklenen durum olayi yerine '{kind}' geldi (akis boslugu?)"
        );
        out.push(event_bytes(&payload));
    }
    out
}
