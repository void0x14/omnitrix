//! Veri odaklı stack kataloğu.
//!
//! Yeni araç eklemek = buraya `StackDef` satırı (+ gerekirse yeni FormatKind).
//! Path adayları XDG/HOME env override'larını da tarar.

use std::path::{Path, PathBuf};

use super::formats::FormatKind;

/// Stack'in ne yapabildiği.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StackCapability {
    pub import: bool,
    pub export: bool,
}

/// Katalog girdisi.
#[derive(Clone, Debug)]
pub struct StackDef {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub format: FormatKind,
    pub capability: StackCapability,
    /// Path üreticileri (ilk var olan kullanılır; ProcessEnv için boş).
    pub path_candidates: fn() -> Vec<PathBuf>,
}

/// Tespit sonucu.
#[derive(Clone, Debug)]
pub struct StackPresence {
    pub def: &'static StackDef,
    pub path: Option<PathBuf>,
    pub key_count_hint: Option<usize>,
}

/// Tüm bilinen stack tanımları.
pub fn all_stack_defs() -> &'static [StackDef] {
    &STACKS
}

pub fn find_stack_def(id: &str) -> Option<&'static StackDef> {
    let needle = id.trim().to_ascii_lowercase();
    STACKS.iter().find(|s| s.id == needle || s.id.eq_ignore_ascii_case(id.trim()))
}

/// Kurulu/tespit edilen stack'ler (path var veya ProcessEnv).
pub fn detect_stacks() -> Vec<StackPresence> {
    let mut out = Vec::new();
    for def in &STACKS {
        if def.format == FormatKind::ProcessEnv {
            out.push(StackPresence {
                def,
                path: None,
                key_count_hint: None,
            });
            continue;
        }
        if let Some(path) = resolve_stack_path(def) {
            out.push(StackPresence {
                def,
                path: Some(path),
                key_count_hint: None,
            });
        }
    }
    out
}

/// Stack için ilk mevcut path (yoksa default aday[0] — export için oluşturma).
pub fn resolve_stack_path(def: &StackDef) -> Option<PathBuf> {
    let cands = (def.path_candidates)();
    cands.into_iter().find(|p| p.exists())
}

pub fn default_or_existing_path(def: &StackDef) -> PathBuf {
    if let Some(p) = resolve_stack_path(def) {
        return p;
    }
    let cands = (def.path_candidates)();
    cands
        .into_iter()
        .next()
        .unwrap_or_else(|| PathBuf::from(def.id))
}

// ── path helpers ──────────────────────────────────────────────────────────

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn xdg_data() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".local/share"))
}

fn xdg_config() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"))
}

fn paths_opencode() -> Vec<PathBuf> {
    vec![
        xdg_data().join("opencode/auth.json"),
        home().join(".local/share/opencode/auth.json"),
        xdg_config().join("opencode/auth.json"),
    ]
}

fn paths_kilo() -> Vec<PathBuf> {
    vec![
        xdg_data().join("kilo/auth.json"),
        home().join(".local/share/kilo/auth.json"),
    ]
}

fn paths_pi() -> Vec<PathBuf> {
    vec![home().join(".pi/agent/auth.json")]
}

fn paths_codex() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(h) = std::env::var_os("CODEX_HOME") {
        v.push(PathBuf::from(h).join("auth.json"));
    }
    v.push(home().join(".codex/auth.json"));
    v
}

fn paths_claude_cred() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(d) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        v.push(PathBuf::from(&d).join(".credentials.json"));
        v.push(PathBuf::from(&d).join("settings.json"));
    }
    if let Some(d) = std::env::var_os("CLAUDE_SECURESTORAGE_CONFIG_DIR") {
        v.push(PathBuf::from(d).join(".credentials.json"));
    }
    v.push(home().join(".claude/.credentials.json"));
    v.push(home().join(".claude/settings.json"));
    v
}

fn paths_gemini() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(d) = std::env::var_os("GEMINI_CLI_HOME") {
        v.push(PathBuf::from(&d).join("settings.json"));
        v.push(PathBuf::from(&d).join("oauth_creds.json"));
    }
    v.push(home().join(".gemini/settings.json"));
    v.push(home().join(".gemini/oauth_creds.json"));
    v
}

