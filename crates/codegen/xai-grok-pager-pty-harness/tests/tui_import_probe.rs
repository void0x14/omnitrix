//! Gerçek TUI davranışı ölçümü: `/import <stack>` komutu — İZole ortam.
//!
//! İki senaryo (`#[ignore]` — gerçek kullanıcı OpenCode kurulumu gerektirir):
//!
//! 1. `import_requires_password_then_auto_imports` — kilitli keychain ile
//!    tam akış: `/import opencode` master password prompt'u gösterir; doğru
//!    şifre girilince pending stack sync otomatik sürer (GERÇEK
//!    `~/.local/share/opencode/auth.json`'tan okur — kullanıcı dosyası asla
//!    silinmez), özet scrollback'e yazılır ve keys modalı kapanır.
//! 2. `import_runs_instantly_when_unlocked` — keychain `/keys` ile önce
//!    açılıp kilit çözülürse `/import opencode` anında çalışır: modal
//!    açılmaz, özet doğrudan scrollback'e düşer.
//!
//! İzolasyon: her test kendi GROK_HOME'unu kullanır (`/tmp/tui-import-test-1`
//! ve `/tmp/tui-import-test-2`; her test başında kendi evini silip yeniden
//! kurar → keychain izole, paralel koşuda `Directory not empty` çakışması
//! olmaz). Her evine fake OAuth `auth.json` + varsayılan modelli minimal
//! `config.toml` tohumlanır (prompt barı açık olan welcome ekranı için).
//! Gate: gerçek cli-chat-proxy'ye fake token ile `GET /v1/settings` 401 döner
//! → `allow_access` bilinmez → shell `tier_allowed=false` → welcome ekranı
//! "Refresh access / Logout / Quit" menüsüne düşer (prompt barı YOK). Bu yüzden
//! her test yerel bir `MockInferenceServer` başlatır (`/v1/settings` →
//! `{"allow_access": true}` döndüren `preset_allow_access`; `/v1/models` →
//! config'teki varsayılan model) ve `GROK_CLI_CHAT_PROXY_BASE_URL` env'i ile
//! pager'ın settings/model fetch'lerini ona yönlendirir.
//! `HOME` bilinçli olarak EZİLMEZ: opencode auth.json
//! adayları katalogda `$HOME`/XDG'den türetilir (`paths_opencode` →
//! `xdg_data()`/`home()`/`xdg_config()`), HOME override'ı gerçek kullanıcı
//! dosyasını bulmayı bozardı.
//!
//! ```bash
//! cargo test -p xai-grok-pager-pty-harness --test tui_import_probe -- --ignored --nocapture
//! ```

use std::path::Path;
use std::time::Duration;

use xai_grok_pager_pty_harness::{PtyHarness, pager_binary};

/// İzole keychain kökleri — her test kendi evini silip yeniden kurar
/// (paralel koşuda `Directory not empty` çakışması olmaz).
const ISOLATED_GROK_HOME_1: &str = "/tmp/tui-import-test-1";
const ISOLATED_GROK_HOME_2: &str = "/tmp/tui-import-test-2";
/// Keys manager modalının varsayılan başlığı (modal AÇIKSA ekrandadır).
const KEYS_MODAL_TITLE: &str = "API Keys (Keychain)";
/// Scrollback'teki import özetinin yön etiketi (`SyncSummary::format_tr`:
/// `stack → omnitrix [opencode]: N key · …`).
const IMPORT_EVIDENCE: &str = "stack → omnitrix";
/// Unlock modu prompt'u (ekranda ` master password: ` olarak render edilir).
const PASSWORD_PROMPT: &str = "master password";
const MASTER_PASSWORD: &str = "test-pass-123";

/// Yerel cli-chat-proxy taklidi: `/v1/settings` → `{"allow_access": true}`
/// (abonelik gate'ini açar; `preset_allow_access`) ve `/v1/models` →
/// config.toml'daki varsayılan model. `_rt` runtime'ı (server'ın tokio
/// task'larını besleyen) test boyunca canlı tutar; drop sırası: önce server
/// (kapanış sinyali runtime ayaktayken gider), sonra runtime.
struct MockProxy {
    server: xai_grok_test_support::MockInferenceServer,
    _rt: tokio::runtime::Runtime,
}

