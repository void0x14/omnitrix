# Git Feature Envanteri — Omnitrix (Kullanıcı İşi)

Tarih: 2026-08-14
Kapsam: `SOURCE_REV (33f0aac)..HEAD` — 89 commit
Toplam tarih: 142 commit
Diff ayak izi: 437 dosya, +54.596 / -79.228 (net: omni-* katmanının eritilmesi nedeniyle silme ağırlıklı)

Kaynak revizyon: `33f0aacfcc95eb7de316308e9a3bc22470e48b89` (no-telemetry patch'lerinden sonra güncellendi, 13b5e1d)

---

## 1. Omnitrix Harness — omni katmanı (faz1–faz10, sonra eritme)

İlk dönem: `omni-*` crate ailesi ayrı katman olarak kuruldu, TUI'ye `omni_bridge` dikişiyle bağlandı; sonra tek ürün yaklaşımıyla doğrudan grok koduna eritildi.

| Feature | Commit |
|---|---|
| `omni_bridge` dikişi + `/omni` `/omni-tasks` slash komutları | fec3529 |
| Omnitrix çekirdeğinin TUI başlangıcına kablolanması (lazy warmup + bridge install) | 855941b |
| `DiffShimFs` — diff görünürlüğü için fs-shim (shell agent builder'a) | 8fea474 |
| Green gate testleri: crash_recovery + diff_visibility | 7437c24, d797db4 |
| `/omni-autonomous` tam döngü + webui mount testi (faz10) | 217adf7 |
| `/omni-routing` — strategy switch + role→model (faz4) | 084df36 |
| Clippy-clean omnitrix (watch_sigint: pager sinyal yolunu sahiplenir) | b282448 |
| Eritme refactor: omni katmanları grok-build koduna işlendi (matrix yok) | ed090c0 |
| `OmniSchedulerBackend` — admission/kuyruk/depth → grok SubagentBackend | f54351c |
| Vizyon entegrasyonu: backup/otonom/anahtar/grounding/persona/computer | 50ab8e8 |
| `omni-*` katmanı tamamen kaldırıldı — tek ürün, tek kod tabanı | 6d6ee1d |
| İlk tam workspace build (tümü yeşil) | b9b93a9 |
| TUI delegasyonu: omnitrix kendi ratatui döngüsü yerine xai-grok-pager | 72ef0c1 |
| Build fix: mevcut olmayan rerun-if-changed yollarından rebuild cascade'i durdur | 3c4dbed |
| sccache/mold/profile.test denemeleri geri alındı | fdd73f4 |

## 2. No-Telemetry (SOURCE_REV öncesi temel)

Not: Bu işler SOURCE_REV'in güncellenmesiyle "temel" sayıldı; envantere dahil:

- Telemetri patch'leri uygulandı (mixpanel nötralize, sentry/OTLP/trace-upload/feedback kapatıldı): 4013b49 (6 patch, 1208 ekleme)
- I2a telemetri-yolu muafiyeti + I8 no-telemetry gate (CI): 33f0aac (SOURCE_REV noktası)
- MASTER-PLAN invariant gate script (CI): 457ddd2, 7e2421b
- No-telemetry tasarım dökümanı: docs/superpowers/specs/2026-07-29-no-telemetry-design.md
- `feat(ci)` doğrulama: `test: assert what the test names claim` 8cf7cbe

## 3. Keyring / Auth (xai-omni-keychain)

| Feature | Commit |
|---|---|
| `xai-omni-keychain` crate iskeleti + AES-GCM/Argon2id crypto | c6ba781 |
| Crypto test modülü + nonce/malformed testleri | 3aa5a56 |
| Store CRUD + kategori + borrow + TTL cache | 9ed046c |
| Kategorili şifreli export/import (`.omx`) | e34cf9e, 3fde189 |
| Provider otomatik tespiti — API key prefix'inden | a61a783 |
| Harici stack senkronu (sync-from/sync-to): Claude/Codex/Hermes/env/JSON formatları | 51c1de4 |
| `/import` `/export` slash komutları + OS keyring + Türkçe dokümantasyon | 8334a5b |
| `keyring_store.rs` (OS keyring), `ttl.rs`, `detect.rs`, `stack/catalog.rs` (634 satır) | — |

İlgili dosyalar: `crates/codegen/xai-omni-keychain/` (crypto, store 1006 satır, export, detect, stack/sync 316, stack/formats/*), pager `keys_cmd.rs` (582), `keys_cmd_tests.rs`.

## 4. Provider Connect — models.dev kataloğu + connect akışı

| Feature | Commit |
|---|---|
| models.dev canlı provider/model katalog client + TTL cache | 27f3c25, 9e3dafa |
| pager: `grok connect` + `grok keys` komutları + provider flagleri | 1dbddbf, bac5646 |
| Connect core: keychain borrow → runtime key injection + session start | 0e8cf74, 2d80c62 |
| Provider picker durum makinesi + canlı provider listesi | 309ac30, 64f614b |
| Connect wizard key/model adımları + app entegrasyonu | 03bfd4d, f7f546e |
| `/connect` + `/keys` slash komutları, palette/welcome girişleri + keys manager TUI | 7459b31, dace7f0 |
| Headless: provider/auth flag entegrasyonu (keychain/runtime key + config yazımı) | 8821f87, 77888da, 974a409 |
| Auto-connect: live provider/region probe + `auto_connect_from_key` orkestrasyonu | 5c0cb53, ad80767, 250ee0b, f0cdc81 |
| Auto-connect wizard yolu (TUI) + `grok connect --auto` | 44e18a6, 03b2035, 6f02a9b, b8bfb0f |
| Wizard sadeleştirme + otomatik kategori sistemi | 06c2f56 |
| Derleme onarımları (keychain + wizard akışları) | 86ea16d, e96d914, 60c8e3f |

İlgili dosyalar: `connect_cmd.rs` (538), `views/provider_picker/*` (mod 906, auto 839, key_input 909, model_select 610, providers 294, apply 255), `views/keys_manager.rs` (2574), `dispatch/connect.rs` (1431), `util/models_dev.rs` (322), `util/provider_probe.rs` (278), `util/auto_connect.rs` (171).

## 5. Routing — mode kataloğu + RouterEngine

| Feature | Commit |
|---|---|
| Mode katalog: 40+ tanım + varsayılan parametre doğrulaması | d4c6491, 6d5db42 |
| Routing modları araştırma notları (docs) | fad83f4, f369c02 |
| Routing mode id + legacy alias'lar (config) | 8c4ae29, c37c570 |
| `RouterEngine` çok-modlu seçim + degrade uç noktaları atla | ed769ec, a75a1b5 |
| Routing mode picker + CLI (UX) | bd98503, cdd6202 |
| Sampler: fallback terminal state, restart API, Eq derives | 2f78fe9, 329315a |
| `/omni-routing` — strategy switch + role→model (faz4) | 084df36 |

İlgili dosyalar: `config/routing_modes.toml` (1074), `sampler/router_engine.rs` (1784), `sampler/routing_config.rs` (345), `util/routing_catalog.rs` (358), `views/routing_picker.rs` (193), `routing_cmd.rs` (116).

## 6. Flow Governor — akış durum makinesi + yargıç + bildirim

| Feature | Commit |
|---|---|
| Tasarım spec + implementasyon planı | 8ff9be6 |
| Saf akış tanımı + durum makinesi (definition/state) | 8c4d3c9 |
| Deterministik sınıflandırıcı + süre sistemi + aşama kapısı | 9930daf |
| Kanıt deposu + olay yayını + FlowGovernor kompozisyonu | 6888e7a |
| `flow_checkpoint` aracı + SessionActor governor alanı + kayıt (S1/S7) | 0a49ae0, 6744ecc |
| Dört karar noktasına governor kancaları (S2–S6) + S2 system-reminder enjeksiyonu | 2ccfa07, 6864847 |
| Full delivery: paralel sorgu analizi, yargıç, çoklu ajan execute, bildirim kanalları, runtime config, TUI paneli | 82be053, d6d9012, e1d6442 |
| Akış şablonları + config örneği + doğrulama notları | 18592cc, 600773f |
| Build: overflow-checks + debug-assertions tüm profillerde | 5432b61 |

İlgili dosyalar: `shell/src/session/flow/*` (governor 724, definition 296, config 244, gate 228, notify 242, parallel 194, classifier 165, judge 113, state 176, store 64, events 66, duration 56), `tools/flow_checkpoint.rs` (129), `views/flow_detail.rs` (153), `config/flow/*.toml.example`.

## 7. Araçlar (tools) — grok-build entegrasyonları

| Feature | Commit |
|---|---|
| `research_tool.rs` — sağlayıcı değiştirilebilir araştırma motoru (faz7) | 7189a40 (omni dönemi), 1297 satır |
| `computer_tool.rs` — computer kullanım aracı (1200 satır) | 50ab8e8 dönemi |
| `omni_scheduler_backend.rs` — SubagentBackend impl (1247 satır) | f54351c |
| `grounding.rs` (674) + `retry.rs` (559) — sampler grounding/retry | 50ab8e8 dönemi |
| `session/backup.rs` (1206) — SigV4 gerçek yedekleme | dc10b5a dönemi |
| `agent/autonomous.rs` (1483) — otonom ajan döngüsü | 217adf7 dönemi |
| `session/fs_shim.rs` (621) — diff görünürlüğü fs-shim | 8fea474 |
| `omnipersona.rs` (401) — persona menajerisi | 37cd55e dönemi |
| Omni testlerinin eritilmesi (root `tests/*` silindi, crate içine taşındı) | 6d6ee1d |

## 8. Docs / Planlar (plan, spec, prompt, checklist)

- Provider connect tasarım + plan: 4ac9885, 750c582, d863f33 (`docs/provider-connect.md` 299 satır)
- Omnitrix harness tamamlanması planı: 49fe62e
- Flow Governor plan (1713 satır) + spec (345): 8ff9be6
- Platform v2 plan/spec/agent prompt/persona checklist: 37cd55e, fad83f4, f369c02, 833b8ca
- Routing modları dokümanı: `docs/routing-modes.md`
- Kullanıcı kılavuzları: `25-omnitrix.md`, `26-provider-keys.md` (292 satır, Türkçe)

## 9. Build / Toolchain / Misc

| Değişiklik | Commit |
|---|---|
| Lockfile: xai-grok-hooks + zeroize/serde | b6cec6b |
| opencode import akışının derlenmesi | 234d0eb |
| REVERSED cursor assertion + runtime_key temizliği (test) | e72a8a4 |
| gitignore güncellemeleri | 6e4101e, 6d3487d |

---

## Özet

- Kullanıcı işi: **89 commit** (SOURCE_REV sonrası), 437 dosya, +54.596 / -79.228
- Üç ana akım: **(1)** keyring/auth + provider connect (keychain→runtime key→wizard/headless), **(2)** routing mod kataloğu + RouterEngine sampler, **(3)** Flow Governor (yargıç + çoklu ajan + bildirim)
- Mimari karar: başlangıçtaki ayrı `omni-*` katmanı (faz1–faz10) daha sonra grok-build koduna eritilerek tek ürün haline getirildi — bu yüzden diff ayak izi silme ağırlıklı.
