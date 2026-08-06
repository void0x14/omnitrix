//! `grok_computer` tool — bilgisayar kullaniminin grok tool sistemine tasinmasi.
//!
//! Kullanicinin vizyonu: "Bilgisayar kullanimi ust seviye olmali — sistemimiz
//! otonom isler yaparken MCP'ler yetmeyecek, kendi bilgisayarimi da otonom
//! kullanabilmeleri gerekmekte." Bu kapinin ta kendisidir: agent, bir MCP
//! sunucusuna ihtiyac duymadan masaustunu (ekran goruntusu, isaretci, tus ve
//! metin eylemleri) dogrudan kullanir.
//!
//! ## Hub gercegi ve tasima sekli
//!
//! `xai-computer-hub-core` bir ekran kontrolu API'si DEGILDIR; transport +
//! registry + resolver duzlemidir (nesne-guvenli `ToolHandle`'in evi).
//! Ekran kontrolu sozlesmesi burada [`ComputerBackend`] trait'i olarak
//! tanimlanir; host katmani (orn. `omni-tools`'taki `ComputerUseSession`)
//! bu trait'i uygular ve `SharedResources`'a `Arc<dyn ComputerBackend>`
//! olarak enjekte eder. Enjeksiyon yoksa yerlesik [`EnvComputerBackend`]
//! devreye girer: ortam tespiti (Wayland once, yoksa X11) ve yaygin CLI
//! suruculeri (X11: `xdotool`, `import`/`scrot`; Wayland: `ydotool`,
//! `wtype`, `grim`/`gnome-screenshot`/`spectacle`). Boylece tool host
//! kurulumundan bagimsiz calisir (otonom kullanim), host ise zengin bir
//! arka uc enjekte ederek davranisi degistirebilir.
//!
//! ## Eylemler
//!
//! | `action` | Anlami |
//! |---|---|
//! | `detect` | Aktif goruntu sunucusunu bildirir (wayland \| x11). |
//! | `screenshot` | Ekrani yakalar, `~/.grok/computer/<ts>.png` dosyasina yazar, yolunu dondurur. |
//! | `click` | `x`/`y` koordinatina tiklar. |
//! | `type` | `text` metnini yazar. |
//! | `scroll` | `scroll_delta` kadar kaydirir (pozitif = asagi, negatif = yukari). |
//! | `move` | Imleci `x`/`y` koordinatina tasir. |
//!
//! ## I6
//!
//! Uretim yolunda `unwrap` / `expect` / `panic!` yoktur. Wayland/X11 yokken
//! tool patlamaz; "görüntü sunucusu bulunamadi" mesaji `service_unavailable`
//! `ToolError` ile doner. Tum dis surucu hatalari `Err` olarak tasinir.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::types::output::{DynamicOutput, ToolOutput};
use crate::types::requirements::{Expr, ToolRequirement};
use crate::types::tool::{ToolKind, ToolNamespace};
use crate::types::tool_io::ToolInput;
use crate::types::tool_metadata::{shared_resources, ToolMetadata};

// ---------------------------------------------------------------------------
// Goruntu sunucusu (omni-tools `DisplayServer::detect` mantiginin tasinmasi)
// ---------------------------------------------------------------------------

/// Linux masaustu goruntu sunucusu. Kapsam: Wayland ve X11.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServerKind {
    /// Wayland oturumu.
    Wayland,
    /// X11 oturumu.
    X11,
}

impl ServerKind {
    /// Kapsanan sunucularin tamami.
    pub const ALL: [ServerKind; 2] = [ServerKind::Wayland, ServerKind::X11];

    /// Kayitlara ve ciktiya giden sabit etiket.
    pub fn as_str(self) -> &'static str {
        match self {
            ServerKind::Wayland => "wayland",
            ServerKind::X11 => "x11",
        }
    }

    /// Surec ortamindan tespit.
    ///
    /// Sira: `XDG_SESSION_TYPE` > `WAYLAND_DISPLAY` > `DISPLAY`. Hicbiri
    /// anlamli degilse `None` — bu "masaustu yok" demektir ve cagrilar
    /// "görüntü sunucusu bulunamadi" ile reddedilir (I6).
    pub fn detect() -> Option<Self> {
        Self::from_env(|key| std::env::var(key).ok())
    }

    /// [`ServerKind::detect`]'in test edilebilir govdesi.
    ///
    /// `lookup` ortam degiskenini cozer; bos deger "yok" sayilir.
    pub fn from_env(lookup: impl Fn(&str) -> Option<String>) -> Option<Self> {
        let non_empty = |key: &str| lookup(key).filter(|value| !value.trim().is_empty());

        if let Some(session_type) = non_empty("XDG_SESSION_TYPE") {
            match session_type.trim().to_ascii_lowercase().as_str() {
                "wayland" => return Some(ServerKind::Wayland),
                "x11" => return Some(ServerKind::X11),
                // Diger degerler (tty, unspecified, ...) baglayici degil;
                // soket degiskenlerine bakilir.
                _ => {}
            }
        }
        if non_empty("WAYLAND_DISPLAY").is_some() {
            return Some(ServerKind::Wayland);
        }
        if non_empty("DISPLAY").is_some() {
            return Some(ServerKind::X11);
        }
        None
    }
}

impl fmt::Display for ServerKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// [`ComputerBackend::detect`] sonucu: hangi sunucu, hangi arka uc.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectInfo {
    /// Tespit edilen goruntu sunucusu.
    pub server: ServerKind,
    /// Arka ucun kayitlara giden adi.
    pub backend: String,
}