/// Mock proxy'yi başlatır ve allow_access gate'ini açar.
fn start_mock_proxy() -> anyhow::Result<MockProxy> {
    let rt = tokio::runtime::Runtime::new()?;
    let server = rt.block_on(xai_grok_test_support::MockInferenceServer::start_with_models(
        vec![xai_grok_test_support::MockModelEntry::new(
            "omni-opencode-deepseek-v4-flash-free",
        )
        .with_api_backend("chat_completions")],
    ))?;
    server.preset_allow_access();
    Ok(MockProxy { server, _rt: rt })
}

/// `===== LABEL =====` + tam ekran dökümü (--nocapture logu kendi kendini
/// belgeler).
fn dump(label: &str, h: &PtyHarness) {
    println!("===== {label} =====");
    println!("{}", h.screen_contents());
    println!("===== /{label} =====");
}

/// Ekran dökümü + scrollback (özet scrollback'e yazıldığı için son aşamada
/// ikisi de basılır).
fn dump_with_scrollback(label: &str, h: &PtyHarness) {
    dump(label, h);
    println!("----- {label} scrollback -----");
    println!("{}", h.scrollback_text());
    println!("----- /{label} scrollback -----");
}

/// İzole GROK_HOME'u sıfırlar (kalıntı keychain silinir, dizin yeniden
/// yaratılır) ve içine fake OAuth `auth.json` + varsayılan modelli minimal
/// `config.toml` tohumlar — `flows.rs` `seed_fake_oauth_raw` ile birebir aynı
/// JSON şekli/alan adları (scope anahtarı `<issuer>::<client_id>`,
/// `auth_mode: "oidc"`, uzak `expires_at`,
/// `coding_data_retention_opt_out: false`). Auth'suz TUI login/approval
/// ekranına düşüp slash komutlarını reddettiği için auth tohumu şarttır;
/// `[models] default` içermeyen config'te de welcome ekranı prompt barı
/// göstermez (sadece menü) — o yüzden config.toml da tohumlanır. GROK_HOME
/// set iken grok home'un kendisi bu dizindir — `.grok` alt dizini YOKTUR,
/// auth.json ve config.toml doğrudan kökte olur.
fn seed_test_auth(home: &Path) -> anyhow::Result<()> {
    // Gerçek kullanıcı keyring'inden ve kalıntı test kayıtlarından bağımsız
    // ol: Secret Service ve keyutils kayıtlarını temizle — aksi halde
    // keyring auto-unlock `/import`'u şifre prompt'u göstermeden anında
    // tamamlar ve "şifre prompt'u bekliyorum" beklentisi bozulur.
    let _ = std::process::Command::new("secret-tool")
        .args(["clear", "service", "omnitrix-keychain"])
        .status();
    if let Ok(out) = std::process::Command::new("keyctl")
        .args(["search", "@u", "keyring", "keyring-rs:master@omnitrix-keychain"])
        .output()
    {
        let id = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()) {
            let _ = std::process::Command::new("keyctl")
                .args(["unlink", &id])
                .status();
        }
    }
    if home.exists() {
        std::fs::remove_dir_all(home).expect("izole GROK_HOME silinemedi");
    }
    std::fs::create_dir_all(home).expect("izole GROK_HOME yaratılamadı");
    if home.exists() {
        std::fs::remove_dir_all(home).expect("izole GROK_HOME silinemedi");
    }
    std::fs::create_dir_all(home).expect("izole GROK_HOME yaratılamadı");
    std::fs::write(
        home.join("auth.json"),
        r#"{
  "https://auth.x.ai::b1a00492-073a-47ea-816f-4c329264a828": {
    "key": "pty-test-oauth-token",
    "auth_mode": "oidc",
    "create_time": "2026-01-01T00:00:00Z",
    "user_id": "tui-probe",
    "email": "tui-probe@test.invalid",
    "expires_at": "2030-01-01T00:00:00Z",
    "refresh_token": "pty-test-refresh-token",
    "oidc_issuer": "https://auth.x.ai",
    "oidc_client_id": "b1a00492-073a-47ea-816f-4c329264a828",
    "coding_data_retention_opt_out": false
  }
}
"#,
    )
    .expect("fake oauth auth.json yazılamadı");
    // Varsayılan model olmadan welcome ekranı prompt barı göstermez (sadece
    // menü) → `/import` gibi slash komutları giriş kutusuna düşmez. Gerçek
    // kullanıcı config.toml'undaki çalışan kalıp birebir tohumlanır:
    // `[models] default` + eşleşen `[model.<id>]` + `[model_providers.<id>]`.
    std::fs::write(
        home.join("config.toml"),
        r#"[model_providers.opencode]
base_url = "https://opencode.ai/zen/v1"
api_backend = "chat_completions"

[models]
default = "omni-opencode-deepseek-v4-flash-free"

[model.omni-opencode-deepseek-v4-flash-free]
model = "deepseek-v4-flash-free"
model_provider = "opencode"
"#,
    )
    .expect("varsayılan modelli config.toml yazılamadı");
    Ok(())
}

