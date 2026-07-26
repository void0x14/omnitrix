//! Onay/mudahale yuzeyi (MASTER-PLAN 6.2 yazma yolu + AS2 + K3).
//!
//! Bu modul karar **vermez**, karar **tasir**: kullanicinin sectigi sonuc
//! `omni_proto::Command::Approve` olarak `omni-control`'e gider, cekirdek
//! uygular ve sonuc `StateEvent` olarak her iki yuze doner. TUI yerel olarak
//! "onaylandi" varsaymaz.

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap},
};

use omni_proto::{AgentId, ApprovalDecision, Command, InterruptView, ToolCallView};

use crate::command::DEFAULT_APPROVER;

/// Onay penceresinin ekrandaki genislik orani (yuzde).
const POPUP_WIDTH_PCT: u16 = 70;

/// Onay penceresinin en fazla yuksekligi (satir).
const POPUP_HEIGHT: u16 = 11;

/// Yetki broker'i onay istemi (K3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalPrompt {
    /// Onayi bekleyen ajan.
    pub agent_id: AgentId,
    /// `capability_audit.capability` — istenen yetenek (tool adi).
    pub capability: String,
    /// `capability_audit.target` — yetenegin uygulanacagi hedef.
    pub target: String,
    /// Kullaniciya gosterilen ayrinti (tool argumanlarinin ozeti).
    pub detail: String,
    /// `capability_audit.approver`.
    pub approver: String,
    /// Su an secili karar.
    pub choice: ApprovalDecision,
}

impl ApprovalPrompt {
    /// Elle istem kurar. Varsayilan secim `Deny`'dir: sessiz kabul yok.
    pub fn new(
        agent_id: AgentId,
        capability: impl Into<String>,
        target: impl Into<String>,
    ) -> Self {
        Self {
            agent_id,
            capability: capability.into(),
            target: target.into(),
            detail: String::new(),
            approver: DEFAULT_APPROVER.to_string(),
            choice: ApprovalDecision::Deny,
        }
    }

    /// Onay bekleyen bir tool cagrisindan istem uretir.
    pub fn from_tool_call(call: &ToolCallView) -> Self {
        let mut istem = Self::new(call.agent_id, call.tool.clone(), tool_target(call));
        istem.detail = crate::agent_detail::arg_preview(call);
        istem
    }

    /// Onay kimligini degistirir.
    pub fn with_approver(mut self, approver: impl Into<String>) -> Self {
        self.approver = approver.into();
        self
    }

    /// Ayrinti metnini degistirir.
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = detail.into();
        self
    }

    /// Secimi ters cevirir.
    pub fn toggle(&mut self) {
        self.choice = match self.choice {
            ApprovalDecision::Allow => ApprovalDecision::Deny,
            ApprovalDecision::Deny => ApprovalDecision::Allow,
        };
    }

    /// Secimi dogrudan atar.
    pub fn set_choice(&mut self, decision: ApprovalDecision) {
        self.choice = decision;
    }

    /// Izin verildi mi?
    pub fn is_allowed(&self) -> bool {
        self.choice == ApprovalDecision::Allow
    }

    /// Kontrol duzlemine gidecek komutu uretir (6.2).
    pub fn command(&self) -> Command {
        Command::Approve {
            agent_id: self.agent_id,
            capability: self.capability.clone(),
            target: self.target.clone(),
            decision: self.choice,
            approver: self.approver.clone(),
        }
    }

    /// Istemi ekranin ortasinda bir pencere olarak cizer.
    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let pencere = centered_rect(POPUP_WIDTH_PCT, POPUP_HEIGHT, area);
        frame.render_widget(Clear, pencere);

        let block = Block::default()
            .title(" Yetki onayi bekleniyor ")
            .borders(Borders::ALL)
            .border_type(BorderType::Double)
            .style(Style::default().fg(Color::Yellow));
        let inner = block.inner(pencere);
        frame.render_widget(block, pencere);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(2),
            ])
            .split(inner);

        if let Some(satir) = chunks.first() {
            let baslik = Line::from(vec![
                Span::styled(
                    format!("ajan #{} ", self.agent_id),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{} ", self.capability),
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!("-> {}", self.target)),
            ]);
            frame.render_widget(Paragraph::new(baslik), *satir);
        }

        if let Some(satir) = chunks.get(1) {
            frame.render_widget(
                Paragraph::new(Span::styled(
                    format!("onaylayan: {}", self.approver),
                    Style::default().fg(Color::DarkGray),
                )),
                *satir,
            );
        }

        if let Some(satir) = chunks.get(2) {
            frame.render_widget(
                Paragraph::new(self.detail.clone())
                    .wrap(Wrap { trim: false })
                    .style(Style::default().fg(Color::White)),
                *satir,
            );
        }

        if let Some(satir) = chunks.get(3) {
            frame.render_widget(
                Paragraph::new(self.choice_line()).alignment(Alignment::Center),
                *satir,
            );
        }
    }

    /// Secim satiri.
    fn choice_line(&self) -> Line<'static> {
        let izin = secim_stili(Color::Green, self.choice == ApprovalDecision::Allow);
        let ret = secim_stili(Color::Red, self.choice == ApprovalDecision::Deny);
        Line::from(vec![
            Span::styled(" [y] izin ver ", izin),
            Span::raw("   "),
            Span::styled(" [n] reddet ", ret),
            Span::styled(
                "   (Tab: degistir · Enter: gonder · Esc: vazgec)",
                Style::default().fg(Color::DarkGray),
            ),
        ])
    }
}

