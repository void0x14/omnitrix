use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PersonaKind {
    Explorer,
    Navigator,
    Fixer,
    Builder,
    Judge,
    Planner,
    Watcher,
    Enforcer,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FsCap {
    pub read: bool,
    pub write: bool,
    pub exec_allowlist: Vec<PathBuf>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NetCap {
    pub http: bool,
    pub host_allowlist: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GuiCap {
    pub input: bool,
    pub screencap: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProcessCap {
    pub spawn_allowlist: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Capability {
    pub fs: FsCap,
    pub net: NetCap,
    pub gui: GuiCap,
    pub process: ProcessCap,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonaSpec {
    pub kind: PersonaKind,
    pub capability: Capability,
}

impl PersonaSpec {
    pub fn new(kind: PersonaKind) -> Self {
        Self { kind, capability: kind.capabilities() }
    }

    pub fn with_capability(mut self, cap: Capability) -> Self {
        self.capability = cap;
        self
    }
}

impl PersonaKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Explorer => "Kaşif (XLR8)",
            Self::Navigator => "Gezgin (Wildmutt)",
            Self::Fixer => "Juryrigg (Grey Matter)",
            Self::Builder => "İnşaatçı (Four Arms)",
            Self::Judge => "Yargıç (Brainstorm)",
            Self::Planner => "Planlayıcı (Ghostfreak)",
            Self::Watcher => "Gözcü (Big Chill)",
            Self::Enforcer => "Denetçi (Heatblast)",
        }
    }

    pub fn capabilities(&self) -> Capability {
        match self {
            Self::Explorer => Capability {
                fs: FsCap { read: true, write: false, exec_allowlist: vec![] },
                net: NetCap { http: true, host_allowlist: vec![] },
                gui: GuiCap { input: false, screencap: false },
                process: ProcessCap { spawn_allowlist: vec![] },
            },
            Self::Navigator => Capability {
                fs: FsCap { read: true, write: false, exec_allowlist: vec![] },
                net: NetCap { http: false, host_allowlist: vec![] },
                gui: GuiCap { input: false, screencap: false },
                process: ProcessCap { spawn_allowlist: vec![] },
            },
            Self::Fixer => Capability {
                fs: FsCap { read: true, write: true, exec_allowlist: vec![] },
                net: NetCap { http: false, host_allowlist: vec![] },
                gui: GuiCap { input: false, screencap: false },
                process: ProcessCap { spawn_allowlist: vec![] },
            },
            Self::Builder => Capability {
                fs: FsCap { read: true, write: true, exec_allowlist: vec![] },
                net: NetCap { http: false, host_allowlist: vec![] },
                gui: GuiCap { input: false, screencap: false },
                process: ProcessCap { spawn_allowlist: vec!["*".to_string()] },
            },
            Self::Judge => Capability {
                fs: FsCap { read: true, write: false, exec_allowlist: vec![] },
                net: NetCap { http: false, host_allowlist: vec![] },
                gui: GuiCap { input: false, screencap: false },
                process: ProcessCap { spawn_allowlist: vec![] },
            },
            Self::Planner => Capability {
                fs: FsCap { read: true, write: false, exec_allowlist: vec![] },
                net: NetCap { http: false, host_allowlist: vec![] },
                gui: GuiCap { input: false, screencap: false },
                process: ProcessCap { spawn_allowlist: vec![] },
            },
            Self::Watcher => Capability {
                fs: FsCap { read: false, write: false, exec_allowlist: vec![] },
                net: NetCap { http: true, host_allowlist: vec![] },
                gui: GuiCap { input: false, screencap: false },
                process: ProcessCap { spawn_allowlist: vec![] },
            },
            Self::Enforcer => Capability {
                fs: FsCap { read: true, write: true, exec_allowlist: vec![] },
                net: NetCap { http: true, host_allowlist: vec![] },
                gui: GuiCap { input: true, screencap: true },
                process: ProcessCap { spawn_allowlist: vec!["*".to_string()] },
            },
        }
    }

    pub fn routing_policy(&self) -> RoutingPolicy {
        match self {
            Self::Explorer => RoutingPolicy::RoundRobin,
            Self::Navigator => RoutingPolicy::Weighted,
            Self::Fixer => RoutingPolicy::Fallback,
            Self::Builder => RoutingPolicy::Jep,
            Self::Judge => RoutingPolicy::Judge,
            Self::Planner => RoutingPolicy::Planner,
            Self::Watcher => RoutingPolicy::Passive,
            Self::Enforcer => RoutingPolicy::ControlPlane,
        }
    }

    pub fn system_prompt_label(&self) -> &'static str {
        match self {
            Self::Explorer => "prompts/explorer.md",
            Self::Navigator => "prompts/deep_explorer.md",
            Self::Fixer => "prompts/quick_fix.md",
            Self::Builder => "prompts/executor.md",
            Self::Judge => "prompts/judge.md",
            Self::Planner => "prompts/planner.md",
            Self::Watcher => "prompts/watcher.md",
            Self::Enforcer => "prompts/enforcer.md",
        }
    }

    pub fn priority(&self) -> u8 {
        match self {
            Self::Enforcer => 255,
            Self::Builder => 200,
            Self::Planner => 150,
            Self::Judge => 100,
            Self::Explorer => 80,
            Self::Navigator => 60,
            Self::Fixer => 40,
            Self::Watcher => 10,
        }
    }
}

impl fmt::Display for PersonaKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.label())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RoutingPolicy {
    RoundRobin,
    Weighted,
    Fallback,
    Jep,
    Judge,
    Planner,
    Passive,
    ControlPlane,
}
