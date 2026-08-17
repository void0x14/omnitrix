# Benchmark ve Akademi — Beyaz Literatür Takviyesi

> **Tarih:** 2026-08-14 · **Kapsam:** Terminal-Bench/SWE-bench CLI harness sıralamaları (2025-2026), context engineering akademik literatürü, agent memory/learning çalışmaları.
> **Yöntem:** tbench.ai leaderboard scrape (tam 142 kayıt), swebench.com resmi leaderboard, Artificial Analysis Coding Agent Index, arXiv/Semantic Scholar taramaları (paper-search-mcp), web aramaları (firecrawl/linkup).
> **Uyarı:** Satıcı-özsundurma (self-report) skorlar ile bağımsız harness skorları arasında fark vardır; her rakamın yanında kaynak ve tarih verildi. Skorlar hızla eskir — tarih etiketine dikkat.

---

## 1. Terminal-Bench — CLI harness sıralamaları

### 1.1 Terminal-Bench 2.0 resmi leaderboard (tbench.ai, 142 kayıt, 2025-10 → 2026-05)

Kaynak: https://www.tbench.ai/leaderboard/terminal-bench/2.0 (scrape: 2026-08-14). 89 elle denetlenmiş terminal görevi, `terminal-bench@2.0` sürümü; Terminal-Bench ekibi tarafından doğrulanmış sonuçlar. Sıralama ham Accuracy'dir.

| Sıra | Ajan | Model | Tarih | Accuracy |
|---|---|---|---|---|
| 1 | NexAU-AHE | GPT-5.5 | 2026-05-14 | 84.7% ± 2.1 |
| 2 | LemonHarness | Multiple | 2026-05-14 | 84.5% ± 2.6 |
| 3 | Capy | GPT-5.5 | 2026-05-14 | 83.1% ± 2.1 |
| **4** | **Codex CLI** | **GPT-5.5** | **2026-04-23** | **82.2% ± 2.2** |
| 6 | WOZCODE | Claude Opus 4.7 | 2026-05-14 | 80.2% ± 2.1 |
| 7 | TongAgents | Gemini 3.1 Pro | 2026-03-13 | 80.2% ± 2.6 |
| 10 | Droid (Factory) | GPT-5.3-Codex | 2026-02-24 | 77.3% ± 2.2 |
| 22 | Junie CLI (JetBrains) | Multiple | 2026-03-07 | 71.0% ± 2.9 |
| 40 | Gemini CLI | Gemini 3.1 Pro | 2026-05-14 | 61.4% ± 4.1 |
| 41 | Warp | Multiple | 2025-12-12 | 61.2% ± 3.0 |
| 46 | Letta Code | Claude Opus 4.5 | 2025-12-17 | 59.1% ± 2.4 |
| **50** | **Claude Code** | **Claude Opus 4.6** | **2026-02-07** | **58.0% ± 2.9** |
| 53 | Grok CLI (Superagent) | Grok 4.20 Reasoning | 2026-04-02 | 57.3% ± N/A |
| 56 | Goose (Block) | Claude Opus 4.5 | 2025-12-11 | 54.3% ± 2.6 |
| 61 | Claude Code | Claude Opus 4.5 | 2025-12-18 | 52.1% ± 2.5 |
| 62 | OpenHands | Claude Opus 4.5 | 2026-01-04 | 51.9% ± 2.9 |
| **64** | **OpenCode** | **Claude Opus 4.5** | **2026-01-12** | **51.7% ± N/A** |
| 82 | Mini-SWE-Agent (Princeton) | Claude Sonnet 4.5 | 2025-11-03 | 42.5% ± 2.8 |
| 85 | Claude Code | Claude Sonnet 4.5 | 2025-11-04 | 40.1% ± 2.9 |
| 114 | Mini-SWE-Agent | Grok 4 | 2025-11-03 | 25.4% ± 2.9 |
| 124 | Gemini CLI | Gemini 2.5 Pro | 2025-11-04 | 19.6% ± 2.9 |

