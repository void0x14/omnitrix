# ALPHA-PLAN.md

## Omnitrix — Gereksinim Netleştirme, Kod Denetimi ve MASTER-PLAN Girdisi

> **Bu belge nedir:** Kullanıcıyla soru-cevap yoluyla netleştirilmiş kararların kaydı + mevcut kod tabanının kanıta dayalı denetimi + fizik/gerçeklik çakışmalarının çözümü + MASTER-PLAN'ı yazacak modele verilecek talimat.
>
> **Bu belge ne değildir:** Mimari plan değildir, kod içermez, implementasyon yol haritası değildir.
>
> **Akış:** `ALPHA-PLAN.md` (bu belge, kararlar netleşti) → `MASTER-PLAN.md` (başka model, mimari) → implementasyon.
>
> Tarih: 2026-07-26 · Depo: `/home/void0x14/Documents/omnitrix` · Branch: `masterplan` · Proje adı: **omnitrix** (kesinleşti)

---

# BÖLÜM 0 — NETLEŞEN KARARLAR (kullanıcı onaylı)

Bu tablo, soru-cevap turlarında **kullanıcının verdiği** kararların kaydıdır. MASTER-PLAN bunları veri olarak alır, yeniden tartışmaz.

| # | Konu | KARAR |
|---|------|-------|
| K1 | Ölçek | **İlk hedef: 100 ajan gerçek zamanlı eşzamanlı.** Nihai hedef: 10.000 eşzamanlı. Kaynak elverdikçe büyür. Aktif ajan sayısı = donanımın verdiği kadar; kod bunu tavanlamaz, dinamik yönetir. |
| K2 | RAM | **İş kutsaldır.** 250MB bir hedef değil, artık geçersiz. RAM dinamik yönetilir: darboğaza girilince sistem davranışını ayarlar (swap-out, kuyruklama) ama **işi asla yarıda bırakmaz.** Hız da RAM de önemli — ikisi birden optimize edilir, biri diğeri için feda edilmez. |
| K3 | Shell politikası | **Allowlist'li shell + acil kaçış.** Normalde yalnızca izinli komutlar geçer (`cargo`, `git`, `npm`, `pytest`...); `grep`/`sed`/`cat`/`awk`/`eof`-hilesi reddedilir. Native tool %99 çalışmaz durumdaysa ajan gerekçe yazıp geçici serbest shell isteyebilir; loglanır, yargıç onaylar. |
| K4 | Mevcut `crates/ork/` kodu | **Adam gibi düzelt: işe yarayanı tut, yaramayanı yok et.** Kanıta dayalı ayrım Bölüm 2'de. |
| K5 | Sandbox | **SANDBOX YOK.** Kullanıcı bunu hiç istemedi; önceki plandan yanlışlıkla taşınmıştı. Sandbox iş düşmanı sayılıyor. İzolasyon gerekirse yalnızca geçici git branch ile (o da uzun vadede kaldırılacak). Yetki zorlaması **tek noktada**: tool katmanı. |
| K6 | Platform | **Linux — hem Wayland hem X11.** macOS/Windows kapsam dışı (şimdilik). |
| K7 | TUI/WebUI | **İki bağımsız istemci, ikisi de daemon'a bağlanır, ikisi de tam yetkili.** Aynı anda çalıştırma zorunluluğu yok; kullanıcı hangisini isterse onu açar. İkisi de aynı çekirdek daemon durumunu okur/yazar (çekirdek tek, yüzler iki). Bu, iki UI'ın *state kaynağı* ortaktır demek — görünümleri farklı olabilir. |
| K8 | WebUI teknolojisi | **Sunucu-taraflı HTML + SSE/WebSocket** (kullanıcı kararı bize bıraktı). Gerekçe: JS build zinciri yok, cold-start en hızlı, telefonda çalışır, megapol ölçeğinde push modeli zaten doğru. İleride zengin istemci çekirdeğe dokunmadan eklenebilir. |
| K9 | Uzak erişim | **Public IPv6 (doğrudan) + Tailscale + Telegram bot + WhatsApp/SMS/arama** — dördü paralel kanal, tek control-plane API üstünde. Public IPv6 açık olduğu için **auth zorunlu** (opsiyonel değil). |
| K10 | Sonlanma koşulu | Görev **ancak** uçtan uca çalışıyorsa + her fonksiyon/özellik/nokta çalışıyorsa + kritik zafiyet/bug/iş-akışı-mantık-hatası yoksa biter. Karar hibrit: **yargıç + kullanıcı**. Döngü mühendisliği bunu garanti eder. |
| K11 | Bütçe | **Tavan yok, gerçekten sonsuz iterasyon** — görev bitene kadar. Fakat iş bitince durur; bitmiş işe kaynak yakmak yasak. İki mod: tam-otonom (pahalı, sonsuz iterasyon) ve kullanıcı-odaklı (bütçe-dostu, kullanıcı yönlendirir). |
| K12 | Persona | **Her persona tam bağımsız tanım, diskte dosya olarak** (TOML/Markdown). Yeni persona = yeni dosya, derleme yok, çalışırken eklenebilir. ~60 persona; sayı büyüyecek. |
| K13 | Anahtar kaynağı | Dış SQLite'tan besleme **kaynak-agnostik** tasarlanır (harici DB → canlılık kontrolü → canlı/ölü ayrımı → doğru provider'a ekleme). Anahtarın meşruiyeti kullanıcının sorumluluğudur. **Sınır:** üçüncü-tarafa ait canlı anahtarları kullanan hat plana konmaz (Bölüm 6.3). |
| K14 | Araştırma altyapısı | **Aşamalı:** başta hazır arama/crawl MCP'leri + anti-detect araçlar; uzun vadede kendi native crawler'ımız. Moda göre karışım (yüzeysel/derin/okyanus). |
| K15 | İsim | **omnitrix** — "aklına gelen her şeyi yapabilen, all-in-one araç" anlamında. `ork-*` isimleri buna göre yeniden adlandırılacak (Bölüm 2.4). |

