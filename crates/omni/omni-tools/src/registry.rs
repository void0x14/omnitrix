//! Faz 1 tool kablolamasi: read · search · write · hashline-edit (MASTER-PLAN 9.4/9.5).
//!
//! Bu fazda persona katalogu YOK; tool seti sabittir ve buradan cozulur.
//! Shell izin listesi ve acil kacis yolu bu fazin KAPSAMI DISINDADIR.
//!
//! Kablolamanin tamami vendored `xai-grok-tools` uzerinden yapilir: builder
//! zaten butun built-in tool'lari kayitli tutar, biz yalnizca `ToolServerConfig`
//! ile SECERIZ. Kendi tool'umuzu bu fazda eklemiyoruz.
//!
//! FS-shim enjeksiyonu: `fs` parametresi disaridan gelir (`crate::fs_shim`
//! diff akisi sarmalayicisini saglar). Tool katmani hangi dosya sisteminin
//! altta oldugunu bilmez — dikis noktasi tektir.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use xai_grok_tools::computer::types::{AsyncFileSystem, TerminalBackend};
use xai_grok_tools::implementations::opencode;
use xai_grok_tools::notification::ToolNotificationHandle;
use xai_grok_tools::registry::types::{
    FinalizedToolset, RequirementError, SessionContext, ToolConfig, ToolRegistryBuilder,
    ToolServerConfig,
};

use crate::error::ToolsError;

/// Hashline anchor semasinin adi. Hashline tool'lari AYNI `HashlineSchemeParams`
/// resource'unu paylasir; degerler ayrisirsa anchor'lar uyusmaz (son yazan kazanir).
const HASHLINE_SCHEME: &str = "chunk";
/// Anchor hash uzunlugu (1..=4).
const HASHLINE_HASH_LEN: i64 = 3;
/// Chunk semasinin parca boyu (> 0).
const HASHLINE_CHUNK_SIZE: i64 = 8;

/// Oturum kalintilarinin (state, log) yazildigi calisma dizini alt klasoru.
const SESSION_DIR: &str = ".omni";
/// `Resources` durumunun yeniden baslatmalar arasi tasindigi dosya.
const STATE_FILE: &str = "tools-state.json";

/// Faz 1 dikey diliminin acik tool seti.
///
/// Dort rol, dort tool:
/// - **read** → `GrokBuildHashline:hashline_read`. Duz `read_file` yerine
///   hashline surumu secilir; ciktisi anchor tasir ve `hashline_edit`
///   dogrudan tuketir (retrieval→edit zinciri kapanir, 9.4).
/// - **search** → `GrokBuildHashline:hashline_grep`. DIKKAT: vendored
///   `search_tool` kod aramasi DEGIL, tool kesfidir; arama rolu grep'e duser.
///   Grep de anchor'li surumdur — asagidaki demet kurali bunu zorunlu kilar.
/// - **write** → `OpenCode:write`. `GrokBuild` altinda adanmis write yoktur;
///   bu id her iki dosya demetinin de disindadir, karisim uyarisi uretmez.
/// - **hashline-edit** → `GrokBuildHashline:hashline_edit`.
///
/// DEMET KURALI (vendored `validate_config`): standart dosya ucusu
/// (`read_file`/`search_replace`/`grep`) ile hashline ucusu ayni sette
/// KARISTIRILAMAZ. 9.4 hashline'i sart kostugu icin demetin tamami hashline'dir.
pub fn faz1_tool_config() -> ToolServerConfig {
    ToolServerConfig {
        tools: vec![
            hashline_params(ToolConfig::from_id("GrokBuildHashline:hashline_read")),
            hashline_params(ToolConfig::from_id("GrokBuildHashline:hashline_grep")),
            ToolConfig::for_tool::<opencode::OpenCodeWriteTool>(),
            hashline_params(ToolConfig::from_id("GrokBuildHashline:hashline_edit")),
        ],
        behavior_preset: None,
    }
}

