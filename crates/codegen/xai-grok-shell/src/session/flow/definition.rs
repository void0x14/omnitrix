//! Akış şablonları: aşamalar, araç grupları, üretilen kanıtlar, yönergeler.

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ToolGroup {
    Read, Search, Write, Bash, Web, Research, Computer, Plan, Task, Meta, All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ArtifactId {
    ProblemList, FindingsArchive, DigestNote, StackChoice, StackVerified,
    DurationDecision, PlanDoc, BuildingBlocks, ExecutionGraph, WorkDone, Verified, Notified,
    ChangesSeen, Staged, Committed, CommittedVerified, Done,
}

impl ArtifactId {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ProblemList => "problem_list",
            Self::FindingsArchive => "findings_archive",
            Self::DigestNote => "digest_note",
            Self::StackChoice => "stack_choice",
            Self::StackVerified => "stack_verified",
            Self::DurationDecision => "duration_decision",
            Self::PlanDoc => "plan_doc",
            Self::BuildingBlocks => "building_blocks",
            Self::ExecutionGraph => "execution_graph",
            Self::WorkDone => "work_done",
            Self::Verified => "verified",
            Self::Notified => "notified",
            Self::ChangesSeen => "changes_seen",
            Self::Staged => "staged",
            Self::Committed => "committed",
            Self::CommittedVerified => "committed_verified",
            Self::Done => "done",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum StageId {
    Analyze, Research, Digest, StackSelect, StackVerify, Duration, Plan,
    Decompose, ParallelQuery, Execute, Verify, Notify,
    Status, Stage, Commit, CommitVerify, Do,
}

impl StageId {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Analyze => "analyze",
            Self::Research => "research",
            Self::Digest => "digest",
            Self::StackSelect => "stack_select",
            Self::StackVerify => "stack_verify",
            Self::Duration => "duration",
            Self::Plan => "plan",
            Self::Decompose => "decompose",
            Self::ParallelQuery => "parallel_query",
            Self::Execute => "execute",
            Self::Verify => "verify",
            Self::Notify => "notify",
            Self::Status => "status",
            Self::Stage => "stage",
            Self::Commit => "commit",
            Self::CommitVerify => "commit_verify",
            Self::Do => "do",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct StageDefinition {
    pub id: StageId,
    pub tool_groups: &'static [ToolGroup],
    pub produces: &'static [ArtifactId],
    pub directive: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub struct FlowDefinition {
    pub name: &'static str,
    pub stages: &'static [StageDefinition],
}

pub fn find_flow(name: &str) -> Option<&'static FlowDefinition> {
    default_flows().iter().find(|f| f.name == name)
}

/// Gömülü varsayılan akışlar. `config/flow/flows.toml` override'ı (Task 3)
/// bu fonksiyonun çıktısının üzerine biner.
pub fn default_flows() -> &'static [FlowDefinition] {
    const UNIVERSAL: &[StageDefinition] = &[
        StageDefinition {
            id: StageId::Analyze,
            tool_groups: &[ToolGroup::Read, ToolGroup::Search, ToolGroup::Meta],
            produces: &[ArtifactId::ProblemList],
            directive: "ADIM 1/12 — ANALİZ: Problemi kelime kelime oku, küçük parçalara ayır ve ne anlattığını anla. Yalnızca okuma/arama araçlarını kullanabilirsin; henüz hiçbir şeye yazamaz, web'e giremez, kod değiştiremezsin. Anladıklarını zihinsel bir listeye dök ve işin bitince flow_checkpoint ile 'analyze' aşamasını kapat (dosyalar alanına boş liste ver; summary'ye parça listesini yaz).",
        },
        StageDefinition {
            id: StageId::Research,
            tool_groups: &[ToolGroup::Web, ToolGroup::Research, ToolGroup::Computer, ToolGroup::Search, ToolGroup::Meta],
            produces: &[ArtifactId::FindingsArchive],
            directive: "ADIM 2/12 — ARAŞTIRMA: Listeyi kullanarak problemi günümüz gerçeğiyle derin araştır (web/research araçları; yüzey → derin → okyanus modlarını kullan). Bulguları depola (bir bulgular dosyasına yaz). İşin bitince flow_checkpoint ile 'research' aşamasını kapat — dosyalar alanına yazdığın bulgular dosyasının yolunu ver (çalışma dizinine göreli).",
        },
        StageDefinition {
            id: StageId::Digest,
            tool_groups: &[ToolGroup::Read, ToolGroup::Search, ToolGroup::Meta],
            produces: &[ArtifactId::DigestNote],
            directive: "ADIM 3/12 — SİNDİRİM: Kaydettiğin araştırma sonuçlarını oku; çok büyükse parçalara bölüp tane tane işle. Özünü çıkar. Bitince flow_checkpoint ile 'digest' aşamasını kapat.",
        },
        StageDefinition {
            id: StageId::StackSelect,
            tool_groups: &[ToolGroup::Read, ToolGroup::Web, ToolGroup::Search, ToolGroup::Meta],
            produces: &[ArtifactId::StackChoice],
            directive: "ADIM 4/12 — STACK SEÇİMİ: Çözüm için uygun yığınları (stack) sen belirle ve gerekçesiyle kaydet. Bitince flow_checkpoint ile 'stack_select' aşamasını kapat.",
        },
        StageDefinition {
            id: StageId::StackVerify,
            tool_groups: &[ToolGroup::Read, ToolGroup::Web, ToolGroup::Search, ToolGroup::Meta],
            produces: &[ArtifactId::StackVerified],
            directive: "ADIM 5/12 — STACK DOĞRULAMA: Sanki hiç seçim yapmamış gibi internetin neyi önerdiğini araştır, kendi önerinle karşılaştır; çürütme yoluyla en doğru stacke ulaşana dek tekrarla. Nihai stacke karar verince flow_checkpoint ile 'stack_verify' aşamasını kapat.",
        },
        StageDefinition {
            id: StageId::Duration,
            tool_groups: &[ToolGroup::Meta],
            produces: &[ArtifactId::DurationDecision],
            directive: "ADIM 6/12 — SÜRE KARARI: Bu aşama sistemindir; araç kullanmana gerek yok. flow_checkpoint ile 'duration' aşamasını kapat (summary'ye 'sistem kararı bekleniyor' yaz).",
        },
        StageDefinition {
            id: StageId::Plan,
            tool_groups: &[ToolGroup::Read, ToolGroup::Plan, ToolGroup::Meta],
            produces: &[ArtifactId::PlanDoc],
            directive: "ADIM 7/12 — PLAN: Önceki aşamaların çıktılarına dayanarak plan çıkar; plan dosyasına yaz. Bitince flow_checkpoint ile 'plan' aşamasını kapat (plan dosyasının yolunu ver).",
        },
        StageDefinition {
            id: StageId::Decompose,
            tool_groups: &[ToolGroup::Read, ToolGroup::Plan, ToolGroup::Write, ToolGroup::Meta],
            produces: &[ArtifactId::BuildingBlocks],
            directive: "ADIM 8/12 — YAPI TAŞLARI: Planı küçük yapı taşlarına böl; her taş için dosya yollarıyla birlikte building_blocks dosyasına yaz. Bitince flow_checkpoint ile 'decompose' aşamasını kapat (dosya yolunu ver).",
        },
        StageDefinition {
            id: StageId::ParallelQuery,
            tool_groups: &[ToolGroup::Meta],
            produces: &[ArtifactId::ExecutionGraph],
            directive: "ADIM 9/12 — PARALELİZM: Sistem yapı taşlarının hangilerinin paralel, hangilerinin sıralı olduğunu belirler; araç gerekmez. flow_checkpoint ile 'parallel_query' aşamasını kapat.",
        },
        StageDefinition {
            id: StageId::Execute,
            tool_groups: &[ToolGroup::All],
            produces: &[ArtifactId::WorkDone],
            directive: "ADIM 10/12 — UYGULAMA: Yapı taşlarını uygula. Paralel taşlar için task araçlarıyla alt ajan görevlendirebilirsin (süre kararı 'mvp' ise alt ajan sayısını 2 ile sınırla). Tüm taşlar bitince flow_checkpoint ile 'execute' aşamasını kapat.",
        },
        StageDefinition {
            id: StageId::Verify,
            tool_groups: &[ToolGroup::Read, ToolGroup::Bash, ToolGroup::Task, ToolGroup::Meta],
            produces: &[ArtifactId::Verified],
            directive: "ADIM 11/12 — DOĞRULAMA: Sonucu doğrula (test/çalıştır/incele; yazma yok). Bitince flow_checkpoint ile 'verify' aşamasını kapat.",
        },
        StageDefinition {
            id: StageId::Notify,
            tool_groups: &[ToolGroup::Meta],
            produces: &[ArtifactId::Notified],
            directive: "ADIM 12/12 — BİLDİRİM: Sistem kullanıcıya bildirim gönderir. flow_checkpoint ile 'notify' aşamasını kapatarak görevi bitir.",
        },
    ];
    const COMMIT: &[StageDefinition] = &[
        StageDefinition {
            id: StageId::Status,
            tool_groups: &[ToolGroup::Read, ToolGroup::Bash, ToolGroup::Meta],
            produces: &[ArtifactId::ChangesSeen],
            directive: "COMMIT ADIMI 1/3 — DURUM: git status/diff/log ile değişiklikleri incele (salt okunur; bu aşamada hiçbir yazma komutu yasaktır). Değişiklikleri gördüğünü flow_checkpoint ile onayla.",
        },
        StageDefinition {
            id: StageId::Stage,
            tool_groups: &[ToolGroup::Bash, ToolGroup::Meta],
            produces: &[ArtifactId::Staged],
            directive: "COMMIT ADIMI 2/3 — STAGE: Yalnızca tracked ve modified dosyaları stage et (git add <yol>). --force/--hard/reset/push/rebase KESİNLİKLE YASAK. Bitince flow_checkpoint ile onayla.",
        },
        StageDefinition {
            id: StageId::Commit,
            tool_groups: &[ToolGroup::Bash, ToolGroup::Meta],
            produces: &[ArtifactId::Committed],
            directive: "COMMIT ADIMI 3/3 — COMMIT: git commit -m '<mesaj>' ile commit at (geçmişe saygı: force/hard yok). Bitince flow_checkpoint ile onayla.",
        },
        StageDefinition {
            id: StageId::CommitVerify,
            tool_groups: &[ToolGroup::Read, ToolGroup::Bash, ToolGroup::Meta],
            produces: &[ArtifactId::CommittedVerified],
            directive: "DOĞRULAMA: git log/show ile commit'i doğrula (salt okunur). flow_checkpoint ile kapat.",
        },
    ];
    const DIRECT: &[StageDefinition] = &[
        StageDefinition {
            id: StageId::Do,
            tool_groups: &[ToolGroup::All],
            produces: &[ArtifactId::Done],
            directive: "GÖREV: İstediğini yap. Yalnızca gerçekten bittiğinde flow_checkpoint ile 'do' aşamasını kapat.",
        },
    ];
    const FLOWS: &[FlowDefinition] = &[
        FlowDefinition { name: "universal", stages: UNIVERSAL },
        FlowDefinition { name: "commit", stages: COMMIT },
        FlowDefinition { name: "direct", stages: DIRECT },
    ];
    FLOWS
}
