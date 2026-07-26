//! TUI kabugu: gorunum secimi, tus isleme, komut uretimi (MASTER-PLAN 6.2).
//!
//! Uygulama **yerel mutasyon yapmaz**. Tus vurusu bir `omni_proto::Command`
//! uretir, komut `omni-control`'e gider, cekirdek uygular ve sonuc `StateEvent`
//! olarak akistan geri gelir. Ekranda gorulen her sey akistan turer — yani TUI
//! kendi gonderdigi komutun sonucunu tahmin etmez (K7 parite kurali).

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use omni_proto::{AgentId, AgentView, ApprovalDecision, StateEvent, StateFrame, SystemSnapshot};
use xai_ratatui_textarea::TextArea;

use crate::TuiError;
use crate::command::CommandOutbox;
use crate::dashboard::Dashboard;
use crate::diff_graph::DiffGraph;
use crate::interrupt_ui::{ApprovalPrompt, render_interrupts};
use crate::state::ApplyOutcome;
use crate::stream::{ControlFrame, ControlOutcome};

/// `tasks.mode` varsayilani. Config'ten degistirilebilir; koda mod listesi
/// gomulmez.
pub const DEFAULT_TASK_MODE: &str = "user_driven";

/// `interrupts.kind` varsayilani (AS2).
pub const DEFAULT_INTERRUPT_KIND: &str = "pause";

/// Girdi kutusunun yuksekligi (cerceve dahil).
const PROMPT_HEIGHT: u16 = 3;

/// Ana gorunumler. Hepsi ayni [`crate::state::UiState`]'ten cizilir.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    /// Ajan tablosu + diff grafigi + kaynak + bildirimler.
    Overview,
    /// Secili ajanin ayrintisi.
    Agent,
    /// Tam ekran diff grafigi (9.3).
    Diff,
    /// Acik mudahaleler (AS2 / 6.8).
    Interrupts,
}

/// Metin isteyen komutlar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    /// `Command::SpawnTask`.
    NewTask,
    /// `Command::WriteToAgent`.
    WriteToAgent,
    /// `Command::Interrupt` gerekcesi.
    InterruptReason,
}

impl PromptKind {
    /// Girdi kutusunun basligi.
    pub fn title(self) -> &'static str {
        match self {
            Self::NewTask => " Yeni gorev (Enter: gonder · Esc: vazgec) ",
            Self::WriteToAgent => " Ajana mesaj (Enter: gonder · Esc: vazgec) ",
            Self::InterruptReason => " Mudahale gerekcesi (Enter: gonder · Esc: vazgec) ",
        }
    }
}

/// Acik metin istemi.
struct Prompt {
    kind: PromptKind,
    agent_id: Option<AgentId>,
    editor: TextArea,
}

impl std::fmt::Debug for Prompt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Prompt")
            .field("kind", &self.kind)
            .field("agent_id", &self.agent_id)
            .field("len", &self.editor.text().len())
            .finish()
    }
}

/// TUI uygulama durumu.
pub struct App {
    dashboard: Dashboard,
    view: View,
    outbox: Option<CommandOutbox>,
    prompt: Option<Prompt>,
    approval: Option<ApprovalPrompt>,
    task_mode: String,
    last_error: Option<String>,
    quit: bool,
}

impl std::fmt::Debug for App {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("App")
            .field("view", &self.view)
            .field("selected", &self.dashboard.selected())
            .field("prompt", &self.prompt)
            .field("approval", &self.approval.is_some())
            .field("quit", &self.quit)
            .finish_non_exhaustive()
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    /// Salt-okunur uygulama (komut ucu baglanmamis).
    pub fn new() -> Self {
        Self {
            dashboard: Dashboard::new(),
            view: View::Overview,
            outbox: None,
            prompt: None,
            approval: None,
            task_mode: DEFAULT_TASK_MODE.to_string(),
            last_error: None,
            quit: false,
        }
    }

    /// Komut ucunu baglar (yazma yolu, 6.2).
    pub fn with_outbox(mut self, outbox: CommandOutbox) -> Self {
        self.outbox = Some(outbox);
        self
    }

    /// `Command::SpawnTask` icin kullanilacak modu degistirir.
    pub fn with_task_mode(mut self, mode: impl Into<String>) -> Self {
        self.task_mode = mode.into();
        self
    }

