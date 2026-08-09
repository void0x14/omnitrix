# Routing Modes Araştırma Notları (P1.1)

> Task P1.1 research-only çıktısı. Hedef: ≥40 unique routing mode ID + 1 cümle blurb, ailelere göre organize, her mod `primitive` (tekil seçim kuralı) / `policy` (koşullu tetikleyici-kısıt) / `composition` (seçici+policy veya öğrenilmiş/dış sistem) olarak sınıflandırılmış. Kaynaklar URL veya paper ID ile; kaynağı olmayanlar açıkça `[SENTEZ]` etiketli.

## 1. Yöntem / Araçlar

- sequential-thinking: 5 adım (görev analizi → araştırma planı → bağlam okuma → sentez → doğrulama)
- Web MCP (firecrawl): 9router, OpenRouter routing, LiteLLM router strategies, AWS Bedrock routing
- paper-search MCP: "load balancing LLM inference routing" + "LLM fallback cascades / FrugalGPT"
- Context7 MCP: `resolve-library-id("LiteLLM")` → `/berriai/litellm` seçildi; `query-docs` ile routing pipeline, simple-shuffle, lowest-latency, usage-based (lowest_tpm_rpm) ve order-based fallback internalleri alındı (kaynak: litellm `router_strategy/*.py` + `router.py`)

## 2. Rakip/Ürün Bulguları (kaynak → davranış)

### 2.1 9router (decolua/9router)
- Lokal proxy/router; OpenAI-uyumlu `/v1` uç. Kaynak: https://github.com/decolua/9router
- Tier mimarisi: Tier1 SUBSCRIPTION → Tier2 CHEAP → Tier3 FREE; quota bitince bir alt tiera düşer (auto fallback, "never hit limits"). Aynı provider'da multi-account round-robin; quota tracking.
- RTK token saver (tool_result sıkıştırma) — routing değil ama maliyet modu olarak ilgili.
- Issue #317: ClawRouter mimarisi önerisi — agentic tier switching, pluggable strategies, hash-keyed LLM classifier cache (tekrar eden prompt pattern'leri için cache'li classifier). Kaynak: https://github.com/decolua/9router/issues/317
- İlgili ekosistem: OpenClaw Router built-in stratejiler: `random` (opsiyonel weighted), `round_robin`, `rules` (keyword→backend), `llm` (küçük router-LLM backend seçer). Kaynak: https://github.com/ulab-uiuc/llmrouter/blob/6db5bbc5e9b61e3bb9ec4fb6d21d19aef587a13c/openclaw_router/README.md

### 2.2 OpenRouter routing
- Model fallbacks: `models` dizisi, priority order, sadece sınıflandırılmış hatalarda tetiklenir (context-length, moderation flag, rate-limit, downtime); 400 malformed request geri döner; liste bir kez yürünür, sonsuz retry yok; son model de fail → hata. Kaynak: https://openrouter.ai/docs/guides/routing/model-fallbacks + https://openrouter.ai/blog/insights/reliability-failover/
- Provider failover: `allow_fallbacks: true` (default); aynı model farklı provider; son 30 saniyede önemli outage olan provider deprioritize edilir; 5xx/429'da otomatik diğer provider. Kaynak: https://openrouter.ai/blog/insights/model-routing/
- Default provider seçimi: en ucuz stabil provider, fiyatın ters karesiyle ağırlıklı; `sort: price|throughput|latency` veya `{by, partition: model|none}`; `max_price` hard cap (karşılanmazsa istek çalışmaz); `preferred_min_throughput`/`preferred_max_latency` p50-p99 eşikleri (garanti değil); `only`/`ignore`/`order`; `require_parameters`; `data_collection: "deny"`; `zdr`; quantizations. Kaynak: https://openrouter.ai/docs/guides/routing/provider-selection + https://github.com/nano-collective/nanocoder/blob/b2464384991232100a31bb93e2f15e16dfdae761/docs/configuration/providers/openrouter.md
- Auto Router: `openrouter/auto` (NotDiamond), prompt başına model seçer; `cost_quality_tradeoff` 0-10 dial (default 7; 0=en güçlü, 10=en ucuz); Auto Beta: task tipi + 7 günlük "Share of Spend" sıralaması + `cost_tier` (low/medium/high/xhigh/max → cost-percentile band); ek ücret yok; `max_price` ile birlikte çalışır. Kaynak: https://openrouter.ai/docs/guides/routing/routers/auto-router
- Kısayollar: `:nitro` (throughput sort), `:floor` (price sort); pareto-code router `min_coding_score`. Kaynak: https://docs.agno.com/models/providers/gateways/openrouter/overview (fallback models), openrouter provider-selection.
- Zero-completion insurance: fail olan istek için fatura yok. Kaynak: https://openrouter.ai/blog/insights/model-routing/