/// Hashline sema ayarlarini tek noktadan uygular.
fn hashline_params(config: ToolConfig) -> ToolConfig {
    config
        .with_param("scheme", HASHLINE_SCHEME)
        .with_param("hash_len", HASHLINE_HASH_LEN)
        .with_param("chunk_size", HASHLINE_CHUNK_SIZE)
}

/// Tool setindeki id'lerin listesi — broker izin listesini beslemek icin.
pub fn toolset_ids(config: &ToolServerConfig) -> Vec<String> {
    config.tools.iter().map(|t| t.id.clone()).collect()
}

/// Modele gorunen tool adlari — K3 broker'inin izin listesi bunlarla kurulur.
///
/// `ToolConfig.id` namespace'li ic addir ("GrokBuild:grep"); modelin cagirdigi
/// ad `name_override` uygulanmis halidir. Broker karari model adina gore
/// verildigi icin dogru kaynak burasidir.
pub fn toolset_tool_names(toolset: &FinalizedToolset) -> Vec<String> {
    toolset
        .tool_definitions()
        .into_iter()
        .map(|def| def.function.name)
        .collect()
}

/// Faz 1 tool setini kurar.
///
/// `fs` disaridan gelir; diff akisi sarmalayicisi (`crate::fs_shim`) buraya
/// takilir ve ajanin butun dosya dokunuslari gorunur hale gelir.
///
/// Calisma dizini surecin gecerli dizinidir; bildirim kanali `noop`'tur —
/// olay gunlugune baglanacak gercek kanal icin [`faz1_toolset_with`] kullanin.
///
/// Aktif bir tokio calisma zamani SARTTIR: `finalize` icerde gorev spawn eder.
pub fn faz1_toolset(
    fs: Arc<dyn AsyncFileSystem>,
    terminal: Arc<dyn TerminalBackend>,
) -> Result<FinalizedToolset, ToolsError> {
    let cwd = std::env::current_dir()
        .map_err(|err| ToolsError::InvalidConfig(format!("calisma dizini cozulemedi: {err}")))?;
    faz1_toolset_with(&cwd, fs, terminal, ToolNotificationHandle::noop())
}

/// Calisma dizini ve bildirim kanali disaridan verilen tam kontrollu kurulum.
///
/// `notifications` olay gunlugunun birincil kaynagidir (`FileWritten`, bash
/// ciktisi, gorev olaylari). `ToolNotificationHandle::noop()` her seyi sessizce
/// duser — basssiz calistirmalar disinda kullanmayin.
pub fn faz1_toolset_with(
    cwd: &Path,
    fs: Arc<dyn AsyncFileSystem>,
    terminal: Arc<dyn TerminalBackend>,
    notifications: ToolNotificationHandle,
) -> Result<FinalizedToolset, ToolsError> {
    let config = faz1_tool_config();
    let builder = ToolRegistryBuilder::new();

    // `finalize` bilinmeyen id'de indeksleme yapar; yapisal hatalari ONCE
    // burada yakalamazsak panik riski dogar (I6: panik yok).
    let errors = builder.validate_config(&config);
    if !errors.is_empty() {
        return Err(requirement_error(&errors));
    }

    let ctx = session_context(cwd, fs, terminal, notifications);
    builder
        .finalize(config, ctx)
        .map_err(|errs| requirement_error(&errs))
}

/// Yapisal `RequirementError` listesini tek satirlik yapilandirma hatasina cevirir.
fn requirement_error(errors: &[RequirementError]) -> ToolsError {
    let detail = errors
        .iter()
        .map(RequirementError::summary)
        .collect::<Vec<_>>()
        .join("; ");
    ToolsError::InvalidConfig(format!("Faz 1 tool seti dogrulanamadi: {detail}"))
}