    /// Pano (ve icindeki kanonik durum).
    pub fn dashboard(&self) -> &Dashboard {
        &self.dashboard
    }

    /// Panoya yazma erisimi (akis surucusu icin).
    pub fn dashboard_mut(&mut self) -> &mut Dashboard {
        &mut self.dashboard
    }

    /// Ilk yukleme goruntusunu yerlestirir.
    pub fn load(&mut self, snapshot: SystemSnapshot) {
        self.dashboard.load(snapshot);
    }

    /// Akistan gelen zarfi uygular.
    pub fn apply(&mut self, frame: StateFrame) -> ApplyOutcome {
        self.dashboard.apply(frame)
    }

    /// Zarfsiz olay uygular.
    pub fn apply_event(&mut self, event: StateEvent) {
        self.dashboard.apply_event(event);
    }

    /// `omni-control` akisindan gelen cerceveyi uygular (6.2 okuma yolu).
    ///
    /// Surucu donen [`ControlOutcome::needs_resync`] `true` ise
    /// `SystemSnapshot`'i yeniden cekip [`App::load`] cagirmalidir. Hata
    /// cercevesi akisi kapatmaz; yardim seridinde gosterilir.
    pub fn apply_control(&mut self, frame: ControlFrame) -> ControlOutcome {
        let sonuc = self.dashboard.apply_control(frame);
        if let ControlOutcome::Failed { code, detail } = &sonuc {
            self.last_error = Some(format!("{code}: {detail}"));
        }
        sonuc
    }

    /// Aktif gorunum.
    pub fn view(&self) -> View {
        self.view
    }

    /// Gorunumu degistirir.
    pub fn set_view(&mut self, view: View) {
        self.view = view;
    }

    /// Kullanici cikis istedi mi (8.1: cikis O(1), flush yok).
    pub fn should_quit(&self) -> bool {
        self.quit
    }

    /// Son hata metni (yardim seridinde gosterilir).
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// Secili ajan.
    pub fn selected_agent(&self) -> Option<&AgentView> {
        self.dashboard
            .selected()
            .and_then(|id| self.dashboard.state().agent(id))
    }

    /// Acik onay istemi.
    pub fn approval(&self) -> Option<&ApprovalPrompt> {
        self.approval.as_ref()
    }

    /// Acik metin istemi turu.
    pub fn prompt_kind(&self) -> Option<PromptKind> {
        self.prompt.as_ref().map(|p| p.kind)
    }

    /// Acik metin isteminin icerigi.
    pub fn prompt_text(&self) -> Option<&str> {
        self.prompt.as_ref().map(|p| p.editor.text())
    }

    // -----------------------------------------------------------------------
    // Tus isleme
    // -----------------------------------------------------------------------

    /// Tus vurusunu isler.
    ///
    /// # Errors
    /// Komut uretilemezse (kanal kapali, secim yok, bos girdi) hata doner.
    /// Hata ayrica [`App::last_error`] icinde saklanir.
    pub fn on_key(&mut self, key: KeyEvent) -> Result<(), TuiError> {
        if key.kind != KeyEventKind::Press {
            return Ok(());
        }
        let sonuc = self.dispatch_key(key);
        match &sonuc {
            Ok(()) => {}
            Err(hata) => {
                self.last_error = Some(hata.to_string());
                tracing::warn!(%hata, "tus islenemedi");
            }
        }
        sonuc
    }

    /// Tus yonlendirmesi: once onay penceresi, sonra metin istemi, sonra global.
    fn dispatch_key(&mut self, key: KeyEvent) -> Result<(), TuiError> {
        if self.is_quit_key(key) {
            self.quit = true;
            return Ok(());
        }
        if self.approval.is_some() {
            return self.approval_key(key);
        }
        if self.prompt.is_some() {
            return self.prompt_key(key);
        }
        self.global_key(key)
    }

    /// Ctrl+C her yerde cikistir; `q` yalnizca metin girisi yokken.
    fn is_quit_key(&self, key: KeyEvent) -> bool {
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c' | 'C'))
        {
            return true;
        }
        self.prompt.is_none() && self.approval.is_none() && key.code == KeyCode::Char('q')
    }