// ---------------------------------------------------------------------------
// Eylem sozlugu
// ---------------------------------------------------------------------------

/// Ajanin isteyebilecegi masaustu eylemleri.
///
/// Kapali kume; arguman dogrulamasi [`validate_action`] ile arka uca
/// ulasmadan once yapilir.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ComputerAction {
    /// Aktif goruntu sunucusunu bildir.
    Detect,
    /// Ekran goruntusu al.
    Screenshot,
    /// Koordinata tikla.
    Click,
    /// Metin yaz.
    Type,
    /// Kaydir.
    Scroll,
    /// Imleci tasi.
    Move,
}

impl ComputerAction {
    /// Kapinin talep ettigi tum eylemler; testler bu diziyi dolasir.
    pub const ALL: [ComputerAction; 6] = [
        ComputerAction::Detect,
        ComputerAction::Screenshot,
        ComputerAction::Click,
        ComputerAction::Type,
        ComputerAction::Scroll,
        ComputerAction::Move,
    ];

    /// Ciktiya ve kayitlara giden sabit etiket.
    pub fn as_str(self) -> &'static str {
        match self {
            ComputerAction::Detect => "detect",
            ComputerAction::Screenshot => "screenshot",
            ComputerAction::Click => "click",
            ComputerAction::Type => "type",
            ComputerAction::Scroll => "scroll",
            ComputerAction::Move => "move",
        }
    }

    /// Arguman degerinden eylem cozer (bosluk/buyuk-kucuk harf hosgorulu).
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "detect" | "test" => Some(ComputerAction::Detect),
            "screenshot" | "ekran" | "ekran_goruntusu" => Some(ComputerAction::Screenshot),
            "click" | "tikla" => Some(ComputerAction::Click),
            "type" | "yaz" | "typing" => Some(ComputerAction::Type),
            "scroll" | "kaydir" => Some(ComputerAction::Scroll),
            "move" | "mousemove" | "tası" | "tasi" => Some(ComputerAction::Move),
            _ => None,
        }
    }
}

impl fmt::Display for ComputerAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// Arka uc sozlesmesi (takilabilir trait)
// ---------------------------------------------------------------------------

/// Masaustu arka ucu — host enjekte eder ya da [`EnvComputerBackend`] devreye
/// girer. Wayland ve X11 icin AYNI sozlesme.
#[async_trait]
pub trait ComputerBackend: Send + Sync {
    /// Kayitlara ve ciktiya giden arka uc adi.
    fn name(&self) -> &str;

    /// Aktif goruntu sunucusunu bildirir; yoksa `Err` (I6, panik yok).
    async fn detect(&self) -> Result<DetectInfo, String>;

    /// Ekran goruntusunu `path` dosyasina yazar (PNG baytlari).
    async fn screenshot(&self, path: &Path) -> Result<(), String>;

    /// Imleci `(x, y)` koordinatina tasir.
    async fn mouse_move(&self, x: i32, y: i32) -> Result<(), String>;

    /// Tiklar. Koordinat verilirse once oraya tasinir; yoksa imlecin
    /// bulundugu nokta kullanilir.
    async fn click(&self, x: Option<i32>, y: Option<i32>) -> Result<(), String>;

    /// `text` metnini yazar.
    async fn type_text(&self, text: &str) -> Result<(), String>;

    /// `delta` kadar kaydirir (pozitif = asagi, negatif = yukari).
    /// Koordinat verilirse once oraya tasinir.
    async fn scroll(&self, x: Option<i32>, y: Option<i32>, delta: i32) -> Result<(), String>;
}

/// Dis komut zaman asimi. Asilan komut oldurulur ve `Err` doner (I6).
const CMD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// X11 giris surucusu.
const X11_TOOL_HINT: &str = "X11 giris eylemleri icin 'xdotool' kurulu olmalidir";

/// Wayland imlec surucusu.
const WAYLAND_TOOL_HINT: &str =
    "Wayland imlec eylemleri icin 'ydotool' (daemon calisiyor olmali) kurulu olmalidir";

/// Tek bir dis komut calistirir.
///
/// Cikti bastirilmaz; yalnizca cikis kodu onemlidir. Spawn hatasi (surucu
/// kurulu degil), hata kodu veya zaman asimi `Err` ile doner.
async fn run_cmd(args: &[&str]) -> Result<(), String> {
    let Some(program) = args.first() else {
        return Err("bos komut".to_string());
    };
    let mut child = tokio::process::Command::new(program)
        .args(&args[1..])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|err| format!("'{program}' calistirilamadi: {err}"))?;

    match tokio::time::timeout(CMD_TIMEOUT, child.wait()).await {
        Ok(Ok(status)) if status.success() => Ok(()),
        Ok(Ok(status)) => Err(format!("'{program}' hata koduyla cikti: {status}")),
        Ok(Err(err)) => Err(format!("'{program}' beklenirken hata: {err}")),
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            Err(format!("'{program}' zaman asimi ({CMD_TIMEOUT:?})"))
        }
    }
}

/// Adaylari sirayla dener; ilk basarili olan kazanir. Hepsinden once
/// kurulu olmayanlar atlanir, kalanlarin hatalari birlestirilir.
async fn run_first(candidates: &[&[&str]]) -> Result<(), String> {
    let mut failures: Vec<String> = Vec::new();
    for candidate in candidates {
        let Some(program) = candidate.first() else {
            continue;
        };
        if which::which(program).is_err() {
            failures.push(format!("'{program}' kurulu degil"));
            continue;
        }
        match run_cmd(candidate).await {
            Ok(()) => return Ok(()),
            Err(err) => failures.push(err),
        }
    }
    Err(format!(
        "hicbir surucu calismadi: {}",
        failures.join("; ")
    ))
}

