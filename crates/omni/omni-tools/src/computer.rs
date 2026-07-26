//! Faz 10 — computer-use (MASTER-PLAN 17.3, AS12, K6).
//!
//! Bu modul yeni bir masaustu otomasyon yigini KURMAZ; mevcut
//! `xai-computer-hub-core` yuzeyini genisletir. Ajanin gordugu tek sey
//! `xai_tool_runtime::Tool` sozlesmesine uyan tek bir arac
//! ([`ComputerUseTool`]); hub tarafinda ise [`computer_use_handle`] ile
//! `ToolHandle`'a, [`ComputerUseTool::registration`] ile `ToolRegistration`'a
//! donusur — mcp-adapter'in urettigi kayit sekliyle birebir ayni sekil,
//! yani ayni kayit duzlemine takilir.
//!
//! **K6 — iki masaustu sunucusu.** [`DesktopBackend`] soyutlamasi Wayland ve
//! X11'i ESIT sekilde kapsar: [`BackendRegistry`] her iki [`DisplayServer`]
//! icin ayri bir arka uc tutar ve [`BackendRegistry::covers_all`] ikisinin de
//! kayitli olup olmadigini soyler. Somut protokol surucusu takildiginda
//! [`DisplayServer::detect`] hangi arka ucun secilecegini belirler; bosluk
//! kalan sunucu icin [`UnavailableBackend`] dikis noktasi olarak durur ve
//! cagriyi sessizce yutmak yerine hata dondurur.
//!
//! **R7 — hasar korumasi.** Iki bagimsiz kural:
//! 1. *Gorunurluk*: her masaustu eylemi, oturumun [`ActionJournal`]'ina bir
//!    JSON satiri olarak yazilir. Gunluk `crate::fs_shim::DiffShimFs`
//!    uzerinden yazildiginda her yazim bir `FileTouch` uretir — yani
//!    computer-use dokunusu diff akisinda gorunur (5.2). Eylem UYGULANMADAN
//!    ONCE "intent", sonrasinda "done"/"failed" satiri yazilir; yarim kalan
//!    eylem crash sonrasi gunlukten okunabilir (AS2 crash-only niyeti).
//! 2. *Geri alinabilirlik*: tehlikeli dokunus ana agacta degil,
//!    [`crate::worktree::Worktree`] ile acilan gecici git calisma agacinda
//!    kosar (K5, `xai-fast-worktree`). Sinirin TEK uygulamasi orasidir; bu
//!    modul yalnizca [`ComputerUseSession::with_boundary`] ile o agaci
//!    oturuma baglar ve yolunu/branch'ini her gunluk satirina, arac ciktisina
//!    ve hub kaydina yazar. Hata → `Worktree::discard`, ana agac bozulmaz;
//!    basari → `Worktree::promote` (kullanici onayli). Otomatik merge YOK (17.4).
//!
//! I6: bu modulde `unwrap`/`expect`/`panic!`/`todo!` yoktur; her hata
//! `Result` ile tasinir. `Drop` yolu bile hata yutmaz, yalnizca kayda gecer.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use base64::Engine as _;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use xai_computer_hub_core::{ErasedTool, ToolHandle};
use xai_grok_tools::computer::types::{AsyncFileSystem, ComputerError};
use xai_tool_protocol::{
    SessionId, ToolCapabilities, ToolId, ToolRegistration, ToolScope, TransportKind, UserId,
};
use xai_tool_runtime::{ContentBlock, ListToolsContext, Tool, ToolCallContext, ToolError, ToolOutput};
use xai_tool_types::ToolDescription;

use crate::error::ToolsError;

// ---------------------------------------------------------------------------
// K6 — masaustu sunucusu
// ---------------------------------------------------------------------------

/// Linux masaustu sunucusu. K6 ikisini de kapsamak zorundadir.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayServer {
    /// Wayland oturumu.
    Wayland,
    /// X11 oturumu.
    X11,
}

impl DisplayServer {
    /// Kapsanmasi zorunlu sunucularin tamami (K6 kapisi bunun uzerinden olculur).
    pub const ALL: [DisplayServer; 2] = [DisplayServer::Wayland, DisplayServer::X11];

    /// Kayitlara ve metadata'ya giden sabit etiket.
    pub fn as_str(self) -> &'static str {
        match self {
            DisplayServer::Wayland => "wayland",
            DisplayServer::X11 => "x11",
        }
    }

    /// Surec ortamindan tespit.
    ///
    /// Sira: `XDG_SESSION_TYPE` > `WAYLAND_DISPLAY` > `DISPLAY`. Hicbiri
    /// anlamli degilse `None` — bu, "masaustu yok" demektir ve cagri
    /// [`ComputerUseError::NoDisplayServer`] ile reddedilir.
    pub fn detect() -> Option<Self> {
        Self::from_env(|key| std::env::var(key).ok())
    }

    /// [`DisplayServer::detect`]'in test edilebilir govdesi.
    ///
    /// `lookup` ortam degiskenini cozer; bos deger "yok" sayilir.
    pub fn from_env(lookup: impl Fn(&str) -> Option<String>) -> Option<Self> {
        let non_empty = |key: &str| lookup(key).filter(|value| !value.trim().is_empty());

        if let Some(session_type) = non_empty("XDG_SESSION_TYPE") {
            match session_type.trim().to_ascii_lowercase().as_str() {
                "wayland" => return Some(DisplayServer::Wayland),
                "x11" => return Some(DisplayServer::X11),
                // Diger degerler (tty, unspecified, ...) baglayici degil;
                // asagidaki soket degiskenlerine bakilir.
                _ => {}
            }
        }
        if non_empty("WAYLAND_DISPLAY").is_some() {
            return Some(DisplayServer::Wayland);
        }
        if non_empty("DISPLAY").is_some() {
            return Some(DisplayServer::X11);
        }
        None
    }
}

impl fmt::Display for DisplayServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// Hata tipi
// ---------------------------------------------------------------------------