    /// Onay penceresi acikken tuslar.
    fn approval_key(&mut self, key: KeyEvent) -> Result<(), TuiError> {
        match key.code {
            KeyCode::Esc => {
                self.approval = None;
                Ok(())
            }
            KeyCode::Tab | KeyCode::Left | KeyCode::Right => {
                if let Some(istem) = self.approval.as_mut() {
                    istem.toggle();
                }
                Ok(())
            }
            KeyCode::Char('y' | 'Y') => {
                self.set_approval_choice(ApprovalDecision::Allow);
                self.submit_approval()
            }
            KeyCode::Char('n' | 'N') => {
                self.set_approval_choice(ApprovalDecision::Deny);
                self.submit_approval()
            }
            KeyCode::Enter => self.submit_approval(),
            _ => Ok(()),
        }
    }

    /// Acik onay isteminin secimini atar.
    fn set_approval_choice(&mut self, decision: ApprovalDecision) {
        if let Some(istem) = self.approval.as_mut() {
            istem.set_choice(decision);
        }
    }

    /// Metin istemi acikken tuslar. Enter gonderir, Esc vazgecer, gerisi
    /// editore gider.
    fn prompt_key(&mut self, key: KeyEvent) -> Result<(), TuiError> {
        match key.code {
            KeyCode::Esc => {
                self.prompt = None;
                Ok(())
            }
            KeyCode::Enter if !key.modifiers.contains(KeyModifiers::ALT) => self.submit_prompt(),
            _ => {
                if let Some(prompt) = self.prompt.as_mut() {
                    prompt.editor.input(key);
                }
                Ok(())
            }
        }
    }

    /// Gorunum/secim/komut kisayollari.
    fn global_key(&mut self, key: KeyEvent) -> Result<(), TuiError> {
        match key.code {
            KeyCode::Esc | KeyCode::Char('1') => self.view = View::Overview,
            KeyCode::Char('2') | KeyCode::Enter => self.view = View::Agent,
            KeyCode::Char('3') => self.view = View::Diff,
            KeyCode::Char('4') => self.view = View::Interrupts,
            KeyCode::Down | KeyCode::Char('j') => self.dashboard.move_selection(1),
            KeyCode::Up | KeyCode::Char('k') => self.dashboard.move_selection(-1),
            KeyCode::Home => self.dashboard.move_selection(isize::MIN / 2),
            KeyCode::End => self.dashboard.move_selection(isize::MAX / 2),
            KeyCode::Char('n') => self.open_prompt(PromptKind::NewTask, None),
            KeyCode::Char('w') => {
                let id = self.require_selection()?;
                self.open_prompt(PromptKind::WriteToAgent, Some(id));
            }
            KeyCode::Char('i') => {
                let id = self.require_selection()?;
                self.open_prompt(PromptKind::InterruptReason, Some(id));
            }
            KeyCode::Char('a') => return self.open_approval(),
            _ => {}
        }
        Ok(())
    }

    /// Secili ajan kimligini ister.
    fn require_selection(&self) -> Result<AgentId, TuiError> {
        self.dashboard.selected().ok_or(TuiError::NoAgentSelected)
    }

    /// Komut ucunu ister.
    fn require_outbox(&self) -> Result<&CommandOutbox, TuiError> {
        self.outbox.as_ref().ok_or(TuiError::NoCommandSink)
    }

    /// Metin istemini acar.
    fn open_prompt(&mut self, kind: PromptKind, agent_id: Option<AgentId>) {
        self.last_error = None;
        self.prompt = Some(Prompt {
            kind,
            agent_id,
            editor: TextArea::new(),
        });
    }

    /// Secili ajan icin onay bekleyen ilk tool cagrisini acar (K3).
    fn open_approval(&mut self) -> Result<(), TuiError> {
        let id = self.require_selection()?;
        let approver = self
            .outbox
            .as_ref()
            .map(|o| o.approver().to_string())
            .unwrap_or_default();
        let istem = self
            .dashboard
            .state()
            .first_pending_approval(id)
            .map(ApprovalPrompt::from_tool_call)
            .ok_or(TuiError::NoPendingApproval)?;
        self.approval = Some(if approver.is_empty() {
            istem
        } else {
            istem.with_approver(approver)
        });
        Ok(())
    }

    /// Onay kararini kontrol duzlemine gonderir.
    fn submit_approval(&mut self) -> Result<(), TuiError> {
        let Some(istem) = self.approval.take() else {
            return Ok(());
        };
        let sonuc = self
            .require_outbox()
            .and_then(|outbox| outbox.send(istem.command()));
        if sonuc.is_err() {
            // Gonderilemedi: istem acik kalsin ki kullanici tekrar denesin.
            self.approval = Some(istem);
        }
        sonuc
    }