/// X11 giris komutu: `run_cmd` + kurulum ipucu.
async fn x11_run(args: &[&str]) -> Result<(), String> {
    run_cmd(args)
        .await
        .map_err(|err| format!("{err} ({X11_TOOL_HINT})"))
}

/// Wayland imlec komutu: `run_cmd` + kurulum ipucu.
async fn wayland_run(args: &[&str]) -> Result<(), String> {
    run_cmd(args)
        .await
        .map_err(|err| format!("{err} ({WAYLAND_TOOL_HINT})"))
}

/// Wayland metin komutu: `run_cmd` + kurulum ipucu.
async fn wayland_type_run(args: &[&str]) -> Result<(), String> {
    run_cmd(args).await.map_err(|err| {
        format!("{err} (Wayland metin girisleri icin 'wtype' kurulu olmalidir)")
    })
}

/// Yerlesik adaptor: ortam tespiti + yaygin CLI suruculeri.
///
/// Host `SharedResources`'a `Arc<dyn ComputerBackend>` enjekte etmediginde
/// bu arka uc kullanilir; boylece tool kurulumdan bagimsiz calisir. Surucu
/// kurulu degilse her eylem acik bir `Err` metni dondurur (I6).
#[derive(Debug, Default)]
pub struct EnvComputerBackend;

#[async_trait]
impl ComputerBackend for EnvComputerBackend {
    fn name(&self) -> &str {
        "env-tools"
    }

    async fn detect(&self) -> Result<DetectInfo, String> {
        let server = ServerKind::detect().ok_or_else(|| {
            "görüntü sunucusu bulunamadi: ne Wayland ne X11 tespit edilebildi".to_string()
        })?;
        Ok(DetectInfo {
            server,
            backend: self.name().to_string(),
        })
    }

    async fn screenshot(&self, path: &Path) -> Result<(), String> {
        let info = self.detect().await?;
        let path_arg = path.display().to_string();
        match info.server {
            ServerKind::X11 => {
                run_first(&[
                    &["import", "-window", "root", path_arg.as_str()],
                    &["scrot", path_arg.as_str()],
                ])
                .await
            }
            ServerKind::Wayland => {
                run_first(&[
                    &["grim", path_arg.as_str()],
                    &["gnome-screenshot", "-f", path_arg.as_str()],
                    &["spectacle", "-b", "-n", "-o", path_arg.as_str()],
                ])
                .await
            }
        }
    }

    async fn mouse_move(&self, x: i32, y: i32) -> Result<(), String> {
        let x = x.to_string();
        let y = y.to_string();
        match self.detect().await?.server {
            ServerKind::X11 => x11_run(&["xdotool", "mousemove", &x, &y]).await,
            ServerKind::Wayland => {
                wayland_run(&["ydotool", "mousemove", "--absolute", "--", &x, &y]).await
            }
        }
    }

    async fn click(&self, x: Option<i32>, y: Option<i32>) -> Result<(), String> {
        let x_str = x.map(|v| v.to_string());
        let y_str = y.map(|v| v.to_string());
        let server = self.detect().await?.server;
        match server {
            ServerKind::X11 => {
                if let (Some(x), Some(y)) = (&x_str, &y_str) {
                    x11_run(&["xdotool", "mousemove", x, y]).await?;
                }
                x11_run(&["xdotool", "click", "1"]).await
            }
            ServerKind::Wayland => {
                if let (Some(x), Some(y)) = (&x_str, &y_str) {
                    wayland_run(&["ydotool", "mousemove", "--absolute", "--", x, y]).await?;
                }
                wayland_run(&["ydotool", "click", "0xC0"]).await
            }
        }
    }

    async fn type_text(&self, text: &str) -> Result<(), String> {
        match self.detect().await?.server {
            ServerKind::X11 => x11_run(&["xdotool", "type", "--delay", "30", "--", text]).await,
            ServerKind::Wayland => wayland_type_run(&["wtype", "--", text]).await,
        }
    }