---

# BÖLÜM 1 — MEVCUT KOD TABANI DENETİMİ (KANIT ODAKLI)

## 1.1 Ne var?

| Katman | Crate | LOC | Derleniyor mu? |
|---|---|---|---|
| Miras (omnitrix/xAI Grok CLI) | `crates/codegen/xai-*`, `crates/common/xai-*` | 80+ crate | Evet, üretim kalitesi |
| Yeni orkestrasyon | `ork-record` | 3.905 | Evet |
| | `ork-runtime` | 2.575 | Evet |
| | `ork-router` | 2.570 | Evet |
| | `ork-storage` | 1.389 | Evet |
| | `ork-provider` | 1.162 | Evet |
| | `ork-daemon` | 411 | **HAYIR** |
| | `ork-tui` | 354 | Evet |
| | `ork-backup` | 302 | Evet |
| | `ork-webui` | 245 | Evet |
| | `ork-notify` | 139 | Evet |

## 1.2 Bulgu B1 — Ana binary derlenmiyor (BLOKLAYICI)

`cargo check -p ork-daemon`:
```
ork-daemon/src/main.rs:6: error[E0433]: cannot find module or crate `ratatui`
ork-daemon/src/main.rs:6: error[E0432]: unresolved import `ratatui`
ork-daemon/src/main.rs:49: error[E0599]: no method named `execute` found for `std::io::Stdout`
error: could not compile `ork-daemon` (bin "orkd") due to 4 previous errors
```
`ork-daemon/Cargo.toml`, `ratatui`/`crossterm`'i deklare etmemiş; `main.rs` ikisini de kullanıyor. **Bu binary hiçbir zaman derlenmedi.** "Hata vermeyen sistem" hedefi ilk adımda ihlal.

## 1.3 Bulgu B2 — "Omnitrix üzerine inşa" fiilen gerçekleşmemiş (KÖK SORUN)

Kanıt — tüm `crates/ork/` ağacında `xai_*` **kullanım** noktası: **3**
```
ork-runtime/src/managed_agent.rs:10  use xai_chat_state::ChatStateHandle;
ork-runtime/src/managed_agent.rs:11  use xai_grok_agent::Agent;
ork-storage/src/wal.rs:4             use xai_sqlite_journal::JournalMode;
```
Buna karşılık `Cargo.toml`'larda **deklare edilmiş** `xai-*` bağımlılığı: **17**. `ork-runtime` 8 tane yazmış, 2 kullanmış. `xai-grok-tools`, `xai-tool-runtime`, `xai-grok-memory`, `xai-grok-subagent-resolution` — deklare, sıfır kullanım.

**"Karmancorman oldu" hissinin ana sebebi budur:** Mevcut altyapıyı genişletme değil, ona paralel ikinci bir evren kurma yapılmış. İki sistem yan yana, birbirine bağlı değil.

## 1.4 Bulgu B3 — Hiçbir LLM çağrısı yok

`crates/ork/` ağacında chat/completions / `async-openai` / streaming yapan tek satır yok. `reqwest` yalnızca: S3 backup, Telegram/Twilio, provider health probe, model listesi çekme. `ork-router`'ın 2.570 satırı (judge/planner/executor/grounding/trust) **hiçbir modele bağlı değil.** `ToolExecutor` trait'ini implemente eden tip yok.

## 1.5 Bulgu B4 — Daemon sahte veri gösteriyor

`ork-daemon/main.rs`: storage/provider/router/scheduler kuruluyor → hepsi `let _` ile atılıyor. Ekrana elle yazılmış tek satır: `id:"waiting", persona:"orkd", state:"ready"`. Ajan yok, görev yok, çağrı yok.

## 1.6 Bulgu B5 — TUI/WebUI ortak modeli yok

TUI `AgentSummary{id,persona,state,uptime,ram_kb,token_count}` kullanıyor; WebUI `WsEvent{event_type,agent_id,payload,timestamp}`. Ortak tip yok. WebUI'da servis edilen HTML/CSS/JS yok — sayfa yok, kontrol endpoint'i yok. **K7 kararı (ortak çekirdek durumu) mevcut kodda karşılanmıyor.**

