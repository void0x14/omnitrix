//! provider_probe modülü için mockable HTTP seam testleri.
//! Gerçek internet'e bağlanmaz: tüm probe'lar 127.0.0.1 üzerindeki yerel
//! mock server'lara gider.

use super::*;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use zeroize::Zeroizing;

const TEST_KEY: &str = "sk-test-0123456789abcdef";

/// Mock HTTP server: isteği oku, sabit status ile yanıtla.
/// `capture` doluysa ham istek (status satırı + header'lar) oraya yazılır.
/// Tek listener birden çok bağlantıyı sırayla kabul eder.
async fn spawn_status_server(
    status: u16,
    capture: Option<Arc<Mutex<Vec<u8>>>>,
) -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (mut socket, _) = match listener.accept().await {
                Ok(s) => s,
                Err(_) => break,
            };
            let mut received = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                let n = socket.read(&mut buf).await.unwrap_or(0);
                if n == 0 {
                    break;
                }
                received.extend_from_slice(&buf[..n]);
                if received.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            if let Some(cap) = &capture {
                *cap.lock().unwrap() = received;
            }
            let reason = match status {
                200 => "OK",
                401 => "Unauthorized",
                403 => "Forbidden",
                404 => "Not Found",
                _ => "Mock",
            };
            let body = format!("mock {status}");
            let response = format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = socket.write_all(response.as_bytes()).await;
        }
    });
    addr
}

/// Bağlantıyı kabul eder ama hiç yanıt vermez (timeout testleri).
async fn spawn_silent_server() -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 4096];
        let _ = socket.read(&mut buf).await;
        let _ = socket.shutdown().await;
    });
    addr
}

/// Kapalı bir port döndürür (connection refused).
async fn refused_addr() -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    addr
}

fn req(provider: &str, urls: Vec<String>, timeout: Duration) -> ProbeRequest {
    ProbeRequest {
        provider_id: provider.to_string(),
        api_key: Zeroizing::new(TEST_KEY.to_string()),
        base_urls: urls,
        timeout,
    }
}

#[tokio::test]
async fn probe_200_is_ok_auth_valid() {
    let addr = spawn_status_server(200, None).await;
    let results = probe_candidates(vec![req(
        "xai",
        vec![format!("http://{addr}/")],
        Duration::from_millis(1000),
    )])
    .await;
    assert_eq!(results.len(), 1);
    let r = &results[0];
    assert_eq!(r.provider_id, "xai");
    // Sondaki `/` normalize edilir.
    assert_eq!(r.base_url, format!("http://{addr}"));
    // 127.0.0.1 — bilinen bir region yok.
    assert_eq!(r.region, None);
    assert!(r.ok);
    assert_eq!(r.http_status, Some(200));
    assert!(r.auth_seems_valid);
    assert!(r.latency_ms <= 1000);
}

#[tokio::test]
async fn probe_401_keeps_status_auth_challenge() {
    let addr = spawn_status_server(401, None).await;
    let results = probe_candidates(vec![req(
        "xai",
        vec![format!("http://{addr}/v1")],
        Duration::from_millis(1000),
    )])
    .await;
    let r = &results[0];
    // HTTP yanıtı alındı → ok; 401 → auth challenge sinyali.
    assert!(r.ok);
    assert_eq!(r.http_status, Some(401));
    assert!(r.auth_seems_valid);
}

#[tokio::test]
async fn probe_403_keeps_status_auth_challenge() {
    let addr = spawn_status_server(403, None).await;
    let results = probe_candidates(vec![req(
        "xai",
        vec![format!("http://{addr}/")],
        Duration::from_millis(1000),
    )])
    .await;
    let r = &results[0];
    assert!(r.ok);
    assert_eq!(r.http_status, Some(403));
    assert!(r.auth_seems_valid);
}

#[tokio::test]
async fn probe_404_wrong_route_auth_invalid() {
    let addr = spawn_status_server(404, None).await;
    let results = probe_candidates(vec![req(
        "xai",
        vec![format!("http://{addr}/")],
        Duration::from_millis(1000),
    )])
    .await;
    let r = &results[0];
    // Host bulundu ama route yanlış → 404, auth geçersiz sinyali.
    assert!(r.ok);
    assert_eq!(r.http_status, Some(404));
    assert!(!r.auth_seems_valid);
}