    async fn scroll(&self, x: Option<i32>, y: Option<i32>, delta: i32) -> Result<(), String> {
        let x_str = x.map(|v| v.to_string());
        let y_str = y.map(|v| v.to_string());
        let server = self.detect().await?.server;
        match server {
            ServerKind::X11 => {
                // xdotool: teker 4 = yukari, 5 = asagi.
                let button = if delta >= 0 { "5" } else { "4" };
                let count = delta.unsigned_abs().max(1).to_string();
                if let (Some(x), Some(y)) = (&x_str, &y_str) {
                    x11_run(&["xdotool", "mousemove", x, y]).await?;
                }
                x11_run(&[
                    "xdotool", "click", "--repeat", &count, "--delay", "20", button,
                ])
                .await
            }
            ServerKind::Wayland => {
                if let (Some(x), Some(y)) = (&x_str, &y_str) {
                    wayland_run(&["ydotool", "mousemove", "--absolute", "--", x, y]).await?;
                }
                // ydotool: pozitif teker degeri = yukari, negatif = asagi.
                let wheel = (-delta).to_string();
                wayland_run(&["ydotool", "mousemove", "--wheel", "--", "0", &wheel]).await
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tool girdisi / ciktisi
// ---------------------------------------------------------------------------

/// `grok_computer` tool girdisi.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GrokComputerInput {
    /// Masaustu eylemi: `detect` | `screenshot` | `click` | `type` |
    /// `scroll` | `move`.
    #[schemars(description = "Desktop action: \"detect\", \"screenshot\", \"click\", \"type\", \"scroll\" or \"move\".")]
    pub action: String,
    /// Yatay piksel — `click` ve `move` icin zorunlu, `scroll` icin opsiyonel.
    #[serde(default)]
    #[schemars(description = "X coordinate in pixels (required for click/move, optional for scroll).")]
    pub x: Option<i32>,
    /// Dikey piksel — `click` ve `move` icin zorunlu, `scroll` icin opsiyonel.
    #[serde(default)]
    #[schemars(description = "Y coordinate in pixels (required for click/move, optional for scroll).")]
    pub y: Option<i32>,
    /// Yazilacak metin — `type` icin zorunlu.
    #[serde(default)]
    #[schemars(description = "Text to type (required for \"type\").")]
    pub text: Option<String>,
    /// Kaydirma miktari — `scroll` icin zorunlu; pozitif = asagi, negatif = yukari.
    #[serde(default)]
    #[schemars(description = "Scroll amount for \"scroll\": positive scrolls down, negative scrolls up.")]
    pub scroll_delta: Option<i32>,
}

/// `grok_computer` tool ciktisi: eylem ozeti + modele hazir markdown.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GrokComputerOutput {
    /// Uygulanan eylemin sabit etiketi.
    pub action: String,
    /// Tespit edilen goruntu sunucusu (`wayland` | `x11`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_server: Option<String>,
    /// Arka uc adi.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
    /// Ekran goruntusu dosya yolu (yalnizca `screenshot`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Modele okunabilir ozet (markdown; ayni veriden uretilir).
    pub content: String,
}

// ---------------------------------------------------------------------------
// Yurutme (arka uctan bagimsiz, test edilebilir cekirdek)
// ---------------------------------------------------------------------------

/// Yurutme hatalari: masaustu yokluğu ile calisma-zamani hatasi ayirt edilir;
/// boylece `service_unavailable` ile `execution` karistirilmaz.
#[derive(Debug)]
pub enum ComputerExecError {
    /// Ne Wayland ne X11 tespit edilebildi.
    NoDisplay(String),
    /// Arka uc / dosya sistemi hatasi.
    Backend(String),
}

impl ComputerExecError {
    /// Tool katmanina donen hata sekli.
    pub fn into_tool_error(self, tool_id: &xai_tool_protocol::ToolId) -> xai_tool_runtime::ToolError {
        match self {
            ComputerExecError::NoDisplay(msg) => {
                xai_tool_runtime::ToolError::service_unavailable(msg)
            }
            ComputerExecError::Backend(msg) => {
                xai_tool_runtime::ToolError::execution(tool_id.clone(), msg)
            }
        }
    }
}

fn no_display(msg: String) -> ComputerExecError {
    ComputerExecError::NoDisplay(msg)
}

fn backend_err(msg: String) -> ComputerExecError {
    ComputerExecError::Backend(msg)
}

/// `~/.grok/computer` dizini.
fn screenshot_dir(home: &Path) -> PathBuf {
    home.join(".grok").join("computer")
}

/// `~/.grok/computer/<ts>.png` yolu.
fn screenshot_path(home: &Path, ts_ms: i64) -> PathBuf {
    screenshot_dir(home).join(format!("{ts_ms}.png"))
}

fn home_dir() -> Result<PathBuf, ComputerExecError> {
    dirs::home_dir()
        .ok_or_else(|| backend_err("ev dizini bulunamadi (~/.grok/computer yazilamaz)".to_string()))
}

/// Ekran goruntusu akisi: dizini kur, arka uca yazdir, dogrula.
///
/// `home` parametresi testlerin `dirs::home_dir`'e baglanmamasini saglar;
/// uretim yolu [`execute_computer`] burayi cozulen ev diziniyle cagirir.
async fn run_screenshot(
    backend: &dyn ComputerBackend,
    home: &Path,
) -> Result<PathBuf, ComputerExecError> {
    let ts_ms = chrono::Utc::now().timestamp_millis();
    let path = screenshot_path(home, ts_ms);
    std::fs::create_dir_all(screenshot_dir(home)).map_err(|err| {
        backend_err(format!(
            "ekran goruntusu dizini olusturulamadi ({}): {err}",
            screenshot_dir(home).display()
        ))
    })?;
    backend.screenshot(&path).await.map_err(backend_err)?;
    verify_screenshot(&path)?;
    Ok(path)
}

/// Ekran goruntusu dosyasi yazildi mi ve bos degil mi?
fn verify_screenshot(path: &Path) -> Result<(), ComputerExecError> {
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_file() && meta.len() > 0 => Ok(()),
        Ok(_) => Err(backend_err(format!(
            "ekran goruntusu bos dosya uretti: {}",
            path.display()
        ))),
        Err(err) => Err(backend_err(format!(
            "ekran goruntusu dosyasi okunamadi ({}): {err}",
            path.display()
        ))),
    }
}

/// Eylem parametrelerini dogrular. Arka uca ulasmadan once yapilir (I6).
fn validate_action(
    action: ComputerAction,
    input: &GrokComputerInput,
) -> Result<(), xai_tool_runtime::ToolError> {
    match action {
        ComputerAction::Click | ComputerAction::Move => {
            if input.x.is_none() || input.y.is_none() {
                return Err(xai_tool_runtime::ToolError::invalid_arguments(format!(
                    "grok_computer: '{action}' icin 'x' ve 'y' koordinatlari gerekli"
                )));
            }
        }
        ComputerAction::Type => {
            let text = input.text.as_deref().map(str::trim).unwrap_or("");
            if text.is_empty() {
                return Err(xai_tool_runtime::ToolError::invalid_arguments(
                    "grok_computer: 'type' icin bos olmayan 'text' gerekli".to_string(),
                ));
            }
        }
        ComputerAction::Scroll => match input.scroll_delta {
            Some(0) | None => {
                return Err(xai_tool_runtime::ToolError::invalid_arguments(
                    "grok_computer: 'scroll' icin sifir olmayan 'scroll_delta' gerekli"
                        .to_string(),
                ));
            }
            Some(_) => {}
        },
        ComputerAction::Detect | ComputerAction::Screenshot => {}
    }
    Ok(())
}

/// Tek eylemi calistirir. Arka uctan ve `SharedResources`'tan bagimsizdir;
/// bu yuzden testler sahte arka uc ile dogrudan dogrular.
async fn execute_computer(
    action: ComputerAction,
    input: &GrokComputerInput,
    backend: &dyn ComputerBackend,
) -> Result<GrokComputerOutput, ComputerExecError> {
    match action {
        ComputerAction::Detect => {
            let info = backend.detect().await.map_err(no_display)?;
            let backend_name = info.backend.to_string();
            Ok(GrokComputerOutput {
                action: action.to_string(),
                display_server: Some(info.server.to_string()),
                backend: Some(info.backend),
                path: None,
                content: format!(
                    "# Detect\n\n- **Display server:** `{}`\n- **Backend:** `{}`\n",
                    info.server, backend_name
                ),
            })
        }
        ComputerAction::Screenshot => {
            let info = backend.detect().await.map_err(no_display)?;
            let backend_name = info.backend.to_string();
            let home = home_dir()?;
            let path = run_screenshot(backend, &home).await?;
            Ok(GrokComputerOutput {
                action: action.to_string(),
                display_server: Some(info.server.to_string()),
                backend: Some(info.backend),
                path: Some(path.display().to_string()),
                content: format!(
                    "# Screenshot\n\n- **Path:** `{}`\n- **Display server:** `{}`\n- **Backend:** `{}`\n",
                    path.display(),
                    info.server,
                    backend_name
                ),
            })
        }
        ComputerAction::Click => {
            let info = backend.detect().await.map_err(no_display)?;
            backend
                .click(input.x, input.y)
                .await
                .map_err(backend_err)?;
            let position = match (input.x, input.y) {
                (Some(x), Some(y)) => format!("({x}, {y})"),
                _ => "current cursor position".to_string(),
            };
            Ok(GrokComputerOutput {
                action: action.to_string(),
                display_server: Some(info.server.to_string()),
                backend: Some(info.backend),
                path: None,
                content: format!(
                    "# Click\n\n- **Position:** {position}\n- **Display server:** `{}`\n",
                    info.server
                ),
            })
        }
        ComputerAction::Type => {
            let text = input.text.as_deref().unwrap_or("").trim();
            let info = backend.detect().await.map_err(no_display)?;
            backend.type_text(text).await.map_err(backend_err)?;
            Ok(GrokComputerOutput {
                action: action.to_string(),
                display_server: Some(info.server.to_string()),
                backend: Some(info.backend),
                path: None,
                content: format!(
                    "# Type\n\n- **Text:** `{}`\n- **Display server:** `{}`\n",
                    inline(text),
                    info.server
                ),
            })
        }
        ComputerAction::Scroll => {
            let delta = input.scroll_delta.unwrap_or(0);
            let info = backend.detect().await.map_err(no_display)?;
            backend
                .scroll(input.x, input.y, delta)
                .await
                .map_err(backend_err)?;
            let direction = if delta < 0 { "up" } else { "down" };
            Ok(GrokComputerOutput {
                action: action.to_string(),
                display_server: Some(info.server.to_string()),
                backend: Some(info.backend),
                path: None,
                content: format!(
                    "# Scroll\n\n- **Delta:** `{delta}` ({direction})\n- **Display server:** `{}`\n",
                    info.server
                ),
            })
        }
        ComputerAction::Move => {
            let (x, y) = match (input.x, input.y) {
                (Some(x), Some(y)) => (x, y),
                _ => {
                    return Err(backend_err(
                        "grok_computer: 'move' icin 'x' ve 'y' gerekli".to_string(),
                    ));
                }
            };
            let info = backend.detect().await.map_err(no_display)?;
            backend.mouse_move(x, y).await.map_err(backend_err)?;
            Ok(GrokComputerOutput {
                action: action.to_string(),
                display_server: Some(info.server.to_string()),
                backend: Some(info.backend),
                path: None,
                content: format!(
                    "# Move\n\n- **Position:** ({x}, {y})\n- **Display server:** `{}`\n",
                    info.server
                ),
            })
        }
    }
}

/// Satir ici markdown icin: satir sonlarini bosluga cevirir, ` ` kacar.
fn inline(s: &str) -> String {
    s.replace(['\r', '\n'], " ").replace('`', "'")
}

// ---------------------------------------------------------------------------
// Tool yuzeyi
// ---------------------------------------------------------------------------

/// `grok_computer` tool'u.
#[derive(Debug, Default)]
pub struct GrokComputerTool;

impl ToolMetadata for GrokComputerTool {
    fn kind(&self) -> ToolKind {
        // Masaustu dokunusu dis dunyayi degistirir (Write), salt-okunur degil.
        ToolKind::Write
    }