    /// Metin istemini komuta cevirip gonderir.
    fn submit_prompt(&mut self) -> Result<(), TuiError> {
        let Some(prompt) = self.prompt.as_ref() else {
            return Ok(());
        };
        let kind = prompt.kind;
        let agent_id = prompt.agent_id;
        let metin = prompt.editor.text().to_string();
        let mode = self.task_mode.clone();

        let sonuc = {
            let outbox = self.require_outbox()?;
            match kind {
                PromptKind::NewTask => outbox.spawn_task(metin, mode, None),
                PromptKind::WriteToAgent => match agent_id {
                    Some(id) => outbox.write_to_agent(id, metin),
                    None => Err(TuiError::NoAgentSelected),
                },
                PromptKind::InterruptReason => match agent_id {
                    Some(id) => {
                        let gerekce = metin.trim();
                        let gerekce = if gerekce.is_empty() {
                            None
                        } else {
                            Some(gerekce.to_string())
                        };
                        outbox.interrupt(id, DEFAULT_INTERRUPT_KIND, gerekce)
                    }
                    None => Err(TuiError::NoAgentSelected),
                },
            }
        };

        if sonuc.is_ok() {
            self.prompt = None;
        }
        sonuc
    }

    // -----------------------------------------------------------------------
    // Cizim
    // -----------------------------------------------------------------------