## 1.7 Bulgu B6 — Küçük ama karakteristik işaretler

- `ork-daemon/main.rs:52` `frame.size()` — ratatui 0.29'da deprecated (`frame.area()` olmalı). Kod hiç çalışmadığı için fark edilmemiş.
- `ork-record`, `ork-webui` workspace bağımlılığı yerine sürümleri elle yazmış; ayrıca **kendi `target/` dizinleri var** → workspace dışında, tek başına derlenmiş.
- `ork-backup/src/target.rs`: S3 PUT **imzasız**, SigV4 `// TODO`. Bulut yedekleme bulut ayağı çalışmıyor.

## 1.8 Kök neden

| Semptom | Sebep |
|---|---|
| 12.500 satır, 0 çalışan akış | Plan "dikey dilim" dedi, dilimi **kabul kriteriyle** tanımlamadı. Model en kolay ölçülebilir çıktıyı üretti: dosya/tip sayısı. |
| Mevcut altyapı kullanılmadı | Plan "genişlet" dedi, hangi trait/metot/tip imza düzeyinde belirtmedi. Sıfırdan yazmak okumaktan ucuz — model ucuzu seçti. |
| Binary hiç derlenmedi | Hiçbir fazda "derlenir + çalışır" kapısı yoktu. |

**Sonuç:** MASTER-PLAN'ın en kritik farkı mimari zarafet değil, **her fazın çalıştırılabilir ve ölçülebilir kabul kapısına bağlanması** olmalı.

---

# BÖLÜM 2 — MEVCUT KODUN KADERİ (K4 kararının uygulaması)

Kullanıcı: "işe yarayanı tut, yaramayanı yok et." Kanıta dayalı ayrım:

## 2.1 TUT (kurtarılabilir, sağlam çekirdek)

