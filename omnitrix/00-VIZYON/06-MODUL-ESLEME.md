# 06 — Modül Eşleme: Omnitrix Modül Haritası

> Tarih: 2026-08-14 · Girdiler: 01–05 + 12 harness raporu (hermes dahil).
> Gerçeklik temeli: `omni-*` ayrı katmanı kullanıcının "matrix içinde matrix yok" emriyle kaldırıldı (6d6ee1d); mantık `xai-grok-*` içine eritildi. Aşağıdaki modüller **mantıksal birimlerdir** — eritilmiş kod tabanının içinde yaşar, tek binary'den yönetilir.

---

## 1. Dil politikası (soğuk başlatma + bellek + manevra gerekçesi)

| Dil | Yeri | Gerekçe |
|---|---|---|
| **Rust** | Çekirdek (tüm ajan/runtime/UI katmanı) | xai-* zorunlu bağımlılık (I2, imza düzeyinde tüketim); tokio ile 10 bin eşzamanlı ajan; jemalloc; tek binary; ms mertebesinde soğuk başlatma; zeroize/panic-abort disiplini hazır (overflow-checks + debug-assertions release'de açık). |
| **Zig** | Bağımsız native'ler: hashline editörü, walker/scan cache, omni-bench runner | xai-*'tan tamamen bağımsız saf veri işleme; sıfır runtime, C ABI ile gömülür; küçük tek-amaç binary'ler 1s soğuk başlatma bandının altında; kullanıcı tercihi. Mevcut Rust hashline **taşınmaz** (çalışıyor, `grok_build_hashline` compat setinde); Zig yalnızca yeni native'lere girer. |
| **Go** | Runtime'a GİRMEZ | crush'ın fikirleri port edilir, Go değil — ikinci runtime + ikinci toolchain = bakım yükü, tek-binary vizyonuna aykırı. |
| **Python** | Araç üretimi (bench analiz, spec) | aider'ın benchmark fikri alınır, Python runtime'ı alınmaz — soğuk başlatma ve bellek için elenir. |

Karar: "Her parça en iyi dilde" kuralı tek binary vizyonuyla dengelenir — N runtime = N bakım yükü. Manevra hızı, tek binary + ms startup ile kazanılır.

---

## 2. Modül haritası

| Modül | Sorumluluk | Dil | Hangi harness'ten ne aldı | Korunan grok-build varlığı | Lobotomi sonrası sadeleşmiş şekli |
|---|---|---|---|---|---|
| **omni-core / agent loop** | Ajan döngüsü, durum makinesi, ACP, session actor | Rust | crush: fold'lu kuyruk + deterministik cancel; opencode: doom-loop tespiti; CC: streaming watchdog dersi | xai-grok-shell session actor + agent runtime (eritilmiş) | ~367K → ~150K: session actor + ACP stdio + `-p` headless + auth çekirdeği + claude_import kalır; upload/telemetry/campaigns/marketplace/gcloud-storage/eski `sampling/` ikizi SİLİNİR. |
| **omni-compact** | 5'li compaction hiyerarşisi + iteratif merge + prune + yeniden enjeksiyon | Rust | CC: full/partial/sub-agent/micro/cross-session cache; pi: iteratif merge + cut-point; opencode: tail-turn bütçe + prune pası; codex: compaction hook'ları | xai-grok-agent compaction + system_reminder | Tek modül, tek eşik noktası (her assistant mesajı sonrası O(1) guard); API-raporlu usage esas; capability seam olarak kurgulanır (deepseek deseni) — backend değiştirilebilir. |
| **omni-edit / search natives** | Content-hash anchor edit, fuzzy fallback, 3-way merge, in-process search | Rust çekirdek + Zig native'ler (walker/scan cache) | oh-my-pi: hashline + stale-anchor reddi; aider: fuzzy zinciri + git cherry-pick 3-way + did-you-mean; oh-my-pi: in-process grep/glob + paylaşılan scan cache | `grok_build_hashline` compat seti + FFS/hashline ailesi (anchor/edit/mutate/grep) | Mevcut Rust hashline korunur; yeni tarayıcı/walker native'leri Zig; sed/awk bash yasağı devam (K3). Edit güvenilirliği ölçülebilir kimliktir. |
| **omni-repomap** | Repo map: tag grafı → pagerank → token bütçeli snippet | Rust | aider: tree-sitter tags + personalized pagerank + binary search; codex: bounded capability keşfi (scan derinliği/entry limitleri) | xai-codebase-graph + codebase_learn tool'u | Yerel, mtime önbellekli, hash dışarı ÇIKMAZ; `codebase_learn` (query/overview/symbol/deps) birincil araç. |
| **omni-permission** | Tool broker (tek yetki noktası), ruleset, allowlist, doom-loop, deny→gizleme | Rust | opencode: wildcard ruleset + tool gizleme + reject-cascade; codex: `*.rules` DSL + BANNED prefix + kurala dönüştürme; deepseek: guard plug-in'leri (loop-hygiene + tool-timeout) | ToolValidator seam + bash deny taxonomy + K3 acil kaçış + `capability_audit` | Precompiled Map + NFKC/confusable normalizasyon; persona allowlist'leri (`config/personas/*.toml`); bash hiçbir personada açık değil — yargıç onaylı acil kaçış. |
| **omni-storage** | CAS + WAL crash-only, checkpoint, trajectory log, blob store | Rust | cursor: content-addressed blob + resumeAction + stall tespiti; codex: ThreadStore trait'i; deepseek: append-only trajectory | SQLite (rusqlite bundled, CVE-patched) + CAS redb + write_journal (I7) + hunk-tracker | Tek writer aktör; tek message tablosu (append-only, part'lar JSON kolonda); event sourcing YOK; kapanış O(1), açılışta replay; blob'lar ana akıştan ayrı. |
| **omni-provider** | Keychain, models.dev, health, anahtar besleme (feeder) | Rust — **artifact dondur** | grok-cli kendi (zaten omnitrix malı); codex dersi: Responses API bağımlılığına GİRME | xai-omni-keychain (AES-256-GCM + Argon2id 64MiB + zeroize + 15dk TTL + OS keyring + stack sync), models.dev client, feeder (verifier.db → live/dead-hold), `--auto` prefix-detect + probe | `crates/common`'a taşınır; üçüncü-taraf sızmış-anahtar hattı kurulmaz (K13); anahtarlar asla diskte değil, config'e `api_key` yazılmaz. |
| **omni-router** | 60+ routing modu, JEP, grounding, yanlışlamacı yargıç | Rust — **artifact dondur** | grok-cli kendi; warp dersi: server-side routing YOK | RouterEngine (1784 satır) + routing_modes.toml (7 aile) + fallback walk (retry.rs) + xai-circuit-breaker | BYOK, istemci-tarafı routing; I5: rol→katalog çözümü, model adı gömülü değil; grounding kapısı: kanıtsız iddia %100 red. |
| **omni-scheduler** | Çok-ajan: tier modeli, kaynak valisi, bütçe zarfı, interrupt, fan-out tavanları | Rust | deepseek: continuable subagent + kontrol tool seti (send_message/interrupt_agent/list_agents/job_*) + typed sonuç; oh-my-pi: izole worktree fan-out; codex: spawn grafı DB'de restore | OmniSchedulerBackend (admission/kuyruk/depth, 1247 satır) + tier modeli (Existing→Sleeping→Queued→Active) | Derinlik tavanı 5, fan-out tavanı 8; çalışan görev asla düşürülmez (K2); aktif slot sayısı kaynak valisinde, donanıma göre dinamik. |
| **omni-mcp** | MCP yaşam döngüsü: status machine, canlı refresh, auth, credential store | Rust — **artifact dondur** + katman | opencode: 5 durum + ToolListChanged canlı refresh + alt süreç temizliği; crush: generation-bazlı stale-connect koruması | xai-grok-mcp (rmcp 2.1 karantina + OAuth dedup + credential store + liveness) | Status machine + reconcile eklenir; OAuth PKCE sonraya (client-credentials başla); tünel/proxy YOK — trafik asla kendi sunucundan geçmez. |
| **omni-control** | Tek kontrol API'si: auth, SSE/WS yayını, komut girişi, interactionQuery | Rust | codex: UI/engine protokol ayrımı (Op/Event, transport-agnostik); cursor: interactionQuery side-channel (PreToolUse/PostToolUse onayı tek stream'de) | omni-control + 127.0.0.1:9876 + auth token (argon2) + ui_parity kapısı | Tek API, iki yüz (TUI+WebUI) aynı event stream'ini tüketir; permission akışı stream içinde protokol seviyesinde. |
| **omni-tui** | Blok-bilinçli TUI: BlockList, streaming markdown, diff view, flow paneli | Rust | warp: BlockList + SumTree + FlatStorage + zero-height gizleme; crush: safe-boundary markdown render + section FNV cache + tema; CC dersi: React/Ink YOK | xai-grok-pager (Elm Action→Effect, omni_bridge dikişi, vim/minimal/headless modlar) | Pager katmanlanır: `xai-grok-pager-render` artifact; telemetry/announcements/marketplace/voice feature-gate; mermaid/resvg kararı: kalırsa bütün halinde dondur, yoksa zincir düşer; 10–20 MB sınıfı. |
| **omni-notify** | Bildirim kanalları: telegram, webhook, SMS, çağrı; dedup/susturma; fail-soft | Rust | grok-cli kendi; warp dersi: komut içeriği telemetry'ye GİRMEZ | Flow Governor notify.rs + notify.toml + NotifyDispatcher | Kanal hatası akışı düşürmez; SMS/çağrı yalnız yüksek-önem eşiğinde (K9); tümü `enabled=false` varsayılan. |
| **omni-record** | Trajectory log + replay; tetiklemeli medya | Rust | deepseek: append-only trajectory + `session_event_read/search/trace` tool'ları (ajan kendi geçmişini okuyabilir) | flow_events.jsonl + events.jsonl + session kaydı | Kill sonrası replay kapısı; kayıt aynı CAS'ta, medya tetiklemeli (AS6). |
| **omni-bench** | Faz kapısı ölçümü: cold-start, shutdown, RSS; I1 komut+metrik+eşik | **Zig** | aider: SWE-bench/polyglot grid fikri (koşum omnitrix'e özgü); grok-cli: I1 disiplini + kapı testleri; deepseek: verify-tool-catalog | omni-bench + tests/* (crash_recovery, diff_visibility, ui_parity, grounding_redteam, tool_allowlist_redteam, provider_fallback, multiagent_fanout, interrupt_granularity) | Eşikler: cold <400ms / sıcak <100ms, shutdown <50ms, RSS <250MB, 100 ajan stabil; Soak 7/24: müdahale gerektiren hata = 0. |
| **omni-memory** | Basit bellek: önemli kararlar/todo'lar; frozen-snapshot enjeksiyon; oturum-sonrası arka plan yazımı | Rust | hermes: **frozen-snapshot memory** (MEMORY/USER dosyaları session başında donmuş enjekte, session içi yazım diske anında ama prompt'a dokunmaz → prefix cache korunur) + nudge sayacı + skill-creation nudge; codex: 2 fazlı pipeline fikri (rollout → konsolidasyon) — lease yerine dosya kilidi; konsolidasyon gece/çapraz-oturum işi | xai-grok-tools memory tool (sqlite) | Her oturumda LLM çağrısı yok; secret redaction; olay-güdümlü, küçük; review fork'u ayrı ajan değil düşük maliyetli modelle kendi tool'ları üzerinden (hermes'in ~30K token/event maliyeti alınmaz). |

---

## 3. xai-* temelinden: ARTIFACT olarak dondurulanlar (build-once, dokunma)

| Crate | Neden dondurulur |
|---|---|
| `xai-grok-sampler` + `xai-grok-sampling-types` (~15K) | 3 katmanlı actor + router engine + 401 attribution — yeniden yazılamaz kalitede, shell'den ayrık |
| `xai-grok-mcp` (~11K) | rmcp 2.1 karantina stratejisi + OAuth dedup + credential store |
| `xai-grok-config` (~11K) | Ed25519 imzalı requirements + fail-closed + TOML deep-merge |
| `xai-grok-sandbox` (~6.4K) | nono pin'i + gerekçesi; CI'da ek güvenlik hattı olarak kalır — **çalışma modeline girmez (K5)** |
| `xai-omni-keychain` (~5K) | Zaten omnitrix malı; `crates/common`'a taşınır |
| `xai-tool-types` / `xai-tool-protocol` / `xai-tool-runtime` | Wire/tip katmanı |
| `xai-grok-agent` (~22K) | Taşınabilir Agent tipi, host-bağımsız |
| `xai-grok-pager-render` | Presentation primitives katmanı (appearance/clipboard/theme/syntax/terminal) |
| `xai-grok-shell/src/session/flow/*` (Flow Governor ~2.5K) | Deterministik akış: universal/commit/direct, sınıflandırıcı, judge, parallel, notify — TAM teslim, korunur |

## 4. xai-* temelinden: ATILANLAR (silme, patch değil)

| Parça | Kader |
|---|---|
| `xai-grok-telemetry`, `xai-mixpanel`, `xai-grok-secrets`, `xai-grok-update`, `xai-grok-announcements`, `campaigns` | **SİL** — no-telemetry patch'leri gereksizleşir (kod yoksa patch'e gerek yok); I2a muafiyeti kalkar, I8 gate kalır |
| `opentelemetry*`, `fastrace*`, `tonic`, `prometheus`, `pprof`, `obfstr`, `tracing-opentelemetry` | Workspace'ten çıkar |
| `gcloud-storage`, `upload/`, `export_github`, `hub_server`, `diag_server`, `preview_supervisor`, `daemonize` | Server/telemetry ekseni — omnitrix kendi runtime'ına devralır ya da silinir |
| `xai-grok-shell/src/sampling/` (eski ikiz) | SİL — sampler'a taşınmış katmanın kopyası |
| Git ikiliği: `gix` (3 crate) vs `git2` vendored (5 crate) | **Tek implementasyon**: git2 kalır (5 crate zaten aktif); gix hattından `gix-status`/`hunk-tracker` üç crate dondurulur, gerisi düşer |
| `mermaid-to-svg` + `dagre_rust` + `graphlib_rust` + `resvg`/`fontdb`/`tiny-skia` | TUI'de mermaid kararına bağlı: yoksa zincir birlikte düşer (en pahalı render hattı) |
| `pdf_oxide` (PDF), `computer` (computer-use) | İsteğe bağlı feature'a çekilir; `research_tool` web ağı omnitrix search'e bağlanır |
| `syntect`, `alacritty_terminal`/`ptyctl`, `termwiz` (ratatui-inline), dev araçları (`dhat`, `criterion`, `wiremock`, `mockito`, `insta`, `serial_test`) | Kırpılır veya feature-gate |
| Versiyon kaosu (0.2.112 / 0.1.220-alpha.4 / 0.1.0) | Tek versiyon hattı: 0.1.0 + git describe |