    /// Tum ekrani cizer.
    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let prompt_h = if self.prompt.is_some() {
            PROMPT_HEIGHT
        } else {
            0
        };
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(4),
                Constraint::Length(prompt_h),
                Constraint::Length(1),
            ])
            .split(area);

        if let Some(ana) = chunks.first() {
            self.render_main(frame, *ana);
        }
        if let Some(alan) = chunks.get(1)
            && self.prompt.is_some()
        {
            self.render_prompt(frame, *alan);
        }
        if let Some(alan) = chunks.get(2) {
            self.render_help(frame, *alan);
        }
        if let Some(istem) = self.approval.as_ref() {
            istem.render(frame, area);
        }
    }

    /// Aktif gorunumu cizer.
    fn render_main(&self, frame: &mut Frame, area: Rect) {
        match self.view {
            View::Overview => self.dashboard.render(frame, area),
            View::Agent => match self.dashboard.selected() {
                Some(id) => {
                    crate::agent_detail::AgentDetail::new(self.dashboard.state(), id)
                        .render(frame, area);
                }
                None => self.dashboard.render(frame, area),
            },
            View::Diff => DiffGraph::per_agent(self.dashboard.state()).render(frame, area),
            View::Interrupts => {
                let acik = self.dashboard.state().open_interrupts();
                render_interrupts(frame, area, &acik);
            }
        }
    }

    /// Metin girdi kutusu.
    fn render_prompt(&self, frame: &mut Frame, area: Rect) {
        let Some(prompt) = self.prompt.as_ref() else {
            return;
        };
        let block = Block::default()
            .title(prompt.kind.title())
            .borders(Borders::ALL)
            .style(Style::default().fg(Color::Cyan));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        frame.render_widget(&&prompt.editor, inner);
        if let Some(konum) = prompt.editor.cursor_pos(inner) {
            frame.set_cursor_position(konum);
        }
    }

    /// Alt yardim/durum seridi.
    fn render_help(&self, frame: &mut Frame, area: Rect) {
        if let Some(hata) = self.last_error.as_deref() {
            let satir = Line::from(Span::styled(
                format!(" {hata}"),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ));
            frame.render_widget(Paragraph::new(satir), area);
            return;
        }

        let etiket = match self.view {
            View::Overview => "genel",
            View::Agent => "ajan",
            View::Diff => "diff",
            View::Interrupts => "mudahale",
        };
        let satir = Line::from(vec![
            Span::styled(
                format!(" [{etiket}] "),
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                " 1/2/3/4 gorunum · j/k sec · n gorev · w mesaj · i mudahale · a onay · q cikis",
                Style::default().fg(Color::DarkGray),
            ),
        ]);
        frame.render_widget(Paragraph::new(satir), area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omni_proto::{AgentState, AgentTier, Command, ToolCallView};
    use ratatui::{Terminal, backend::TestBackend};
    use tokio::sync::mpsc::UnboundedReceiver;

    fn tus(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn harf(c: char) -> KeyEvent {
        tus(KeyCode::Char(c))
    }

    fn ajan(id: AgentId) -> AgentView {
        AgentView {
            id,
            persona: format!("p{id}"),
            tier: AgentTier::Active,
            task_id: 1,
            parent_id: None,
            state: AgentState::AwaitingModel,
            rss_kb: 1024,
            tokens_in: 1,
            tokens_out: 1,
            cost: 0.0,
            trust: 1.0,
            depth: 0,
            last_event_seq: 1,
        }
    }

    fn uygulama() -> (App, UnboundedReceiver<Command>) {
        let (outbox, rx) = CommandOutbox::channel();
        let mut app = App::new().with_outbox(outbox);
        app.apply_event(StateEvent::AgentUpserted(ajan(1)));
        app.apply_event(StateEvent::AgentUpserted(ajan(2)));
        (app, rx)
    }

    fn yaz(app: &mut App, metin: &str) {
        for ch in metin.chars() {
            let _ = app.on_key(harf(ch));
        }
    }

    fn ciz(app: &App) -> String {
        let backend = TestBackend::new(140, 30);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| app.render(frame, frame.area()))
            .expect("cizim");
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect()
    }

    #[test]
    fn gorunum_kisayollari_calisir() {
        let (mut app, _rx) = uygulama();
        assert_eq!(app.view(), View::Overview);
        let _ = app.on_key(harf('3'));
        assert_eq!(app.view(), View::Diff);
        let _ = app.on_key(harf('4'));
        assert_eq!(app.view(), View::Interrupts);
        let _ = app.on_key(tus(KeyCode::Enter));
        assert_eq!(app.view(), View::Agent);
        let _ = app.on_key(tus(KeyCode::Esc));
        assert_eq!(app.view(), View::Overview);
    }

    #[test]
    fn secim_tuslari_calisir() {
        let (mut app, _rx) = uygulama();
        assert_eq!(app.selected_agent().map(|a| a.id), Some(1));
        let _ = app.on_key(harf('j'));
        assert_eq!(app.selected_agent().map(|a| a.id), Some(2));
        let _ = app.on_key(harf('k'));
        assert_eq!(app.selected_agent().map(|a| a.id), Some(1));
        let _ = app.on_key(tus(KeyCode::End));
        assert_eq!(app.selected_agent().map(|a| a.id), Some(2));
        let _ = app.on_key(tus(KeyCode::Home));
        assert_eq!(app.selected_agent().map(|a| a.id), Some(1));
    }

    #[test]
    fn cikis_tuslari() {
        let (mut app, _rx) = uygulama();
        assert!(!app.should_quit());
        let _ = app.on_key(harf('q'));
        assert!(app.should_quit());

        let (mut app, _rx) = uygulama();
        let _ = app.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(app.should_quit());
    }

    #[test]
    fn gorev_istemi_komut_uretir() {
        let (mut app, mut rx) = uygulama();
        let _ = app.on_key(harf('n'));
        assert_eq!(app.prompt_kind(), Some(PromptKind::NewTask));
        yaz(&mut app, "dikey dilim");
        assert_eq!(app.prompt_text(), Some("dikey dilim"));
        app.on_key(tus(KeyCode::Enter)).expect("gonderim");
        assert!(app.prompt_kind().is_none());

        match rx.try_recv().expect("komut") {
            Command::SpawnTask { title, mode, .. } => {
                assert_eq!(title, "dikey dilim");
                assert_eq!(mode, DEFAULT_TASK_MODE);
            }
            other => panic!("beklenmeyen komut: {}", other.kind()),
        }
    }

    #[test]
    fn bos_gorev_istemi_acik_kalir() {
        let (mut app, mut rx) = uygulama();
        let _ = app.on_key(harf('n'));
        assert!(matches!(
            app.on_key(tus(KeyCode::Enter)),
            Err(TuiError::EmptyInput)
        ));
        assert_eq!(app.prompt_kind(), Some(PromptKind::NewTask));
        assert!(rx.try_recv().is_err());
        assert!(app.last_error().is_some());
    }

    #[test]
    fn ajana_mesaj_secili_ajana_gider() {
        let (mut app, mut rx) = uygulama();
        let _ = app.on_key(harf('j'));
        let _ = app.on_key(harf('w'));
        yaz(&mut app, "devam et");
        app.on_key(tus(KeyCode::Enter)).expect("gonderim");
        match rx.try_recv().expect("komut") {
            Command::WriteToAgent { agent_id, content } => {
                assert_eq!(agent_id, 2);
                assert_eq!(content, "devam et");
            }
            other => panic!("beklenmeyen komut: {}", other.kind()),
        }
    }

    #[test]
    fn mudahale_gerekcesiz_de_gonderilir() {
        let (mut app, mut rx) = uygulama();
        let _ = app.on_key(harf('i'));
        assert_eq!(app.prompt_kind(), Some(PromptKind::InterruptReason));
        app.on_key(tus(KeyCode::Enter)).expect("gonderim");
        match rx.try_recv().expect("komut") {
            Command::Interrupt {
                agent_id,
                kind,
                reason,
                ..
            } => {
                assert_eq!(agent_id, 1);
                assert_eq!(kind, DEFAULT_INTERRUPT_KIND);
                assert!(reason.is_none());
            }
            other => panic!("beklenmeyen komut: {}", other.kind()),
        }
    }

    #[test]
    fn onay_bekleyen_yoksa_hata() {
        let (mut app, _rx) = uygulama();
        assert!(matches!(
            app.on_key(harf('a')),
            Err(TuiError::NoPendingApproval)
        ));
        assert!(app.approval().is_none());
    }

    #[test]
    fn onay_komutu_uretir() {
        let (mut app, mut rx) = uygulama();
        app.apply_event(StateEvent::ToolCall(ToolCallView {
            id: Some(1),
            agent_id: 1,
            tool: "write".into(),
            args: Some(serde_json::json!({ "path": "src/lib.rs" })),
            result_ref: None,
            status: "pending".into(),
            capability_ok: None,
            ts: omni_proto::now(),
        }));

        app.on_key(harf('a')).expect("istem acilir");
        assert!(app.approval().is_some());
        assert!(ciz(&app).contains("Yetki onayi"));

        app.on_key(harf('y')).expect("gonderim");
        assert!(app.approval().is_none());
        match rx.try_recv().expect("komut") {
            Command::Approve {
                agent_id,
                capability,
                decision,
                ..
            } => {
                assert_eq!(agent_id, 1);
                assert_eq!(capability, "write");
                assert_eq!(decision, ApprovalDecision::Allow);
            }
            other => panic!("beklenmeyen komut: {}", other.kind()),
        }
    }

    #[test]
    fn komut_ucu_yoksa_hata_verir() {
        let mut app = App::new();
        app.apply_event(StateEvent::AgentUpserted(ajan(1)));
        let _ = app.on_key(harf('n'));
        yaz(&mut app, "x");
        assert!(matches!(
            app.on_key(tus(KeyCode::Enter)),
            Err(TuiError::NoCommandSink)
        ));
    }

    #[test]
    fn secim_yokken_mesaj_reddedilir() {
        let (outbox, _rx) = CommandOutbox::channel();
        let mut app = App::new().with_outbox(outbox);
        assert!(matches!(
            app.on_key(harf('w')),
            Err(TuiError::NoAgentSelected)
        ));
    }

    #[test]
    fn ekran_gorunume_gore_cizilir() {
        let (mut app, _rx) = uygulama();
        assert!(ciz(&app).contains("Ajanlar"));
        let _ = app.on_key(harf('3'));
        assert!(ciz(&app).contains("Diff akisi"));
        let _ = app.on_key(harf('4'));
        assert!(ciz(&app).contains("Acik mudahaleler"));
        let _ = app.on_key(harf('1'));
        let _ = app.on_key(harf('n'));
        assert!(ciz(&app).contains("Yeni gorev"));
    }

    #[test]
    fn istem_esc_ile_kapanir() {
        let (mut app, _rx) = uygulama();
        let _ = app.on_key(harf('n'));
        yaz(&mut app, "abc");
        let _ = app.on_key(tus(KeyCode::Esc));
        assert!(app.prompt_kind().is_none());
    }

    #[test]
    fn zarf_akisi_uygulanir() {
        let (mut app, _rx) = uygulama();
        assert_eq!(
            app.apply(StateFrame::new(1, StateEvent::AgentUpserted(ajan(3)))),
            ApplyOutcome::Applied
        );
        assert_eq!(app.dashboard().state().agents().len(), 3);
    }
}