| Parça | Neden tutulur |
|---|---|
| `ork-storage` — CAS, WAL, redb, tek-writer aktör | Bağımsız çalışıyor, `xai-sqlite-journal` ile gerçekten entegre (B2'nin istisnası). CAS + WAL tasarımı sağlam. |
| `ork-provider` — `detection.rs`, `keyring.rs`, `health.rs` | Anahtar tespiti, keyring (chacha20poly1305 + zeroize), health probe gerçek çalışan kod. K13 için temel. |
| `migrations/0001-0007` | Şema taslağı makul; revize edilerek kullanılır. |
| `ork-notify` — telegram/twilio iskeleti | K9 için başlangıç. |

## 2.2 YENİDEN YAZ (tasarım hatası yapısal, yamalanmaz)

| Parça | Neden yeniden yazılır |
|---|---|
| `ork-router` (2.570 satır) | Hiçbir modele bağlı değil (B3). Judge/planner/executor gerçek LLM çağrısı üzerine oturmalı — mevcut iskelet buna uygun değil. |
| `ork-runtime` (2.575 satır) | `xai-*` entegrasyonu sahte (B2). Sandbox varsayımları K5 ile çelişiyor — temizlenmeli. |
| `ork-daemon` | Sahte TUI (B4). Lazy-init + gerçek state bağlama ile sıfırdan. |
| `ork-tui` + `ork-webui` | Ortak durum modeli yok (B5). K7/K8 ortak çekirdek üstüne yeniden. |

## 2.3 YOK ET

| Parça | Neden |
|---|---|
| `ork-record`, `ork-webui`, `ork-runtime` altındaki `target/` dizinleri | Workspace kirliliği. |
| `ork-backup` S3 imzasız PUT | Çalışmıyor; sonraki bir fazda doğru SigV4 ile yeniden. |

## 2.4 Yeniden adlandırma (K15)

`ork-*` → omnitrix isimlendirmesine geçilir. MASTER-PLAN somut isim şemasını önermeli (örn. `omni-runtime`, `omni-router`, `omni-provider`... veya tek binary `omnitrix` + iç modüller). Düşük riskli mekanik iş; erken fazda yapılmalı ki isim borcu birikmesin.

---

# BÖLÜM 3 — GERÇEKLİK ÇÖZÜMLERİ (kararlarla uzlaştırılmış)

Kullanıcı kararları verildikten sonra kalan fizik gerçekleri. Bunlar tartışma değil, MASTER-PLAN'ın uyması gereken kısıtlar.

## 3.1 RAM gerçeği (K2 ışığında)

Kullanıcı 250MB'ı kaldırdı, iş kutsal dedi. Fakat şu gerçek durur: **inference uzakta olsa da ajan bağlamı + HTTP stream bufferları senin makinende yaşar.**
- Aktif ajanın 50k token bağlamı ≈ ham ~200KB. **100 aktif ajan ≈ 20MB bağlam + ~30-60MB soket/TLS buffer ≈ 1-3GB toplam** (runtime + iki UI + SQLite page cache dahil).
- Bu, K1'in ilk hedefi (100 ajan) için **kabul edilebilir**. 10.000 için tek yol: **aktif ≠ var-olan** ayrımı.

| Terim | Tanım | Ölçek |
|---|---|---|
| Var-olan | Diskte kaydı olan ajan | 10.000+ (disk sınırı) |
| Uyuyan | Bağlamı sıkıştırılıp diske alınmış, RAM'de sadece metadata | 10.000+ (~200 byte/kayıt) |
| Kuyrukta | Çalışmaya hazır, sıra bekliyor | binlerce |
| Aktif | Bağlamı RAM'de, in-flight çağrısı olan | donanımın verdiği kadar (dinamik) |

**MASTER-PLAN kuralı:** "10.000 aktif" değil, "10.000 var-olan, N aktif (N = dinamik, RAM'e göre)". K2'yi ihlal etmeden K1'i karşılar.

## 3.2 Anlık kapanma + veri kaybı yok = crash-only tasarım (zorunlu)

"Ctrl+C'de ZINK kapanma, 100GB olsa bile veri kaybı yok" — bu ikisi ancak **crash-only** mimariyle birlikte mümkün:
- Her yan etkili işlem, yapılmadan **önce** WAL'a idempotent niyet kaydı yazılır.
- Ctrl+C → hiç flush yok, hiç bekleme yok, süreç ölür. Kapanış süresi **O(1)**, veri hacminden bağımsız.
- Açılışta `applied=false` niyet kayıtları replay edilir.
- **Normal kapanış ile SIGKILL arasında fark olmamalı** — tek kod yolu.

Bedeli: her işlem için ekstra WAL yazımı + tüm işlemlerin idempotent olma zorunluluğu. "Anlık kapanma" gereksiniminin fiyatı budur. Mevcut `ork-storage` WAL katmanı bunun temeli (2.1'de tutuldu).

## 3.3 Cold-start (< algı eşiği) = lazy-init zorunlu

"İnsan algısının anlayamayacağı gecikme" ≈ <100ms. 80+ crate'lik binary ile ulaşılabilir, **şartı:** her şeyi lazy başlat. İlk TUI frame'i hiçbir provider'a bağlanmadan, hiçbir ajan yüklemeden çizilmeli; ağ/storage/provider arka planda ısınmalı. Mevcut `ork-daemon` tam tersini yapıyor (her şey TUI'den önce) — yeniden yazımda düzeltilecek.

## 3.4 "AI yalan söylemesin" = süreç-katmanı determinizmi (model-katmanı değil)

`temperature=0` bile determinizm vermez (sağlayıcı batch/GPU non-determinizm/sürüm rotasyonu). Determinizm **süreç katmanında** kurulur — K10'un ("kritik zafiyet/bug/mantık hatası yoksa biter") mekanik karşılığı:
1. **Kanıt zorunluluğu:** Her olgusal iddia bir tool çıktısının belirli aralığına referans verir; referanssız iddia reddedilir.
2. **Doğrulama ayrımı:** İddiayı üreten ajan ≠ doğrulayan ajan, farklı model. Yargıç tool çıktısını **kendi yeniden çalıştırabilir**.
3. **Yanlışlamacı yargıç:** Görevi "doğru mu?" değil "**çürütebilir miyim?**". Onaylamacı yargıç dalkavukluğu ödüllendirir.
4. **İhlal maliyeti:** Kanıtsız iddia → interrupt + trust düşür + tekrarında karantina.

Bu çerçeve K10'un "biter" kararını objektif kılar: yargıç kanıtla puanlar + otomatik doğrulama (build/test/şema) geçer + kullanıcı onayı → biter.

## 3.5 Shell zorlaması gerçeği (K3 ışığında)

"LLM grep üretemesin bile" — uzak API'de token-seviyesi engelleme imkânsız (grammar-constrained decoding yalnızca yerel modelde). Ama gereksiz: **shell tool'u expose edilmezse model ne üretirse üretsin çalışmaz.** K3 kararı (allowlist + acil kaçış) tam bunu yapar — asıl zorlama noktası broker/tool katmanı, model katmanı değil. `eof`/`sed`/`cat` reddi bir parse + policy meselesi.

---

# BÖLÜM 4 — ÖLÇÜLEBİLİR KABUL KRİTERLERİ (her faz kapısı)

Önceki denemenin başarısızlık sebebi tam olarak bu tablonun yokluğuydu. MASTER-PLAN'da her satır bir faza bağlanmalı; faz bu test geçmeden "tamamlandı" sayılmamalı. Rakamlar öneri.

| Gereksinim | Ölçülebilir kriter | Ölçüm |
|---|---|---|
| İş kutsal (K2) | Verilen görev, kaynak darlığında bile yarıda kesilmez; darboğazda swap-out/kuyruk devreye girer, görev tamamlanır | Kaynak-kısıtlı görev testi |
| Cold-start | exec→ilk TUI frame < 100ms (sıcak cache), < 400ms (soğuk) | binary-içi timestamp + `hyperfine` |
| Anlık kapanma | SIGINT→exit < 50ms, **veri hacminden bağımsız** | 100GB aktif yazma altında test |
| Veri kaybı yok | kill -9 sonrası restart: her taahhüt işlem ya tam ya replay; hiçbiri yarım değil | Chaos: rastgele kill + tutarlılık doğrulama |
| 100 ajan eşzamanlı (K1) | 100 aktif ajan gerçek LLM çağrısı yaparken sistem stabil; RSS ölçülür raporlanır | Yük testi |
| Hata vermeyen | `cargo clippy -- -D warnings` temiz; üretim yolunda `unwrap`/`expect`=0; panic=0 | CI kapısı |
| AI yalan söylemesin | Kanıt referansı olmayan iddia **%100 reddedilir**; kırmızı-takım prompt setinde sızma=0 | Grounding testi + adversaryal set |
| Yerleşik araç zorunlu (K3) | İzinsiz tool/komut denemesi %100 reddedilir + loglanır; sızma=0 | Kırmızı-takım: modele yasak araç kullandırma seti |
| TUI/WebUI ortak durum (K7) | Her iki UI aynı çekirdek durumu okur; birinden yapılan değişiklik diğerinde görünür | Parite testi |
| Değişen dosyalar görünür (5.2) | Ajan hangi dosyaya ne yazdı — **çalışma dizini dışı dahil** — diff olarak CLI/UI'da izlenir | Diff-izleme testi |
| 7/24 otonom | 7 gün kesintisiz; müdahale gerektiren hata=0; bellek/FD sızıntısı yok | Soak testi |

---

# BÖLÜM 5 — TOOL SİSTEMİ (kullanıcının en somut girdisi)

Kullanıcı, opencode/mevcut araçlarda onu **yakan** eksiklikleri tek tek verdi. Bunlar tool tasarımının en değerli girdisidir; MASTER-PLAN bunları çözüm zorunluluğu olarak almalı.

## 5.1 Kod tabanı öğrenme — mevcut çözümlerin hepsi yetersiz (AÇIK PROBLEM)

Kullanıcı net: "read ile okuması hoş değil" **ve** "sembol grafiği veya mimari özet de istemiyorum — codebase-memory-mcp o yolda çıktı, memnun değilim." Yani:
- Ne ham dosya okuma (context yakar),
- Ne de mevcut graph/özet yaklaşımı (kullanıcı denedi, tatmin olmadı).

**MASTER-PLAN'a görev:** Kod tabanı öğrenmeyi yeni bir açıdan çözecek native tool tasarla. Kullanıcının reddettiği iki yolu tekrarlama. (`xai-codebase-graph` bileşen olabilir ama çıktı formu/etkileşimi farklı düşünülmeli.) Bu, çözülmemiş bir tasarım problemi — Bölüm 7/AS1.

## 5.2 Diff görünürlüğü — kayıp özellik

Kullanıcı: "ajanın dokunduğu/değiştirdiği dosyaların `+`/`-` diff grafiği eskiden opencode'da görünürdü, şimdi yok. **Ayrıca ajan, aracın çalıştığı dizin dışında bir dosyaya dokunsa bile onu CLI'dan görebilmeliyiz.**"
- Zorunluluk: her ajan tool-yazımı → global diff akışına düşer, **path fark etmeksizin**.
- Workspace'te `xai-hunk-tracker` ve `xai-gix-status` var — temel olabilir. Kapsam çalışma-dizini değil, **tüm dosya sistemi dokunuşları**.

## 5.3 Edit tool — sağlamlaştırma

Kullanıcı: "edit sık sık old_string/new_string ile patlıyor, ajan sonra sed'e kaçıyor."
- Edit tool, string-eşleşme kırılganlığını azaltacak şekilde tasarlanmalı (örn. anchor/aralık tabanlı, fuzzy-tolerant eşleşme, hata olduğunda modele **düzeltilebilir** geri bildirim).
- Edit patladığında kaçış yolu (sed) K3 tarafından zaten kapalı; asıl çözüm edit'i patlamaz yapmak.

## 5.4 Arama tool — grep'e düşürtmeyecek kadar iyi

Kullanıcı: "genelde grep/find'a düşülüyor." K3 grep'i yasakladığı için native arama **zorunlu** olarak yeterli olmalı — yoksa ajan tıkanır. Semantik + iyi sıralama + ayarlanabilir kapsam.

---

# BÖLÜM 6 — KALAN TASARIM NOKTALARI

## 6.1 Persona (K12)

Her persona bağımsız, diskte dosya. MASTER-PLAN persona dosya şemasını tanımlamalı. Persona başına alanlar (öneri): isim, sistem prompt, model tercihi/rol, bütçe politikası, tool allowlist (K3 ile bağlı), sıcaklık, routing politikası. ~60 persona ilk mesajdaki listeden; sayı büyüyecek. Görevleri kullanıcı sonra dolduracak — MASTER-PLAN yalnızca **şemayı ve yükleme mekanizmasını** kurar, 60 personayı doldurmaz.

## 6.2 Araştırma (K14)

Yüzeysel/derin/okyanus modları. Başta hazır MCP + anti-detect crawl araçları; uzun vadede native crawler. MASTER-PLAN, araştırma motorunu **sağlayıcı-değiştirilebilir** tasarlamalı (bugün Firecrawl/Exa, yarın native — çekirdek değişmeden). Sonuç formatı: hem insan hem AI okunur (öneri: JSON kanonik + Markdown türetilmiş). İsteğe bağlı video/ekran/DOM/computer-use kaydı ilk mesajdaki gibi tetiklemeli.

## 6.3 Anahtar besleme (K13) — kapsam sınırı

İncelenen kaynak: `/home/void0x14/Documents/ihsan-agama-verilen-destek/backshoot/data/` → `gitsex.db` (git sır tarayıcı), `verifier.db` (kimlik doğrulayıcı, `usable_credentials` tablosu). Bu hat, herkese açık repolardan **sızmış üçüncü-taraf canlı anahtarları** topluyor.

- **Tasarlanır:** Harici SQLite → okuma → canlılık kontrolü → canlı/ölü ayrımı → ölüler ayrı bölümde saklanır (ileride canlanabilir) → canlılar doğru provider'a eklenir. Bu mekanizma **kaynak-agnostik**; kullanıcının kendi/yetkili anahtarları için birebir çalışır. "Gerçek zamanlı okuma" = DB dosya izleme (fsnotify) veya periyodik poll (MASTER-PLAN seçer).
- **Tasarlanmaz:** Üçüncü-tarafa ait sızmış canlı anahtarları omnitrix'e besleyip onların hesabından inference yaptıran hat. Bu yetkisiz kimlik kullanımıdır. Anahtarların meşruiyeti kullanıcının sorumluluğundadır; sistem kaynağı sorgulamaz ama plan bu kullanımı operasyonelleştirmez.

Canlılık kontrolü (MASTER-PLAN çözecek): `/models` listesi ucuz ama kesin değil; 1-token completion kesin ama maliyetli. Öneri: iki kademeli (önce `/models`, şüpheli ise minimal completion).

## 6.4 Provider tespiti (Y1)

İki mod: (1) anahtar girilir → ön-ek heuristiği ile aday provider daraltılır → **canlı metadata çağrısıyla doğrulanır** (ön-ek yeterli değil; proxy'ler ve çakışan formatlar var). (2) kullanıcı listeden provider seçer → anahtar girer. Ön-ek örnekleri: `sk-ant-api03-`/`sk-ant-oat01-` (Anthropic), `sk-proj-`/`sk-svcacct-`/`sk-admin-` (OpenAI), `sk-or-v1-` (OpenRouter), `gsk_` (Groq), xAI/DeepSeek/Mistral/Cohere/Google kendi formatları. Mevcut `ork-provider/detection.rs` temel (2.1'de tutuldu).

## 6.5 Routing (Y2)

Üç mod (ilk mesajdaki tanım): (1) **route/round-balance** — yük tüm anahtarlara dağıtılır, maksimize. (2) **fallback** — bir anahtar hata verince (bakiye bitti vb.) yukarıdan aşağı sıradaki çalışana geçilir. (3) **JEP (yargıç-executor-planner)** — yargıç/executor/planner rolleri **config'te seçilir**, model isimleri gömülmez (AS7). Tüm modeller değiştirilebilir. Router her çağrıda health tablosundan canlılık okur, düşenleri zincirden çıkarır.

## 6.6 Uzak erişim & bildirim (K9)

Tek control-plane API; dört kanal (IPv6 doğrudan + Tailscale + Telegram + WhatsApp/SMS/arama) bu API'nin istemcileri. Auth zorunlu (public IPv6). Bildirim tetikleyicileri: görev bitimi + kritik hata + insan-onayı gereken interrupt. SMS/arama yalnızca yüksek-önem eşiğinde. Gürültü kontrolü: tekrar eden olayda susturma.

## 6.7 Döngü mühendisliği (K10, K11)

İki mod (ilk mesajdaki tanım korunur):
- **Tam-otonom:** kullanıcı yalnızca problemi verir; ajan a'dan z'ye planlar/araştırır/fixler/çözer; sonsuz iterasyon; pahalı.
- **Kullanıcı-odaklı:** kullanıcı görev tanımı + kısıtları verir; bütçe-dostu.

Her iki modda da ilk mesajdaki **zorunlu akış** uygulanır: problemi kelime kelime oku → parçala/anla → kalıcı listeye kaydet → web'den araştır (mod: yüzeysel/derin/okyanus) → bulguları insan+AI okunur formatta sakla → oku (büyükse parçala) → uygun stack belirle (AI önerir → internet ne diyor araştırır → çürütme döngüsü → nihai stack) → süre belirle (MVP mi tam mı — AI değil, ayrı "problem-süre sistemi" karar verir) → plan çıkar → yapı taşlarına böl → paralel/sıralı sorgula → multiajan görevlendir → akış sürer → biter → bildir.

## 6.8 Ajan kontrolü & müdahale (Y5)

Kullanıcı gereksinimi: her ajana hem AI hem kullanıcı prompt yazabilir; ajanlar gerçek zamanlı izlenir; kural ihlalinde interrupt + uyarı + ceza; ayrı tab'da tüm ajanlar (devam eden/biten) görünür. Subagent "aç-unut" değil — her ajana geri yazılabilir. MASTER-PLAN interrupt granülaritesini tanımlamalı (Bölüm 7/AS2). Rekürsif görevlendirme: ajan alt-ajan açar, o da açar, belli derinlikten sonra açamaz (sınır MASTER-PLAN'da — AS3).

