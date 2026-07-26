//! Sunucu-tarafli HTML (maud) — MASTER-PLAN Bolum 5 / K8.
//!
//! Tum render girdileri `omni-proto` kanonik tipleridir; bu modul kendi durum
//! tipini icat etmez (I3). TUI ile ayni akistan turer, yalnizca render farklidir
//! (K7).
//!
//! Her canli guncellenebilir eleman **kararli bir DOM kimligi** tasir: satirlar
//! `agent-<id>` / `task-<id>` / `provider-<id>`, kapsayicilar [`ids`] altinda.
//! `live::Fragment` bu kimlikleri hedefler; boylece ayni parca hem ilk yuklemede
//! (SSR) hem de akista (SSE/WS) ayni HTML'i uretir.

use maud::{DOCTYPE, Markup, PreEscaped, html};
use omni_proto::{
    AgentId, AgentView, FileTouch, InterruptView, NoticeView, ProviderId, ProviderView,
    ResourceGauge, SystemSnapshot, TaskId, TaskView, Timestamp, ToolCallView,
};

use crate::assets;

/// Canli guncelleme hedefi olan sabit kapsayici kimlikleri.
pub mod ids {
    /// Tum panoyu saran ana kapsayici; akis kopuklugunda tumden yenilenir.
    pub const BOARD: &str = "board";
    /// Ajan satirlarinin `tbody`'si.
    pub const AGENTS: &str = "agents";
    /// Gorev satirlarinin `tbody`'si.
    pub const TASKS: &str = "tasks";
    /// Saglayici satirlarinin `tbody`'si.
    pub const PROVIDERS: &str = "providers";
    /// Kaynak valisi paneli (7.2).
    pub const RESOURCE: &str = "resource";
    /// Dosya dokunusu akisi (5.2 diff gorunurlugu).
    pub const TOUCHES: &str = "touches";
    /// Tool cagrisi akisi (K3 broker denetimi).
    pub const TOOLS: &str = "tools";
    /// Mudahale akisi (AS2 / 6.8).
    pub const INTERRUPTS: &str = "interrupts";
    /// Bildirim akisi (Bolum 13).
    pub const NOTICES: &str = "notices";
    /// Akis baglanti gostergesi.
    pub const LINK: &str = "link-state";
}

/// Ajan satirinin DOM kimligi.
#[must_use]
pub fn agent_dom_id(id: AgentId) -> String {
    format!("agent-{id}")
}

/// Gorev satirinin DOM kimligi.
#[must_use]
pub fn task_dom_id(id: TaskId) -> String {
    format!("task-{id}")
}

/// Saglayici satirinin DOM kimligi.
#[must_use]
pub fn provider_dom_id(id: ProviderId) -> String {
    format!("provider-{id}")
}

/// Akis kaydinin DOM kimligi.
///
/// DB kimligi henuz yoksa (kayit yazilmadan once yayina dusen olay) zaman
/// damgasindan tureyen benzersiz bir kimlik uretilir; boylece parca eklenir,
/// var olan bir satirin uzerine yazmaz.
#[must_use]
pub fn feed_dom_id(prefix: &str, id: Option<i64>, ts: Timestamp) -> String {
    match id {
        Some(value) => format!("{prefix}-{value}"),
        None => format!("{prefix}-t{}", ts.timestamp_micros()),
    }
}

/// Kibibayti insan okunur MiB'a cevirir.
fn mib(kb: u64) -> String {
    format!("{:.1} MiB", kb as f64 / 1024.0)
}

/// Saat:dakika:saniye (yerel degil, kanonik UTC).
fn clock(ts: Timestamp) -> String {
    ts.format("%H:%M:%S").to_string()
}

/// Bos kapsayici yer tutucusu (tablo).
fn placeholder_row(cols: usize, text: &str) -> Markup {
    html! {
        tr class="placeholder" {
            td colspan=(cols) { (text) }
        }
    }
}

/// Bos kapsayici yer tutucusu (akis listesi).
fn placeholder_item(text: &str) -> Markup {
    html! { li class="placeholder" { (text) } }
}

/// Tam sayfa: ilk yuklemede `SystemSnapshot` HTML olarak gelir (Bolum 6.2).
#[must_use]
pub fn page(snapshot: &SystemSnapshot) -> Markup {
    html! {
        (DOCTYPE)
        html lang="tr" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                meta name="color-scheme" content="dark light";
                title { "Omnitrix" }
                style { (PreEscaped(assets::STYLE)) }
            }
            body data-stream=(crate::PATH_STREAM) {
                (top_bar(snapshot))
                (board(snapshot))
                script { (PreEscaped(assets::SCRIPT)) }
            }
        }
    }
}