### 2.3 LiteLLM router strategies
- Stratejiler: `simple-shuffle` (default; weight/rpm/tpm varsa weighted random, yoksa uniform), `least-busy`, `latency-based-routing` (EWMA; TTFT streaming için; `lowest_latency_buffer` ile en hızlının %X yakınına random dağıtım; TPM/RPM kısıtlarına saygı), `cost-based-routing`, `usage-based-routing` (dakikalık TPM/RPM sayaçları, en düşük TPM, random yok), `usage-based-routing-v2`, `provider-budget-routing`, `priority-based-routing`, `rate-limit-aware`, `lar1` (latency-aware özel varyant). Kaynaklar: https://docs.litellm.ai/docs/routing , https://docs.litellm.ai/docs/proxy/load_balancing , https://github.com/majiayu000/litellm-rs/blob/4cd554c285f3e3264a6109e9a39bd8504d05b167/docs/comparison/07-routing.md
- Context7 kod düzeyinde: `simple_shuffle.py` (weight/rpm/tpm weighted pick), `lowest_latency.py` (per-deployment latency geçmişi, buffer, streaming TTFT), `lowest_tpm_rpm.py` (en düşük TPM, ilk kazanan), `router.py` pipeline: pre-routing hooks → routing group strategy çözümü → healthy deployment filtresi → strateji seçimi → retry + cross-group fallbacks. Kaynak: https://github.com/berriai/litellm/blob/litellm_internal_staging/router.py
- Order-based fallback: deployment'lar `order` seviyelerinde; mevcut seviyedekiler fail edince artan sırayla üst seviyelere geçer (`_target_order`). Kaynak: https://github.com/berriai/litellm/blob/litellm_internal_staging/litellm/router.py
- Fallback tipleri: `fallbacks` (tüm hatalar, default), `context_window_fallbacks` (ContextWindowExceededError), `content_policy_fallbacks` (ContentPolicyViolationError), `default_fallbacks` (grup bulunamazsa wildcard), `max_fallbacks` (default 5), `enable_pre_call_checks` (context-window + EU-region pre-filter). Kaynak: https://docs.litellm.ai/docs/proxy/reliability , https://docs.litellm.ai/docs/proxy/config_settings
- Cooldown/circuit: `allowed_fails` (default 3/dakika), `cooldown_time` (default 5s), per-error `allowed_fails_policy` (ör. RateLimitErrorAllowedFails), `retry_policy` per error tipi (ör. InternalServerErrorRetries: 4), `enable_weighted_failover` (retryable failure → aynı grup içinde weighted re-pick, sonra cross-group fallback). Kaynak: https://docs.litellm.ai/docs/routing , https://docs.litellm.ai/docs/proxy/config_settings
- routing_groups: grup başına farklı `routing_strategy` + `routing_strategy_args.ttl`. Kaynak: https://docs.litellm.ai/docs/proxy/ui/routing_groups
- Redis-backed shared state (multi-instance). Kaynak: https://docs.litellm.ai/docs/proxy/load_balancing

### 2.4 AWS Bedrock routing
- Cross-Region inference: inference profile (geo: `us.`/`eu.`/`apac.` vs `global.`); Bedrock kendi bölgeyi seçer; geo = coğrafya içinde, global = dünya çapında; in-region = bölge asla terk edilmez. Kaynak: https://docs.aws.amazon.com/bedrock/latest/userguide/cross-region-inference.html
- Intelligent prompt routing: `create-prompt-router`, aynı model ailesi içinde 2 model (ör. Haiku fallback anchor + Sonnet); request başına her modelin response quality'sini tahmin eder; `routing-criteria: {responseQualityDifference: 0.5}` — kalite farkı eşiği aşılmazsa fallback model çalışır; default vs configured routers; sadece İngilizce optimize; uygulama verisiyle ayarlanamaz. Kaynak: https://docs.aws.amazon.com/bedrock/latest/userguide/prompt-routing.html