## 6.9 Kayıt & yedekleme (Y13)

Kayıt: ilk mesajdaki gibi isteğe bağlı — video, ekran görüntüsü, DOM işlem kaydı, computer-use. Öneri (AS6): ajan akışı → event-log (ucuz, replay); computer-use → tetiklemeli video/DOM. Yedekleme 3-2-1: yerel + bulut senkron. Doğru SigV4 + şifreleme (AS10).

---

# BÖLÜM 7 — MASTER-PLAN'DA HÂLÂ ÇÖZÜLMESİ GEREKENLER

Kararı kullanıcıya değil, **tasarımcı modele** düşen açık noktalar. Bloklamıyor ama MASTER-PLAN'ın açıkça çözmesi gerekiyor — "sonra bakılır" bırakılmamalı.

| # | Açık tasarım noktası |
|---|---|
| AS1 | Kod tabanı öğrenme tool'u: kullanıcı hem ham-okumayı hem graph/özet'i reddetti. Yeni bir yaklaşım tasarlanmalı (5.1). |
| AS2 | Interrupt granülaritesi: ajan durdurulunca yarım tool çağrısı ne olur (rollback/tamamla/bırak)? Bağlam korunur mu? Ceza bağlama metin olarak mı girer (davranışı değiştirir) yoksa sadece trust skoru mu (yönlendirmeyi değiştirir)? |
| AS3 | Rekürsiyon: derinlik sınırı kaç? Her seviyede fan-out sınırı? |
| AS4 | Bütçe kalıtımı: alt ajan açılınca bütçe ebeveynden mi düşer, bağımsız mı, ebeveyn mi tahsis eder? (K11 sonsuz ama iş bitince durur — alt-ajan muhasebesi gerekir.) |
| AS5 | Ajan izolasyonu + panic: sandbox yok (K5). Bir ajanın panic'i tüm sistemi düşürmemeli — ama workspace `panic="abort"`. Tool yürütmeleri ayrı process'e mi? `catch_unwind` mı? K5'teki geçici branch mekanizması bununla nasıl bağlanır? |
| AS6 | Kayıt stratejisi: event-log vs tetiklemeli video/DOM. Retention politikası. |
| AS7 | Model kataloğu: model isimleri (sonnet 5/deepseek v4/grok 4.5) **plana gömülmemeli** — katalog güdümlü, rol→model config'te (K5.5/6.5). |
| AS8 | Config nerede yaşar: dosya mı DB mi ikisi mi? WebUI'dan değişen ayar diske yazılır mı? Çakışma önceliği? |
| AS9 | Gözlemlenebilirlik: workspace'te OpenTelemetry + Prometheus + fastrace var — kullanılacak mı? RAM için ayarlanabilir mi? |
| AS10 | 3-2-1 yedekleme: doğru SigV4 (B6 düzeltmesi) + şifreleme. Anahtarlar yedeklenirse şifreleme anahtarı nerede? (En büyük sızıntı riski.) |
| AS11 | Self-hosting: omnitrix kendi kodunu geliştirecek mi? Evetse kendini-bozma koruması + geri alma baştan tasarlanmalı. |
| AS12 | Computer-use araç seti (Y14): mevcut `xai-computer-hub-*` genişletilir mi, yoksa piyasa araçları (ComputerUse-rs/RustAutoGUI) mı? Wayland+X11 ikisi de (K6). |
| AS13 | "Problem-süre sistemi" (MVP vs tam çözüm kararı): girdileri ne? Kullanıcı elle mi, kural tabanlı mı, geçmişten öğrenilen istatistik mi? (Kullanıcı bunu AI-dışı tanımladı ama kuralları vermedi.) |