**Okuma:** TB 2.0'da isimli CLI ajanların en iyisi **Codex CLI + GPT-5.5 = 82.2%** (sıra 4). Tahta lideri üçüncü-parti harness'ler (NexAU-AHE 84.7%). Claude Code'un kendi skoru (Opus 4.6, %58.0) modelin harness içindeki potansiyelinden (örn. vix/Opus 4.7, %90.2 — aşağıda) çok düşüktür; ajan+model çifti değil, harness kalitesi skoru belirler. Aider ve Crush bu leaderboard'da yoktur; aider kendi polyglot leaderboard'ında %88.0 (gpt-5 high) bildirir [kaynak: morphllm.com/ai-coding-agent].

### 1.2 Terminal-Bench 2.1 (Terminus 2 harness, bağımsız) ve yeni model çiftleri

Kaynak: https://www.morphllm.com/ai-coding-agent (2026-08 itibarı) + ssojet.com/blog/best-cli-coding-agents-ranked (doğrulama: 2026-06-07). Artificial Analysis'ın Terminus 2 harness'ı üzerinde 89 görevlik TB 2.1:

- **GPT-5.6 Sol (xhigh): 89.5%** — Codex CLI'ın varsayılan modeli (GPT-5.6 GA: 2026-07-09)
- Claude Opus 5 (max effort): 89.1% — Claude Code varsayılanı (Opus 5: 2026-07-24)
- GPT-5.6 Terra (max): 88.0%
- Claude Code + Fable 5: 83.8% (satıcı gönderimi, tbench.ai)
- Codex CLI + GPT-5.5: 83.1%
- Claude Code + Opus 4.8: 78.9%

Ayrıca: TB 2.0 tahtasının 2026-05-15'teki lideri "vix" harness (Claude Opus 4.7) **90.2%** [kaynak: ssojet.com, doğrulama 2026-06-07].

### 1.3 Artificial Analysis Coding Agent Index — süre ve maliyet

Kaynak: https://artificialanalysis.ai/agents/coding-agents + benchlm.ai/blog/posts/ai-coding-agents (2026-07-14 okuması):

| Ajan | Model | Index | Ort. süre/görev | Ort. maliyet/görev |
|---|---|---|---|---|
| Codex | GPT-5.6 Sol (xhigh) | **80** (78.7) | 7.4 dk | raporlanmadı |
| Claude Code | Fable 5 (max) | 77 (Opus 4.6 med. 71.1) | 8.0 dk | $1.26 |
| Gemini CLI | — | 43 | — | — |
| Opencode | Opus 4.7 (medium) | 64.5 | 12.2 dk | $2.93 |

**Harness karşılaştırması (aynı model, Opus 4.7):** Opencode **65** > Cursor CLI **60** > Claude Code **57** — harness kalitesi tek başına ~8 puan oynatıyor [kaynak: benchlm.ai, 2026-07-14]. Token kullanımı/görev 1.1M–12.2M aralığında; Opus 5 (max) 12.2M token ile en yüksek [kaynak: artificialanalysis.ai].

---

## 2. SWE-bench Verified — CLI/ajan sonuçları

### 2.1 Resmi leaderboard (swebench.com, 2026-08 okuması)

| # | Model | Ajan | % Resolved | Ort. $ | Tarih |
|---|---|---|---|---|---|
| 1 | Claude 4.5 Opus (med.) | live-SWE-agent | **79.20** | — | 2025-12-15 |
| 2 | Claude 4.5 Opus | Sonar Foundation Agent | 79.20 | — | 2025-12-05 |
| — | Claude 4.5 Opus (high) | mini-SWE-agent v2.0.0 | 76.80 | $0.75 | 2026-02-17 |
| — | Claude 4.6 Opus | mini-SWE-agent v2.0.0 | 75.60 | $0.55 | 2026-02-17 |
| — | GPT 5.2 Codex | mini-SWE-agent v2.0.0 | 72.80 | $0.45 | 2026-02-19 |
| — | Claude 4.5 Sonnet (high) | mini-SWE-agent v2.0.0 | 71.40 | $0.66 | 2026-02-17 |
| — | Claude 4 Sonnet | mini-SWE-agent v1.0.0 | 64.93 | $0.37 | 2025-07-26 |
| — | GLM 4.6 | mini-SWE-agent v1.17.1 | 55.40 | $0.10 | 2025-12-01 |

Maliyet eğrisi: en iyi skorlar ~$0.55-0.75/görev; GLM 4.6 $0.10 ile en ucuz. [kaynak: swebench.com]

### 2.2 Bağımsız CLI harness ölçümleri (aynı/kıyas modeller)