## 3. Paper Bulguları

### 3.1 load balancing LLM inference
| Paper | ID / DOI | Bulgu |
|---|---|---|
| Lodestar: An Online-Learning LLM Inference Router | arXiv:2606.00946 (DOI 10.48550/arXiv.2606.00946) | Online reward predictor ile request→GPU instance routing; TTFT minimize; 1.41x daha düşük avg TTFT; vLLM ile çalışır |
| Performance Aware LLM Load Balancer for Mixed Workloads | DOI 10.1145/3721146.3721947 | prefill/decode faz ayrımı + response-length predictor + RL; %11 daha düşük E2E latency |
| Beyond Accuracy and Cost: Latency-Aware LLM Query Routing for Dynamic Workloads | arXiv:2607.18253 | Hafif TTFT estimator (prefill/decode iş yükü simülasyonu); latency+accuracy+cost joint optimizasyon; %40 accuracy-cost utility iyileşmesi |
| NexusSched: Predictive Two-Layer Scheduling for LLM Serving | arXiv:2509.23384 (DOI 10.48550/arXiv.2509.23384) | Çift katman: engine LENS (SLO-aware batching) + cluster PRISM (predictive state-driven routing); %43 SLO attainment artışı |
| Adaptive Load Balancing for Multi-Tier LLM Inference with Dynamic Model Routing | DOI 10.1109/SoutheastCon63549.2026.11476301 | RL ile tier atama (edge/mid/cloud); round-robin'e göre %51 latency, %70 cost düşüşü; "request complexity → model capacity" eşleme |
| A Control-Oriented Survey of Load Balancing for LLM Inference Serving | DOI 10.2139/ssrn.6516666 | Survey: prefill/decode faz asimetrisi, KV-cache state, heavy-tailed workload; SLO-goodput ölçümü; TTFT/token cadence |

### 3.2 fallback cascades
| Paper | ID / DOI | Bulgu |
|---|---|---|
| FrugalGPT: How to Use LLMs While Reducing Cost and Improving Performance | arXiv:2305.05176 | LLM cascade: öğrenilmiş, query başına model kombinasyonu; GPT-4 kalitesi %98 maliyet indirimiyle; confidence-based deferral |
| Conformal Cascade: Distribution-Free Accuracy Guarantees for Multi-Tier LLM Inference | arXiv:2607.25018 | Deferral kuralı = conformal prediction set boyutu; kalibrasyonsuz, dağıtım-bağımsız 1−Kα accuracy garantisi; threshold tuning derdini kaldırır |
| Learning to Cascade: Confidence Calibration for Cascade Inference Systems | arXiv:2104.09286 | Cascade sistemleri için özel confidence calibration; yanlış kalibre güven cascade'i kötüleştirebilir |
| Cascade: Token-Sharded Private LLM Inference | arXiv:2507.05228 | Gizlilik: sequence-dimension sharding ile SMPC'ye alternatif; privacy-routing fikri için ilgili |

## 4. Mod Kataloğu (65 ID)

Açıklamalar: `[kaynak URL veya paper ID]` — dış kaynak; `[SENTEZ]` — doğrudan tek kaynağı olmayan, yukarıdaki bulgulardan türetilmiş birleştirme.