    fn tool_namespace(&self) -> ToolNamespace {
        ToolNamespace::GrokBuild
    }

    fn description_template(&self) -> &str {
        "Control the desktop: detect the active display server, take screenshots, and perform \
         mouse/keyboard actions. Actions: \"detect\" (report display server: wayland or x11), \
         \"screenshot\" (capture the screen to ~/.grok/computer/<timestamp>.png and return its \
         path), \"click\" (click at x/y; moves the pointer first when coordinates are given), \
         \"type\" (type text), \"scroll\" (scroll by scroll_delta; positive = down, negative = \
         up; optional x/y), \"move\" (move the pointer to x/y). When no display server is \
         present the tool returns a clear error message instead of failing hard."
    }

    fn requires_expr(&self) -> Expr<ToolRequirement> {
        Expr::True
    }
}

/// `Tool::id` icin sabit kimlik.
///
/// I6: uretim yolunda `unwrap` / `expect` / `panic!` yoktur. Buradaki
/// adaylar derleme aninda sabit statik dizelerdir ("grok_computer"; bos
/// degil, `[a-zA-Z0-9_-]+` biciminde, ayrilmis on ek tasimiyor) —
/// kullanicidan veya config'ten gelen hicbir deger bu fonksiyondan gecmez,
/// bu yuzden son hata dali gerceklesemez (testler bunu dogrular).
fn tool_id() -> xai_tool_protocol::ToolId {
    static ID: std::sync::OnceLock<xai_tool_protocol::ToolId> = std::sync::OnceLock::new();
    ID.get_or_init(|| {
        xai_tool_protocol::ToolId::new("grok_computer").unwrap_or_else(|_| {
            xai_tool_protocol::ToolId::new("grok_computer_tool").unwrap_or_else(|_| {
                xai_tool_protocol::ToolId::new("computer").unwrap_or_else(|_| {
                    unreachable!("statik tool id adaylari gecerlidir")
                })
            })
        })
    })
    .clone()
}

/// Arguman degerinden eylem cozer; gecersiz eylemde `ToolError` doner.
fn parse_action(raw: &str) -> Result<ComputerAction, xai_tool_runtime::ToolError> {
    ComputerAction::parse(raw).ok_or_else(|| {
        xai_tool_runtime::ToolError::invalid_arguments(format!(
            "grok_computer: invalid action '{raw}' — expected one of: detect | screenshot | click | type | scroll | move"
        ))
    })
}

impl xai_tool_runtime::Tool for GrokComputerTool {
    type Args = GrokComputerInput;
    type Output = GrokComputerOutput;