- **TraceProbe** (Shu et al., arXiv 2026-07-07; 2500 trajektori, 5 üretim kurulumu): Claude Code + Opus 4.6 **77.4%** (medyan 27 adım, kalıcı edit oranı %27.6), Codex + GPT-5.4 **78.4%** (24 adım, %36.2), OpenCode-GLM **71.6%**, OpenCode-Opus **71.4%**, OpenCode-GPT **64.2%**. Search-loop anti-paterni: resolved görevlerin %41.1'inde, failed görevlerin %56.1'inde. [kaynak: codex.danielvaughan.com/2026/07/08 + arXiv]
- **andrew.ooo (Nisan 2026):** OpenCode+Opus 4.7 **73.2% @ $0.40**, Claude Code **74.6% @ $0.55**, Codex+GPT-5.4 **71.8% @ $0.48**; Terminal-Bench: 67/69/64.
- **NxCode (2026):** Claude Code + Opus 4.6 **80.9%** (bir ajanın en yüksek kayıtlı skoru olduğu iddiası), Codex + GPT-5.4 ~%80; Token verimliliği: Codex ~4x daha az token.
- **Sanj (2026):** Claude Code + Opus 4.8 **88.6%** SWE-V; Codex + GPT-5.5 **82.1%**; TB 2.1'de tersine Codex **83.4%** > CC **78.9%** — "SWE-V'de Claude, TB'de Codex lider" ayrışması.
- **vexp-swe-bench** (100 görevlik açık alt küme): vexp destekli Claude Code **73% @ $0.67/görev** — test edilenler arasında en iyi maliyet-etkinlik [kaynak: github.com/Vexp-ai/vexp-swe-bench].

### 2.3 Yeni nesil benchmarklar (2026)

- **SWE-bench Pro** (2000+ kontaminasyonsuz problem): Fable 5 **80.3%**, Opus 4.8 **69.2%**, GPT-5.5 **58.6%**, Gemini 3.1 Pro **54.2%**; GPT-5.4-Codex **56.8%** [kaynak: llm-stats.com/benchmarks/swe-bench-pro; thoughts.jock.pl].
- **SWE-Chain** (Lam et al., 2026-05; ardışık sürüm yükseltme zincirleri): ort. çözüm %44.8; CC+Opus 4.7 **60.8%** (F1 68.5), Codex+GPT-5.5 **57.5%**, Codex+GPT-5.4 **47.5%** vs aynı modelde OpenCode **43.4%** (harness farkı +4.1pp resolving, +8.4pp precision), OpenCode+GLM-5.1 **38.1%**, OpenCode+MiniMax-M2.7-HS **20.2%**. RoadmapBench: CC+Opus 4.7 sadece **39.1%**.
- **Aider polyglot:** gpt-5 (high) %88.0 — aider'in kendi leaderboard'ı [kaynak: morphllm.com].

**Özet okuma:** CLI ajanlar arasında SWE-V'de liderlik ~%74-81 aralığında CC/Codex düellosu; OpenCode aynı modelle ~2-4pp geride ama maliyette avantajlı ($0.40 vs $0.55). Tüm kaynaklar ortak: **harness farkı model farkından büyük** (aynı Opus 4.7: 65/60/57).

---

## 3. Context Engineering — akademik çerçeve (2025-2026)

### 3.1 Teorik temel: entropi-azaltma / minimal-yeterlilik