/// İzole ortamda pager'ı başlatır: keychain `$GROK_HOME` altına yazılır
/// (izole), opencode auth.json çözümü ise kullanıcının gerçek `$HOME`'unu
/// (inherit) kullanır — gerçek dosya okunur, silinmez. Dizin kurulumu ve
/// tohumlama testin başındaki `seed_test_auth(home)` ile yapılır.
/// `proxy_url` yerel `MockProxy`'nin adresidir: `GROK_CLI_CHAT_PROXY_BASE_URL`
/// olarak verilir, böylece settings/model fetch'leri gerçek proxy yerine
/// mock'a gider ve `allow_access` gate'i açık kalır.
fn spawn_isolated(home: &Path, proxy_url: &str) -> anyhow::Result<PtyHarness> {
    let grok_home_env = home.to_string_lossy().into_owned();
    PtyHarness::new_inherited_env(
        &pager_binary()?,
        50,
        120,
        &[],
        &[
            ("GROK_HOME", &grok_home_env),
            // Fake OAuth token'ı gerçek proxy'ye götürmeyiz: settings/model
            // fetch'leri yerel mock'a gider (gate açık kalır).
            ("GROK_CLI_CHAT_PROXY_BASE_URL", proxy_url),
            // OS keyring auto-unlock'u devre dışı bırak: boş dbus adresi →
            // keyring crate Secret Service'e bağlanamaz → manuel master
            // password prompt'una düşer (gerçek kullanıcı keyring'i asla
            // okunmaz/yazılmaz).
            ("DBUS_SESSION_BUS_ADDRESS", ""),
        ],
        Some(home),
    )
}

/// Tam akış: kilitli keychain → `/import opencode` şifre sorar → doğru şifre
/// → import GERÇEK `~/.local/share/opencode/auth.json`'tan çalışır ve izole
/// keychain'e yazar → modal kapanır, özet scrollback'te.
#[test]
#[ignore]
fn import_requires_password_then_auto_imports() -> anyhow::Result<()> {
    let _proxy = start_mock_proxy()?;
    seed_test_auth(Path::new(ISOLATED_GROK_HOME_1))?;
    let mut h = spawn_isolated(Path::new(ISOLATED_GROK_HOME_1), &_proxy.server.url())?;

    // Hoş geldin ekranı render olur.
    h.update(Duration::from_secs(6));
    dump("WELCOME", &h);

    // Gate açılınca `has_access` menüsü ("Resume session") ve prompt barı
    // render olur — gated ekranda yalnızca "Refresh access / Logout / Quit"
    // vardır ve slash komutları hiçbir yere düşmez.
    h.wait_for_text("Resume session", Duration::from_secs(30))?;
    dump("WELCOME_ACCESS", &h);
    // `/import opencode` → keychain kilitli olduğundan keys modalı Unlock
    // modunda açılır ("/import opencode — keychain şifresi" başlığı) ve
    // master password prompt'u görünür; iş kuyruğa alınır
    // (pending_stack_sync).
    h.inject_keys(b"/import opencode\r")?;
    // Debug binary'de shell başlangıcı yavaş olabildiğinden sabit bekleme
    // yerine koşullu bekleme (20s).
    h.wait_for_text(PASSWORD_PROMPT, Duration::from_secs(20))?;
    dump("IMPORT_PROMPT", &h);
    h.update(Duration::from_secs(2));
    dump("IMPORT_PROMPT", &h);

    // Doğru master password → dispatch_keychain_unlock pending stack
    // sync'i sürdürür: import gerçek `~/.local/share/opencode/auth.json`'tan
    // okur (kullanıcı dosyası silinmez) ve izole keychain'e merge eder;
    // özet scrollback'e yazılır, modal kapanır.
    h.inject_keys(format!("{MASTER_PASSWORD}\r").as_bytes())?;
    // Import özeti scrollback'e düşene kadar koşullu bekle (debug yavaş).
    h.wait_for_full_text(IMPORT_EVIDENCE, Duration::from_secs(25))?;
    h.update(Duration::from_millis(500));
    dump_with_scrollback("AFTER_PASSWORD", &h);
    h.update(Duration::from_secs(6));
    dump_with_scrollback("AFTER_PASSWORD", &h);

    // Modal kapandı (başlık ekranda kalmadı) + import özeti kanıtı
    // ekranda/scrollback'te.
    assert!(
        !h.contains_text(KEYS_MODAL_TITLE),
        "import sonrası keys modalı kapalı olmalı:\n{}",
        h.full_text()
    );
    assert!(
        h.contains_full_text(IMPORT_EVIDENCE),
        "import özeti ({IMPORT_EVIDENCE:?}) ekranda/scrollback'te olmalı:\n{}",
        h.full_text()
    );

    h.quit()?;
    Ok(())
}

