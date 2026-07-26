//! Bildirim tetikleyicileri (MASTER-PLAN Bolum 13).
//!
//! Uc tetikleyici vardir ve **yalnizca** bunlar bildirim uretir:
//! 1. Gorev bitimi (`tasks.closed_at` dolar ya da durum bitis kumesine girer),
//! 2. Kritik hata (`NoticeView.level` disariya tasinacak seviyede),
//! 3. Insan onayi bekleyen mudahale (acik `InterruptView`, onay turu).
//!
//! Girdi `omni-proto`'nun `StateEvent` akisidir — bildirim katmani kendi olay
//! tipini icat etmez (I3). Iki UI ne goruyorsa bildirim de onu gorur.

use omni_proto::{AgentId, NoticeLevel, StateEvent, TaskId, Timestamp};

use crate::policy::NotifyPolicy;

/// Bildirimi doguran tetikleyici.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NotifyTrigger {
    /// Gorev bitti (basarili ya da basarisiz).
    TaskFinished,
    /// Kritik hata bildirimi.
    CriticalError,
    /// Insan onayi bekleniyor.
    ApprovalRequired,
}

impl NotifyTrigger {
    /// Kanonik ad (imza ve log icin).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TaskFinished => "task_finished",
            Self::CriticalError => "critical_error",
            Self::ApprovalRequired => "approval_required",
        }
    }
}

/// Kanallara verilecek, tasima-bagimsiz bildirim.
#[derive(Debug, Clone)]
pub struct Notification {
    /// Tetikleyici.
    pub trigger: NotifyTrigger,
    /// Onem seviyesi — kanal esigi bununla karsilastirilir.
    pub level: NoticeLevel,
    /// Makine tarafinda eslenebilir kisa kod (dedup imzasinin govdesi).
    pub code: String,
    /// Tek satirlik baslik.
    pub title: String,
    /// Ayrinti metni.
    pub body: String,
    /// Ilgili ajan.
    pub agent_id: Option<AgentId>,
    /// Ilgili gorev.
    pub task_id: Option<TaskId>,
    /// Olay ani.
    pub ts: Timestamp,
}

impl Notification {
    /// Durum olayindan bildirim uretir; olay uc tetikleyiciden birine
    /// girmiyorsa `None` doner.
    #[must_use]
    pub fn from_state_event(event: &StateEvent, policy: &NotifyPolicy) -> Option<Self> {
        match event {
            StateEvent::TaskUpserted(task) => {
                let bitti = task.closed_at.is_some() || policy.is_done_status(&task.status);
                let basarisiz = policy.is_failed_status(&task.status);
                if !bitti && !basarisiz {
                    return None;
                }
                let level = if basarisiz {
                    NoticeLevel::Error
                } else {
                    NoticeLevel::Info
                };
                let baslik = if basarisiz {
                    format!("Gorev basarisiz: {}", task.title)
                } else {
                    format!("Gorev bitti: {}", task.title)
                };
                let butce = match (task.budget_allocated, task.budget_spent) {
                    (Some(zarf), Some(harcanan)) => {
                        format!(" · butce {harcanan:.2}/{zarf:.2}")
                    }
                    (None, Some(harcanan)) => format!(" · harcanan {harcanan:.2}"),
                    _ => String::new(),
                };
                Some(Self {
                    trigger: NotifyTrigger::TaskFinished,
                    level,
                    code: format!("task_finished:{}", task.id),
                    title: baslik,
                    body: format!(
                        "gorev #{} · mod {} · durum {} · derinlik {}{}",
                        task.id, task.mode, task.status, task.depth, butce
                    ),
                    agent_id: None,
                    task_id: Some(task.id),
                    ts: task.closed_at.unwrap_or(task.created_at),
                })
            }

            StateEvent::Notice(notice) => {
                if !notice.level.notifies_externally() {
                    return None;
                }
                Some(Self {
                    trigger: NotifyTrigger::CriticalError,
                    level: notice.level,
                    code: notice.code.clone(),
                    title: format!("Kritik: {}", notice.code),
                    body: notice.message.clone(),
                    agent_id: notice.agent_id,
                    task_id: notice.task_id,
                    ts: notice.ts,
                })
            }

            StateEvent::Interrupt(interrupt) => {
                if !interrupt.is_open() || !policy.is_approval_kind(&interrupt.kind) {
                    return None;
                }
                let gerekce = interrupt.reason.clone().unwrap_or_default();
                Some(Self {
                    trigger: NotifyTrigger::ApprovalRequired,
                    // Insan mudahalesi gerektiren tek durum; telefonu calan da budur.
                    level: NoticeLevel::Critical,
                    code: format!("approval:{}:{}", interrupt.kind, interrupt.agent_id),
                    title: format!("Onay bekleniyor: ajan #{}", interrupt.agent_id),
                    body: if gerekce.is_empty() {
                        format!("tur {} · kaynak {}", interrupt.kind, interrupt.source)
                    } else {
                        format!(
                            "tur {} · kaynak {} · {}",
                            interrupt.kind, interrupt.source, gerekce
                        )
                    },
                    agent_id: Some(interrupt.agent_id),
                    task_id: None,
                    ts: interrupt.ts,
                })
            }

            StateEvent::AgentUpserted(_)
            | StateEvent::FileTouched(_)
            | StateEvent::ToolCall(_)
            | StateEvent::ResourceTick(_) => None,
        }
    }