/// Computer-use yuzeyinin hata tipi.
#[derive(Debug, thiserror::Error)]
pub enum ComputerUseError {
    /// Ne Wayland ne X11 tespit edilebildi.
    #[error("masaustu oturumu yok: ne Wayland ne X11 tespit edilebildi")]
    NoDisplayServer,

    /// Tespit edilen sunucu icin kayitli arka uc yok (K6 boslugu).
    #[error("{0} icin kayitli masaustu arka ucu yok")]
    BackendMissing(DisplayServer),

    /// Arka uc bu eylemi desteklemiyor.
    #[error("{server} arka ucu '{action}' eylemini desteklemiyor")]
    Unsupported {
        /// Eylemi reddeden sunucu.
        server: DisplayServer,
        /// Reddedilen eylemin sabit etiketi.
        action: &'static str,
    },

    /// Arka ucun kendi hatasi.
    #[error("masaustu arka uc hatasi: {0}")]
    Backend(String),

    /// Eylem gunlugu yazilamadi — R7 gorunurlugu saglanamadigi icin eylem dusurulur.
    #[error("eylem gunlugu yazilamadi: {0}")]
    Journal(#[from] ComputerError),

    /// K5 geri-alma siniri kurulamadi ya da atilamadi.
    #[error("geri-alma siniri hatasi: {0}")]
    Boundary(String),

    /// Model gecersiz bir eylem parametresi uretti.
    #[error("gecersiz eylem parametresi: {0}")]
    InvalidAction(String),
}

impl ComputerUseError {
    /// Tool katmanina donen hata sekli.
    ///
    /// Yetki/ortam eksikligi ile calisma-zamani hatasi ayirt edilir; boylece
    /// yargic ve denetim kaydi "yapilamaz" ile "yapilamadi"yi karistirmaz.
    pub fn into_tool_error(self, tool_id: &ToolId) -> ToolError {
        match self {
            ComputerUseError::NoDisplayServer | ComputerUseError::BackendMissing(_) => {
                ToolError::service_unavailable(self.to_string())
            }
            ComputerUseError::Unsupported { .. } | ComputerUseError::InvalidAction(_) => {
                ToolError::invalid_arguments(self.to_string())
            }
            ComputerUseError::Backend(_)
            | ComputerUseError::Journal(_)
            | ComputerUseError::Boundary(_) => {
                ToolError::execution(tool_id.clone(), self.to_string())
            }
        }
    }
}

impl From<ComputerUseError> for ToolsError {
    fn from(value: ComputerUseError) -> Self {
        match value {
            ComputerUseError::Journal(inner) => ToolsError::FileSystem(inner),
            other => ToolsError::InvalidConfig(other.to_string()),
        }
    }
}

// ---------------------------------------------------------------------------
// Eylem sozlugu
// ---------------------------------------------------------------------------

/// Isaretci dugmesi.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PointerButton {
    /// Sol dugme (varsayilan).
    #[default]
    Left,
    /// Orta dugme.
    Middle,
    /// Sag dugme.
    Right,
}

impl PointerButton {
    /// Kayitlara giden sabit etiket.
    pub fn as_str(self) -> &'static str {
        match self {
            PointerButton::Left => "left",
            PointerButton::Middle => "middle",
            PointerButton::Right => "right",
        }
    }
}

/// `count` alaninin varsayilani.
fn one() -> u32 {
    1
}

/// Ajanin isteyebilecegi masaustu eylemleri.
///
/// Kapali kume: arka uclar bu sozlugu genisletemez, yalnizca uygular ya da
/// [`ComputerUseError::Unsupported`] dondurur. Boylece Wayland ve X11
/// uygulamalari ayni modele ayni yuzeyi gosterir (K6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum DesktopAction {
    /// Ekran goruntusu al.
    Screenshot,
    /// Isaretcinin bulundugu noktayi sor.
    CursorPosition,
    /// Isaretciyi tasi.
    MouseMove {
        /// Yatay piksel.
        x: i32,
        /// Dikey piksel.
        y: i32,
    },
    /// Tikla. Koordinat verilmezse isaretcinin bulundugu nokta kullanilir.
    Click {
        /// Dugme.
        #[serde(default)]
        button: PointerButton,
        /// Hedef yatay piksel.
        #[serde(default)]
        x: Option<i32>,
        /// Hedef dikey piksel.
        #[serde(default)]
        y: Option<i32>,
        /// Ardisik tiklama sayisi (cift tiklama icin 2).
        #[serde(default = "one")]
        count: u32,
    },
    /// Basili tutarak surukle.
    Drag {
        /// Baslangic yatay piksel.
        from_x: i32,
        /// Baslangic dikey piksel.
        from_y: i32,
        /// Bitis yatay piksel.
        to_x: i32,
        /// Bitis dikey piksel.
        to_y: i32,
        /// Basili tutulan dugme.
        #[serde(default)]
        button: PointerButton,
    },
    /// Kaydir.
    Scroll {
        /// Kaydirmanin uygulanacagi yatay piksel.
        x: i32,
        /// Kaydirmanin uygulanacagi dikey piksel.
        y: i32,
        /// Yatay kaydirma miktari.
        delta_x: i32,
        /// Dikey kaydirma miktari.
        delta_y: i32,
    },
    /// Tus kombinasyonu gonder (ornek: `["ctrl", "s"]`).
    KeyPress {
        /// Ayni anda basilacak tuslar.
        keys: Vec<String>,
    },
    /// Metin yaz.
    TypeText {
        /// Yazilacak metin.
        text: String,
    },
    /// Bekle.
    Wait {
        /// Milisaniye.
        millis: u64,
    },
}

