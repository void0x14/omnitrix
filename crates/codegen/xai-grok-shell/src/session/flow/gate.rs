//! Aşama kapısı: tool id → grup eşlemesi ve bash kural denetimi.
//! Eşleme statiktir (registry gerçekleri); `config/flow/rules.toml` override'ı
//! Task 3'te eklenebilir.

use super::definition::ToolGroup;
use super::state::FlowStateMachine;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BashVerdict {
    Allow,
    Deny(&'static str),
}

/// Tool id (ToolDefinition.name) → grup eşleme tablosu.
/// Registry: crates/codegen/xai-grok-tools/src/registry/types.rs:666+.
pub fn tool_group_of(tool_id: &str) -> Option<ToolGroup> {
    let g = match tool_id {
        "read_file" | "read_file_concise" | "list_dir" | "grep" | "hashline_read"
        | "codex_read_file" | "opencode_read" | "search" => ToolGroup::Read,
        // "grep"/"list_dir" yukarıda Read'e düşer (ilk kol kazanır) — burada
        // tekrar edilmez (unreachable_pattern uyarısı).
        "search_tool" => ToolGroup::Search,
        "search_replace" | "apply_patch" | "opencode_edit" | "opencode_write" | "hashline_edit"
        | "image_gen" | "image_edit" => ToolGroup::Write,
        "bash" | "opencode_bash" => ToolGroup::Bash,
        "web_search" | "web_fetch" => ToolGroup::Web,
        "grok_research" => ToolGroup::Research,
        // client adi "computer" (agent_rebuild.rs register_mcp_tools("computer", ...));
        // "grok_computer" geriye dönük aday olarak korunur.
        "computer" | "grok_computer" => ToolGroup::Computer,
        "enter_plan_mode" | "exit_plan_mode" => ToolGroup::Plan,
        "task" | "task_output" | "wait_tasks" | "workflow" | "scheduler_create"
        | "scheduler_delete" | "scheduler_list" => ToolGroup::Task,
        // meta: her aşamada açık
        "ask_user_question" | "todo_write" | "update_goal" | "use_tool" | "skill"
        | "memory_search" | "memory_get" => ToolGroup::Meta,
        _ => return None, // bilinmeyen id: kapı yok sayar (mevcut mekanizmalar korur)
    };
    Some(g)
}

pub struct FlowGate;

impl FlowGate {
    pub fn tool_allowed(tool_id: &str, sm: &FlowStateMachine) -> bool {
        match tool_group_of(tool_id) {
            None => true,
            Some(group) => sm.tool_unlocked(group),
        }
    }

    /// Bash komut kelime denetimi — aşamaya göre. Kural matcher'ı deterministiktir;
    /// derin koruma için mevcut CompiledPolicy ikinci savunma olarak kalır.
    pub fn bash_command_allowed(command: &str, sm: &FlowStateMachine) -> BashVerdict {
        let stage = match sm.current_stage() {
            None => return BashVerdict::Allow,
            Some(s) => s,
        };
        use super::definition::StageId;
        // universal akış: yalnızca execute/verify aşamalarında bash açık (grup zaten
        // kapıyor) — burada yalnızca commit akışının komut kuralları.
        // Yerleşik araç zorunluluğu (TÜM akışlarda geçerli): dosya okuma/yazma
        // ve arama, yerleşik read_file/search_replace/grep araçlarıyla yapılır;
        // bash ile "tembellik" yolları yasak (kullanıcı direktifi).
        let lower = command.to_lowercase();
        let builtin_tool_deny: &[(&str, &str)] = &[
            (
                "cat ",
                "cat yasak: dosya okuma için read_file aracını kullan",
            ),
            (
                "less ",
                "less yasak: dosya okuma için read_file aracını kullan",
            ),
            (
                "more ",
                "more yasak: dosya okuma için read_file aracını kullan",
            ),
            (
                "head ",
                "head yasak: dosya okuma için read_file aracını kullan",
            ),
            (
                "tail ",
                "tail yasak: dosya okuma için read_file aracını kullan",
            ),
            (
                "grep ",
                "grep yasak: arama için yerleşik grep/search araçlarını kullan",
            ),
            (
                "sed ",
                "sed yasak: düzenleme için search_replace aracını kullan",
            ),
            (
                "awk ",
                "awk yasak: düzenleme/okuma için yerleşik araçları kullan",
            ),
            (
                "echo >",
                "echo yönlendirme yasak: yazma için write/search_replace aracını kullan",
            ),
            (
                ">>",
                "yönlendirme yasak: yazma için write/search_replace aracını kullan",
            ),
            ("rm ", "rm yasak: silme yerleşik dosya araçlarıyla yapılır"),
        ];
        for (pat, reason) in builtin_tool_deny {
            if lower.contains(pat) {
                return BashVerdict::Deny(reason);
            }
        }
        if sm.flow().name != "commit" {
            return BashVerdict::Allow;
        }
        let deny: &[(&str, &str)] = &[
            ("--force", "force komutları yasak: geçmişe saygı (I-kural)"),
            ("--hard", "hard reset yasak"),
            ("push", "push yasak (commit akışı yalnızca yerel)"),
            ("rebase", "rebase yasak"),
            ("cherry-pick", "cherry-pick yasak"),
            ("reset", "reset yasak"),
            ("merge", "merge yasak"),
            ("clean", "clean yasak"),
            ("reflog delete", "reflog delete yasak"),
            ("filter-branch", "filter-branch yasak"),
            ("gc", "gc yasak"),
        ];
        for (pat, reason) in deny {
            if lower.contains(pat) {
                return BashVerdict::Deny(reason);
            }
        }
        // Aşama kuralı: status/verify aşamasında yalnızca salt-okunur git komutları
        match stage {
            StageId::Status | StageId::CommitVerify => {
                let readonly = [
                    "git status",
                    "git diff",
                    "git log",
                    "git show",
                    "git stash list",
                    "git branch",
                    "git remote",
                    "git config",
                    "git rev-parse",
                    "git ls-files",
                ];
                if lower.starts_with("git") && !readonly.iter().any(|c| lower.starts_with(c)) {
                    return BashVerdict::Deny(
                        "bu aşamada yalnızca salt-okunur git komutlarına izin var",
                    );
                }
            }
            StageId::Stage => {
                let allowed = [
                    "git add",
                    "git rm",
                    "git status",
                    "git diff",
                    "git ls-files",
                ];
                if lower.starts_with("git") && !allowed.iter().any(|c| lower.starts_with(c)) {
                    return BashVerdict::Deny(
                        "stage aşamasında yalnızca git add/rm/status/diff/ls-files",
                    );
                }
            }
            StageId::Commit => {
                if lower.starts_with("git") && !lower.starts_with("git commit") {
                    return BashVerdict::Deny("commit aşamasında yalnızca git commit");
                }
            }
            _ => {}
        }
        BashVerdict::Allow
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::flow::definition::{StageId, ToolGroup, default_flows};
    use crate::session::flow::state::FlowStateMachine;

    fn commit_sm() -> FlowStateMachine {
        let flows = default_flows();
        let commit = flows.iter().find(|f| f.name == "commit").unwrap();
        let mut sm = FlowStateMachine::idle();
        sm.start(commit);
        sm
    }

    #[test]
    fn web_locked_outside_research() {
        let flows = default_flows();
        let universal = flows.iter().find(|f| f.name == "universal").unwrap();
        let mut sm = FlowStateMachine::idle();
        sm.start(universal);
        assert!(!sm.tool_unlocked(ToolGroup::Web));
        assert!(!FlowGate::tool_allowed("web_search", &sm));
        assert!(FlowGate::tool_allowed("read_file", &sm));
        assert!(FlowGate::tool_allowed("ask_user_question", &sm)); // meta
    }

    #[test]
    fn force_flag_denied() {
        let sm = commit_sm();
        assert_eq!(
            FlowGate::bash_command_allowed("git reset --hard HEAD~1", &sm),
            BashVerdict::Deny("hard reset yasak")
        );
    }

    #[test]
    fn status_stage_readonly() {
        let sm = commit_sm();
        assert_eq!(
            FlowGate::bash_command_allowed("git commit -m x", &sm),
            BashVerdict::Deny("bu aşamada yalnızca salt-okunur git komutlarına izin var")
        );
        assert_eq!(
            FlowGate::bash_command_allowed("git status", &sm),
            BashVerdict::Allow
        );
    }
}