/// Acik mudahalelerin ozet satirlarini cizer (AS2 / 6.8).
pub fn render_interrupts(frame: &mut Frame, area: Rect, interrupts: &[&InterruptView]) {
    let satirlar: Vec<Line<'static>> = interrupts
        .iter()
        .map(|kayit| {
            let ts = kayit.ts.format("%H:%M:%S").to_string();
            Line::from(vec![
                Span::styled(format!("{ts} "), Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("#{} ", kayit.agent_id),
                    Style::default().fg(Color::Cyan),
                ),
                Span::styled(
                    format!("{} ", kayit.kind),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("({}) ", kayit.source),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::raw(kayit.reason.clone().unwrap_or_default()),
            ])
        })
        .collect();

    let baslik = format!(" Acik mudahaleler ({}) ", interrupts.len());
    let paragraf =
        Paragraph::new(satirlar).block(Block::default().title(baslik).borders(Borders::ALL));
    frame.render_widget(paragraf, area);
}

/// Tool argumanlarindan insan okunur hedef cikarir.
///
/// Hedef alan adi tool'a gore degisir; bilinen adlar sirayla denenir, hicbiri
/// yoksa `-` yazilir (koda tool listesi gomulmez).
fn tool_target(call: &ToolCallView) -> String {
    const ANAHTARLAR: [&str; 5] = ["path", "file_path", "target", "command", "url"];
    let Some(args) = call.args.as_ref() else {
        return "-".to_string();
    };
    for anahtar in ANAHTARLAR {
        if let Some(deger) = args.get(anahtar).and_then(serde_json::Value::as_str) {
            return deger.to_string();
        }
    }
    "-".to_string()
}

/// Secili/secili-degil buton stili.
fn secim_stili(renk: Color, secili: bool) -> Style {
    let temel = Style::default().fg(renk);
    if secili {
        temel.add_modifier(Modifier::REVERSED | Modifier::BOLD)
    } else {
        temel
    }
}

/// Verilen alanin ortasinda, genisligi yuzde / yuksekligi satir olan dikdortgen.
fn centered_rect(width_pct: u16, height: u16, area: Rect) -> Rect {
    let genislik = area.width.saturating_mul(width_pct.min(100)) / 100;
    let genislik = genislik.min(area.width);
    let yukseklik = height.min(area.height);
    let x = area.x + (area.width.saturating_sub(genislik)) / 2;
    let y = area.y + (area.height.saturating_sub(yukseklik)) / 2;
    Rect {
        x,
        y,
        width: genislik,
        height: yukseklik,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    fn cagri() -> ToolCallView {
        ToolCallView {
            id: Some(1),
            agent_id: 4,
            tool: "write".into(),
            args: Some(serde_json::json!({ "path": "/etc/hosts", "content": "x" })),
            result_ref: None,
            status: "pending".into(),
            capability_ok: None,
            ts: omni_proto::now(),
        }
    }

    fn ciz(istem: &ApprovalPrompt) -> String {
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| istem.render(frame, frame.area()))
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
    fn varsayilan_secim_reddetmektir() {
        let istem = ApprovalPrompt::from_tool_call(&cagri());
        assert_eq!(istem.choice, ApprovalDecision::Deny);
        assert!(!istem.is_allowed());
    }

    #[test]
    fn hedef_argumanlardan_cozulur() {
        let istem = ApprovalPrompt::from_tool_call(&cagri());
        assert_eq!(istem.agent_id, 4);
        assert_eq!(istem.capability, "write");
        assert_eq!(istem.target, "/etc/hosts");
        assert!(istem.detail.contains("hosts"));
    }

    #[test]
    fn hedefsiz_cagri_tire_verir() {
        let mut call = cagri();
        call.args = Some(serde_json::json!({ "nothing": 1 }));
        assert_eq!(ApprovalPrompt::from_tool_call(&call).target, "-");
        call.args = None;
        assert_eq!(ApprovalPrompt::from_tool_call(&call).target, "-");
    }

    #[test]
    fn secim_komuta_donusur() {
        let mut istem = ApprovalPrompt::from_tool_call(&cagri()).with_approver("kullanici");
        istem.toggle();
        assert!(istem.is_allowed());
        match istem.command() {
            Command::Approve {
                agent_id,
                capability,
                target,
                decision,
                approver,
            } => {
                assert_eq!(agent_id, 4);
                assert_eq!(capability, "write");
                assert_eq!(target, "/etc/hosts");
                assert_eq!(decision, ApprovalDecision::Allow);
                assert_eq!(approver, "kullanici");
            }
            other => panic!("beklenmeyen komut: {}", other.kind()),
        }

        istem.set_choice(ApprovalDecision::Deny);
        assert!(!istem.is_allowed());
    }

    #[test]
    fn pencere_ortalanir_ve_tasmaz() {
        let alan = Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 4,
        };
        let pencere = centered_rect(70, POPUP_HEIGHT, alan);
        assert!(pencere.width <= alan.width);
        assert!(pencere.height <= alan.height);
        assert!(pencere.x + pencere.width <= alan.width);
    }

    #[test]
    fn istem_cizilir() {
        let cikti = ciz(&ApprovalPrompt::from_tool_call(&cagri()));
        assert!(cikti.contains("Yetki onayi"));
        assert!(cikti.contains("write"));
        assert!(cikti.contains("izin ver"));
        assert!(cikti.contains("reddet"));
    }

    #[test]
    fn mudahaleler_cizilir() {
        let kayit = InterruptView {
            id: Some(1),
            agent_id: 7,
            kind: "pause".into(),
            source: "tui".into(),
            reason: Some("operator durdurdu".into()),
            ts: omni_proto::now(),
            resolved_at: None,
        };
        let liste = vec![&kayit];
        let backend = TestBackend::new(100, 6);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| render_interrupts(frame, frame.area(), &liste))
            .expect("cizim");
        let cikti: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect();
        assert!(cikti.contains("pause"));
        assert!(cikti.contains("operator durdurdu"));
    }
}