- **The Root Theorem of Context Engineering** (Odriozola Schick, arXiv 2604.20874, 2026-03-29): tek yönetim ilkesi — *"bounded, lossy kanallarda sinyal-token oranını maksimize et"*. Türevler: (1) kalite fonksiyonu enjekte edilen token hacmiyle monoton azalır (pencere boyutundan bağımsız); (2) sinyal ve token sayısı bağımsız optimizasyon değişkenleridir; (3) gating mekanizması **kapasite sınırına değil, doğruluk (fidelity) eşiğine** bağlı olmalı; (4) **homeostatik kalıcılık** (accumulate → compress → rewrite → shed) tek sürdürülebilir mimari; (5) sıkıştırıcı sıkıştırdığı kanalın içinde çalıştığı için harici doğrulama kapısı zorunlu. Append-only sistemler sonlu zamanda etkin pencereyi aşar; 60+ oturumluk kurulumda stabil bellek ayak izi ile kanıtlanmış. [kaynak: arXiv 2604.20874]
- **Context Engineering 2.0: The Context of Context Engineering** (Hua, Ye, Fu, ..., Liu, arXiv 2510.26493, 2025-10-30): context engineering'in sistematik tanımı + tarihsel aşamaları (1990'lardan HCI'ye uzanır); insan-ajan etkileşimi paradigmasında tasarım boyutları.
- **Contexting as Recommendation — NCCE** (Zhu et al., arXiv 2605.15721, 2026-05-15): context engineering'i global arama yerine **instance-wise yönlendirme (routing)** problemi olarak formüle eder; NCF tabanlı context router dinamik context stratejisi atar, doğruluğu anlamlı artırır.
- **Implicit Fine-tuning via Context Engineering — PTFEA** (Hong et al., arXiv 2607.10532, 2026-07-12): context engineering ≈ fine-tuning matematiksel eşdeğerliği; Qwen2.5-72B'de token tüketimi 2200-3000 → 200-400 (>%80 azalma), 21 saat → 1 saat runtime; H@1 farkı 72B/14B arasında %0.6'ya daralır.

### 3.2 Compaction / bağlam yönetimi — ampirik bulgular

- **The Complexity Trap** (Lindenbauer et al., arXiv 2508.21433v3, 2025-08): SWE-agent + SWE-bench Verified, 5 model konfigürasyonu. **Basit observation masking (eski gözlemleri atma), ham ajana göre maliyeti YARIYA indirir ve LLM-summarization'ın çözüm oranıyla eşitlenir/bazen geçer.** Hibrit (masking+summarization) tek başına masking'den −%7, summarization'dan −%11 ek tasarruf. Sonuç: saf LLM-summarization trendi sorgulanır; OpenHands scaffold'una genelleme kanıtı var. [kaynak: arXiv 2508.21433]
- **ACON — Agent Context Optimization** (Microsoft, arXiv 2510.00615, 2025-10): doğal dil uzayında sıkıştırma kılavuzlarının hata-analiziyle iteratif iyileştirilmesi; tepe token kullanımı **%26-54 azalır**, başarı artar; distile küçük modellerde context distraction azalmasıyla **%46'ya varan** iyileşme. [kaynak: arXiv 2510.00615]
- **Context-Folding** (Sun et al., arXiv 2510.11967, 2025-10): ajan alt-görevi prosedürel "fold" eder (ara adımlar silinir, özet kalır); FoldGRPO ile öğrenilebilir. Deep Research ve SWE'de ReAct ile eşit/üstün performans, **10x daha küçük aktif context**; summarization tabanlı yönetimi açar. [kaynak: arXiv 2510.11967]
- **SUPO** (Lu et al., arXiv 2510.06727, 2025-10): summarization tabanlı context yönetimini RL'ye entegre eder; sabit pencere ötesinde uzun-horizon eğitimi; test-time'da ekstra özetleme turu ile ölçeklenir. [kaynak: arXiv 2510.06727]
- **LongSeeker / Context-ReAct** (Lu et al., arXiv 2605.05191, 2026-05): 5 atomik operasyon — **Skip, Compress, Rollback, Snippet, Delete**; Compress operatörünün ifade gücü tamlık (expressive completeness) ile ispatlanır; diğerleri verimlilik/doğruluk garantisi verir. BrowseComp **%61.5** vs Tongyi DeepResearch %43.2, AgentFold %36.2. [kaynak: arXiv 2605.05191]
- **CWL — Context Window Lifecycle** (Semenov & Dorofeev, arXiv 2606.11213, 2026-05): tip-bağımlı episode grafiği + deterministik, LLM'siz öncelikli eviction; **tek oturumda 89 ardışık görev, 80M token, izole oturumlara kıyasla ölçülebilir degradasyon yok**. Summarization-compaction'ın 4 limitasyonunu (öngörülemez kayıp, nedensel yapı tahribi, bloklayıcı maliyet, sıkıştırma-halüsinasyonu) listeler. [kaynak: arXiv 2606.11213]
- **ARC — Addressable Recall Compaction** (Dang et al., arXiv 2607.25066, 2026-07): arşiv depo + ID-adresli alıntılar; Qwen3-8B/32B; Needle-in-a-Haystack **%99.40 vs en iyi baseline %88.12**; LongBench-v2 Hard **%29.97 vs %28.25**; tahmini serving süresi ve HBM trafiği düşer. [kaynak: arXiv 2607.25066]
- **Parallel Context Compaction** (Cim et al., arXiv 2605.23296, 2026-05): senkron compaction çağrısı ajan inference'ını **onlarca saniye stall eder**; prompt hacmi talimatla kontrol edilemez ve run-to-run tutarsızdır; paralel compaction eşit decode hacminde uçtan uca duvar süresini düşürür (8B–120B, 4 backbone, HotpotQA/LoCoMo). [kaynak: arXiv 2605.23296]
- **Governance Decay** (Chen, arXiv 2606.22528, 2026-06): **compaction güvenlik kısıtlarını sessizce siler** — ConstraintRot benchmark, 1323 episode, 7 model ailesi: ihlal tam bağlamda %0'dan compaction sonrası **%30'a** (bazı modellerde %59) çıkar; kısıt özette korunursa %0, düşerse %38. Constraint Pinning (kısıtları kayıplı katmandan ayırma) ihlali benchmark'ta %0'a döndürür. Compaction-Eviction attack ile özetleyici manipüle edilebilir. [kaynak: arXiv 2606.22528]
- **Four-Axis Decision Alignment** (Srininvasan, arXiv 2604.19457, 2026-04): 6 memory mimarisinde fact-preservation prompt'lu düz summarization, FRP/RCS/EDA/CRR eksenlerinde güçlü baseline çıkar; "summarization faktüel hatırlamayı kırpar" ön-kaydı veriyle terslenir (axis-level reversal). [kaynak: arXiv 2604.19457]
- **LongCoT** (Motwani et al., arXiv 2604.14140, 2026-04): 2500 problem; frontier modeller <**%10** (GPT 5.2: %9.8, Gemini 3 Pro: %6.1) — uzun-horizon düşünme hâlâ açık boşluk. [kaynak: arXiv 2604.14140]