---

# BÖLÜM 8 — ÖNERİLEN İLK DİKEY DİLİM (Faz 1)

Önceki başarısızlığın dersi: geniş iskelet değil, **dar ve tam** dikey dilim. Öneri — *"Tek görev, tek ajan, uçtan uca çalışır"*:

1. API anahtarı gir → provider otomatik tespit + canlı doğrulama (6.4, tek anahtar)
2. TUI açılır < 100ms (lazy-init, 3.3)
3. Görev yaz
4. Tek ajan gerçek LLM'e bağlanır, gerçek tool çağrıları yapar (read/write/edit/search — Bölüm 5 tool'ları)
5. Her adım event-log'a; her dosya dokunuşu global diff akışına (5.2)
6. Ctrl+C → 50ms kapanır; restart'ta hiçbir işlem yarım değil (crash-only, 3.2)
7. RSS ölçülür, raporlanır

Bu dilim çalışınca: provider, storage, tool zorlaması (K3), diff görünürlüğü, dayanıklılık — her birinin **bir gerçek örneği** kanıtlanır. Bu dilimde **yok** (bilinçli): multiagent, JEP/yargıç, persona kataloğu, computer-use, video, yedekleme, bildirim, araştırma modları — hepsi sonraki fazlar.

---

# BÖLÜM 9 — RİSK KAYDI