#[tokio::test]
async fn probe_connection_error_is_not_ok() {
    let addr = refused_addr().await;
    let results = probe_candidates(vec![req(
        "xai",
        vec![format!("http://{addr}/")],
        Duration::from_millis(1000),
    )])
    .await;
    let r = &results[0];
    assert!(!r.ok);
    assert_eq!(r.http_status, None);
    assert!(!r.auth_seems_valid);
    assert_eq!(r.base_url, format!("http://{addr}"));
}

#[tokio::test]
async fn probe_timeout_honors_request_override() {
    let addr = spawn_silent_server().await;
    let started = Instant::now();
    let results = probe_candidates(vec![req(
        "xai",
        vec![format!("http://{addr}/")],
        Duration::from_millis(200),
    )])
    .await;
    let elapsed = started.elapsed();
    let r = &results[0];
    assert!(!r.ok);
    assert_eq!(r.http_status, None);
    assert!(!r.auth_seems_valid);
    // Açık timeout (200ms) onaylandı: default 2500ms olsaydı çok daha uzun sürerdi.
    assert!(
        elapsed < Duration::from_millis(1500),
        "elapsed {elapsed:?} — request timeout override uygulanmadı"
    );
}

#[tokio::test]
async fn probe_runs_region_endpoints_concurrently() {
    let a = spawn_silent_server().await;
    let b = spawn_silent_server().await;
    let started = Instant::now();
    let results = probe_candidates(vec![req(
        "xai",
        vec![format!("http://{a}/"), format!("http://{b}/")],
        Duration::from_millis(300),
    )])
    .await;
    let elapsed = started.elapsed();
    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|r| !r.ok));
    // Seri olsaydı ~600ms+; eşzamanlı ~300ms. Geniş toleransla doğrula.
    assert!(
        elapsed < Duration::from_millis(550),
        "elapsed {elapsed:?} — endpoint'ler eşzamanlı probe edilmedi"
    );
}

#[tokio::test]
async fn probe_normalizes_base_url_and_preserves_order() {
    let addr = spawn_status_server(200, None).await;
    let results = probe_candidates(vec![req(
        "xai",
        vec![format!("http://{addr}/"), format!("http://{addr}/v1/")],
        Duration::from_millis(1000),
    )])
    .await;
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].base_url, format!("http://{addr}"));
    assert_eq!(results[1].base_url, format!("http://{addr}/v1"));
    assert_eq!(results[0].provider_id, "xai");
    assert_eq!(results[1].provider_id, "xai");
}

#[tokio::test]
async fn probe_sends_bearer_header_and_never_key_in_url() {
    let capture = Arc::new(Mutex::new(Vec::new()));
    let addr = spawn_status_server(404, Some(capture.clone())).await;
    let results = probe_candidates(vec![req(
        "xai",
        vec![format!("http://{addr}/v1/")],
        Duration::from_millis(1000),
    )])
    .await;
    assert_eq!(results[0].http_status, Some(404));

    let raw = String::from_utf8_lossy(&capture.lock().unwrap().clone()).to_string();
    // Key Bearer header'ında taşınır (header adı büyük/küçük harf fark eder).
    assert!(
        raw.to_ascii_lowercase()
            .contains(&format!("authorization: bearer {TEST_KEY}")),
        "bearer header eksik:\n{raw}"
    );
    // ...ama istek satırında (URL) asla görünmez.
    let request_line = raw.lines().next().unwrap_or_default().to_string();
    assert!(
        request_line.contains("/v1/models"),
        "request line: {request_line}"
    );
    assert!(
        !request_line.contains(TEST_KEY),
        "API key URL'ye sızdı: {request_line}"
    );
}