fn paths_hermes() -> Vec<PathBuf> {
    vec![home().join(".hermes/auth.json")]
}

fn paths_aider() -> Vec<PathBuf> {
    vec![
        home().join(".aider.conf.yml"),
        xdg_config().join("aider/aider.conf.yml"),
        PathBuf::from(".aider.conf.yml"),
    ]
}

fn paths_continue() -> Vec<PathBuf> {
    vec![
        home().join(".continue/config.json"),
        home().join(".continue/config.yaml"),
        xdg_config().join("continue/config.json"),
    ]
}

fn paths_copilot() -> Vec<PathBuf> {
    vec![
        home().join(".copilot/config.json"),
        xdg_config().join("github-copilot/hosts.json"),
        xdg_config().join("github-copilot/apps.json"),
    ]
}

fn paths_cursor() -> Vec<PathBuf> {
    vec![
        xdg_config().join("cursor/auth.json"),
        home().join(".cursor/mcp.json"),
        home().join(".config/Cursor/User/globalStorage/state.vscdb"),
    ]
}

fn paths_windsurf() -> Vec<PathBuf> {
    vec![
        home().join(".codeium/windsurf/mcp_config.json"),
        xdg_config().join("Windsurf/User/globalStorage"),
        home().join(".windsurf"),
    ]
}

fn paths_cline() -> Vec<PathBuf> {
    vec![
        xdg_config().join("Code/User/globalStorage/saoudrizwan.claude-dev/settings"),
        xdg_config().join("Cursor/User/globalStorage/saoudrizwan.claude-dev/settings"),
    ]
}

fn paths_roo() -> Vec<PathBuf> {
    vec![
        xdg_config().join("Code/User/globalStorage/rooveterinaryinc.roo-cline/settings"),
        xdg_config().join("Cursor/User/globalStorage/rooveterinaryinc.roo-cline/settings"),
    ]
}

fn paths_kilocode_ext() -> Vec<PathBuf> {
    vec![
        xdg_config().join("Code/User/globalStorage/kilocode.kilo-code/settings"),
        xdg_config().join("Cursor/User/globalStorage/kilocode.kilo-code/settings"),
    ]
}

fn paths_qwen() -> Vec<PathBuf> {
    vec![
        home().join(".qwen/auth.json"),
        home().join(".qwen/oauth_creds.json"),
        xdg_data().join("qwen/auth.json"),
    ]
}

fn paths_kimi() -> Vec<PathBuf> {
    vec![
        home().join(".kimi/auth.json"),
        xdg_data().join("kimi/auth.json"),
    ]
}

fn paths_crush() -> Vec<PathBuf> {
    vec![
        home().join(".crush/auth.json"),
        xdg_config().join("crush/auth.json"),
    ]
}

fn paths_antigravity() -> Vec<PathBuf> {
    vec![
        home().join(".antigravity/auth.json"),
        home().join(".gemini/antigravity-auth.json"),
        xdg_config().join("antigravity/auth.json"),
    ]
}

fn paths_kiro() -> Vec<PathBuf> {
    vec![
        home().join(".kiro/auth.json"),
        xdg_config().join("kiro/auth.json"),
    ]
}

fn paths_amp() -> Vec<PathBuf> {
    vec![
        home().join(".amp/auth.json"),
        xdg_config().join("amp/auth.json"),
        home().join(".config/amp/auth.json"),
    ]
}

fn paths_factory() -> Vec<PathBuf> {
    vec![
        home().join(".factory/auth.json"),
        xdg_config().join("factory/auth.json"),
    ]
}

fn paths_poolside() -> Vec<PathBuf> {
    vec![xdg_config().join("poolside/credentials.json")]
}

fn paths_higgsfield() -> Vec<PathBuf> {
    vec![xdg_config().join("higgsfield/credentials.json")]
}

fn paths_context7() -> Vec<PathBuf> {
    vec![home().join(".context7/credentials.json")]
}

fn paths_dotenv_project() -> Vec<PathBuf> {
    vec![
        PathBuf::from(".env"),
        PathBuf::from(".env.local"),
        home().join(".env"),
    ]
}