### 3.3 Uygulama katmanı

- **Everything is Context** (Xu et al., arXiv 2512.05470, 2025-12): "her şey dosyadır" Unix soyutlaması ile context artefaktlarının (memory, tool, insan girdisi) mount/metadata/ACL altında yönetimi; AIGNE framework; Context Constructor/Loader/Evaluator zinciri.
- **AgenticRepair** (Fu et al., arXiv 2607.29422, 2026-07): çok yüzlü program context mühendisliği (kod-yapı + runtime + commit-geçmişi); SEC-Bench **%73 başarı, en güçlü baseline'dan +29pp**.
- **Invasive Context Engineering** (Rivasseau, arXiv 2512.03001, 2025-12): **jailbreak olasılığı context uzunluğuyla artar**; kontrol cümleleriyle eğitimsiz kontrol.
- **Context Engineering for Multi-Agent LLM Code Assistants** (Haseeb, arXiv 2508.08322, 2025-08): intent translator + RAG + NotebookLM + Claude Code multi-agent; hedefli context enjeksiyonu ve rol ayrıştırmanın tek-ajan baseline'dan üstün olduğu gösterimi.

---

## 4. Memory / Learning — ajan kendini geliştirme (2025-2026)

### 4.1 Survey'ler ve teorik çerçeve

- **Externalization in LLM Agents** (Zhou et al., arXiv 2604.08224, 2026-04): "weights → context → harness" tarihsel geçişi; memory = zaman üzerinden state, skills = prosedürel uzmanlık, protocols = etkileşim yapısı, **harness engineering = birleştirme katmanı**; self-evolving harnesses ve paylaşılan altyapı açık yönler.
- **A Survey of Agent Memory in the Second Half** (Huang et al., arXiv 2602.06052, 2026-01): 2025'te yüzlerce memory paper; üç eksen (substrate, cognitive mechanism, subject); **memory yönetimi öğrenilebilir yetenek** — RL-curated context, decision-time experience consolidation, taşınabilir skill ekosistemi.
- **Self-Improvements in Modern Agentic Systems: A Survey** (Ren, ..., Schmidhuber et al., arXiv 2607.13104, 2026-07): ajan = foundation model + scaffold (prompt, memory, tool, kontrol) konfigürasyonu; **self-improvement = kendi başlattığı update operatörü** (parametre veya scaffold hedefli); kontrollü evrim çerçevesi.
- **Agent Skills for LLMs: Architecture, Acquisition, Security** (Xu & Yan, arXiv 2602.12430, 2026-02): SKILL.md + progressive disclosure + MCP; **topluluk skill'lerinin %26.1'i güvenlik açığı içeriyor**; dört kademeli Skill Trust & Lifecycle Governance önerisi.