### Balance (13)
| ID | Sınıf | Blurb (1 cümle) |
|---|---|---|
| `rr` | primitive | Sıradaki sağlıklı endpoint'e sırayla gönder, listeyi döngüsel dolaş (OpenClaw `round_robin`, litellm-rs `RoundRobin`). [openclaw_router README] |
| `wrr` | primitive | Endpoint'lere ağırlık ver, ağırlık oranında (weight/rpm/tpm) weighted pick yap (LiteLLM `simple-shuffle` weighted dalı). [docs.litellm.ai/docs/routing] |
| `random` | primitive | Sağlıklı havuzdan uniform random seç (OpenClaw `random`, LiteLLM `simple-shuffle` weightsiz dalı). [openclaw_router README] |
| `least-busy` | primitive | Anlık en az in-flight istek olan endpoint'e yönlendir (LiteLLM `least-busy`). [docs.litellm.ai/docs/proxy/load_balancing] |
| `ewma-latency` | primitive | Kayıtlı yanıt süresi geçmişinin ortalaması en düşük endpoint'i seç; streaming'de TTFT kullan (LiteLLM `latency-based-routing` EWMA). [docs.litellm.ai/docs/routing] |
| `latency-buffer` | primitive | En hızlının `lowest_latency_buffer` (ör. %50) yakınındaki endpoint'lere random dağıt, tek kutuya yüklenmeyi önle (LiteLLM). [docs.litellm.ai/docs/routing] |
| `usage-based-tpm` | primitive | Dakikalık sayaçlarla en düşük TPM kullanımlı, kota limitini aşmayacak endpoint'i seç (LiteLLM `usage-based-routing`). [github litellm lowest_tpm_rpm.py] |
| `usage-based-v2` | primitive | TPM/RPM headroom'unu daha dengeli kullanan gelişmiş usage stratejisi (LiteLLM `usage-based-routing-v2`). [litellm-rs comparison] |
| `rate-limit-aware` | primitive | Endpoint rpm/tpm limitlerini istek öncesi kontrol edip aşacak adayları eleyen seçici (LiteLLM `rate-limit-aware`, pre-call checks). [docs.litellm.ai/docs/routing] |
| `token-bucket-fair` | primitive | Her endpoint'e token-bucket havuzu ver, dolan bucket'ları atla; kodlayan kullanım ahlakı `usage-based-tpm` + `rate-limit-aware`'dan. [SENTEZ] |
| `sticky-session` | policy | Aynı oturum/prompt-hash aynı endpoint'e sabitlensin, dağıtık state gerektirir (9router classifier cache hash-keying, OpenClaw per-agent binding). [9router issue #317; openclaw_router README] |
| `hash-prompt` | primitive | `prompt_hash` üzerinden deterministik endpoint seçimi (consistent hashing) — test tekrarlanabilirliği ve cache affinity için. [SENTEZ; OpenClaw `rules` deterministiklik prensibi] |
| `model-group-alias` | policy | `gpt-4` isteğini `gpt-3.5-turbo` grubuna yönlendiren alias haritası (LiteLLM `model_group_alias`). [docs.litellm.ai/docs/proxy/load_balancing] |

### Failover (12)
| ID | Sınıf | Blurb |
|---|---|---|
| `fallback-strict` | policy | Öncelik sıralı endpoint listesini sırayla dene, ilk başarıda dur (OpenRouter `models` dizisi; 9router tier cascade). [openrouter model-fallbacks; 9router README] |
| `fallback-soft` | policy | Sadece sınıflandırılmış hatalarda (rate-limit, downtime, context-length, moderation) fallback yap; malformed 400 geri döner (OpenRouter). [openrouter reliability-failover] |
| `backup-only-on-429` | policy | Yalnızca 429/rate-limit hatalarında yedek endpoint zincirine geç (litellm-rs `RateLimit` fallback tipi). [litellm-rs comparison] |
| `backup-on-5xx` | policy | Yalnızca 5xx/InternalServerError'da yedeklere geç (OpenRouter provider failover 5xx; LiteLLM InternalServerErrorRetries). [openrouter model-routing] |
| `circuit-break-cascade` | policy | `allowed_fails`/dakika aşılınca endpoint'i `cooldown_time` boyunca havuzdan çıkar, kalanlarla kaskad devam (LiteLLM cooldown; OpenRouter 30s deprioritize). [docs.litellm.ai/docs/routing] |
| `provider-failover` | policy | Aynı modeli farklı provider'da canlı tut; 5xx/429'da diğer provider'a düş (OpenRouter `allow_fallbacks`). [openrouter model-routing] |
| `order-level-fallback` | policy | `order` seviyelerinde dene; seviye tamamen fail edince sonraki seviyeye geç (LiteLLM order-based fallback `_target_order`). [github litellm router.py] |
| `context-window-fallback` | policy | ContextWindowExceeded'da daha geniş context'li modele düş; `enable_pre_call_checks` ile çağrı öncesi yakala (LiteLLM). [docs.litellm.ai/docs/proxy/reliability] |
| `content-policy-fallback` | policy | ContentPolicyViolation'da (moderation) alternatif modele geç (LiteLLM `content_policy_fallbacks`; OpenRouter moderation flags). [docs.litellm.ai/docs/proxy/reliability] |
| `default-fallback` | policy | Gruba özel fallback yoksa wildcard/genel zincir kullan (`*` → fallback; `default_fallbacks`; `max_fallbacks` 5). [litellm-rs comparison; docs config_settings] |
| `weighted-failover` | composition | Retryable failure'da önce aynı grup içinde weighted re-pick, tükenirse cross-group fallback (LiteLLM `enable_weighted_failover`). [docs config_settings] |
| `hedge-p95` | composition | P95 gecikme riskinde ikinci endpoint'e paralel duplicate istek gönder, ilk biten cevabı kullan — maliyet 2x ama p95 kesilir. [SENTEZ; plan spec hedge; OpenRouter zero-completion insurance mantığı] |