/// Ust serit: baslik, akis gostergesi, oturum cikisi.
fn top_bar(snapshot: &SystemSnapshot) -> Markup {
    html! {
        header class="top" {
            h1 { "Omnitrix" }
            span id=(ids::LINK) class="link" { "baglaniyor" }
            span class="grow" {}
            span class="dim mono" { (clock(snapshot.ts)) }
            form method="post" action=(crate::PATH_LOGOUT) {
                button class="ghost" type="submit" { "cikis" }
            }
        }
    }
}

/// Panonun tamami. Akis kopuklugunda tek parca olarak yeniden gonderilir.
#[must_use]
pub fn board(snapshot: &SystemSnapshot) -> Markup {
    html! {
        main id=(ids::BOARD) {
            (resource_panel(&snapshot.resource))
            section class="panel" {
                h2 { "Ajanlar" }
                div class="scroll" {
                    table {
                        thead {
                            tr {
                                th { "#" } th { "Persona" } th { "Katman" } th { "Durum" }
                                th { "Gorev" } th { "Derinlik" } th { "Token" }
                                th { "Maliyet" } th { "RSS" }
                            }
                        }
                        tbody id=(ids::AGENTS) {
                            @if snapshot.agents.is_empty() {
                                (placeholder_row(9, "henuz ajan yok"))
                            }
                            @for agent in &snapshot.agents { (agent_row(agent)) }
                        }
                    }
                }
            }
            section class="panel" {
                h2 { "Gorevler" }
                div class="scroll" {
                    table {
                        thead {
                            tr {
                                th { "#" } th { "Baslik" } th { "Mod" } th { "Durum" }
                                th { "Derinlik" } th { "Butce" } th { "Acilis" }
                            }
                        }
                        tbody id=(ids::TASKS) {
                            @if snapshot.tasks.is_empty() {
                                (placeholder_row(7, "henuz gorev yok"))
                            }
                            @for task in &snapshot.tasks { (task_row(task)) }
                        }
                    }
                }
            }
            section class="panel" {
                h2 { "Saglayicilar" }
                div class="scroll" {
                    table {
                        thead {
                            tr {
                                th { "#" } th { "Ad" } th { "Tur" } th { "Saglik" }
                                th { "Gecikme" } th { "Anahtar" } th { "Model" }
                            }
                        }
                        tbody id=(ids::PROVIDERS) {
                            @if snapshot.providers.is_empty() {
                                (placeholder_row(7, "saglayici tespit edilmedi"))
                            }
                            @for provider in &snapshot.providers { (provider_row(provider)) }
                        }
                    }
                }
            }
            (feeds())
            (command_panel(snapshot))
        }
    }
}

/// Kaynak valisi paneli (7.2). Kod tavan koymaz; olculen basinci gosterir.
#[must_use]
pub fn resource_panel(gauge: &ResourceGauge) -> Markup {
    let ratio = gauge.rss_ratio().unwrap_or(0.0).clamp(0.0, 1.0);
    let width = format!("width:{:.1}%", f64::from(ratio) * 100.0);
    let admission = if gauge.admission_open {
        "acik"
    } else {
        "kapali"
    };
    let admission_class = if gauge.admission_open {
        "tag active"
    } else {
        "tag failed"
    };
    html! {
        section id=(ids::RESOURCE) class="panel" {
            h2 { "Kaynak" }
            div class="gauge" {
                div class="cell" {
                    span class="k" { "aktif" }
                    span class="v" { (gauge.active) }
                }
                div class="cell" {
                    span class="k" { "kuyrukta" }
                    span class="v" { (gauge.queued) }
                }
                div class="cell" {
                    span class="k" { "uyuyan" }
                    span class="v" { (gauge.sleeping) }
                }
                div class="cell" {
                    span class="k" { "var-olan" }
                    span class="v" { (gauge.total_agents()) }
                }
                div class="cell" {
                    span class="k" { "rss" }
                    span class="v" { (mib(gauge.rss_kb)) }
                    @if gauge.rss_limit_kb.is_some() {
                        div class="bar" { span style=(width) {} }
                    }
                }
                div class="cell" {
                    span class="k" { "cpu" }
                    span class="v" { (format!("{:.0}%", gauge.cpu_pct)) }
                }
                div class="cell" {
                    span class="k" { "fd" }
                    span class="v" { (gauge.open_fds) }
                    @if let Some(limit) = gauge.fd_limit {
                        span class="dim mono" { " / " (limit) }
                    }
                }
                div class="cell" {
                    span class="k" { "kabul" }
                    span class=(admission_class) { (admission) }
                }
            }
        }
    }
}