## 5. Kullanıcı varlıkları: korunma durumu

| Varlık | Durum | Not |
|---|---|---|
| **Keychain** (xai-omni-keychain) | KORUNUR | AES-256-GCM + Argon2id + zeroize + 15dk TTL + OS keyring + stack sync; `crates/common`'a taşınır |
| **Routing** (60+ mod, 7 aile) | KORUNUR (artifact) | RouterEngine + routing_modes.toml + selector plug-in mimarisi; JEP rolleri config'te (I5) |
| **Flow Governor** | KORUNUR (TAM teslim) | universal/commit/direct akışları + yargıç + çoklu-ajan execute + notify; AI inisiyatifi yok (D2) |
| **Personas** (9 dolu → 61 hedef) | KORUNUR | `config/personas/*.toml` + `prompts/*.md`; hot-reload (fsnotify); yazma yetkisi yalnız executor/quick_fix; içerik kullanıcı tarafından doldurulur |
| **Feeder** (verifier.db → live/dead-hold) | KORUNUR | read-only SQLite + detect + probe; `// NOT: sütun geçici` işareti; dead'ler silinmez, revive poll |
| **models.dev client** | KORUNUR | 24s TTL cache + bayat-fallback zinciri; npm→ApiBackend eşleme |
| **DiffShimFs + CAS diff akışı** | KORUNUR | pre_ref/post_ref + file_touches + StateEvent::FileTouched |
| **Grounding + yanlışlamacı yargıç** | KORUNUR | üreten ≠ doğrulayan; ihlal → interrupt + trust düşüşü → karantina |
| **No-telemetry** | EVRİLİR | patch'lerden **kod silmeye** geçiş; I8 gate kalıcı (6 fonksiyon literal assert) |
| **OmniSchedulerBackend** | KORUNUR | admission/kuyruk/depth; tier geçişleri; bütçe zarfı |

## 6. Ölçek notu (100 → 10.000 ajan, trilyon satır kod)

- Aktif ≠ var-olan: `Existing` (~0 RAM, DB satırı) → `Sleeping` (~200B metadata, bağlam CAS'ta) → `Queued` → `Active` (~200KB bağlam). 10.000 var-olan, N aktif — N donanıma göre, kod tavanlamaz.
- Trilyon satır ölçeği: repo map ve kod-öğrenme **kalıcı sembol grafiği tutmaz** (kullanıcı reddetti) — soru-güdümlü kanıt getirme + hashline anchor atıfı; scan'ler bounded (derinlik/entry limitleri, codex disiplini).
- Soğuk başlatma bandı (1s) her komutta; sıcak cache/daemon yasak — hızın kaynağı yerel indeksler + Zig native'ler + lazy-init, arka plan işleri değil.

*Kaynaklar: 01–05 + grok-cli RAPOR (crate kimliği/dondurma/atma), 03-git-feature-envanteri (kullanıcı işi 89 commit), 04-config-persona-envanteri (varlıklar).*
