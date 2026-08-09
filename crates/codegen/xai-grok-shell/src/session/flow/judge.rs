//! Yargıç (judge) — verify aşamasının yanlışlamacı doğrulayıcısı.
//!
//! AI kendi işini kendisi onaylayamaz (üreten ≠ doğrulayan). Verify
//! aşamasında SİSTEM, `judge` persona'lı bir alt ajanı görevlendirir
//! (config/personas/judge.toml + prompts/judge.md — kod tabanında mevcut);
//! alt ajan kanıt dosyalarını okur, çıktıyı yanlışlamaya çalışır ve puan verir.
//! Bu modül yalnızca karar yapısını + prompt kurulumunu taşır; alt ajanın
//! görevlendirilmesi ve beklenmesi S6 kancasında (session tarafı) yapılır.

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct JudgeVerdict {
    /// Judge alt ajanın ürettiği ham çıktı.
    pub raw_output: String,
    /// Eşik altı / yanlışlama başarılı → false (aşama reddedilir).
    pub accepted: bool,
    /// Karar gerekçesi (modele verilecek düzeltici direktifin çekirdeği).
    pub reason: String,
}

/// Judge prompt'u kurar: görev özeti + kanıt dosyaları + rubric talimatı.
pub fn build_judge_prompt(work_summary: &str, evidence_files: &[String]) -> String {
    let files = if evidence_files.is_empty() {
        "(kanıt dosyası verilmedi)".to_string()
    } else {
        evidence_files.join("\n")
    };
    format!(
        "Sen yanlışlamacı yargıçsın (Brainstorm). Aşağıdaki görevin sonucunu \
         KANITA DAYALI olarak doğrula. Her iddiayı bir dosya/komut çıktısına \
         bağlamak zorundasın; kanıtsız kabul YOKTUR. Rubric (4 eksen, 25'er \
         puan): Grounding / Plan uyumu / Format-sema / Güven. 70 puanın \
         altı = REDDET. Önce kısa puan dökümü, sonra tek satır karar: \
         KABUL veya RED + gerekçe.\n\n\
         == GÖREV ÖZETİ ==\n{work_summary}\n\n\
         == KANIT DOSYALARI ==\n{files}\n\n\
         Salt-okunur araçlarla kanıtları incele; hiçbir yazma/spawn \
         yapma. Kararını ver."
    )
}

/// Ham judge çıktısından karar çıkarır (deterministik sözcük matcher).
/// "KABUL"/"kabul" içeriyor ve "RED" içermiyor → kabul; aksi halde red.
/// Çıktı boşsa → red (kanıtsız kabul yoktur).
pub fn parse_verdict(raw_output: &str) -> JudgeVerdict {
    let upper = raw_output.to_uppercase();
    let accepted = !raw_output.trim().is_empty()
        && upper.contains("KABUL")
        && !upper.contains("RED");
    let reason = if accepted {
        "Yargıç onayladı (kanıt zorunluluğu karşılandı).".to_string()
    } else {
        // Red gerekçesini çıkar: "RED" sonrası metin. Byte indeksi char
        // sınırına taşabilir (Türkçe çok baytlı karakterler) — bu yüzden
        // char-tabanlı dilimleme kullanılır; asla panic etmez (I6).
        let after = upper
            .find("RED")
            .map(|i| {
                let n = upper[..i].chars().count();
                raw_output.chars().skip(n).collect::<String>()
            })
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "yargıç reddetti; gerekçe çıktıda yok".to_string());
        after.chars().take(400).collect()
    };
    JudgeVerdict { raw_output: raw_output.to_string(), accepted, reason }
}

/// Task aracı (`run_in_background: false`) sonucundan alt ajanın çıktı
/// metnini çıkarır. TaskOutput değilse `prompt_text` döner (modelin
/// göreceği metin — yargıç için yeterli).
pub fn extract_task_output_text(
    run: &xai_grok_tools::types::output::ToolRunResult,
) -> String {
    use xai_grok_tools::types::output::ToolOutput;
    use xai_tool_types::TaskOutputOutput;
    match &run.output {
        ToolOutput::TaskOutput(TaskOutputOutput::Result(r)) => r.output.clone(),
        ToolOutput::TaskOutput(TaskOutputOutput::MultiResult(m)) => m.summary.clone(),
        _ => run.prompt_text.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accept_parsed() {
        let v = parse_verdict("Grounding 22, plan 20, format 21, güven 20 → 83. KABUL: kanıtlar yeterli.");
        assert!(v.accepted);
    }

    #[test]
    fn reject_when_no_accept_keyword() {
        let v = parse_verdict("Grounding 5. Kanıt yok.");
        assert!(!v.accepted);
    }

    #[test]
    fn reject_explicit_red() {
        let v = parse_verdict("KABUL görünümü var ama RED: dosya yok.");
        assert!(!v.accepted);
    }

    #[test]
    fn empty_output_rejected() {
        let v = parse_verdict("");
        assert!(!v.accepted);
    }
}