/// Tek ajan satiri.
#[must_use]
pub fn agent_row(agent: &AgentView) -> Markup {
    let tier_class = format!("tag {}", agent.tier.as_db_str());
    let state_class = format!("tag {}", agent.state.as_db_str());
    html! {
        tr id=(agent_dom_id(agent.id)) {
            td data-label="#" class="mono" { (agent.id) }
            td data-label="Persona" { (agent.persona) }
            td data-label="Katman" { span class=(tier_class) { (agent.tier.as_db_str()) } }
            td data-label="Durum" { span class=(state_class) { (agent.state.as_db_str()) } }
            td data-label="Gorev" class="mono" { (agent.task_id) }
            td data-label="Derinlik" class="num" { (agent.depth) }
            td data-label="Token" class="num mono" {
                (agent.tokens_in) span class="dim" { " / " } (agent.tokens_out)
            }
            td data-label="Maliyet" class="num mono" { (format!("{:.4}", agent.cost)) }
            td data-label="RSS" class="num mono" { (mib(agent.rss_kb)) }
        }
    }
}

/// Tek gorev satiri.
#[must_use]
pub fn task_row(task: &TaskView) -> Markup {
    let indent = "· ".repeat(usize::from(task.depth));
    html! {
        tr id=(task_dom_id(task.id)) {
            td data-label="#" class="mono" { (task.id) }
            td data-label="Baslik" { span class="dim mono" { (indent) } (task.title) }
            td data-label="Mod" { span class="tag" { (task.mode) } }
            td data-label="Durum" { span class="tag" { (task.status) } }
            td data-label="Derinlik" class="num" { (task.depth) }
            td data-label="Butce" class="num mono" {
                @match task.budget_remaining() {
                    Some(kalan) => { (format!("{kalan:.2}")) },
                    None => { span class="dim" { "sinirsiz" } },
                }
            }
            td data-label="Acilis" class="mono dim" { (clock(task.created_at)) }
        }
    }
}

/// Tek saglayici satiri.
#[must_use]
pub fn provider_row(provider: &ProviderView) -> Markup {
    let health_class = format!("tag {}", provider.health.as_db_str());
    html! {
        tr id=(provider_dom_id(provider.id)) {
            td data-label="#" class="mono" { (provider.id) }
            td data-label="Ad" { (provider.name) }
            td data-label="Tur" { span class="tag" { (provider.kind) } }
            td data-label="Saglik" {
                span class=(health_class) { (provider.health.as_db_str()) }
            }
            td data-label="Gecikme" class="num mono" {
                @match provider.latency_ms {
                    Some(ms) => { (format!("{ms} ms")) },
                    None => { span class="dim" { "—" } },
                }
            }
            td data-label="Anahtar" class="num" { (provider.active_keys) }
            td data-label="Model" class="num" { (provider.models.len()) }
        }
    }
}

/// Akis panelleri: dosya dokunuslari, tool cagrilari, mudahaleler, bildirimler.
fn feeds() -> Markup {
    html! {
        div class="grid2" {
            section class="panel" {
                h2 { "Diff akisi" }
                ul id=(ids::TOUCHES) class="feed" {
                    (placeholder_item("dosyaya dokunulmadi"))
                }
            }
            section class="panel" {
                h2 { "Tool cagrilari" }
                ul id=(ids::TOOLS) class="feed" {
                    (placeholder_item("cagri yok"))
                }
            }
            section class="panel" {
                h2 { "Mudahaleler" }
                ul id=(ids::INTERRUPTS) class="feed" {
                    (placeholder_item("mudahale yok"))
                }
            }
            section class="panel" {
                h2 { "Bildirimler" }
                ul id=(ids::NOTICES) class="feed" {
                    (placeholder_item("bildirim yok"))
                }
            }
        }
    }
}

