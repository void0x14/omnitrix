//! HTML form → `omni_proto::Command` cozumu (Bolum 6.2 yazma ucu).
//!
//! Bu modul **komut tipi tanimlamaz**; yalnizca `application/x-www-form-urlencoded`
//! govdesini kanonik [`Command`] varyantina cevirir (I3). Boylece JS kapaliyken
//! de yazma yolu calisir: klasik form POST → `omni-control` → cekirdek → sonuc
//! `StateEvent` olarak her iki yuze yayilir.
//!
//! Tum alanlar `Option<String>` olarak alinir: HTML formu doldurulmayan alani
//! bos dize olarak gonderir, sayisal tipe dogrudan cozulemez.

use omni_control::ControlError;
use omni_proto::Command;
use serde::Deserialize;

/// Komutun geldigi yuzun kanonik adi (`interrupts.source`).
pub const SOURCE: &str = "webui";

/// Form govdesinin duz gosterimi.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct CommandForm {
    /// Hangi komut: `Command::kind()` degerleriyle ayni.
    pub cmd: Option<String>,
    /// `SpawnTask.title`.
    pub title: Option<String>,
    /// `SpawnTask.mode`.
    pub mode: Option<String>,
    /// `SpawnTask.parent_id`.
    pub parent_id: Option<String>,
    /// `SpawnTask.persona`.
    pub persona: Option<String>,
    /// `SpawnTask.duration_target`.
    pub duration_target: Option<String>,
    /// `SpawnTask.budget`.
    pub budget: Option<String>,
    /// `WriteToAgent`/`Interrupt` hedefi.
    pub agent_id: Option<String>,
    /// `WriteToAgent.content`.
    pub content: Option<String>,
    /// `Interrupt.kind`.
    pub kind: Option<String>,
    /// `Interrupt.reason`.
    pub reason: Option<String>,
}

/// Bos dizeyi `None`'a indirger.
fn text(value: &Option<String>) -> Option<&str> {
    value.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

/// Zorunlu metin alanini okur.
fn required<'a>(value: &'a Option<String>, field: &str) -> Result<&'a str, ControlError> {
    text(value).ok_or_else(|| ControlError::BadRequest(format!("'{field}' alani zorunlu")))
}

/// Opsiyonel sayisal alani cozer.
fn number<T>(value: &Option<String>, field: &str) -> Result<Option<T>, ControlError>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    match text(value) {
        None => Ok(None),
        Some(raw) => raw
            .parse::<T>()
            .map(Some)
            .map_err(|err| ControlError::BadRequest(format!("'{field}' sayisal degil: {err}"))),
    }
}

impl CommandForm {
    /// Formu kanonik komuta cevirir.
    ///
    /// # Errors
    /// Zorunlu alan eksikse, sayisal alan cozulemezse ya da `cmd` taninmiyorsa
    /// [`ControlError::BadRequest`] doner.
    pub fn into_command(self) -> Result<Command, ControlError> {
        let cmd = required(&self.cmd, "cmd")?;
        match cmd {
            "spawn_task" => Ok(Command::SpawnTask {
                title: required(&self.title, "title")?.to_string(),
                mode: required(&self.mode, "mode")?.to_string(),
                parent_id: number(&self.parent_id, "parent_id")?,
                persona: text(&self.persona).map(str::to_string),
                duration_target: text(&self.duration_target).map(str::to_string),
                budget: number(&self.budget, "budget")?,
            }),
            "write_to_agent" => Ok(Command::WriteToAgent {
                agent_id: number(&self.agent_id, "agent_id")?.ok_or_else(|| {
                    ControlError::BadRequest("'agent_id' alani zorunlu".to_string())
                })?,
                content: required(&self.content, "content")?.to_string(),
            }),
            "interrupt" => Ok(Command::Interrupt {
                agent_id: number(&self.agent_id, "agent_id")?.ok_or_else(|| {
                    ControlError::BadRequest("'agent_id' alani zorunlu".to_string())
                })?,
                kind: required(&self.kind, "kind")?.to_string(),
                source: SOURCE.to_string(),
                reason: text(&self.reason).map(str::to_string),
            }),
            other => Err(ControlError::BadRequest(format!(
                "bilinmeyen komut: {other}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form(cmd: &str) -> CommandForm {
        CommandForm {
            cmd: Some(cmd.to_string()),
            ..CommandForm::default()
        }
    }

    #[test]
    fn gorev_acma_cozulur() {
        let mut f = form("spawn_task");
        f.title = Some("dikey dilim".into());
        f.mode = Some("user_driven".into());
        f.budget = Some("2.5".into());
        // Bos dize doldurulmamis alandir, hata degil.
        f.persona = Some("  ".into());
        f.duration_target = Some(String::new());
        let command = f.into_command().expect("cozulmeli");
        assert_eq!(command.kind(), "spawn_task");
        match command {
            Command::SpawnTask {
                budget, persona, ..
            } => {
                assert_eq!(budget, Some(2.5));
                assert_eq!(persona, None);
            }
            _ => unreachable!("spawn_task bekleniyordu"),
        }
    }

    #[test]
    fn eksik_zorunlu_alan_bad_request() {
        let err = form("spawn_task")
            .into_command()
            .expect_err("hata bekleniyor");
        assert_eq!(err.code(), "bad_request");
    }

    #[test]
    fn bozuk_sayi_bad_request() {
        let mut f = form("spawn_task");
        f.title = Some("x".into());
        f.mode = Some("m".into());
        f.budget = Some("elli".into());
        let err = f.into_command().expect_err("hata bekleniyor");
        assert!(err.to_string().contains("budget"));
    }

    #[test]
    fn ajana_yazma_cozulur() {
        let mut f = form("write_to_agent");
        f.agent_id = Some("11".into());
        f.content = Some("devam et".into());
        let command = f.into_command().expect("cozulmeli");
        assert_eq!(command.target_agent(), Some(11));
    }

    #[test]
    fn mudahale_kaynagi_webui() {
        let mut f = form("interrupt");
        f.agent_id = Some("3".into());
        f.kind = Some("pause".into());
        let command = f.into_command().expect("cozulmeli");
        match command {
            Command::Interrupt { source, reason, .. } => {
                assert_eq!(source, SOURCE);
                assert_eq!(reason, None);
            }
            _ => unreachable!("interrupt bekleniyordu"),
        }
    }

    #[test]
    fn bilinmeyen_komut_reddedilir() {
        let err = form("format_disk")
            .into_command()
            .expect_err("hata bekleniyor");
        assert!(err.to_string().contains("bilinmeyen komut"));
    }

    #[test]
    fn ajan_kimligi_zorunlu() {
        let mut f = form("write_to_agent");
        f.content = Some("x".into());
        let err = f.into_command().expect_err("hata bekleniyor");
        assert!(err.to_string().contains("agent_id"));
    }
}