### 4.2 Skill edinimi / self-improvement yöntemleri

- **GEPA — Genetic-Pareto** (Stanford/UCB, Agrawal et al., arXiv 2507.19457, 2025-07): doğal dil refleksiyonuyla prompt evrimi; 6 görevde **GRPO'dan ort. %6, en fazla %20 üstün, 35x'e kadar daha az rollout**; MIPROv2'den %10+ üstün (+%12 AIME-2025); kod optimizasyonunda inference-time arama olarak da çalışır. Hermes'in self-evolution hattının atıf kaynağı; hermes RAPOR'una göre **ICLR 2026 Oral** kabulü, ~$2-10/optimizasyon maliyeti [kaynak: hermes/RAPOR.md:84-85].
- **Skill-α** (Shen et al., arXiv 2608.01678, 2026-08): RL ile progresif skill üretimi; rollback reward (edit'lerin downstream etkisini ölçer); GPT-4o işçisinde CL-Bench +3.3, tau2-bench +6.7 puan.
- **Skill-SD** (Wang et al., arXiv 2604.10674, 2026-04): tamamlanan trajektoriler özetlenip teacher-only koşul olur; GRPO üzerinde AppWorld +%14.0, Sokoban +%10.9; OPD üzerinde +%42.1/+%40.6.
- **Skill-as-Pseudocode** (Li et al., arXiv 2605.27955, 2026-05): markdown skill kütüphanelerini tipli pseudocode'a derler; ALFWorld unseen'da 82/402 vs GoS 47/402 kazanma (p=8.2e-5); **−%22.8 input token, −%14.5 LLM çağrısı**.
- **SkCC** (Ouyang et al., arXiv 2605.03353, 2026-05): skill derleyicisi (SkIR); pass rate Claude Code'ta %21.1→%33.3, Kimi CLI'da %35.1→%48.7; **%10-46 runtime token tasarrufu**; %94.8 proaktif güvenlik tetik oranı.
- **ASG-SI — Audited Skill-Graph Self-Improvement** (Huang & Huang, arXiv 2512.23760, 2025-12): her iyileştirme trajektoriden çıkarılır, doğrulanabilir arayüzle skill'e normalize edilir, **verifier tabanlı replay ve kontrat kontrollerinden geçmeden yükselmez**; audit-log ile yeniden üretilebilir ölçüm.
- **LongSeeker (fine-tuning hattı):** Qwen3-30B-A3B, 10k sentez trajektori; yukarıda §3.2.

### 4.3 Memory sistemleri ve maliyet

- **Oracle Agent Memory** (Alake et al., arXiv 2607.13157, 2026-07): database-native memory; LongMemEval **%93.8 doğruluk, düz geçmiş baseline'ına göre ~10.7x daha az token**; memory lifecycle (ingestion→extraction→consolidation→retrieval→summarization→revision).
- **Persistent Q4 KV Cache** (Shkolnikov, arXiv 2603.04428, 2026-02): edge'de KV cache'i diske Q4 ile kalıcılaştırma; **time-to-first-token 136x'e kadar hızlanır** (Gemma 3 12B: 22-136x; 4K-32K), Q4 ile aynı belleğe 4x daha fazla ajan context'i sığar; perplexity etkisi −0.7%…+3.0%. Agent persistence'i sıfırlamanın gerçek maliyeti: 4K context'te ajan başına 15.7s re-prefill.
- **Are We Ready For An Agent-Native Memory System?** (Zhou et al., arXiv 2606.24775, 2026-06): 12 memory sistemi, 5 workload, 11 dataset; **hiçbir mimari her senaryoda domine etmiyor**; workload-bottleneck uyumu belirleyici; **lokal bakım (localized maintenance) global reorganizasyondan daha maliyet-etkin**.
- **Zombie Agents** (Yang et al., arXiv 2602.15654, 2026-02): self-evolving ajanların belleğine web üzerinden kalıcı payload enjeksiyonu; sliding-window ve RAG memory'ye özgü kalıcılık stratejileri; per-session prompt filtrelemenin yetersiz olduğu gösterimi. (Memory'ye güven ≠ güvenli hafıza.)
- **Skill spec güvenilirliği** (Wen, arXiv 2605.19362, 2026-05): 878 siber-güvenlik skill'inde yalnızca **%2.3'ü dört anlama çapasının (operasyonel temel, çıktı kontratı, sınır bildirimi, örnek gösterim) tamamına sahip**; %19.0'da örnek gösterim var — skill'ler kullanıcıya karşı capability disclosure olarak değerlendirilmeli.