    fn id(&self) -> xai_tool_protocol::ToolId {
        tool_id()
    }

    fn description(
        &self,
        _ctx: &::xai_tool_runtime::ListToolsContext,
    ) -> xai_tool_types::ToolDescription {
        xai_tool_types::ToolDescription::new(
            "grok_computer",
            ToolMetadata::description_template(self),
        )
    }

    fn capabilities(&self) -> xai_tool_protocol::ToolCapabilities {
        xai_tool_protocol::ToolCapabilities {
            is_read_only: false,
            tool_scope: Some(xai_tool_protocol::ToolScope::Write),
            max_concurrency: Some(1),
            timeout_ms: Some(30_000),
            ..Default::default()
        }
    }

    #[tracing::instrument(name = "tool.grok_computer", skip_all)]
    async fn run(
        &self,
        ctx: xai_tool_runtime::ToolCallContext,
        input: GrokComputerInput,
    ) -> Result<GrokComputerOutput, xai_tool_runtime::ToolError> {
        let action = parse_action(&input.action)?;
        validate_action(action, &input)?;

        let resources = shared_resources(&ctx)?;
        let backend: Arc<dyn ComputerBackend> = {
            let res = resources.lock().await;
            // I6: host arka uc enjekte etmediyse panik yok — yerlesik
            // ortam adaptoru devreye girer.
            match res.require::<Arc<dyn ComputerBackend>>() {
                Ok(backend) => backend.clone(),
                Err(_) => Arc::new(EnvComputerBackend),
            }
        };

        execute_computer(action, &input, backend.as_ref())
            .await
            .map_err(|err| err.into_tool_error(&tool_id()))
    }
}

impl xai_tool_runtime::ToolOutput for GrokComputerOutput {}

impl From<GrokComputerInput> for ToolInput {
    fn from(input: GrokComputerInput) -> Self {
        ToolInput::Dynamic(serde_json::json!({
            "action": input.action,
            "x": input.x,
            "y": input.y,
            "text": input.text,
            "scroll_delta": input.scroll_delta,
        }))
    }
}

impl From<GrokComputerOutput> for ToolOutput {
    fn from(output: GrokComputerOutput) -> Self {
        ToolOutput::Dynamic(DynamicOutput {
            value: serde_json::to_value(&output).unwrap_or_default(),
        })
    }
}

// ---------------------------------------------------------------------------
// Testler
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::types::ToolRegistryBuilder;
    use crate::types::resources::Resources;
    use crate::types::tool_metadata::test_ctx_with_call_id;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Cagrilari sayan, agsiz sahte arka uc. `detect` davranisi kurulabilir.
    #[derive(Debug)]
    struct StubBackend {
        name: String,
        server: Option<ServerKind>,
        clicks: AtomicUsize,
        moves: AtomicUsize,
        types: AtomicUsize,
        scrolls: AtomicUsize,
        shots: AtomicUsize,
    }

