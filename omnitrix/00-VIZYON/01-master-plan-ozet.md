# Omnitrix — Master Plan Özeti (Vizyon Ekstraktı)

> **Kaynak:** `MASTER-PLAN.md` (780 satır) + `ALPHA-PLAN.md` (378 satır).
> Bu dosyalar repo kökünde artık yok — `6d6ee1d` commit'i ("refactor: omni-* katmani tamamen kaldirildi — tek urun, tek kod tabani", 2026-08-07) ile silindiler; içerik `6d6ee1d~1`'den kurtarıldı.
> **Güncel durum notu:** Planın uygulanmasından sonra kullanıcı "matrix içinde matrix yok" emriyle ayrı `omni-*` katmanını kaldırttı; mantık `xai-grok-*` içine taşındı. Aşağıdaki özet **plan döneminin vizyonunu** eksiksiz yansıtır; mimari isimlendirme güncel koddan farklı olabilir.
> Tarih: 2026-07-26 · Branch: `masterplan` · Rust 1.92.0 · Hedef: x86_64/aarch64-unknown-linux-gnu

---

## 1. VİZYON CÜMLELERİ (NE / NEDEN / ÖLÇEK)

- **Ne:** Omnitrix = **orkestrasyon katmanıdır.** Tek-ajan çalışma zamanını, LLM taşımasını ve tool sistemini `xai-*` (vendored) sağlar; omnitrix üzerine **çok-ajan planlama, yönlendirme, persona, sağlayıcı-besleme, uzak erişim ve dayanıklılık** ekler. Sıfırdan ajan yazmaz.
- **Neden:** Mevcut `ork-*` kodu, olgun `xai-*` altyapısını kullanmak yerine ona **paralel ikinci bir evren** kurmuştu (17 deklare, 3 kullanım — Bulgu B2); hiçbir LLM çağrısı yoktu (B3), ana binary hiç derlenmiyordu (B1), daemon sahte veri gösteriyordu (B4). Plan bu kök sorunu tersine çevirir.
- **Ölçek hedefi (K1):** İlk hedef **100 ajan gerçek zamanlı eşzamanlı**; nihai hedef **10.000 eşzamanlı**. Aktif ajan sayısı donanımın verdiği kadardır, kod tavanlamaz, dinamik yönetir.
- **İsim (K15):** "aklına gelen her şeyi yapabilen, all-in-one araç" — tek binary `omnitrix` + iç crate'ler `omni-*`.
- **Merkezi ayrım:** `omni-*` → `xai-*`'a bağımlıdır, tersi değil; `xai-*` **düzenlenmez** (vendored, `SOURCE_REV` ile senkronlu, invariant I2).

---

## 2. KULLANICI KARARLARI (K1–K15, tartışılmaz veri)