---

## 5. Omnitrix için çıkarımlar (sentez)

1. **Harness > model:** Aynı Opus 4.7'de 65/60/57 (opencode/cursor/claude) ve TB 2.0'da Codex CLI 82.2 vs Claude Code 58.0 — omnitrix'in tek-ajan çalışma zamanı kalitesi, seçilen modelden daha büyük skor etkeni.
2. **Basitlik kazanır:** Observation masking, LLM-summarization'a eşit çözüm oranında yarı maliyet (§3.2 Complexity Trap). Omnitrix'in tier modeli (Sleeping → Active) ve hashline diff akışı bu bulguyla uyumlu — summarization'ı varsayılan değil, eşiklenmiş seçenek yap.
3. **Gating fidelity eşiğine bağlı olmalı** (Root Theorem), token/cache maliyeti ikinci öncelik; compaction güvenlik kısıtlarını silmemeli (Governance Decay → Constraint Pinning benzeri ayrı katman).
4. **Memory = lifecycle, öğrenilebilir operatör:** frozen-snapshot (hermes), nudge tabanlı skill review, verifier kapılı skill promotion (ASG-SI), lokal bakım (agent-native memory). GEPA (ICLR 2026 Oral) skill evriminde rollout verimliliği açısından GRPO'dan üstün — persona/skill prompt evriminde değerlendir.

---

## 6. Kaynakça

**Leaderboard'lar:** tbench.ai/leaderboard/terminal-bench/2.0 · swebench.com · artificialanalysis.ai/agents/coding-agents · llm-stats.com/benchmarks/swe-bench-pro · morphllm.com/ai-coding-agent · ssojet.com/blog/best-cli-coding-agents-ranked · benchlm.ai/blog/posts/ai-coding-agents · andrew.ooo/answers/opencode-vs-claude-code-vs-codex-cli-april-2026 · sanj.dev/post/claude-code-vs-codex-cli · nxcode.io · codex.danielvaughan.com (TraceProbe, SWE-Chain) · github.com/Vexp-ai/vexp-swe-bench · github.com/tbenchai/Terminal-Bench

**Akademik (arXiv):** 2604.20874 (Root Theorem) · 2510.26493 (CE 2.0) · 2605.15721 (NCCE) · 2607.10532 (PTFEA) · 2508.21433 (Complexity Trap) · 2510.00615 (ACON) · 2510.11967 (Context-Folding) · 2510.06727 (SUPO) · 2605.05191 (LongSeeker) · 2606.11213 (CWL) · 2607.25066 (ARC) · 2605.23296 (Parallel Compaction) · 2606.22528 (Governance Decay) · 2604.19457 (Four-Axis) · 2604.14140 (LongCoT) · 2512.05470 (Everything is Context) · 2607.29422 (AgenticRepair) · 2512.03001 (Invasive CE) · 2508.08322 (CE Code Assistants) · 2604.08224 (Externalization) · 2602.06052 (Memory Survey) · 2607.13104 (Self-Improvements Survey) · 2602.12430 (Agent Skills Survey) · 2507.19457 (GEPA) · 2608.01678 (Skill-α) · 2604.10674 (Skill-SD) · 2605.27955 (SaP) · 2605.03353 (SkCC) · 2512.23760 (ASG-SI) · 2607.13157 (Oracle Memory) · 2603.04428 (Q4 KV Cache) · 2606.24775 (Agent-Native Memory) · 2602.15654 (Zombie Agents) · 2605.19362 (Skill Specs)

**İç kaynak:** omnitrix/hermes/RAPOR.md (GEPA/DSPy, ICLR 2026 Oral, §2-B)