/// Dosya dokunusu kaydi (5.2).
#[must_use]
pub fn touch_item(touch: &FileTouch) -> Markup {
    html! {
        li id=(feed_dom_id("touch", touch.id, touch.ts)) {
            span class="dim mono" { (clock(touch.ts)) }
            span class="mono" { (touch.path) }
            span class="plus mono" { "+" (touch.added) }
            span class="minus mono" { "-" (touch.removed) }
            @if touch.outside_workspace {
                span class="tag failed" { "calisma alani disi" }
            }
            @if touch.is_creation() { span class="tag" { "yeni" } }
        }
    }
}

/// Tool cagrisi kaydi (K3 broker denetimi).
#[must_use]
pub fn tool_item(call: &ToolCallView) -> Markup {
    html! {
        li id=(feed_dom_id("tool", call.id, call.ts)) {
            span class="dim mono" { (clock(call.ts)) }
            span class="mono" { (call.tool) }
            span class="tag" { (call.status) }
            span class="dim mono" { "ajan " (call.agent_id) }
            @if call.is_denied() { span class="tag failed" { "yetki reddi" } }
        }
    }
}

/// Mudahale kaydi (AS2 / 6.8).
#[must_use]
pub fn interrupt_item(interrupt: &InterruptView) -> Markup {
    let class = if interrupt.is_open() {
        "tag interrupted"
    } else {
        "tag done"
    };
    html! {
        li id=(feed_dom_id("interrupt", interrupt.id, interrupt.ts)) {
            span class="dim mono" { (clock(interrupt.ts)) }
            span class=(class) { (interrupt.kind) }
            span class="dim mono" { "ajan " (interrupt.agent_id) }
            span class="dim" { (interrupt.source) }
            @if let Some(reason) = &interrupt.reason { span { (reason) } }
        }
    }
}

/// Bildirim kaydi (Bolum 13). Seviye `notifies_externally` ile ayni renk esigi.
#[must_use]
pub fn notice_item(notice: &NoticeView) -> Markup {
    let class = if notice.level.notifies_externally() {
        "tag failed"
    } else {
        "tag"
    };
    html! {
        li id=(feed_dom_id("notice", None, notice.ts)) {
            span class="dim mono" { (clock(notice.ts)) }
            span class=(class) { (notice.code) }
            span { (notice.message) }
        }
    }
}

/// Komut paneli — **JS'siz calisir**: klasik form POST'u ile `Command` uretir.
///
/// Mod ve persona adlari koda gomulmez; `datalist` anlik goruntude gorulen
/// degerlerden turer (I5).
fn command_panel(snapshot: &SystemSnapshot) -> Markup {
    let mut modes: Vec<&str> = snapshot.tasks.iter().map(|t| t.mode.as_str()).collect();
    modes.sort_unstable();
    modes.dedup();
    let mut personas: Vec<&str> = snapshot.agents.iter().map(|a| a.persona.as_str()).collect();
    personas.sort_unstable();
    personas.dedup();

    html! {
        section class="panel" {
            h2 { "Komut" }
            details open[snapshot.tasks.is_empty()] {
                summary { "yeni gorev" }
                form class="stack" method="post" action=(crate::PATH_COMMAND) {
                    input type="hidden" name="cmd" value="spawn_task";
                    label {
                        "baslik"
                        input type="text" name="title" required placeholder="ne yapilacak";
                    }
                    div class="grid2" {
                        label {
                            "mod"
                            input type="text" name="mode" list="modes" required;
                        }
                        label {
                            "persona"
                            input type="text" name="persona" list="personas";
                        }
                        label {
                            "sure hedefi"
                            input type="text" name="duration_target";
                        }
                        label {
                            "butce"
                            input type="text" inputmode="decimal" name="budget";
                        }
                    }
                    datalist id="modes" {
                        @for mode in &modes { option value=(mode) {} }
                    }
                    datalist id="personas" {
                        @for persona in &personas { option value=(persona) {} }
                    }
                    button type="submit" { "gorev ac" }
                }
            }
            @if !snapshot.agents.is_empty() {
                details {
                    summary { "ajana yaz" }
                    form class="stack" method="post" action=(crate::PATH_COMMAND) {
                        input type="hidden" name="cmd" value="write_to_agent";
                        label {
                            "ajan"
                            (agent_select(snapshot))
                        }
                        label {
                            "mesaj"
                            textarea name="content" required {}
                        }
                        button type="submit" { "gonder" }
                    }
                }
                details {
                    summary { "mudahale" }
                    form class="stack" method="post" action=(crate::PATH_COMMAND) {
                        input type="hidden" name="cmd" value="interrupt";
                        label {
                            "ajan"
                            (agent_select(snapshot))
                        }
                        div class="grid2" {
                            label {
                                "tur"
                                input type="text" name="kind" required;
                            }
                            label {
                                "gerekce"
                                input type="text" name="reason";
                            }
                        }
                        button type="submit" { "kes" }
                    }
                }
            }
        }
    }
}