/// Açık keychain: `/keys` ile unlock (browse), Esc ile modal kapatılır;
/// ardından `/import opencode` anında çalışır — modal HİÇ açılmaz ve özet
/// scrollback'e düşer.
#[test]
#[ignore]
fn import_runs_instantly_when_unlocked() -> anyhow::Result<()> {
    let _proxy = start_mock_proxy()?;
    seed_test_auth(Path::new(ISOLATED_GROK_HOME_2))?;
    let mut h = spawn_isolated(Path::new(ISOLATED_GROK_HOME_2), &_proxy.server.url())?;

    // Hoş geldin ekranı render olur.
    h.update(Duration::from_secs(6));
    dump("WELCOME", &h);

    // Gate açılınca `has_access` menüsü ("Resume session") ve prompt barı
    // render olur — gated ekranda yalnızca "Refresh access / Logout / Quit"
    // vardır ve slash komutları hiçbir yere düşmez.
    h.wait_for_text("Resume session", Duration::from_secs(30))?;
    dump("WELCOME_ACCESS", &h);
    // `/keys` → kilitli keychain → master password prompt'u.
    h.inject_keys(b"/keys\r")?;
    h.wait_for_text(PASSWORD_PROMPT, Duration::from_secs(20))?;
    dump("KEYS_PROMPT", &h);
    h.update(Duration::from_secs(2));
    dump("KEYS_PROMPT", &h);

    // Unlock → Browse (keychain artık oturumda açık). Esc ile modal kapat.
    h.inject_keys(format!("{MASTER_PASSWORD}\r").as_bytes())?;
    // Unlock sonrası modal Browse'a geçer (küçük bir gecikme bırak).
    h.update(Duration::from_secs(3));
    dump("KEYS_UNLOCKED", &h);
    h.update(Duration::from_secs(3));
    dump("KEYS_UNLOCKED", &h);
    h.inject_keys(b"\x1b")?;
    h.update(Duration::from_secs(1));
    dump("AFTER_ESC", &h);

    // Keychain açıkken `/import opencode` anında çalışır: modal açılmaz
    // (dispatch_stack_import_export `app.keychain.is_none()` değil → doğrudan
    // run_stack_sync), özet scrollback'e yazılır.
    h.inject_keys(b"/import opencode\r")?;
    h.wait_for_full_text(IMPORT_EVIDENCE, Duration::from_secs(25))?;
    h.update(Duration::from_millis(500));
    dump_with_scrollback("IMPORT_DONE", &h);
    h.update(Duration::from_secs(5));
    dump_with_scrollback("IMPORT_DONE", &h);

    // Modal hiç görünmedi + import özeti kanıtı ekranda/scrollback'te.
    assert!(
        !h.contains_text(KEYS_MODAL_TITLE),
        "açık keychain'de `/import` modal açmamalı:\n{}",
        h.full_text()
    );
    assert!(
        h.contains_full_text(IMPORT_EVIDENCE),
        "import özeti ({IMPORT_EVIDENCE:?}) ekranda/scrollback'te olmalı:\n{}",
        h.full_text()
    );

    h.quit()?;
    Ok(())
}