impl DesktopAction {
    /// Gunluge ve hata metnine giden sabit etiket.
    pub fn label(&self) -> &'static str {
        match self {
            DesktopAction::Screenshot => "screenshot",
            DesktopAction::CursorPosition => "cursor_position",
            DesktopAction::MouseMove { .. } => "mouse_move",
            DesktopAction::Click { .. } => "click",
            DesktopAction::Drag { .. } => "drag",
            DesktopAction::Scroll { .. } => "scroll",
            DesktopAction::KeyPress { .. } => "key_press",
            DesktopAction::TypeText { .. } => "type_text",
            DesktopAction::Wait { .. } => "wait",
        }
    }

    /// Eylem dis dunyayi degistiriyor mu?
    ///
    /// Salt-okunur eylemler de gunluge yazilir (gorunurluk), ama yalnizca
    /// degistiren eylemler K5 sinirini zorunlu kilar.
    pub fn mutates(&self) -> bool {
        !matches!(
            self,
            DesktopAction::Screenshot | DesktopAction::CursorPosition | DesktopAction::Wait { .. }
        )
    }

    /// Parametre dogrulamasi. Arka uca ulasmadan once yapilir.
    pub fn validate(&self) -> Result<(), ComputerUseError> {
        match self {
            DesktopAction::Click { count, .. } => {
                if *count == 0 {
                    return Err(ComputerUseError::InvalidAction(
                        "tiklama sayisi en az 1 olmali".to_owned(),
                    ));
                }
                Ok(())
            }
            DesktopAction::KeyPress { keys } => {
                if keys.is_empty() || keys.iter().any(|key| key.trim().is_empty()) {
                    return Err(ComputerUseError::InvalidAction(
                        "tus listesi bos olamaz".to_owned(),
                    ));
                }
                Ok(())
            }
            DesktopAction::TypeText { text } => {
                if text.is_empty() {
                    return Err(ComputerUseError::InvalidAction(
                        "yazilacak metin bos olamaz".to_owned(),
                    ));
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

/// Ekran olculeri.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenGeometry {
    /// Genislik (piksel).
    pub width: u32,
    /// Yukseklik (piksel).
    pub height: u32,
}

/// Arka uctan donen ham ekran goruntusu.
#[derive(Clone, PartialEq, Eq)]
pub struct ScreenImage {
    /// Ornek: `image/png`.
    pub mime_type: String,
    /// Kodlanmamis goruntu baytlari.
    pub bytes: Vec<u8>,
}

impl fmt::Debug for ScreenImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ScreenImage")
            .field("mime_type", &self.mime_type)
            .field("bytes", &self.bytes.len())
            .finish()
    }
}

/// Base64'e kodlanmis, modele/istemciye giden goruntu.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EncodedImage {
    /// Ornek: `image/png`.
    pub mime_type: String,
    /// Standart base64 govde.
    pub data: String,
}

impl From<ScreenImage> for EncodedImage {
    fn from(value: ScreenImage) -> Self {
        Self {
            mime_type: value.mime_type,
            data: base64::engine::general_purpose::STANDARD.encode(value.bytes),
        }
    }
}

/// Arka ucun tek bir eylem icin urettigi sonuc.
#[derive(Debug, Clone, PartialEq)]
pub enum DesktopOutcome {
    /// Eylem uygulandi, dondurulecek veri yok.
    Done,
    /// Isaretci konumu.
    Point {
        /// Yatay piksel.
        x: i32,
        /// Dikey piksel.
        y: i32,
    },
    /// Serbest metin (arka ucun tanilamasi).
    Text(String),
    /// Ekran goruntusu.
    Image(ScreenImage),
}

// ---------------------------------------------------------------------------
// Arka uc soyutlamasi (K6)
// ---------------------------------------------------------------------------

/// Masaustu arka ucu — Wayland ve X11 icin AYNI sozlesme.
///
/// Nesne-guvenli tutuldu: [`BackendRegistry`] arka uclari `Arc<dyn ...>`
/// olarak tasir, boylece Wayland ve X11 uygulamalari ayni surecte yan yana
/// yasayabilir ve calisma zamaninda secilebilir.
#[async_trait]
pub trait DesktopBackend: Send + Sync + fmt::Debug {
    /// Bu arka ucun konustugu masaustu sunucusu.
    fn display_server(&self) -> DisplayServer;

    /// Kayitlara giden arka uc adi.
    fn name(&self) -> &str;

    /// Ekran olculeri.
    async fn geometry(&self) -> Result<ScreenGeometry, ComputerUseError>;

    /// Tek bir eylemi uygular.
    ///
    /// Uygulanamayan eylem icin [`ComputerUseError::Unsupported`] doner;
    /// sessizce basarili donmek YASAK — R7 gorunurlugu buna dayanir.
    async fn perform(&self, action: &DesktopAction) -> Result<DesktopOutcome, ComputerUseError>;
}

/// Henuz surucusu takilmamis bir sunucu icin dikis noktasi.
///
/// K6 boslugunu gorunur kilar: kayitta yerini alir, listelerde gorunur, ama
/// her cagriyi acik bir hatayla reddeder. Somut Wayland/X11 surucusu
/// takildiginda [`BackendRegistry::with_backend`] ile bunun uzerine yazilir.
#[derive(Debug, Clone)]
pub struct UnavailableBackend {
    server: DisplayServer,
    name: String,
}

impl UnavailableBackend {
    /// `server` icin yer tutucu arka uc.
    pub fn new(server: DisplayServer) -> Self {
        Self {
            server,
            name: format!("{server}-unavailable"),
        }
    }
}

#[async_trait]
impl DesktopBackend for UnavailableBackend {
    fn display_server(&self) -> DisplayServer {
        self.server
    }

    fn name(&self) -> &str {
        &self.name
    }

    async fn geometry(&self) -> Result<ScreenGeometry, ComputerUseError> {
        Err(ComputerUseError::BackendMissing(self.server))
    }

    async fn perform(&self, action: &DesktopAction) -> Result<DesktopOutcome, ComputerUseError> {
        Err(ComputerUseError::Unsupported {
            server: self.server,
            action: action.label(),
        })
    }
}

/// Sunucu -> arka uc esleme tablosu.
///
/// K6 kapisi: [`BackendRegistry::covers_all`] hem Wayland hem X11 icin bir
/// arka uc kayitliysa `true` doner.
#[derive(Debug, Default)]
pub struct BackendRegistry {
    backends: BTreeMap<DisplayServer, Arc<dyn DesktopBackend>>,
}