/// Anlik goruntudeki ajanlardan kurulu secim kutusu.
fn agent_select(snapshot: &SystemSnapshot) -> Markup {
    html! {
        select name="agent_id" required {
            @for agent in &snapshot.agents {
                option value=(agent.id) {
                    "#" (agent.id) " " (agent.persona) " (" (agent.state.as_db_str()) ")"
                }
            }
        }
    }
}

/// Oturum acma sayfasi (K9: kimliksiz istek panoyu goremez).
///
/// Tarayici basligi kendi kendine ekleyemedigi icin token buradan alinir ve
/// `HttpOnly` cerezine yazilir; dogrulama yine `omni-control` tarafindadir.
#[must_use]
pub fn login_page(error: Option<&str>) -> Markup {
    html! {
        (DOCTYPE)
        html lang="tr" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                meta name="color-scheme" content="dark light";
                title { "Omnitrix — giris" }
                style { (PreEscaped(assets::STYLE)) }
            }
            body {
                header class="top" { h1 { "Omnitrix" } }
                main {
                    section class="panel" {
                        h2 { "Giris" }
                        @if let Some(text) = error {
                            p class="err" { (text) }
                        }
                        form class="stack" method="post" action=(crate::PATH_LOGIN) {
                            label {
                                "token"
                                input type="password" name="token" autocomplete="current-password"
                                    required autofocus;
                            }
                            button type="submit" { "gir" }
                        }
                    }
                }
            }
        }
    }
}