### RoleSplit (7)
| ID | Sınıf | Blurb |
|---|---|---|
| `jep-classic` | policy | judge/executor/planner rollerini sabit endpoint sınıflarına bağla (repo `FallbackRouter`/JEP + plan spec 3.2). [SENTEZ — mevcut repo JEP mimarisi + plan] |
| `jep-cheap-plan` | policy | Planner ucuz küçük modele, judge/executor güçlü kalsın — FrugalGPT'nin "basit işe küçük model" prensibi. [arXiv:2305.05176] |
| `jep-strong-judge` | policy | Judge her zaman en güçlü/doğruluk lideri modelde, geri kalanlar maliyete göre. [SENTEZ; FrugalGPT anchor fikri] |
| `planner-only-chain` | policy | İlk aşama yalnızca planner modelinden, alt aşamalar diğer rollerden geçer (OpenClaw request-level routing granularity notu). [openclaw_router README] |
| `executor-swarm` | composition | Birden çok executor endpoint'ine görev böl, sonuçları karşılaştır/çoğunlukla seç (Conformal Cascade çok katman deferral + FrugalGPT kombinasyon). [arXiv:2607.25018; arXiv:2305.05176] |
| `role-sticky` | policy | Rol başına endpoint affinity: aynı rol hep aynı endpoint havuzunda (OpenClaw strict per-agent binding). [openclaw_router README] |
| `prompt-role-classify` | composition | Hash-keyed classifier ile prompt'u role göre sınıflandır, sonra o role ait havuzdan seç (9router/ClawRouter classifier cache). [9router issue #317] |

### Hybrid (8)
| ID | Sınıf | Blurb |
|---|---|---|
| `balance-then-fallback` | composition | Sağlıklı havuzda dengeli dağıt, zincir tükenince sıralı fallback kaskadı (LiteLLM simple-shuffle + fallbacks; OpenRouter price-balance + models array). [docs.litellm.ai/docs/routing; openrouter blog] |
| `jep-with-rr-executors` | composition | JEP rollerine round-robin executor havuzu ekle (plan spec v1). [plan spec 3.2] |
| `canary-10` | composition | Trafiğin %10'unu yeni/aday endpoint'e, kalanını stabil hatta gönder; hata oranına göre otomatik geri al (klasik canary; LiteLLM weight 9:1 örneği). [docs.litellm.ai/docs/routing weight örneği] |
| `shadow-mirror` | composition | Üretim isteğinin kopyasını shadow endpoint'e de gönder ama cevabı kullanma — davranışı risksiz ölç (klasik shadow traffic). [SENTEZ] |
| `hedge-on-latency` | composition | Primary latency budget'ı aşınca hedge başlat (patel latency-aware router: TTFT estimator ile budget). [arXiv:2607.18253] |
| `tier-cascade` | composition | Subscription → Cheap → Free tier'ları arasında quota/budget bitince düş (9router tier mimarisi + quota tracking). [github.com/decolua/9router] |
| `frugal-cascade` | composition | Query başına öğrenilmiş model zinciri; düşük güven/kalite tahmininde güçlü modele defer (FrugalGPT LLM cascade). [arXiv:2305.05176] |
| `conformal-cascade` | composition | Conformal prediction set boyutu tek elemana inince kabul, değilse üst tiera defer; dağıtım-bağımsız accuracy garantisi (Conformal Cascade). [arXiv:2607.25018] |