impl BackendRegistry {
    /// Bos tablo.
    pub fn new() -> Self {
        Self::default()
    }

    /// Her iki sunucu icin yer tutucu arka uc iceren tablo.
    ///
    /// Somut surucu takilana kadar kullanilan taban: kapsam K6'yi karsilar,
    /// davranis ise acik hata dondurur.
    pub fn seam() -> Self {
        let mut registry = Self::new();
        for server in DisplayServer::ALL {
            registry.insert(Arc::new(UnavailableBackend::new(server)));
        }
        registry
    }

    /// Arka ucu kendi bildirdigi sunucuya kaydeder; ayni sunucudaki onceki
    /// kaydin uzerine yazar.
    pub fn insert(&mut self, backend: Arc<dyn DesktopBackend>) -> Option<Arc<dyn DesktopBackend>> {
        self.backends.insert(backend.display_server(), backend)
    }

    /// [`BackendRegistry::insert`]'in zincirlenebilir bicimi.
    pub fn with_backend(mut self, backend: Arc<dyn DesktopBackend>) -> Self {
        self.insert(backend);
        self
    }

    /// Kayitli arka ucu getirir.
    pub fn get(&self, server: DisplayServer) -> Option<&Arc<dyn DesktopBackend>> {
        self.backends.get(&server)
    }

    /// Kayitli sunucularin listesi (kararli sirada).
    pub fn servers(&self) -> Vec<DisplayServer> {
        self.backends.keys().copied().collect()
    }

    /// K6: hem Wayland hem X11 kayitli mi?
    pub fn covers_all(&self) -> bool {
        DisplayServer::ALL
            .iter()
            .all(|server| self.backends.contains_key(server))
    }

    /// Verilen sunucunun arka ucunu cozer.
    pub fn resolve(
        &self,
        server: DisplayServer,
    ) -> Result<Arc<dyn DesktopBackend>, ComputerUseError> {
        self.backends
            .get(&server)
            .cloned()
            .ok_or(ComputerUseError::BackendMissing(server))
    }

    /// Ortamdan tespit edilen sunucunun arka ucunu cozer.
    pub fn resolve_detected(&self) -> Result<Arc<dyn DesktopBackend>, ComputerUseError> {
        let server = DisplayServer::detect().ok_or(ComputerUseError::NoDisplayServer)?;
        self.resolve(server)
    }
}

// ---------------------------------------------------------------------------
// R7 (1) — diff akisinda gorunurluk
// ---------------------------------------------------------------------------

/// Gunluk satirinin evresi.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalPhase {
    /// Eylem uygulanmadan once yazilir (crash-only niyet).
    Intent,
    /// Eylem basariyla bitti.
    Done,
    /// Eylem hata verdi.
    Failed,
}

/// Gunluge yazilan tek satir.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalEntry {
    /// Oturum icinde artan sira numarasi.
    pub seq: u64,
    /// RFC3339 zaman damgasi.
    pub at: String,
    /// Evre.
    pub phase: JournalPhase,
    /// Eylemin sabit etiketi.
    pub action: String,
    /// Eylemi uygulayan sunucu.
    pub display_server: DisplayServer,
    /// Arka uc adi.
    pub backend: String,
    /// Eylemin tam parametreleri / sonucu.
    pub detail: serde_json::Value,
}

/// Computer-use eylem gunlugu — R7 gorunurluk yarisi.
///
/// `fs` olarak `crate::fs_shim::DiffShimFs` verildiginde her yazim shim'den
/// gecer ve bir `FileTouch` uretir; boylece masaustu dokunusu TUI/WebUI'nin
/// diff akisinda belirir. Alt katman `AsyncFileSystem` ekleme (append)
/// sunmadigi icin gunluk bellekte biriktirilir ve her satirda dosyanin
/// tamami yeniden yazilir — shim satir farkini kendi hesaplar.
pub struct ActionJournal {
    fs: Arc<dyn AsyncFileSystem>,
    path: PathBuf,
    lines: Mutex<Vec<String>>,
}