| # | Risk | Erken uyarı |
|---|---|---|
| R1 | Aynı hata: geniş iskelet, çalışan akış yok | Faz sonunda "derlenir+çalışır+ölçülür" kapısı yoksa |
| R2 | `xai-*` entegrasyonu yine sahte olur (B2 tekrarı) | Entegrasyon imza düzeyinde belirtilmediyse |
| R3 | TUI/WebUI ortak durumu bozulur (K7) | Ortak çekirdek durum tek kaynak değilse |
| R4 | Kod-öğrenme tool'u yine tatmin etmez (AS1) | Kullanıcının reddettiği iki yol tekrarlanırsa |
| R5 | Sandbox varsayımı geri sızarsa (K5 ihlali) | Yeni kodda sandbox/seccomp/bwrap referansı belirirse |
| R6 | Sonsuz iterasyon işi bitmişken durmaz, kaynak yakar (K11) | Sonlanma oracle'ı (K10) + "iş bitti" tespiti zayıfsa |
| R7 | Computer-use ana makinede hasar (sandbox yok, K5) | Diff görünürlüğü + geri alma (git) zayıfsa |
| R8 | Kapsam sürekli büyür, hiçbir şey bitmez | İlk dilim (Bölüm 8) sabitlenmezse |

---

# BÖLÜM 10 — MASTER-PLAN'I YAZACAK MODELE TALİMAT