| # | Konu | Karar |
|---|---|---|
| K1 | Ölçek | 100 → 10.000 eşzamanlı; dinamik, donanıma göre |
| K2 | RAM | **İş kutsaldır.** 250MB hedefi geçersiz; darboğazda swap-out/kuyruklama ama iş asla yarıda kesilmez. Hız da RAM de önemli |
| K3 | Shell | Allowlist'li shell + acil kaçış (yargıç onaylı, loglu); grep/sed/cat/awk/eof-hilesi reddedilir |
| K4 | Eski kod | "İşe yarayanı tut, yaramayanı yok et" (TUT/YENİDEN-YAZ/YOK-ET) |
| K5 | Sandbox | **SANDBOX YOK** (seccomp/bwrap/landlock yasak). İzolasyon = geçici git branch. Yetki tek noktada: tool broker |
| K6 | Platform | Linux — Wayland **ve** X11 |
| K7 | UI'lar | İki bağımsız istemci (TUI+WebUI), ikisi daemon'a bağlanır, tam yetkili; **çekirdek tek, yüzler iki** |
| K8 | WebUI | Sunucu-taraflı HTML + SSE/WebSocket (JS build zinciri yok, telefonda çalışır) |
| K9 | Uzak erişim | Public IPv6 + Tailscale + Telegram + WhatsApp/SMS/arama — tek control-plane API, auth **zorunlu** |
| K10 | Sonlanma | Hibrit: otomatik doğrulama + yanlışlamacı yargıç + kullanıcı onayı |
| K11 | Bütçe | Tavan yok, gerçekten sonsuz iterasyon; ama iş bitince durur (bitmiş işe kaynak yakmak yasak). İki mod: tam-otonom / kullanıcı-odaklı |
| K12 | Persona | Her persona diskte bağımsız dosya; derleme yok, çalışırken eklenebilir; ~60 persona |
| K13 | Anahtar besleme | Kaynak-agnostik (harici SQLite → canlılık → canlı/ölü ayrımı → provider'a ekle); kapsam sınırı: **üçüncü-taraf sızmış anahtar hattı kurulmaz** |
| K14 | Araştırma | Aşamalı: önce MCP (Firecrawl/Exa) + anti-detect, sonra native crawler; sağlayıcı-değiştirilebilir |
| K15 | İsim | **omnitrix**; `ork-*` → `omni-*` |

---

## 3. OMNI-* MODÜLLERİ (19) VE SORUMLULUKLARI (MASTER-PLAN 3.2)

| Crate | Sorumluluk | Eski karşılık / kader |
|---|---|---|
| `omni-proto` | Kanonik ortak durum modeli, olay+komut tipleri — **tek kaynak (K7/I3)** | yeni |
| `omni-core` | Domain tipleri, ajan/görev durum makinesi, orkestrasyon çekirdeği | ork-runtime (kısmı) / YENİDEN YAZ |
| `omni-storage` | Kalıcılık: **CAS**, **WAL**, SQLite/redb, tek-writer aktör, checkpoint | ork-storage / **TUT** |
| `omni-provider` | Sağlayıcı tespiti (detection), keyring, health, **anahtar besleme (ingestion)** | ork-provider / **TUT** |
| `omni-scheduler` | Çok-ajan zamanlayıcı: `impl SubagentBackend`, **tier modeli**, kaynak valisi, interrupt, bütçe | ork-runtime / YENİDEN YAZ |
| `omni-agent` | `xai_grok_agent::Agent` sarmalayıcı; persona → `AgentDefinition` | yeni (ince) |
| `omni-router` | Yönlendirme: round/fallback/**JEP** + grounding; gerçek `SamplerHandle`'a bağlı | ork-router / YENİDEN YAZ |
| `omni-tools` | Tool broker (K3), diff-stream fs-shim, kod-öğrenme (AS1), edit/search kablolaması | yeni |
| `omni-config` | Katmanlı config: env > DB (`config_kv`) > dosya (AS8) | ork-runtime config / YENİ |
| `omni-control` | Tek kontrol düzlemi API: token auth, SSE/WS durum yayını, komut girişi | ork-webui (kısmı) / YENİDEN YAZ |
| `omni-webui` | SSR HTML (maud) + SSE/WS yüzü; JS derleme zinciri yok | ork-webui / YENİDEN YAZ |
| `omni-notify` | Bildirim/webhook: kanallar, dedup, politika, tetikleyici | ork-notify / **TUT** (genişlet) |
| `omni-research` | Sağlayıcı-değiştirilebilir araştırma motoru (surface/deep/ocean) | yeni |
| `omni-record` | Oturum kaydı, event-log + tekrar oynatma; tetiklemeli medya (AS6) | ork-record / YENİDEN YAZ (geç faz) |
| `omni-backup` | Yedekleme 3-2-1, doğru **SigV4** + istemci-taraflı şifreleme (AS10) | ork-backup / YENİDEN YAZ |
| `omni-tests` | Entegrasyon/kapı test koşum takımı (tests/*.rs) | yeni |
| `omni-bench` | Faz kapısı ölçüm aracı: cold-start, shutdown, RSS — komut+metrik+eşik (I1) | yeni |
| `omnitrix` (bin) | Lazy-init entrypoint + alt-komutlar | ork-daemon / YENİDEN YAZ |

YOK-EDİLENLER: bağımsız `target/` dizinleri, elle yazılmış sürüm bağımlılıkları, imzasız S3 PUT, `sandbox_profile` kolonu.

---

## 4. İNVARİANTLAR VE KAPILAR

| # | İnvariant | Zorlama noktası |
|---|---|---|
| I1 | Her faz kapısı = komut + metrik + eşik | CI + `omni-bench` + tests varlığı |
| I2 | `omni-*` → `xai-*` tek yön; `xai-*` düzenlenmez (diff = 0; deklare edilen her xai-* bağımlılığı fiilen kullanılır) | CI `use xai_` sayım kapısı |
| I3 | Tek ortak durum kaynağı; iki yüz ondan türer | `omni-proto` tek tanım |
| I4 | Sandbox yok; yetki tek noktada (tool broker) | CI yasak sembol taraması |
| I5 | Model ismi/fiyatı gömülü değil; katalogdan | CI literal model-adı taraması |
| I6 | Üretim yolunda `unwrap`/`expect`/`panic!` = 0 | `clippy -D warnings` |
| I7 | Her yan etkili işlem önce WAL niyet kaydı | `write_journal` şeması |
| I8 | Her olgusal iddia kanıt referanslı | grounding kapısı |
| IP | `Path::canonicalize` yasak, `dunce::canonicalize` | clippy disallowed-methods + tarama |

---

## 5. FAZLAR, HEDEFLER, METRİKLER, EŞİKLER

| Faz | Kapsam | Kabul kapısı (komut + metrik + eşik) |
|---|---|---|
| **Faz 0** | `ork-*`→`omni-*` yeniden adlandırma; yeşil workspace; sandbox temizliği | `omnitrix --version` çalışır · `cargo check --all-targets --workspace` yeşil · clippy temiz · xai-* diff=0 (I2) · yasak sembol=0 (I4) |
| **Faz 1** | **Dikey dilim:** tek görev, tek ajan, uçtan uca — anahtar gir→detect→LLM turu→tool'lar→diff akışı | cold-start < **100ms sıcak / 400ms soğuk** (hyperfine) · SIGINT→exit < **50ms** · crash_recovery yeşil · diff_visibility yeşil · RSS ölçülür |
| **Faz 2** | `omni-proto` ortak durum + `omni-control` SSE/WS+auth + `omni-webui` | `ui_parity.rs`: TUI komutu WebUI akışında bit-eş görünür · kimliksiz istek 401 |
| **Faz 3** | Çok-ajan scheduler `impl SubagentBackend`; tier makinesi; kaynak valisi; AS3/AS4 | `multiagent_fanout.rs`: **100 aktif ajan** gerçek çağrıda stabil, RSS < eşik · kaynak-kısıtlı testte **tüm görevler tamamlanır** (iş kutsal) |
| **Faz 4** | Router modları + JEP + grounding (yanlışlamacı yargıç, kanıt zorunluluğu) | `provider_fallback.rs` · `grounding_redteam.rs`: kanıtsız iddia **%100 red**, sızma 0 · I5 literal taraması 0 |
| **Faz 5** | Persona şema + hot-reload; allowlist broker; shell parser + acil kaçış | `tool_allowlist_redteam.rs`: yasak araç %100 red + log, sızma 0 · yeni persona derlemesiz yüklenir |
| **Faz 6** | Uzak erişim: IPv6/Tailscale/Telegram/Twilio tek API'de; bildirim + susturma | dört kanaldan komut+bildirim çalışır · SMS/arama yalnız yüksek-önem eşiğinde · auth her kanalda |
| **Faz 7** | Araştırma motoru (sağlayıcı-değiştirilebilir, 3 mod) | sonuç `research_findings`'e; sağlayıcı değişince çekirdek değişmez |
| **Faz 8** | Anahtar besleme hattı (kaynak-agnostik) | kullanıcının kendi anahtar DB'sinden uçtan uca besleme · ölü anahtar ayrı bölümde · sızmış-anahtar hattı **yok** |
| **Faz 9** | Kayıt (event-log + tetiklemeli medya) + 3-2-1 yedek (SigV4 + şifreleme) | kill sonrası event-log replay · restore doğrulanır · şifre anahtarı yedeğin **dışında** (OS keyring) |
| **Faz 10** | Computer-use (Wayland+X11) + self-hosting (kapılı) + tam-otonom döngü | computer-use dokunuşu diff akışında + geri alınabilir · self-modify kullanıcı onaysız merge etmez · sonlanma oracle'ı bitmiş işte durur |
| **Soak** | 7/24 | 7 gün kesintisiz; müdahale gerektiren hata = 0; bellek/FD sızıntısı yok |

**Eşikler özeti:** cold-start 100/400ms · shutdown 50ms (veri hacminden bağımsız, 100GB yazma altında) · 100 ajan stabil + RSS eşik · derinlik tavanı **5**, fan-out tavanı **8** · 10.000 var-olan, N aktif (dinamik).

---

## 6. FEATURE HARİTASI

- **Routing modları:** `round_robin`/`weighted` (yük dağıtımı), `fallback` (hata/bakiye → sıradaki canlı), `jep` (Judge-Executor-Planner rolleri config'te; model adları gömülmez). Router her çağrıda `provider_health` okur, `xai-circuit-breaker` ile düşenleri zincirden çıkarır.
- **Persona sistemi:** `config/personas/*.toml` — şema: name, system_prompt (dosya ref), role (JEP), temperature, **tool allowlist/disallowed**, budget, routing, max_depth, recording. `xai-fsnotify` ile hot-reload; ~60 persona kullanıcı tarafından doldurulur.
- **WAL + CAS (crash-only):** her yan etkili işlem önce `write_journal`'a idempotent niyet (`op_id` UNIQUE, applied=0) → işlem → applied=1. Ctrl+C/SIGKILL = flush yok, kapanış **O(1)**; açılışta replay. CAS içerik-adresli, immutable, dedup, refcount GC. DB yalnız `*_ref` tutar.
- **Tier modeli (aktif ≠ var-olan):** `Existing` (~0 RAM, DB satırı) → `Sleeping` (~200B metadata, bağlam CAS'ta, compaction ile sıkıştırılır) → `Queued` (RAM'de hazır) → `Active` (~200KB bağlam + 30-60MB soket/TLS). Geçişleri kaynak valisi yönetir; **asla çalışan görev düşürülmez** (K2).
- **Provider besleme (ingestion):** harici SQLite → fsnotify/poll → iki kademeli canlılık (`/models` ucuz → şüpheliyse 1-token completion kesin) → canlı/ölü ayrımı (ölüler `status='dead'`, ileride canlanabilir) → doğru provider'a ekle.
- **Grounding ("AI yalan söylemesin"):** kanıt zorunluluğu (her iddia tool çıktısı aralığına ref), üreten ≠ doğrulayan ajan (farklı model), yanlışlamacı yargıç ("çürütebilir miyim?"), ihlal → interrupt + trust düşüşü + tekrarında karantina.
- **Sonlanma oracle'ı (K10/R6):** Done ancak 1) otomatik doğrulama (build+test+lint) 2) yargıç onayı 3) kullanıcı onayı. "İş bitti" → alt-ajan topla + bütçe kes.
- **Problem-süre sistemi (AS13):** kural tabanlı skorer (AI değil) — görev sınıfı + kullanıcı bayrağı + kapsam sinyalleri (bilinmeyen sayısı, tahmini dosya) + geçmiş istatistik → `duration_target` (mvp|full) + iterasyon zarfı; deterministik, kullanıcı geçersiz kılabilir.
- **WebUI:** axum + SSR (maud/askama) + SSE birincil/WS çift yön; HTMX opsiyonel. WebUI değişiklikleri `config_kv`'ye (DB katmanı) anında etkili, opsiyonel dosyaya dışa aktar.
- **Notify:** görev bitimi + kritik hata + insan-onayı interrupt → kanallar; tekrar eden olayda dedup/susturma penceresi.
- **Research motoru:** yüzeysel/derin/okyanus; çıktı JSON kanonik + Markdown türetilmiş (`research_findings`); `xai-grok-mcp` ile Firecrawl/Exa, ileride native crawler.
- **Diff görünürlüğü:** `AgentBuilder::with_fs` üstünden path-agnostik FS-shim; her yazım öncesi/sonrası CAS `pre_ref`/`post_ref`, hunk `xai-hunk-tracker`, repo `xai-gix-status` → `file_touches` + `StateEvent::FileTouched`; çalışma dizini dışı dokunuşlar da görünür.
- **Edit tool:** hashline `AnchorScheme` — satır başına hash anchor, `find_shifted` drift-tolerant, `validate` modele düzeltilebilir geri bildirim; sed'e kaçış gerekmez.
- **Arama tool:** `search_tool` + `ToolSearchIndex` semantik — K3 grep'i yasakladığı için **birincil** yoldur.
- **Kod öğrenme (AS1):** soru-güdümlü kanıt getirme — minimal span'lar + "neden ilgili" + hashline anchor atıfı (retrieval→edit zinciri); ham dosya okuma yok, kalıcı sembol grafiği yok (kullanıcı iki yolu da reddetti).
- **Interrupt (AS2):** yarım tool = crash-only niyet (applied=0) → idempotent tamamla ya da iptal işaretle; bağlam CAS'ta; ceza hem trust hem system-reminder; iptal `SamplerHandle` abort + `SubagentBackend::cancel`.
- **Panic izolasyonu (AS5):** tool'lar per-ajan supervisor'da `catch_unwind`; daemon `panic="unwind"`; tehlikeli native için K5 geçici git branch; `xai-crash-handler`.
- **Computer-use (AS12):** `xai-computer-hub-core/sdk/mcp-adapter` genişletilir (Wayland+X11); boşlukta RustAutoGUI/ComputerUse-rs.
- **Self-hosting (AS11):** zorunlu geçici branch; `self_modify` yetkisi kullanıcı onaylı + `capability_audit`; aday build üretilir, **otomatik merge yok**; geri alma = git.
- **Bütçe (AS4):** ebeveyn zarf tahsis eder, çocuk ebeveyn kalanından harcar; kök ∞ (otonom) / sonlu (kullanıcı); iş bitince kesilir.
- **Gözlemlenebilirlik (AS9):** tracing + fastrace + opsiyonel Prometheus; RAM'e göre örnekleme, low profile'da kapalı.
- **Yedek (AS10):** 3-2-1 yerel+bulut; doğru SigV4; istemci-taraflı chacha20poly1305/age; şifre anahtarı OS keyring'de, asla yedeğin içinde değil.

---

## 7. DENETİM BULGULARI (B1–B6) VE RİSK KAYDI (R1–R8)

**Bulgular:** B1 ana binary derlenmiyor (ratatui/crossterm deklare edilmemiş) · B2 kök sorun: 17 deklare xai-* bağımlılığı, 3 kullanım — paralel evren · B3 hiçbir LLM çağrısı yok (ork-router 2.570 satırı modele bağsız) · B4 daemon sahte veri gösteriyor · B5 TUI/WebUI ortak modeli yok · B6 karakteristik: deprecated API, bağımsız target/, imzasız S3 PUT.

**Riskler:** R1 geniş iskelet/çalışan akış yok · R2 sahte xai-* entegrasyonu tekrarı · R3 UI ortak durumu bozulur · R4 kod-öğrenme tatmin etmez · R5 sandbox geri sızar · R6 sonsuz iterasyon durmaz · R7 computer-use hasar · R8 kapsam büyür.

---

## 8. TEKNOLOJİ SEÇİMLERİ (özet)

Rust 1.92.0 + tokio (multi-thread) · LLM taşıma `xai-grok-sampler` (OpenAI+Responses+Anthropic tek çatı) · ajan `xai-grok-agent` · depo SQLite (WAL) + CAS `redb` · TUI ratatui+crossterm · WebUI axum+maud+SSE/WS · auth token (argon2) + opsiyonel mTLS · git `gix` · bellek `jemalloc` · config TOML katmanlı · wire JSON (dış) + postcard/bincode (CAS-içi) · **reddedilen:** Leptos/WASM, harici computer-use, sandbox.

---

## 9. ÇIKARILAN DERS (kullanıcı vizyonunun özü)

> Önceki deneme başarısızlığının sebebi mimari zarafet eksikliği değil, **her fazın çalıştırılabilir ve ölçülebilir kabul kapısına bağlanmaması**ydı. Omnitrix'in vizyonu: "dikey dilim önce, ölçümle, iş kutsal, sandbox yok, sıfırdan yazma — inşa et, fork etme."