impl fmt::Debug for ActionJournal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ActionJournal")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl ActionJournal {
    /// `fs` diff akisini besleyen dosya sistemi, `path` gunluk dosyasi.
    pub fn new(fs: Arc<dyn AsyncFileSystem>, path: PathBuf) -> Self {
        Self {
            fs,
            path,
            lines: Mutex::new(Vec::new()),
        }
    }

    /// Gunluk dosyasinin yolu.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Su ana kadar yazilmis satir sayisi.
    pub async fn len(&self) -> usize {
        self.lines.lock().await.len()
    }

    /// Gunluk bos mu?
    pub async fn is_empty(&self) -> bool {
        self.len().await == 0
    }

    /// Bir satir ekler ve dosyayi yeniden yazar.
    ///
    /// Yazim basarisiz olursa satir bellekten de geri alinir; boylece gunluk
    /// ile diskteki dosya ayrisamaz.
    pub async fn record(&self, entry: &JournalEntry) -> Result<(), ComputerUseError> {
        let line = serde_json::to_string(entry).map_err(|err| {
            ComputerUseError::Journal(ComputerError::io(format!(
                "gunluk satiri kodlanamadi: {err}"
            )))
        })?;

        let mut lines = self.lines.lock().await;
        lines.push(line);
        let mut body = lines.join("\n");
        body.push('\n');

        match self.fs.write_file(&self.path, body.as_bytes()).await {
            Ok(()) => Ok(()),
            Err(err) => {
                lines.pop();
                Err(ComputerUseError::Journal(err))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Oturum
// ---------------------------------------------------------------------------

/// Tek bir computer-use oturumu: arka uc + gunluk + K5 sinir kimligi.
///
/// Sinirin KENDISI burada tutulmaz. [`crate::worktree::Worktree`] degeri
/// cagiran katmanin (omni-agent / omnitrix) elindedir, cunku atma
/// (`discard`) / terfi (`promote`) karari gorev sonucuna bakan yerde ve
/// kullanici onayiyla verilir. Oturum yalnizca sinirin KIMLIGINI —yol ve
/// branch— tasir ve her kayda yazar.
pub struct ComputerUseSession {
    backend: Arc<dyn DesktopBackend>,
    journal: ActionJournal,
    boundary_path: Option<PathBuf>,
    boundary_branch: Option<String>,
    seq: AtomicU64,
}

impl fmt::Debug for ComputerUseSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ComputerUseSession")
            .field("backend", &self.backend.name())
            .field("display_server", &self.backend.display_server())
            .field("journal", &self.journal)
            .field("boundary_path", &self.boundary_path)
            .field("boundary_branch", &self.boundary_branch)
            .finish()
    }
}

impl ComputerUseSession {
    /// Arka uc ve gunlukten oturum kurar.
    pub fn new(backend: Arc<dyn DesktopBackend>, journal: ActionJournal) -> Self {
        Self {
            backend,
            journal,
            boundary_path: None,
            boundary_branch: None,
            seq: AtomicU64::new(0),
        }
    }

    /// K5 geri-alma sinirini oturuma baglar (R7 ikinci yarisi).
    ///
    /// Agacin sahipligi cagiranda kalir; burada yalnizca yol ve branch adi
    /// kopyalanir. Bagli oldugunda her gunluk satiri, arac ciktisi ve hub
    /// kaydi hangi gecici agacta calisildigini gosterir — yani dokunus
    /// yalnizca gorunur degil, ADRESLENEBILIR sekilde geri alinabilir olur.
    pub fn with_boundary(self, worktree: &crate::worktree::Worktree) -> Self {
        self.with_boundary_parts(
            worktree.path().to_path_buf(),
            Some(worktree.branch().to_owned()),
        )
    }

    /// K5 sinirinin yolunu (ve varsa branch adini) dogrudan baglar.
    ///
    /// [`ComputerUseSession::with_boundary`] bunu `Worktree` uzerinden cagirir;
    /// dogrudan bicim yalnizca sinir baska bir yerde kurulmusken gerekir.
    pub fn with_boundary_parts(mut self, path: PathBuf, branch: Option<String>) -> Self {
        self.boundary_path = Some(path);
        self.boundary_branch = branch;
        self
    }

    /// Oturumun arka ucu.
    pub fn backend(&self) -> &Arc<dyn DesktopBackend> {
        &self.backend
    }

    /// Gunluk dosyasinin yolu.
    pub fn journal_path(&self) -> &Path {
        self.journal.path()
    }

    /// Bagli K5 sinirinin yolu.
    pub fn boundary_path(&self) -> Option<&Path> {
        self.boundary_path.as_deref()
    }

    /// Bagli K5 sinirinin gecici branch adi.
    pub fn boundary_branch(&self) -> Option<&str> {
        self.boundary_branch.as_deref()
    }

    /// Ekran olculeri.
    pub async fn geometry(&self) -> Result<ScreenGeometry, ComputerUseError> {
        self.backend.geometry().await
    }

    /// Tek eylem calistirir.
    ///
    /// Sira: dogrula → "intent" satiri → arka uc → "done"/"failed" satiri.
    /// Gunluk yazilamazsa eylem HIC calistirilmaz; gorunurluk saglanamayan
    /// dokunusa izin verilmez (R7).
    pub async fn perform(
        &self,
        action: &DesktopAction,
    ) -> Result<ComputerUseOutput, ComputerUseError> {
        action.validate()?;

        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        let detail = serde_json::to_value(action).unwrap_or(serde_json::Value::Null);
        self.write_entry(seq, JournalPhase::Intent, action.label(), detail)
            .await?;

        match self.backend.perform(action).await {
            Ok(outcome) => {
                let output = self.build_output(action, outcome);
                self.write_entry(
                    seq,
                    JournalPhase::Done,
                    action.label(),
                    output.journal_detail(),
                )
                .await?;
                Ok(output)
            }
            Err(err) => {
                let detail = serde_json::json!({ "error": err.to_string() });
                // Basarisizlik satiri yazilamasa bile asil hata korunur.
                if let Err(journal_err) = self
                    .write_entry(seq, JournalPhase::Failed, action.label(), detail)
                    .await
                {
                    tracing::warn!(
                        target: "omni_tools::computer",
                        error = %journal_err,
                        "basarisizlik satiri gunluge yazilamadi"
                    );
                }
                Err(err)
            }
        }
    }

    /// Gunluk satiri uretir ve yazar.
    async fn write_entry(
        &self,
        seq: u64,
        phase: JournalPhase,
        action: &str,
        detail: serde_json::Value,
    ) -> Result<(), ComputerUseError> {
        let entry = JournalEntry {
            seq,
            at: chrono::Utc::now().to_rfc3339(),
            phase,
            action: action.to_owned(),
            display_server: self.backend.display_server(),
            backend: self.backend.name().to_owned(),
            detail,
        };
        self.journal.record(&entry).await
    }

    /// Arka uc sonucunu modele donen sekle cevirir.
    fn build_output(&self, action: &DesktopAction, outcome: DesktopOutcome) -> ComputerUseOutput {
        let mut output = ComputerUseOutput {
            action: action.label().to_owned(),
            display_server: self.backend.display_server(),
            backend: self.backend.name().to_owned(),
            journal_path: self.journal.path().to_path_buf(),
            boundary_path: self.boundary_path.clone(),
            boundary_branch: self.boundary_branch.clone(),
            cursor: None,
            text: None,
            image: None,
        };
        match outcome {
            DesktopOutcome::Done => {}
            DesktopOutcome::Point { x, y } => output.cursor = Some(CursorPoint { x, y }),
            DesktopOutcome::Text(text) => output.text = Some(text),
            DesktopOutcome::Image(image) => output.image = Some(image.into()),
        }
        output
    }
}

/// Isaretci konumu (modele donen sekil).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorPoint {
    /// Yatay piksel.
    pub x: i32,
    /// Dikey piksel.
    pub y: i32,
}