1. **Bölüm 0 kararları (K1-K15) veri olarak alınır, yeniden tartışılmaz.**
2. **Her fazın sonunda çalıştırılabilir kabul kapısı** (Bölüm 4 tablosu). "Dikey dilim üretir" yasak — hangi komut, hangi çıktı, hangi metrik hangi eşik.
3. **Faz 0 çıktısı: `omnitrix --version` çalıştıran, derlenen binary.** B1 tekrar etmesin.
4. **`xai-*` entegrasyonu imza düzeyinde:** hangi trait, hangi metot, hangi tip. "Genişletilir" yasak — B2 tekrar etmesin.
5. **Bölüm 2 kaderi uygulanır:** TUT/YENİDEN-YAZ/YOK-ET listesi + `ork-*`→omnitrix yeniden adlandırma.
6. **Sandbox yok (K5)** — hiçbir yeni kodda sandbox/seccomp/bwrap yer almaz.
7. **Ortak çekirdek durum, iki UI'dan önce tanımlanır (K7/K8).**
8. **Bölüm 5 tool eksiklikleri çözüm zorunluluğudur** — özellikle AS1 (kod-öğrenme) açık problem olarak işaretli.
9. **Model isimleri/fiyatlar gömülmez (AS7)** — katalog güdümlü.
10. **Bölüm 8 ilk dilim = Faz 1.** Multiagent/JEP/persona/computer-use sonraki fazlar.
11. **Bölüm 7 açık noktaları (AS1-AS13) MASTER-PLAN'da çözülür**, "sonra bakılır" bırakılmaz.
12. **Anahtar besleme kapsam sınırı (6.3)** korunur.

---

## Kaynaklar (web araştırması)

- [Terminal UI: BubbleTea (Go) vs Ratatui (Rust)](https://www.glukhov.org/post/2026/02/tui-frameworks-bubbletea-go-vs-ratatui-rust/)
- [awesome-ratatui — TUI/web parite ekosistemi](https://github.com/ratatui/awesome-ratatui)
- [Ratatui — immediate mode render](https://ratatui.rs/)
- [Rust + HTMX + SSE](https://blog.nashtechglobal.com/rust-htmx-and-sse/)
- [Leptos — SSR/WASM](https://github.com/leptos-rs/leptos)
- [Your Rust Service Isn't Leaking — It Could Be the Allocator](https://pranitha.dev/posts/rust-and-memory-allocators/)
- [jemalloc vs tcmalloc vs mimalloc](https://beefed.ai/en/choose-memory-allocator-jemalloc-tcmalloc-mimalloc)
- [SQLite Write-Ahead Logging (resmi)](https://sqlite.org/wal.html)
- [SQLite's Durability Settings are a Mess](https://www.agwa.name/blog/post/sqlite_durability)
- [ComputerUse-rs — Rust computer use SDK](https://crates.io/crates/computeruse-rs)
- [RustAutoGUI — GUI otomasyonu](https://github.com/DavorMar/rustautogui)
- [Anthropic API key formatı](https://vibekit.bot/anthropic-api-key-format)
- [OpenAI API key formatı](https://vibekit.bot/openai-api-key-format)
- [Desteklenen sağlayıcı anahtar formatları](https://www.testmyapikey.com/providers)