    /// Susturma imzasi: ayni tetikleyici + kod + hedef, ayni olay sayilir.
    /// Zaman damgasi **girmez** — girseydi hicbir tekrar eslesmezdi.
    #[must_use]
    pub fn signature(&self) -> String {
        format!(
            "{}|{}|a{}|t{}",
            self.trigger.as_str(),
            self.code,
            self.agent_id.unwrap_or(-1),
            self.task_id.unwrap_or(-1),
        )
    }

    /// Tasima-bagimsiz duz metin govdesi.
    #[must_use]
    pub fn plain_text(&self, suppressed: u32) -> String {
        let mut metin = format!("[{}] {}\n{}", self.level_tag(), self.title, self.body);
        if suppressed > 0 {
            metin.push_str(&format!("\n(+{suppressed} tekrar bastirildi)"));
        }
        metin
    }

    /// Seviyenin metin etiketi.
    #[must_use]
    pub fn level_tag(&self) -> &'static str {
        match self.level {
            NoticeLevel::Info => "INFO",
            NoticeLevel::Warn => "WARN",
            NoticeLevel::Error => "ERROR",
            NoticeLevel::Critical => "CRITICAL",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omni_proto::{InterruptView, NoticeView, TaskView};

    fn policy() -> NotifyPolicy {
        NotifyPolicy::default()
    }

    fn task(status: &str, closed: bool) -> TaskView {
        let now = omni_proto::now();
        TaskView {
            id: 7,
            parent_id: None,
            root_id: 7,
            title: "dilim".into(),
            mode: "mvp".into(),
            status: status.into(),
            depth: 0,
            budget_allocated: Some(10.0),
            budget_spent: Some(2.5),
            duration_target: None,
            created_at: now,
            closed_at: if closed { Some(now) } else { None },
        }
    }

    #[test]
    fn calisan_gorev_bildirim_uretmez() {
        let event = StateEvent::TaskUpserted(task("running", false));
        assert!(Notification::from_state_event(&event, &policy()).is_none());
    }

    #[test]
    fn biten_gorev_info_seviyesinde_bildirir() {
        let event = StateEvent::TaskUpserted(task("done", true));
        let bildirim = match Notification::from_state_event(&event, &policy()) {
            Some(n) => n,
            None => panic!("bildirim bekleniyordu"),
        };
        assert_eq!(bildirim.trigger, NotifyTrigger::TaskFinished);
        assert_eq!(bildirim.level, NoticeLevel::Info);
        assert_eq!(bildirim.task_id, Some(7));
        assert!(bildirim.body.contains("butce 2.50/10.00"));
    }

    #[test]
    fn basarisiz_gorev_error_seviyesinde_bildirir() {
        let event = StateEvent::TaskUpserted(task("failed", true));
        let bildirim = match Notification::from_state_event(&event, &policy()) {
            Some(n) => n,
            None => panic!("bildirim bekleniyordu"),
        };
        assert_eq!(bildirim.level, NoticeLevel::Error);
        assert!(bildirim.title.starts_with("Gorev basarisiz"));
    }

    #[test]
    fn dusuk_seviyeli_notice_bildirim_uretmez() {
        let notice = NoticeView::new(NoticeLevel::Warn, "depth_cap", "tavan", omni_proto::now());
        let event = StateEvent::Notice(notice);
        assert!(Notification::from_state_event(&event, &policy()).is_none());
    }

    #[test]
    fn kritik_notice_bildirim_uretir() {
        let notice = NoticeView::new(
            NoticeLevel::Critical,
            "provider_down",
            "saglayici dustu",
            omni_proto::now(),
        )
        .with_agent(3);
        let event = StateEvent::Notice(notice);
        let bildirim = match Notification::from_state_event(&event, &policy()) {
            Some(n) => n,
            None => panic!("bildirim bekleniyordu"),
        };
        assert_eq!(bildirim.trigger, NotifyTrigger::CriticalError);
        assert_eq!(bildirim.code, "provider_down");
        assert_eq!(bildirim.agent_id, Some(3));
    }

    fn interrupt(kind: &str, resolved: bool) -> InterruptView {
        let now = omni_proto::now();
        InterruptView {
            id: Some(1),
            agent_id: 5,
            kind: kind.into(),
            source: "broker".into(),
            reason: Some("exec onayi".into()),
            ts: now,
            resolved_at: if resolved { Some(now) } else { None },
        }
    }

    #[test]
    fn acik_onay_mudahalesi_critical_uretir() {
        let event = StateEvent::Interrupt(interrupt("capability_approval", false));
        let bildirim = match Notification::from_state_event(&event, &policy()) {
            Some(n) => n,
            None => panic!("bildirim bekleniyordu"),
        };
        assert_eq!(bildirim.trigger, NotifyTrigger::ApprovalRequired);
        assert_eq!(bildirim.level, NoticeLevel::Critical);
    }

    #[test]
    fn cozulmus_mudahale_bildirim_uretmez() {
        let event = StateEvent::Interrupt(interrupt("capability_approval", true));
        assert!(Notification::from_state_event(&event, &policy()).is_none());
    }

    #[test]
    fn onay_disi_mudahale_bildirim_uretmez() {
        let event = StateEvent::Interrupt(interrupt("pause", false));
        assert!(Notification::from_state_event(&event, &policy()).is_none());
    }

    #[test]
    fn imza_zaman_damgasini_icermez() {
        let ilk = StateEvent::Notice(NoticeView::new(
            NoticeLevel::Error,
            "oom",
            "bellek",
            omni_proto::now(),
        ));
        let ikinci = StateEvent::Notice(NoticeView::new(
            NoticeLevel::Error,
            "oom",
            "bellek",
            omni_proto::now() + chrono::Duration::seconds(90),
        ));
        let a = Notification::from_state_event(&ilk, &policy()).map(|n| n.signature());
        let b = Notification::from_state_event(&ikinci, &policy()).map(|n| n.signature());
        assert_eq!(a, b);
    }

    #[test]
    fn bastirilan_sayisi_metne_eklenir() {
        let event = StateEvent::Notice(NoticeView::new(
            NoticeLevel::Error,
            "oom",
            "bellek",
            omni_proto::now(),
        ));
        let bildirim = match Notification::from_state_event(&event, &policy()) {
            Some(n) => n,
            None => panic!("bildirim bekleniyordu"),
        };
        assert!(bildirim.plain_text(0).ends_with("bellek"));
        assert!(bildirim.plain_text(4).contains("+4 tekrar bastirildi"));
    }
}