### Specialty (10)
| ID | Sınıf | Blurb |
|---|---|---|
| `research-ocean-prefer` | policy | Research modlarında derin/ocean araştırma endpoint'lerini öncelikle seç (repo `research_tool` surface/deep/ocean + plan spec). [SENTEZ — repo research_tool] |
| `coding-long-context` | policy | Uzun bağlamlı kodlama istekleri için yeterli context window'u olan endpoint'leri filtrele (Bedrock in-region/context kısıtları prensibi; plan spec). [SENTEZ; docs.aws.amazon.com inference-profiles] |
| `json-strict-model` | policy | JSON/tool-call şeması ispatlanmış endpoint'lere sabitle (OpenRouter `require_parameters` benzeri kısıt). [nanocoder openrouter.md] |
| `vision-capable-only` | policy | Görüntü girişi olan istekleri yalnızca vision-capable endpoint'lere yönlendir (plan spec v1). [SENTEZ — plan spec] |
| `tool-call-reliable` | policy | Tool-call başarı oranı yüksek endpoint'leri öncelikle seç (OpenClaw router-LLM kalite sinyali; OpenRouter throughput/outage sinyalleri). [SENTEZ; openrouter provider-selection] |
| `router-llm` | composition | Küçük bir LLM prompt içeriğine göre endpoint seçer (OpenClaw `llm` stratejisi; 9router classifier). [openclaw_router README] |
| `keyword-rules` | primitive | Keyword→endpoint kural tablosu, ilk eşleşen kural kazanır (OpenClaw `rules`). [openclaw_router README] |
| `auto-router` | composition | Harici prompt-bazlı router (NotDiamond `openrouter/auto`): task tipi + 7 günlük spend-share + `cost_quality_tradeoff` dial. [openrouter auto-router] |
| `quality-diff-router` | composition | Aynı aile içi model adaylarına request başına response quality tahmini yap, fallback anchor'a göre `responseQualityDifference` eşiğini aşanı seç (Bedrock intelligent prompt routing). [docs.aws.amazon.com prompt-routing] |
| `learned-router` | composition | Eğitilmiş ML router (KNN/SVM/MLP/Elo/graph) veya online reward predictor ile request→endpoint ataması (LLMRouter kütüphanesi; Lodestar). [openclaw_router README; arXiv:2606.00946] |

### Cost (7)
| ID | Sınıf | Blurb |
|---|---|---|
| `cheapest-alive` | primitive | Sağlıklı endpoint'lerden en ucuzunu seç (LiteLLM `cost-based-routing`; OpenRouter price sort default). [docs.litellm.ai/docs/routing] |
| `budget-aware` | policy | Provider/grup bazlı zaman pencereli bütçe; bütçe tükenince o grubu baypas et (LiteLLM `provider-budget-routing`; 9router quota tracking). [litellm-rs comparison; 9router README] |
| `quality-floor` | composition | En ucuz ama minimum kalite/throughput eşiğini karşılayan endpoint (OpenRouter `preferred_min_throughput` + price sort; pareto-code `min_coding_score`). [openrouter provider-selection; hermes-integration] |
| `deadline-aware` | policy | SLO/deadline'a yetişebilecek en ucuz endpoint; aşılacaksa daha hızlı ama pahalıya geç (NexusSched SLO-aware; latency-aware router). [arXiv:2509.23384; arXiv:2607.18253] |
| `price-cap` | policy | `max_price` tavanı: üstünde endpoint yoksa isteği çalıştırma (OpenRouter max_price hard stop). [nanocoder openrouter.md; openrouter auto-router] |
| `cost-quality-dial` | policy | 0-10 cost/quality dial'ı ile aday havuzunu cost bandına göre daralt (OpenRouter `cost_quality_tradeoff` / `cost_tier`). [openrouter auto-router] |
| `quota-aware` | policy | Abonelik kotası dolana kadar o tiera yönlendir, sonra diğer havuzlara geç (9router "maximize subscriptions, use every bit before reset"). [github.com/decolua/9router] |