fn paths_empty() -> Vec<PathBuf> {
    Vec::new()
}

fn both() -> StackCapability {
    StackCapability {
        import: true,
        export: true,
    }
}

fn import_only() -> StackCapability {
    StackCapability {
        import: true,
        export: false,
    }
}

static STACKS: [StackDef; 28] = [
    StackDef {
        id: "opencode",
        label: "OpenCode",
        description: "OpenCode auth.json (provider→api key haritası)",
        format: FormatKind::ProviderAuthMap,
        capability: StackCapability {
            import: true,
            export: true,
        },
        path_candidates: paths_opencode,
    },
    StackDef {
        id: "kilo",
        label: "Kilo CLI",
        description: "Kilo auth.json (OpenCode-uyumlu harita)",
        format: FormatKind::ProviderAuthMap,
        capability: StackCapability {
            import: true,
            export: true,
        },
        path_candidates: paths_kilo,
    },
    StackDef {
        id: "pi",
        label: "pi coding-agent",
        description: "~/.pi/agent/auth.json",
        format: FormatKind::ProviderAuthMap,
        capability: StackCapability {
            import: true,
            export: true,
        },
        path_candidates: paths_pi,
    },
    StackDef {
        id: "codex",
        label: "OpenAI Codex CLI",
        description: "~/.codex/auth.json",
        format: FormatKind::CodexAuth,
        capability: StackCapability {
            import: true,
            export: true,
        },
        path_candidates: paths_codex,
    },
    StackDef {
        id: "claude-code",
        label: "Claude Code",
        description: "~/.claude credentials + settings env",
        format: FormatKind::ClaudeCredentials,
        capability: StackCapability {
            import: true,
            export: true,
        },
        path_candidates: paths_claude_cred,
    },
    StackDef {
        id: "gemini-cli",
        label: "Gemini CLI",
        description: "~/.gemini settings/oauth",
        format: FormatKind::JsonKeyScan,
        capability: StackCapability {
            import: true,
            export: false,
        },
        path_candidates: paths_gemini,
    },
    StackDef {
        id: "hermes",
        label: "Hermes Agent",
        description: "~/.hermes/auth.json credential_pool",
        format: FormatKind::HermesAuth,
        capability: StackCapability {
            import: true,
            export: true,
        },
        path_candidates: paths_hermes,
    },
    StackDef {
        id: "aider",
        label: "Aider",
        description: ".aider.conf.yml",
        format: FormatKind::AiderYaml,
        capability: StackCapability {
            import: true,
            export: true,
        },
        path_candidates: paths_aider,
    },
    StackDef {
        id: "continue",
        label: "Continue.dev",
        description: "~/.continue/config.json",
        format: FormatKind::ContinueConfig,
        capability: StackCapability {
            import: true,
            export: true,
        },
        path_candidates: paths_continue,
    },
    StackDef {
        id: "copilot-cli",
        label: "GitHub Copilot CLI",
        description: "~/.copilot + github-copilot config",
        format: FormatKind::JsonKeyScan,
        capability: StackCapability {
            import: true,
            export: false,
        },
        path_candidates: paths_copilot,
    },
    StackDef {
        id: "cursor",
        label: "Cursor",
        description: "Cursor auth/mcp (okunabilir dosyalar)",
        format: FormatKind::JsonKeyScan,
        capability: StackCapability {
            import: true,
            export: false,
        },
        path_candidates: paths_cursor,
    },
    StackDef {
        id: "windsurf",
        label: "Windsurf",
        description: "Windsurf/Codeium config",
        format: FormatKind::JsonKeyScan,
        capability: StackCapability {
            import: true,
            export: false,
        },
        path_candidates: paths_windsurf,
    },
    StackDef {
        id: "cline",
        label: "Cline",
        description: "VS Code/Cursor Cline globalStorage",
        format: FormatKind::JsonKeyScan,
        capability: StackCapability {
            import: true,
            export: false,
        },
        path_candidates: paths_cline,
    },
    StackDef {
        id: "roo",
        label: "Roo Code",
        description: "Roo Code globalStorage",
        format: FormatKind::JsonKeyScan,
        capability: StackCapability {
            import: true,
            export: false,
        },
        path_candidates: paths_roo,
    },
    StackDef {
        id: "kilocode",
        label: "Kilo Code (VS Code)",
        description: "Kilo Code extension storage",
        format: FormatKind::JsonKeyScan,
        capability: StackCapability {
            import: true,
            export: false,
        },
        path_candidates: paths_kilocode_ext,
    },
    StackDef {
        id: "qwen",
        label: "Qwen Code",
        description: "~/.qwen auth",
        format: FormatKind::JsonKeyScan,
        capability: StackCapability {
            import: true,
            export: true,
        },
        path_candidates: paths_qwen,
    },
    StackDef {
        id: "kimi",
        label: "Kimi CLI",
        description: "~/.kimi auth",
        format: FormatKind::JsonKeyScan,
        capability: StackCapability {
            import: true,
            export: true,
        },
        path_candidates: paths_kimi,
    },
    StackDef {
        id: "crush",
        label: "Crush",
        description: "~/.crush auth",
        format: FormatKind::JsonKeyScan,
        capability: StackCapability {
            import: true,
            export: true,
        },
        path_candidates: paths_crush,
    },
    StackDef {
        id: "antigravity",
        label: "Google Antigravity",
        description: "Antigravity auth",
        format: FormatKind::JsonKeyScan,
        capability: StackCapability {
            import: true,
            export: false,
        },
        path_candidates: paths_antigravity,
    },
    StackDef {
        id: "kiro",
        label: "Kiro",
        description: "AWS Kiro auth",
        format: FormatKind::JsonKeyScan,
        capability: StackCapability {
            import: true,
            export: false,
        },
        path_candidates: paths_kiro,
    },
    StackDef {
        id: "amp",
        label: "Amp",
        description: "Amp auth",
        format: FormatKind::JsonKeyScan,
        capability: StackCapability {
            import: true,
            export: true,
        },
        path_candidates: paths_amp,
    },
    StackDef {
        id: "factory",
        label: "Factory Droid",
        description: "Factory auth",
        format: FormatKind::JsonKeyScan,
        capability: StackCapability {
            import: true,
            export: true,
        },
        path_candidates: paths_factory,
    },
    StackDef {
        id: "poolside",
        label: "Poolside",
        description: "~/.config/poolside/credentials.json",
        format: FormatKind::JsonKeyScan,
        capability: StackCapability {
            import: true,
            export: true,
        },
        path_candidates: paths_poolside,
    },
    StackDef {
        id: "higgsfield",
        label: "Higgsfield",
        description: "~/.config/higgsfield/credentials.json",
        format: FormatKind::JsonKeyScan,
        capability: StackCapability {
            import: true,
            export: true,
        },
        path_candidates: paths_higgsfield,
    },
    StackDef {
        id: "context7",
        label: "Context7",
        description: "~/.context7/credentials.json",
        format: FormatKind::JsonKeyScan,
        capability: StackCapability {
            import: true,
            export: true,
        },
        path_candidates: paths_context7,
    },
    StackDef {
        id: "dotenv",
        label: ".env dosyası",
        description: "Proje/home .env (OPENAI_API_KEY=…)",
        format: FormatKind::DotEnv,
        capability: StackCapability {
            import: true,
            export: true,
        },
        path_candidates: paths_dotenv_project,
    },
    StackDef {
        id: "env",
        label: "Süreç ortamı",
        description: "Çalışan shell ortam değişkenleri",
        format: FormatKind::ProcessEnv,
        capability: StackCapability {
            import: true,
            export: false,
        },
        path_candidates: paths_empty,
    },
    StackDef {
        id: "json-scan",
        label: "Özel JSON (tara)",
        description: "Herhangi bir auth/credentials JSON — alan tarama",
        format: FormatKind::JsonKeyScan,
        capability: StackCapability {
            import: true,
            export: false,
        },
        path_candidates: paths_empty,
    },
];

// suppress unused warnings for helpers referenced only via fn pointers in const
#[allow(dead_code)]
fn _touch_caps() {
    let _ = both();
    let _ = import_only();
    let _ = Path::new(".");
}