    impl StubBackend {
        fn new(name: &str, server: Option<ServerKind>) -> Self {
            Self {
                name: name.to_string(),
                server,
                clicks: AtomicUsize::new(0),
                moves: AtomicUsize::new(0),
                types: AtomicUsize::new(0),
                scrolls: AtomicUsize::new(0),
                shots: AtomicUsize::new(0),
            }
        }

        fn calls(&self) -> usize {
            self.clicks.load(Ordering::SeqCst)
                + self.moves.load(Ordering::SeqCst)
                + self.types.load(Ordering::SeqCst)
                + self.scrolls.load(Ordering::SeqCst)
                + self.shots.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl ComputerBackend for StubBackend {
        fn name(&self) -> &str {
            &self.name
        }

        async fn detect(&self) -> Result<DetectInfo, String> {
            match self.server {
                Some(server) => Ok(DetectInfo {
                    server,
                    backend: self.name.clone(),
                }),
                None => Err("görüntü sunucusu bulunamadi: stub".to_string()),
            }
        }

        async fn screenshot(&self, path: &Path) -> Result<(), String> {
            self.shots.fetch_add(1, Ordering::SeqCst);
            std::fs::write(path, b"\x89PNG-stub").map_err(|err| err.to_string())
        }

        async fn mouse_move(&self, _x: i32, _y: i32) -> Result<(), String> {
            self.moves.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn click(&self, _x: Option<i32>, _y: Option<i32>) -> Result<(), String> {
            self.clicks.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn type_text(&self, _text: &str) -> Result<(), String> {
            self.types.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn scroll(&self, _x: Option<i32>, _y: Option<i32>, _delta: i32) -> Result<(), String> {
            self.scrolls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn input(action: &str) -> GrokComputerInput {
        GrokComputerInput {
            action: action.to_string(),
            x: None,
            y: None,
            text: None,
            scroll_delta: None,
        }
    }

    #[test]
    fn sunucu_tespiti_session_type_onceliklidir() {
        let env: std::collections::HashMap<&str, &str> = std::collections::HashMap::from([
            ("XDG_SESSION_TYPE", "wayland"),
            ("DISPLAY", ":0"),
        ]);
        let detected = ServerKind::from_env(|key| env.get(key).map(|v| (*v).to_owned()));
        assert_eq!(detected, Some(ServerKind::Wayland));
    }

    #[test]
    fn sunucu_tespiti_soket_degiskenlerine_duser() {
        let env: std::collections::HashMap<&str, &str> =
            std::collections::HashMap::from([("XDG_SESSION_TYPE", "tty"), ("DISPLAY", ":0")]);
        let detected = ServerKind::from_env(|key| env.get(key).map(|v| (*v).to_owned()));
        assert_eq!(detected, Some(ServerKind::X11));

        let empty: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
        assert_eq!(
            ServerKind::from_env(|key| empty.get(key).map(|v| (*v).to_owned())),
            None
        );
    }

    #[test]
    fn sunucu_tespiti_wayland_soketini_onceler() {
        let env: std::collections::HashMap<&str, &str> = std::collections::HashMap::from([
            ("XDG_SESSION_TYPE", "tty"),
            ("WAYLAND_DISPLAY", "wayland-0"),
            ("DISPLAY", ":0"),
        ]);
        let detected = ServerKind::from_env(|key| env.get(key).map(|v| (*v).to_owned()));
        assert_eq!(detected, Some(ServerKind::Wayland));
    }

    #[test]
    fn eylem_ayristirma_aliases_ile_calisir() {
        for action in ComputerAction::ALL {
            assert_eq!(ComputerAction::parse(action.as_str()), Some(action));
        }
        assert_eq!(ComputerAction::parse("TIKLA"), Some(ComputerAction::Click));
        assert_eq!(ComputerAction::parse("  scroll  "), Some(ComputerAction::Scroll));
        assert_eq!(ComputerAction::parse("kayip"), None);
        assert_eq!(ComputerAction::parse(""), None);
    }

    #[test]
    fn eksik_parametreler_reddedilir() {
        let err = validate_action(ComputerAction::Click, &input("click"))
            .expect_err("click koordinatsiz reddedilmeli");
        assert!(err.to_string().contains("koordinat"));

        let err = validate_action(ComputerAction::Move, &input("move"))
            .expect_err("move koordinatsiz reddedilmeli");
        assert!(err.to_string().contains("koordinat"));

        let err = validate_action(ComputerAction::Type, &input("type"))
            .expect_err("type metinsiz reddedilmeli");
        assert!(err.to_string().contains("text"));

        let mut scroll = input("scroll");
        scroll.scroll_delta = Some(0);
        let err = validate_action(ComputerAction::Scroll, &scroll)
            .expect_err("sifir delta reddedilmeli");
        assert!(err.to_string().contains("scroll_delta"));

        assert!(validate_action(ComputerAction::Detect, &input("detect")).is_ok());
        assert!(validate_action(ComputerAction::Screenshot, &input("screenshot")).is_ok());
    }

    #[test]
    fn tool_kimligi_sabit_ve_gecerli() {
        assert!(xai_tool_protocol::ToolId::new("grok_computer").is_ok());
        assert_eq!(tool_id().as_str(), "grok_computer");
    }

    #[test]
    fn registryye_kayitli() {
        let builder = ToolRegistryBuilder::new();
        assert!(
            builder.has_tool_id("GrokBuild:grok_computer"),
            "grok_computer registry'ye kayitli olmali"
        );
    }

    #[test]
    fn ekran_goruntusu_yolu_beklenen_dizinde() {
        let home = Path::new("/home/test");
        let path = screenshot_path(home, 1_700_000_000_123);
        assert_eq!(
            path,
            PathBuf::from("/home/test/.grok/computer/1700000000123.png")
        );
    }

    #[tokio::test]
    async fn detect_eylemi_sunucuyu_bildirir() {
        let backend = StubBackend::new("stub", Some(ServerKind::Wayland));
        let out = execute_computer(ComputerAction::Detect, &input("detect"), &backend)
            .await
            .expect("detect calisir");
        assert_eq!(out.display_server.as_deref(), Some("wayland"));
        assert_eq!(out.backend.as_deref(), Some("stub"));
        assert!(out.content.contains("## Detect"));
    }

    #[tokio::test]
    async fn click_eylemi_arka_uca_ulasiyor() {
        let backend = StubBackend::new("stub", Some(ServerKind::X11));
        let mut args = input("click");
        args.x = Some(10);
        args.y = Some(20);
        let out = execute_computer(ComputerAction::Click, &args, &backend)
            .await
            .expect("click calisir");
        assert_eq!(backend.calls(), 1);
        assert_eq!(out.display_server.as_deref(), Some("x11"));
        assert!(out.content.contains("(10, 20)"));
    }

    #[tokio::test]
    async fn screenshot_eylemi_dosyaya_yazar() {
        let backend = StubBackend::new("stub", Some(ServerKind::X11));
        let tmp = tempfile::tempdir().expect("temp dizin");
        let home = tmp.path().join("home");

        let path = run_screenshot(&backend, &home)
            .await
            .expect("goruntu yazilir");
        assert_eq!(backend.shots.load(Ordering::SeqCst), 1);
        assert!(path.starts_with(&home));
        assert!(path.ends_with(".png"));
        let meta = std::fs::metadata(&path).expect("dosya var");
        assert!(meta.len() > 0, "dosya bos olmamali");
    }

    /// I6: masaustu yokken hata doner, panik yok; `NoDisplay` varyanti
    /// "görüntü sunucusu bulunamadi" metnini tasir.
    #[tokio::test]
    async fn masaustu_yokken_durust_hata_metni_doner() {
        let backend = StubBackend::new("stub", None);
        let err = execute_computer(ComputerAction::Detect, &input("detect"), &backend)
            .await
            .expect_err("masaustu yokken reddedilmeli");
        assert!(matches!(err, ComputerExecError::NoDisplay(_)));
        let msg = format!("{err:?}");
        assert!(msg.contains("görüntü sunucusu bulunamadi"), "{msg}");
    }

    #[tokio::test]
    async fn masaustu_yokken_eylemler_de_reddedilir() {
        let backend = StubBackend::new("stub", None);
        let mut args = input("click");
        args.x = Some(1);
        args.y = Some(2);
        let result = execute_computer(ComputerAction::Click, &args, &backend).await;
        assert!(matches!(result, Err(ComputerExecError::NoDisplay(_))));
        assert_eq!(backend.calls(), 0, "arka uca ulasilmamali");
    }

    #[tokio::test]
    async fn tool_enjekte_edilen_arka_ucu_kullanir() {
        let mut resources = Resources::new();
        let backend: Arc<dyn ComputerBackend> = Arc::new(StubBackend::new("host", Some(ServerKind::X11)));
        resources.insert(backend);
        let tool = GrokComputerTool;

        let out = xai_tool_runtime::Tool::run(
            &tool,
            test_ctx_with_call_id(resources.into_shared(), "call-host"),
            GrokComputerInput {
                action: "detect".into(),
                x: None,
                y: None,
                text: None,
                scroll_delta: None,
            },
        )
        .await
        .expect("tool calisir");
        assert_eq!(out.backend.as_deref(), Some("host"));
        assert_eq!(out.display_server.as_deref(), Some("x11"));
    }

    #[tokio::test]
    async fn tool_yerlesik_arka_uca_duser() {
        // Host enjeksiyonu YOK: yerlesik `EnvComputerBackend` devreye girer.
        // Bu test calisma ortamindaki gercek ekran tespitine bagli oldugu
        // icin yalnizca "panik yok" dogrulanir: ya `detect` basarir ya da
        // duzgun bir `ToolError` doner.
        let resources = Resources::new();
        let tool = GrokComputerTool;
        let result = xai_tool_runtime::Tool::run(
            &tool,
            test_ctx_with_call_id(resources.into_shared(), "call-env"),
            GrokComputerInput {
                action: "detect".into(),
                x: None,
                y: None,
                text: None,
                scroll_delta: None,
            },
        )
        .await;
        match result {
            Ok(out) => assert_eq!(out.action, "detect"),
            Err(err) => {
                let msg = err.to_string();
                assert!(
                    msg.contains("görüntü sunucusu bulunamadi"),
                    "beklenmeyen hata: {msg}"
                );
            }
        }
    }

    #[tokio::test]
    async fn gecersiz_eylem_reddedilir() {
        let resources = Resources::new();
        let tool = GrokComputerTool;
        let result = xai_tool_runtime::Tool::run(
            &tool,
            test_ctx_with_call_id(resources.into_shared(), "call-bad"),
            GrokComputerInput {
                action: "teleport".into(),
                x: None,
                y: None,
                text: None,
                scroll_delta: None,
            },
        )
        .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid action"));
    }
}