### Privacy (8)
| ID | Sınıf | Blurb |
|---|---|---|
| `local-first` | policy | Önce lokal/self-hosted endpoint'ler, cloud yalnızca lokal havuz boşsa (9router lokal proxy mimarisi; plan spec). [9router README] |
| `no-train-providers` | policy | Eğitim/data toplama yapan provider'ları dışla (OpenRouter `data_collection: deny`, `ignore`). [nanocoder openrouter.md] |
| `eu-region-only` | policy | Yalnızca EU bölgesi endpoint'leri (Bedrock geo `eu.` profile; LiteLLM EU-region pre-call check). [docs.aws.amazon.com cross-region-inference; docs.litellm.ai/docs/routing] |
| `in-region-strict` | policy | İstek kaynak bölgeyi asla terk etmesin (Bedrock in-region compliance). [docs.aws.amazon.com models-region-compatibility] |
| `geo-profile` | policy | Coğrafi kapsam kısıtı: `us.`/`apac.` gibi coğrafya profilleri arasında (Bedrock geographic cross-region). [docs.aws.amazon.com geographic-cross-region-inference] |
| `zdr-only` | policy | Yalnızca zero-data-retention (veri saklamayan) provider'lar (OpenRouter `zdr`). [nanocoder openrouter.md] |
| `secure-compute` | policy | Hassas işleri SMPC/sharded private inference uçlarına yönlendir (Cascade token-sharded private inference). [arXiv:2507.05228] |
| `no-fallback-offshore` | composition | `eu-region-only` + `fallback-strict` kompozisyonu: fallback zinciri bile coğrafya dışına çıkmasın. [SENTEZ] |

## 5. Kanıt Matrisi (source → behavior → mode IDs)

| Kaynak (URL/paper) | Öğrenilen davranış | Aday mod ID'leri |
|---|---|---|
| openrouter.ai/docs/guides/routing/model-fallbacks | models[] sıralı, sınıflandırılmış hata → fallback; bir kez yürür | fallback-strict, fallback-soft |
| openrouter.ai/blog/insights/reliability-failover | Provider failover default açık; 400 geri döner; garbage-200 fallback tetiklemez | fallback-soft, provider-failover |
| openrouter.ai/blog/insights/model-routing | 30s outage deprioritize; inverse-square price weighting; zero-completion | circuit-break-cascade, cheapest-alive, hedge-p95 |
| openrouter.ai/docs/guides/routing/provider-selection | sort price/throughput/latency; partition model/none; max_price hard cap; p90 eşikler | cheapest-alive, quality-floor, price-cap, quality-diff-router |
| openrouter.ai/docs/guides/routing/routers/auto-router | NotDiamond auto; cost_quality_tradeoff dial; cost_tier band; 7-gün spend share | auto-router, cost-quality-dial |
| docs.litellm.ai/docs/routing (+ Context7 internals) | simple-shuffle weighted; latency EWMA+buffer; TPM/RPM; cooldown; pre-call checks | rr, wrr, random, ewma-latency, latency-buffer, usage-based-tpm, rate-limit-aware, circuit-break-cascade |
| docs.litellm.ai/docs/proxy/reliability | 3 fallback tipi; enable_pre_call_checks; max_fallbacks | context-window-fallback, content-policy-fallback, default-fallback |
| github litellm router.py (order fallback) | order seviyeleri, _target_order yükselme | order-level-fallback |
| docs config_settings | enable_weighted_failover; allowed_fails_policy; retry_policy | weighted-failover, backup-on-5xx, backup-only-on-429 |
| github.com/decolua/9router | Tier1→2→3 quota kaskadı; multi-account RR; classifier cache | tier-cascade, quota-aware, sticky-session, prompt-role-classify |
| openclaw_router README (LLMRouter repo) | random/round_robin/rules/llm stratejileri | random, rr, keyword-rules, router-llm, role-sticky |
| docs.aws.amazon.com prompt-routing.html | Aile içi 2 model; responseQualityDifference eşiği; fallback anchor | quality-diff-router (bkz. Specialty notu) |
| docs.aws.amazon.com cross-region-inference / models-region-compatibility | in-region vs geo vs global | eu-region-only, in-region-strict, geo-profile |
| arXiv:2305.05176 FrugalGPT | LLM cascade, öğrenilmiş model kombinasyonu, %98 cost | frugal-cascade, jep-cheap-plan |
| arXiv:2607.25018 Conformal Cascade | conformal set boyutu deferral; garantili accuracy | conformal-cascade, executor-swarm |
| arXiv:2606.00946 Lodestar | online reward predictor, TTFT | learned-router (bkz. Specialty), deadline-aware |
| arXiv:2607.18253 latency-aware | TTFT estimator, joint latency+accuracy+cost | hedge-on-latency, deadline-aware, quality-floor |
| arXiv:2509.23384 NexusSched | predictif çift katman, SLO attainment | deadline-aware |
| DOI 10.1109/SoutheastCon63549.2026.11476301 | tier assignment, request complexity→capacity | executor-swarm, deadline-aware |
| arXiv:2507.05228 Cascade | token-sharded private inference | secure-compute |