/// Faz 1 oturum baglami. `SessionContext`'in `Default`'u YOKTUR; alanlarin
/// tamami acikca doldurulur. Kapali birakilan her yetenek (web, lsp, alt-ajan,
/// bellek) sonraki fazlara aittir.
fn session_context(
    cwd: &Path,
    fs: Arc<dyn AsyncFileSystem>,
    terminal: Arc<dyn TerminalBackend>,
    notifications: ToolNotificationHandle,
) -> SessionContext {
    let session_folder: PathBuf = cwd.join(SESSION_DIR);
    let state_path = session_folder.join(STATE_FILE);

    SessionContext {
        backend: terminal,
        fs,
        cwd: cwd.to_path_buf(),
        session_folder,
        session_env: Arc::new(HashMap::new()),
        notification_handle: notifications,
        owner_session_id: None,
        subagent: None,
        parent_scheduler_handle: None,
        skills: Vec::new(),
        state_path,
        memory_backend: None,
        web_search_config: Default::default(),
        web_fetch_config: Default::default(),
        lsp: None,
        image_gen_config: Default::default(),
        video_gen_config: Default::default(),
        app_builder_deployer_config: Default::default(),
        api_key_provider: None,
        auth_provider: None,
        attribution_callback: None,
        system_reminder_tag: xai_grok_tools::reminders::DEFAULT_REMINDER_TAG,
    }
}

/// Vendored calisma zamani hatasini omni-tools hatasina tasir.
///
/// `FinalizedToolset::call*` `xai_tool_runtime::ToolError` dondurur; tur
/// dongusunu suren omni-router bu donusumu tek noktadan yapar.
pub fn map_tool_error(err: xai_tool_runtime::ToolError) -> ToolsError {
    ToolsError::Runtime(err)
}

/// Tool bulunamadi / ad uyusmadi durumu icin calisma zamani hatasi uretir.
pub fn unknown_tool_error(tool_name: &str) -> ToolsError {
    ToolsError::Runtime(xai_tool_runtime::ToolError::invalid_arguments(format!(
        "'{tool_name}' Faz 1 tool setinde yok"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn faz1_seti_dort_tool_tasir() {
        let config = faz1_tool_config();
        assert_eq!(config.tools.len(), 4);
    }

    #[test]
    fn toolset_ids_namespace_tasir() {
        let ids = toolset_ids(&faz1_tool_config());
        assert!(ids.iter().all(|id| id.contains(':')), "{ids:?}");
        assert!(
            ids.iter().any(|id| id.ends_with(":hashline_edit")),
            "{ids:?}"
        );
    }

    #[test]
    fn hashline_ucusu_ayni_semayi_paylasir() {
        let config = faz1_tool_config();
        let params: Vec<_> = config
            .tools
            .iter()
            .filter(|t| t.id.starts_with("GrokBuildHashline:"))
            .map(|t| t.params.clone())
            .collect();
        assert_eq!(params.len(), 3);
        assert!(params.iter().all(|p| *p == params[0]), "{params:?}");
        assert!(params[0].is_some());
    }

    #[test]
    fn standart_dosya_demeti_karistirilmaz() {
        let ids = toolset_ids(&faz1_tool_config());
        for standart in [
            "GrokBuild:read_file",
            "GrokBuild:search_replace",
            "GrokBuild:grep",
        ] {
            assert!(!ids.iter().any(|id| id == standart), "{ids:?}");
        }
    }

    #[test]
    fn config_yapisal_olarak_gecerli() {
        let builder = ToolRegistryBuilder::new();
        let errors = builder.validate_config(&faz1_tool_config());
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[tokio::test]
    async fn toolset_kurulur_ve_dort_tool_yayinlar() {
        let dir = std::env::temp_dir().join("omni-tools-faz1-test");
        let built = faz1_toolset_with(
            &dir,
            crate::local_fs(),
            crate::local_terminal(),
            ToolNotificationHandle::noop(),
        );
        // Hata durumunda da assert'e dusulur; test yolunda dahi kontrolsuz
        // cikis yok (I6 uslubu).
        let names = match built {
            Ok(toolset) => toolset_tool_names(&toolset),
            Err(err) => vec![format!("kurulum hatasi: {err}")],
        };
        assert_eq!(names.len(), 4, "{names:?}");
        assert!(names.iter().any(|n| n == "write"), "{names:?}");
    }

    #[test]
    fn bilinmeyen_tool_hatasi_adi_tasir() {
        let err = unknown_tool_error("bash");
        assert!(err.to_string().contains("bash"));
    }
}