#[tokio::test]
async fn probe_never_logs_or_exposes_api_key() {
    struct BufWriter(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for BufWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for BufWriter {
        type Writer = BufWriter;
        fn make_writer(&'a self) -> Self::Writer {
            BufWriter(self.0.clone())
        }
    }

    let log_buf = Arc::new(Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_writer(BufWriter(log_buf.clone()))
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    // Hata yolu: debug log tetiklenmeli (404 → auth geçersiz, log üretir).
    let addr = spawn_status_server(404, None).await;
    let key = "sk-super-secret-leak-check-0001";
    let results = probe_candidates(vec![ProbeRequest {
        provider_id: "xai".to_string(),
        api_key: Zeroizing::new(key.to_string()),
        base_urls: vec![format!("http://{addr}/")],
        timeout: Duration::from_millis(1000),
    }])
    .await;
    assert!(!results[0].auth_seems_valid);

    let logs = String::from_utf8_lossy(&log_buf.lock().unwrap().clone()).to_string();
    assert!(!logs.contains(key), "API key log çıktısına sızdı:\n{logs}");
    // ProbeResult Debug çıktısı da secret içermemeli.
    assert!(
        !format!("{:?}", results).contains(key),
        "key Debug çıktısına sızdı"
    );
}

#[tokio::test]
async fn probe_empty_requests_empty_results() {
    let results = probe_candidates(Vec::new()).await;
    assert!(results.is_empty());
}

#[tokio::test]
async fn probe_strips_userinfo_never_leaks_in_request_logs_or_debug() {
    struct BufWriter(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for BufWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for BufWriter {
        type Writer = BufWriter;
        fn make_writer(&'a self) -> Self::Writer {
            BufWriter(self.0.clone())
        }
    }

    let log_buf = Arc::new(Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_writer(BufWriter(log_buf.clone()))
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    // 404: ok=true (endpoint erişilebilir) + non-success log yolu tetiklenir.
    let capture = Arc::new(Mutex::new(Vec::new()));
    let addr = spawn_status_server(404, Some(capture.clone())).await;
    let userinfo_secret = "sk-userinfo-secret-0001";
    let results = probe_candidates(vec![ProbeRequest {
        provider_id: "xai".to_string(),
        api_key: Zeroizing::new(TEST_KEY.to_string()),
        base_urls: vec![format!("http://{userinfo_secret}@{addr}/")],
        timeout: Duration::from_millis(1000),
    }])
    .await;

    // Endpoint userinfo'ya rağmen erişilebilir.
    let r = &results[0];
    assert!(r.ok);
    assert_eq!(r.http_status, Some(404));
    // Userinfo sonuçtan temizlendi (Debug'a da sızmaz).
    assert_eq!(r.base_url, format!("http://{addr}"));
    assert!(!r.base_url.contains('@'));
    assert!(!r.base_url.contains(userinfo_secret));

    // İstekte secret yok; userinfo Basic Authorization'a dönüşmedi.
    let raw = String::from_utf8_lossy(&capture.lock().unwrap().clone()).to_string();
    assert!(
        !raw.contains(userinfo_secret),
        "userinfo isteğe sızdı:\n{raw}"
    );
    assert!(
        !raw.to_ascii_lowercase().contains("authorization: basic"),
        "userinfo Basic Authorization'a dönüştü:\n{raw}"
    );
    // Normal Bearer akışı korundu.
    assert!(
        raw.to_ascii_lowercase()
            .contains(&format!("authorization: bearer {TEST_KEY}")),
        "bearer header eksik:\n{raw}"
    );
    let request_line = raw.lines().next().unwrap_or_default().to_string();
    assert!(
        request_line.contains("/models"),
        "request line: {request_line}"
    );

    // Loglar ve ProbeResult Debug secret içermiyor.
    let logs = String::from_utf8_lossy(&log_buf.lock().unwrap().clone()).to_string();
    assert!(
        !logs.contains(userinfo_secret),
        "userinfo log çıktısına sızdı:\n{logs}"
    );
    assert!(
        !format!("{:?}", results).contains(userinfo_secret),
        "userinfo Debug çıktısına sızdı"
    );
}

#[test]
fn region_from_url_derives_known_region() {
    assert_eq!(
        region_from_url("https://us-west-2.api.nvidia.com/v1"),
        Some("us-west-2".to_string())
    );
    assert_eq!(
        region_from_url("https://api.us-west-2.anthropic.com/v1"),
        Some("us-west-2".to_string())
    );
    assert_eq!(
        region_from_url("https://eu-central-1.api.x.com/"),
        Some("eu-central-1".to_string())
    );
    assert_eq!(
        region_from_url("https://europe-west4.gcp.example.com/v1"),
        Some("europe-west4".to_string())
    );
    assert_eq!(region_from_url("https://api.anthropic.com/v1"), None);
    assert_eq!(region_from_url("https://openrouter.ai/api/v1"), None);
    assert_eq!(region_from_url("http://127.0.0.1:8080/"), None);
    assert_eq!(region_from_url("not a url"), None);
}

fn result(provider: &str, ok: bool, auth: bool, latency: u64) -> ProbeResult {
    ProbeResult {
        provider_id: provider.to_string(),
        base_url: format!("https://{provider}.invalid/"),
        region: None,
        ok,
        http_status: if ok { Some(200) } else { None },
        latency_ms: latency,
        auth_seems_valid: auth,
    }
}

fn candidate(provider: &str, confidence: u8) -> xai_omni_keychain::DetectCandidate {
    xai_omni_keychain::DetectCandidate {
        provider_id: provider.to_string(),
        confidence,
        reason: format!("prefix:{provider}"),
        suggested_regions: Vec::new(),
    }
}

#[test]
fn pick_winner_empty_results_is_none() {
    assert_eq!(pick_winner(&[], &[]), None);
}

#[test]
fn pick_winner_prefers_ok_over_failed() {
    let failed = result("xai", false, false, 5);
    let ok = result("xai", true, false, 100);
    let winner = pick_winner(&[failed.clone(), ok.clone()], &[]).expect("winner");
    assert_eq!(winner.base_url, ok.base_url);
}

#[test]
fn pick_winner_prefers_auth_valid_over_404() {
    let not_found = result("xai", true, false, 10);
    let auth = result("xai", true, true, 200);
    let winner = pick_winner(&[not_found.clone(), auth.clone()], &[]).expect("winner");
    assert_eq!(winner.base_url, auth.base_url);
}

#[test]
fn pick_winner_breaks_tie_by_offline_confidence() {
    let low = result("xai", true, true, 50);
    let high = result("deepseek", true, true, 60);
    let offline = vec![candidate("xai", 35), candidate("deepseek", 90)];
    let winner = pick_winner(&[low.clone(), high.clone()], &offline).expect("winner");
    assert_eq!(winner.base_url, high.base_url);
}

#[test]
fn pick_winner_breaks_tie_by_latency_then_input_order() {
    let slow = result("xai", true, true, 300);
    let fast = result("xai", true, true, 100);
    let winner = pick_winner(&[slow.clone(), fast.clone()], &[]).expect("winner");
    assert_eq!(winner.base_url, fast.base_url);

    // Tamamen eşit: input sırası korunur (stable sort) — ilk aday kazanır.
    let a = result("xai", true, true, 42);
    let b = result("xai", true, true, 42);
    let winner = pick_winner(&[a.clone(), b.clone()], &[]).expect("winner");
    assert_eq!(winner.base_url, a.base_url);
}

#[test]
fn pick_winner_missing_offline_candidate_defaults_to_zero_confidence() {
    // offline'da aday yok → confidence 0; latency karar verir.
    let slow = result("xai", true, true, 300);
    let fast = result("deepseek", true, true, 100);
    let winner = pick_winner(&[slow.clone(), fast.clone()], &[]).expect("winner");
    assert_eq!(winner.base_url, fast.base_url);
}

/// http_status'u açıkça verilen ok=true sonucu; auth sinyali status'tan
/// `probe_one` ile aynı kuraldan türetilir (2xx veya 401/403).
fn result_status(provider: &str, status: u16, latency: u64) -> ProbeResult {
    ProbeResult {
        provider_id: provider.to_string(),
        base_url: format!("https://{provider}.invalid/"),
        region: None,
        ok: true,
        http_status: Some(status),
        latency_ms: latency,
        auth_seems_valid: (200..300).contains(&status) || status == 401 || status == 403,
    }
}

#[test]
fn pick_winner_prefers_real_2xx_over_auth_challenge() {
    // Yüksek confidence'lı 401 (auth challenge) vs düşük confidence'lı 200
    // (gerçek başarı): gerçek 2xx her zaman üst sıradadır; confidence ve
    // latency onu geçemez.
    let challenge = result_status("xai", 401, 50);
    let success = result_status("deepseek", 200, 200);
    let offline = vec![candidate("xai", 90), candidate("deepseek", 40)];
    let winner = pick_winner(&[challenge.clone(), success.clone()], &offline).expect("winner");
    assert_eq!(winner.provider_id, "deepseek");
    assert_eq!(winner.base_url, success.base_url);
}