/// Computer-use aracinin ciktisi.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComputerUseOutput {
    /// Uygulanan eylemin sabit etiketi.
    pub action: String,
    /// Eylemi uygulayan masaustu sunucusu.
    pub display_server: DisplayServer,
    /// Arka uc adi.
    pub backend: String,
    /// R7: bu dokunusun diff akisina dustugu gunluk dosyasi.
    pub journal_path: PathBuf,
    /// K5 geri-alma sinirinin yolu (kuruluysa).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boundary_path: Option<PathBuf>,
    /// K5 geri-alma sinirinin gecici branch adi (kuruluysa).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boundary_branch: Option<String>,
    /// Isaretci konumu (sorulduysa).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<CursorPoint>,
    /// Arka ucun serbest metni.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Ekran goruntusu (alindiysa).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<EncodedImage>,
}

impl ComputerUseOutput {
    /// Gunlugun "done" satirina giden ozet.
    ///
    /// Goruntu govdesi gunluge YAZILMAZ; yalnizca boyutu gecer — gunluk
    /// dosyasi diff akisina dustugu icin megabaytlarca base64 tasimaz.
    pub fn journal_detail(&self) -> serde_json::Value {
        serde_json::json!({
            "action": self.action,
            "cursor": self.cursor,
            "text": self.text,
            "image_bytes": self.image.as_ref().map(|image| image.data.len()),
            "boundary_path": self.boundary_path,
            "boundary_branch": self.boundary_branch,
        })
    }

    /// Modele giden kisa ozet satiri.
    pub fn summary(&self) -> String {
        match (&self.cursor, &self.text) {
            (Some(point), _) => format!(
                "{} ({}) -> ({}, {})",
                self.action, self.display_server, point.x, point.y
            ),
            (None, Some(text)) => format!("{} ({}) -> {text}", self.action, self.display_server),
            (None, None) => format!("{} ({}) tamam", self.action, self.display_server),
        }
    }
}

impl ToolOutput for ComputerUseOutput {
    fn model_output(&self) -> Vec<ContentBlock> {
        let mut blocks = vec![ContentBlock::Text {
            text: self.summary(),
        }];
        if let Some(image) = &self.image {
            blocks.push(ContentBlock::Image {
                mime_type: image.mime_type.clone(),
                data: image.data.clone(),
                media_id: None,
                filename: None,
                path: None,
                metadata: std::collections::HashMap::new(),
            });
        }
        blocks
    }
}

// ---------------------------------------------------------------------------
// Hub araci
// ---------------------------------------------------------------------------

/// Modelin gordugu argüman sekli — dogrudan eylem sozlugu.
pub type ComputerUseArgs = DesktopAction;

/// `xai_tool_runtime::Tool` sozlesmesine uyan computer-use araci.
///
/// [`computer_use_handle`] ile hub'in `ToolHandle` duzlemine,
/// [`ComputerUseTool::registration`] ile `ToolRegistration` kaydina cevrilir.
pub struct ComputerUseTool {
    id: ToolId,
    session: Arc<ComputerUseSession>,
}

impl fmt::Debug for ComputerUseTool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ComputerUseTool")
            .field("id", &self.id)
            .field("session", &self.session)
            .finish()
    }
}

impl ComputerUseTool {
    /// Arac ad uzayi.
    pub const NAMESPACE: &'static str = "omni";
    /// Arac adi.
    pub const NAME: &'static str = "computer";

    /// Oturumdan arac kurar.
    ///
    /// `ToolId` kurulusta bir kez dogrulanir; sonrasinda `Tool::id` hatasiz
    /// calisir (I6 — sicak yolda `Result` acilmaz).
    pub fn new(session: Arc<ComputerUseSession>) -> Result<Self, ToolsError> {
        let id = ToolId::new(format!("{}:{}", Self::NAMESPACE, Self::NAME))
            .map_err(|err| ToolsError::InvalidConfig(err.to_string()))?;
        Ok(Self { id, session })
    }

    /// Aracin kimligi.
    pub fn tool_id(&self) -> &ToolId {
        &self.id
    }

    /// Aracin bagli oldugu oturum.
    pub fn session(&self) -> &Arc<ComputerUseSession> {
        &self.session
    }

    /// Argüman JSON semasi.
    pub fn args_schema() -> serde_json::Value {
        schemars::schema_for!(ComputerUseArgs).to_value()
    }

    /// Model-yuzu tanim.
    pub fn describe() -> ToolDescription {
        ToolDescription::new(
            Self::NAME,
            "Masaustunu kullanir: ekran goruntusu, isaretci, tus ve metin \
             eylemleri. Her eylem oturum gunlugune yazilir ve diff akisinda \
             gorunur; gecici calisma agaci acikken yapilan degisiklikler geri \
             alinabilir.",
        )
        .with_namespace(Self::NAMESPACE)
        .with_kind("computer_use")
        .with_arguments_schema(Self::args_schema())
    }

    /// Hub kayit satiri.
    ///
    /// mcp-adapter'in urettigiyle ayni sekil; ayni `ToolRegistry` duzlemine
    /// takilir. `metadata` alani K6/R7 durumunu tasir, boylece kayit
    /// listelendiginde hangi sunucunun ve hangi geri-alma sinirinin gecerli
    /// oldugu gorunur.
    pub fn registration(&self, user_id: UserId, sessions: Vec<SessionId>) -> ToolRegistration {
        ToolRegistration {
            tool_id: self.id.clone(),
            sessions: Some(sessions),
            user_id,
            server_id: None,
            description: Self::describe(),
            input_schema: Some(Self::args_schema()),
            capabilities: Some(Tool::capabilities(self)),
            notification_schemas: None,
            transport_kind: TransportKind::Local,
            if_match_generation: None,
            metadata: Some(serde_json::json!({
                "display_server": self.session.backend().display_server(),
                "backend": self.session.backend().name(),
                "journal_path": self.session.journal_path(),
                "boundary_path": self.session.boundary_path(),
                "boundary_branch": self.session.boundary_branch(),
            })),
        }
    }
}