Not: `quality-diff-router` ve `learned-router` birer mod olarak Specialty'de yer alır (bkz. §4 tablosunda `auto-router` yanında); matriste davranış kaynağına bağlanmıştır.

## 6. Tasarım Uyarıları (RoutingModeDef / RouterEngine için)

1. **Primitive/policy/composition ayrımı TOML'a yansımalı**: `selector = "rr"` vs `selector = "fallback"` (P1.3 planındaki gibi). Composition modlar `on_result`/`on_error` callback'i zorunlu kılar; `pick` yeterli değil. Uyarı kaynağı: LiteLLM router.py pipeline (healthy filtre → strateji → retry+fallback) — retry/fallback döngüsü pick'ten ayrı bir state machine.
2. **Hata sınıflandırma şart**: fallback-soft/backup-only-on-429/context-window-fallback gibi modlar hangi hata sınıfının fallback tetiklediğine bağlı; `RouteError` enum'ı 429/5xx/context/moderation/timeout'u ayırmalı (OpenRouter "classified errors" + LiteLLM RetryPolicy kanıtı).
3. **Cooldown/circuit state mutable**: `RouterEngine.on_result` per-endpoint failure sayacı ve cooldown timestamp tutmalı; process-restart'da kaybolabilir (LiteLLM Redis shared state; tekil süreçte in-memory yeterli).
4. **Determinizm vs dağıtık state**: `sticky-session`, `usage-based-tpm`, `budget-aware` sayaçlıdır; testlerde injectable clock/counter gerektirir (P0.3 test izolasyonu geleneği).
5. **"Garbage 200" tuzağı**: fallback yalnızca hata sınıflarında tetiklenir; 200 ama bozuk içerik fallback başlatmaz (OpenRouter blog uyarısı) — quality-floor gibi modlar için cevap doğrulama ayrı katman.
6. **max_fallbacks/retry sınırı**: sonsuz kaskad riski; LiteLLM default `max_fallbacks: 5`, OpenRouter liste bir kez — engine'de üst sınır parametrik olmalı.
7. **Hedge/canary/shadow maliyet & yan etki**: duplicate istek fatura/tool-call yan etkisi yaratır; `hedge-p95`, `canary-10`, `shadow-mirror` kompozisyonlarında tekrarlanan yan etkili (tool-call) isteklerde hedge kapatılmalı.
8. **Öğrenilmiş modlar (frugal-cascade, auto-router, learned-router) veri/bağımlılık ister**: FrugalGPT öğrenme verisi, NotDiamond harici API, Lodestar reward predictor eğitimi; offline/senaryosuz ortamda bu modların degrade davranışı (ör. `fallback-strict`'e düş) tanımlanmalı.
9. **Bölge/privacy filtreleri pick öncesi health kümesini daraltır**: `eu-region-only` + `fallback-strict` kompozisyonunda fallback zinciri de filtreye tabi (no-fallback-offshore); filtreyi yalnızca ilk pick'e uygulamak sızıntı yaratır.
10. **`blurb`/`long_help` içine model/pricing literal koyma**: katalog config'ten gelmeli, kod içine sabitlenmemeli (P1.1 kısıtıyla uyumlu; TUI picker blurb pane kullanıyor).

## 7. Sayımlar ve Doğrulama

- Toplam unique mod ID: **65** (Balance 13, Failover 12, RoleSplit 7, Hybrid 8, Specialty 10, Cost 7, Privacy 8)
- Placeholder/TBD: 0
- Dış kaynaklı her iddia URL veya paper ID taşıyor; `[SENTEZ]` etiketli 11 mod açıkça işaretli (rr dışındaki primitive'lerin çoğu kaynaklı).
- Araçlar: firecrawl search ×4 alan + 2 ek; paper-search (arxiv/semantic/crossref) ×2; Context7 resolve + query (LiteLLM); exa fetch ×1 (9router README).