/// Hata sayfasi. Ayrinti kullaniciya gosterilir ama kimlik ayrintisi sizmaz —
/// auth hatalari bu yola hic girmez, giris sayfasina duser.
#[must_use]
pub fn error_page(code: &str, detail: &str) -> Markup {
    html! {
        (DOCTYPE)
        html lang="tr" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { "Omnitrix — hata" }
                style { (PreEscaped(assets::STYLE)) }
            }
            body {
                header class="top" { h1 { "Omnitrix" } }
                main {
                    section class="panel" {
                        h2 { (code) }
                        p { (detail) }
                        p { a href=(crate::PATH_INDEX) { "panoya don" } }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omni_proto::{AgentState, AgentTier, NoticeLevel, ProviderHealthState};

    fn ts() -> Timestamp {
        omni_proto::now()
    }

    fn agent() -> AgentView {
        AgentView {
            id: 7,
            persona: "planner".into(),
            tier: AgentTier::Active,
            task_id: 1,
            parent_id: None,
            state: AgentState::RunningTool,
            rss_kb: 4096,
            tokens_in: 120,
            tokens_out: 45,
            cost: 0.0123,
            trust: 0.9,
            depth: 1,
            last_event_seq: 12,
        }
    }

    fn task(title: &str) -> TaskView {
        TaskView {
            id: 1,
            parent_id: None,
            root_id: 1,
            title: title.into(),
            mode: "user_driven".into(),
            status: "running".into(),
            depth: 0,
            budget_allocated: Some(10.0),
            budget_spent: Some(2.0),
            duration_target: None,
            created_at: ts(),
            closed_at: None,
        }
    }

    fn snapshot() -> SystemSnapshot {
        let mut snap = SystemSnapshot::empty(ts());
        snap.agents.push(agent());
        snap.tasks.push(task("dikey dilim"));
        snap.providers.push(ProviderView {
            id: 3,
            name: "yerel".into(),
            kind: "openai_compatible".into(),
            base_url: "http://127.0.0.1:1234".into(),
            health: ProviderHealthState::Healthy,
            health_model: None,
            latency_ms: Some(42),
            checked_at: Some(ts()),
            detail: None,
            active_keys: 2,
            models: vec!["a".into(), "b".into()],
        });
        snap
    }

    #[test]
    fn sayfa_iskeleti_tam() {
        let html = page(&snapshot()).into_string();
        assert!(html.starts_with("<!DOCTYPE html>"));
        assert!(html.contains("name=\"viewport\""));
        assert!(html.contains("data-stream=\"/ui/stream\""));
        assert!(html.contains("id=\"board\""));
        assert!(html.contains("<style>"));
        assert!(html.contains("<script>"));
    }

    #[test]
    fn satirlar_kararli_kimlik_tasir() {
        let html = board(&snapshot()).into_string();
        assert!(html.contains("id=\"agent-7\""));
        assert!(html.contains("id=\"task-1\""));
        assert!(html.contains("id=\"provider-3\""));
        assert!(html.contains("id=\"agents\""));
    }

    #[test]
    fn bos_goruntu_yer_tutucu_gosterir() {
        let html = board(&SystemSnapshot::empty(ts())).into_string();
        // Uc tablo + dort akis listesi bos yer tutucu gosterir.
        assert_eq!(html.matches("class=\"placeholder\"").count(), 7);
        assert!(html.contains("henuz ajan yok"));
    }

    #[test]
    fn kullanici_metni_kacislanir() {
        let mut snap = SystemSnapshot::empty(ts());
        snap.tasks.push(task("<script>alert(1)</script>"));
        let html = board(&snap).into_string();
        assert!(!html.contains("<script>alert(1)</script>"));
        assert!(html.contains("&lt;script&gt;"));
    }

    #[test]
    fn ajan_satiri_katman_ve_durum_etiketler() {
        let html = agent_row(&agent()).into_string();
        assert!(html.contains("tag active"));
        assert!(html.contains("tag running_tool"));
        assert!(html.contains("data-label=\"Persona\""));
    }

    #[test]
    fn butce_kalani_gosterilir() {
        let html = task_row(&task("x")).into_string();
        assert!(html.contains("8.00"));
        let mut serbest = task("y");
        serbest.budget_allocated = None;
        assert!(task_row(&serbest).into_string().contains("sinirsiz"));
    }

    #[test]
    fn kaynak_paneli_kabul_durumunu_gosterir() {
        let mut gauge = ResourceGauge::at(ts());
        gauge.rss_kb = 2048;
        gauge.rss_limit_kb = Some(4096);
        gauge.admission_open = false;
        let html = resource_panel(&gauge).into_string();
        assert!(html.contains("id=\"resource\""));
        assert!(html.contains("kapali"));
        assert!(html.contains("width:50.0%"));
    }

    #[test]
    fn akis_kimlikleri_dbsiz_kayitta_benzersiz() {
        let touch = FileTouch {
            id: None,
            agent_id: 1,
            path: "a.rs".into(),
            outside_workspace: true,
            added: 3,
            removed: 1,
            pre_ref: None,
            post_ref: Some("blake3:x".into()),
            ts: ts(),
        };
        let html = touch_item(&touch).into_string();
        assert!(html.contains("id=\"touch-t"));
        assert!(html.contains("calisma alani disi"));
        assert!(html.contains("yeni"));
    }

    #[test]
    fn tool_reddi_isaretlenir() {
        let call = ToolCallView {
            id: Some(9),
            agent_id: 2,
            tool: "write_file".into(),
            args: None,
            result_ref: None,
            status: "denied".into(),
            capability_ok: Some(false),
            ts: ts(),
        };
        let html = tool_item(&call).into_string();
        assert!(html.contains("id=\"tool-9\""));
        assert!(html.contains("yetki reddi"));
    }

    #[test]
    fn mudahale_ve_bildirim_render_olur() {
        let interrupt = InterruptView {
            id: Some(4),
            agent_id: 2,
            kind: "pause".into(),
            source: "webui".into(),
            reason: Some("bekle".into()),
            ts: ts(),
            resolved_at: None,
        };
        assert!(
            interrupt_item(&interrupt)
                .into_string()
                .contains("id=\"interrupt-4\"")
        );

        let notice = NoticeView::new(NoticeLevel::Critical, "depth_cap", "tavan asildi", ts());
        let html = notice_item(&notice).into_string();
        assert!(html.contains("tag failed"));
        assert!(html.contains("tavan asildi"));
    }

    #[test]
    fn komut_paneli_moddan_datalist_kurar() {
        let html = board(&snapshot()).into_string();
        assert!(html.contains("value=\"spawn_task\""));
        assert!(html.contains("<datalist id=\"modes\">"));
        assert!(html.contains("value=\"user_driven\""));
        assert!(html.contains("value=\"write_to_agent\""));
    }

    #[test]
    fn giris_ve_hata_sayfalari_render_olur() {
        let html = login_page(Some("token gecersiz")).into_string();
        assert!(html.contains("name=\"token\""));
        assert!(html.contains("token gecersiz"));
        let html = error_page("bad_request", "govde cozulemedi").into_string();
        assert!(html.contains("bad_request"));
    }
}