impl Tool for ComputerUseTool {
    type Args = ComputerUseArgs;
    type Output = ComputerUseOutput;

    fn id(&self) -> ToolId {
        self.id.clone()
    }

    fn description(&self, _ctx: &ListToolsContext) -> ToolDescription {
        Self::describe()
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            // Dis dunyayi degistirir: cok-ajanli kosuda yalnizca lider ajana
            // yonlendirilmelidir.
            tool_scope: Some(ToolScope::Write),
            is_read_only: false,
            max_concurrency: Some(1),
            timeout_ms: Some(DEFAULT_ACTION_TIMEOUT_MS),
            ..ToolCapabilities::default()
        }
    }

    fn run(
        &self,
        _ctx: ToolCallContext,
        args: Self::Args,
    ) -> impl std::future::Future<Output = Result<Self::Output, ToolError>> + Send {
        let session = Arc::clone(&self.session);
        let tool_id = self.id.clone();
        async move {
            session
                .perform(&args)
                .await
                .map_err(|err| err.into_tool_error(&tool_id))
        }
    }
}

/// Tek eylem icin varsayilan zaman asimi.
const DEFAULT_ACTION_TIMEOUT_MS: u64 = 30_000;

/// Araci hub'in nesne-guvenli `ToolHandle` duzlemine tasir.
pub fn computer_use_handle(tool: ComputerUseTool) -> Arc<dyn ToolHandle> {
    Arc::new(ErasedTool::new(tool))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex as StdMutex;

    use crate::diff::FileTouch;
    use crate::fs_shim::{DiffShimFs, touch_channel};

    /// Bellekte yasayan dosya sistemi — diske hic dokunmaz.
    #[derive(Debug, Default)]
    struct MemFs {
        files: StdMutex<HashMap<PathBuf, Vec<u8>>>,
    }

    #[async_trait]
    impl AsyncFileSystem for MemFs {
        async fn read_file(&self, path: &Path) -> Result<Vec<u8>, ComputerError> {
            match self.files.lock() {
                Ok(files) => files
                    .get(path)
                    .cloned()
                    .ok_or_else(|| ComputerError::io_with_kind("yok", std::io::ErrorKind::NotFound)),
                Err(err) => Err(ComputerError::io(err.to_string())),
            }
        }

        async fn write_file(&self, path: &Path, data: &[u8]) -> Result<(), ComputerError> {
            match self.files.lock() {
                Ok(mut files) => {
                    files.insert(path.to_path_buf(), data.to_vec());
                    Ok(())
                }
                Err(err) => Err(ComputerError::io(err.to_string())),
            }
        }

        async fn delete_file(&self, path: &Path) -> Result<(), ComputerError> {
            match self.files.lock() {
                Ok(mut files) => {
                    files.remove(path);
                    Ok(())
                }
                Err(err) => Err(ComputerError::io(err.to_string())),
            }
        }
    }

    /// Cagrilari sayan sahte arka uc.
    #[derive(Debug)]
    struct FakeBackend {
        server: DisplayServer,
        name: String,
    }

    impl FakeBackend {
        fn new(server: DisplayServer) -> Self {
            Self {
                server,
                name: format!("{server}-fake"),
            }
        }
    }

    #[async_trait]
    impl DesktopBackend for FakeBackend {
        fn display_server(&self) -> DisplayServer {
            self.server
        }

        fn name(&self) -> &str {
            &self.name
        }

        async fn geometry(&self) -> Result<ScreenGeometry, ComputerUseError> {
            Ok(ScreenGeometry {
                width: 1920,
                height: 1080,
            })
        }

        async fn perform(
            &self,
            action: &DesktopAction,
        ) -> Result<DesktopOutcome, ComputerUseError> {
            match action {
                DesktopAction::Screenshot => Ok(DesktopOutcome::Image(ScreenImage {
                    mime_type: "image/png".to_owned(),
                    bytes: vec![1, 2, 3, 4],
                })),
                DesktopAction::CursorPosition => Ok(DesktopOutcome::Point { x: 12, y: 34 }),
                _ => Ok(DesktopOutcome::Done),
            }
        }
    }

    fn session_with_shim() -> (ComputerUseSession, tokio::sync::mpsc::UnboundedReceiver<FileTouch>)
    {
        let (sink, stream) = touch_channel();
        let root = std::env::temp_dir();
        let shim = DiffShimFs::new(Arc::new(MemFs::default()), root.clone(), sink);
        let journal = ActionJournal::new(Arc::new(shim), root.join("omni-computer-use.jsonl"));
        let backend = Arc::new(FakeBackend::new(DisplayServer::Wayland));
        (ComputerUseSession::new(backend, journal), stream)
    }

    #[test]
    fn display_server_detection_prefers_session_type() {
        let env: HashMap<&str, &str> = HashMap::from([
            ("XDG_SESSION_TYPE", "wayland"),
            ("DISPLAY", ":0"),
        ]);
        let detected = DisplayServer::from_env(|key| env.get(key).map(|v| (*v).to_owned()));
        assert_eq!(detected, Some(DisplayServer::Wayland));
    }

    #[test]
    fn display_server_detection_falls_back_to_display_socket() {
        let env: HashMap<&str, &str> =
            HashMap::from([("XDG_SESSION_TYPE", "tty"), ("DISPLAY", ":0")]);
        let detected = DisplayServer::from_env(|key| env.get(key).map(|v| (*v).to_owned()));
        assert_eq!(detected, Some(DisplayServer::X11));

        let empty: HashMap<&str, &str> = HashMap::new();
        assert_eq!(
            DisplayServer::from_env(|key| empty.get(key).map(|v| (*v).to_owned())),
            None
        );
    }

    #[test]
    fn seam_registry_covers_wayland_and_x11() {
        let registry = BackendRegistry::seam();
        assert!(registry.covers_all(), "K6: iki sunucu da kapsanmali");
        assert_eq!(
            registry.servers(),
            vec![DisplayServer::Wayland, DisplayServer::X11]
        );
    }

    #[tokio::test]
    async fn seam_backend_refuses_instead_of_pretending() {
        let registry = BackendRegistry::seam();
        let backend = registry
            .resolve(DisplayServer::X11)
            .expect("dikis arka ucu kayitli");
        let result = backend.perform(&DesktopAction::Screenshot).await;
        assert!(matches!(
            result,
            Err(ComputerUseError::Unsupported { .. })
        ));
    }

    #[tokio::test]
    async fn concrete_backend_overrides_seam() {
        let registry =
            BackendRegistry::seam().with_backend(Arc::new(FakeBackend::new(DisplayServer::X11)));
        let backend = registry.resolve(DisplayServer::X11).expect("arka uc var");
        assert_eq!(backend.name(), "x11-fake");
        assert!(registry.covers_all());
    }

    /// R7 kapisi: her masaustu eylemi diff akisinda gorunur.
    #[tokio::test]
    async fn every_action_lands_in_the_diff_stream() {
        let (session, mut stream) = session_with_shim();

        let output = session
            .perform(&DesktopAction::Click {
                button: PointerButton::Left,
                x: Some(10),
                y: Some(20),
                count: 1,
            })
            .await
            .expect("eylem calismali");

        assert_eq!(output.action, "click");
        assert_eq!(output.display_server, DisplayServer::Wayland);

        // intent + done => iki dokunus.
        let first = stream.try_recv().expect("intent dokunusu");
        let second = stream.try_recv().expect("done dokunusu");
        assert_eq!(first.path, session.journal_path());
        assert_eq!(second.path, session.journal_path());
        assert!(first.added >= 1, "intent satiri +1 uretmeli");
        assert!(second.added >= 1, "done satiri +1 uretmeli");
        assert_eq!(session.journal.len().await, 2);
    }

    #[tokio::test]
    async fn screenshot_becomes_an_image_content_block() {
        let (session, _stream) = session_with_shim();
        let output = session
            .perform(&DesktopAction::Screenshot)
            .await
            .expect("ekran goruntusu");
        let image = output.image.as_ref().expect("goruntu var");
        assert_eq!(image.mime_type, "image/png");

        let blocks = output.model_output();
        assert!(
            blocks
                .iter()
                .any(|block| matches!(block, ContentBlock::Image { .. })),
            "goruntu blogu uretilmeli"
        );

        // Gunluge base64 govdesi degil, yalnizca boyut gider.
        let detail = output.journal_detail();
        assert!(detail.get("image_bytes").is_some());
        assert!(detail.to_string().len() < 512);
    }

    #[tokio::test]
    async fn invalid_action_never_reaches_the_backend() {
        let (session, mut stream) = session_with_shim();
        let err = session
            .perform(&DesktopAction::KeyPress { keys: Vec::new() })
            .await
            .expect_err("bos tus listesi reddedilmeli");
        assert!(matches!(err, ComputerUseError::InvalidAction(_)));
        assert!(
            stream.try_recv().is_err(),
            "dogrulama oncesi gunluge yazilmamali"
        );
    }

    /// R7 ikinci yarisi: bagli K5 siniri her kayda adreslenebilir sekilde gecer.
    #[tokio::test]
    async fn boundary_identity_reaches_output_journal_and_registration() {
        let (session, _stream) = session_with_shim();
        let session = session.with_boundary_parts(
            PathBuf::from("/tmp/omni-k5/faz10"),
            Some("omni/k5/faz10".to_owned()),
        );

        let output = session
            .perform(&DesktopAction::Wait { millis: 0 })
            .await
            .expect("eylem calismali");
        assert_eq!(
            output.boundary_branch.as_deref(),
            Some("omni/k5/faz10"),
            "cikti sinirin branch'ini tasimali"
        );
        assert_eq!(
            output.journal_detail().get("boundary_branch"),
            Some(&serde_json::json!("omni/k5/faz10")),
            "gunluk satiri sinirin branch'ini tasimali"
        );

        let tool = ComputerUseTool::new(Arc::new(session)).expect("arac kurulmali");
        let user_id = UserId::new("omnitrix").expect("kullanici kimligi");
        let registration = tool.registration(user_id, Vec::new());
        let metadata = registration.metadata.expect("metadata");
        assert_eq!(
            metadata.get("boundary_branch"),
            Some(&serde_json::json!("omni/k5/faz10"))
        );
    }

    #[tokio::test]
    async fn registration_matches_the_derived_tool_id() {
        let (session, _stream) = session_with_shim();
        let tool = ComputerUseTool::new(Arc::new(session)).expect("arac kurulmali");

        let user_id = UserId::new("omnitrix").expect("kullanici kimligi");
        let session_id = SessionId::new("faz10").expect("oturum kimligi");
        let registration = tool.registration(user_id, vec![session_id]);

        assert_eq!(registration.transport_kind, TransportKind::Local);
        assert_eq!(
            registration.derive_tool_id().expect("kimlik turetilmeli"),
            registration.tool_id
        );
        assert_eq!(registration.tool_id.as_str(), "omni:computer");

        let capabilities = registration.capabilities.expect("yetenekler");
        assert_eq!(capabilities.tool_scope, Some(ToolScope::Write));
        assert!(!capabilities.is_read_only);
    }

    #[tokio::test]
    async fn handle_exposes_the_tool_on_the_hub_plane() {
        let (session, _stream) = session_with_shim();
        let tool = ComputerUseTool::new(Arc::new(session)).expect("arac kurulmali");
        let handle = computer_use_handle(tool);
        assert_eq!(handle.id().as_str(), "omni:computer");
        assert_eq!(
            handle.description(&ListToolsContext::default()).name,
            ComputerUseTool::NAME
        );
    }

    #[test]
    fn errors_split_unavailable_from_failed() {
        let tool_id = ToolId::new("omni:computer").expect("kimlik");
        let unavailable = ComputerUseError::NoDisplayServer.into_tool_error(&tool_id);
        let failed = ComputerUseError::Backend("kapali".to_owned()).into_tool_error(&tool_id);
        assert_ne!(unavailable.variant_name(), failed.variant_name());
    }
}
