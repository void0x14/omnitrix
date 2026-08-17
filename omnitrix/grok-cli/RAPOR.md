# grok-cli (xai-* temeli) — Feature Ekstraksiyon Raporu

Tarih: 2026-08-14 · Kaynak rev: `33f0aac` (no-telemetry patch sonrası) · Workspace: 93 üye, Cargo.lock'ta 1327 crate · Rust 1.97.1 (edition 2024) · `crates/codegen` + `crates/common` ağacı

## 1. Kimlik (crate haritası + işlev)

| Crate | Versiyon | LOC (rs) | İşlev |
|---|---|---|---|
| **xai-grok-pager** | 0.2.112 | ~473K | TUI monoliti: AppView/AgentView, Elm tarzı Action→Effect dispatcher, event loop, scrollback, slash registry, PromptWidget (`@` dosya ara, `/` komut, `!` bash modu, history), welcome, modallar (extensions/hooks/plugins/marketplace/skills/MCP), vim/minimal/headless modlar, mermaid (in-process), syntect, voice, pty_wrap, inline media (ffmpeg), wrap/export/share/keys/memory/trace/routing/worktree komutları + **omnitrix dikişi** (`omni_bridge.rs`, `omni_runtime.rs`, `/omni`, `/omni-dashboard`) |
| **xai-grok-shell** | 0.2.112 | ~367K | İkinci monolit: session actor, agent alt sistemi, sampling (eski, `sampling/`), auth (browser/API key/OIDC), config, extensions (hooks/plugins/marketplace/skills), claude_import, remote/SSH passthrough, relay/leader, managed_config, telemetry, upload, mcp_doctor, trace_classifier, heap_profile. Deps: rusqlite bundled (FTS5, CVE patched), gcloud-storage, git2 vendored, nucleo, moka, axum(ws), reqwest middleware, rustls |
| **xai-grok-tools** | 0.1.220-alpha.4 | ~128K | Tool registry + 50+ tool: read_file (pdf_oxide/pptx/zip/image), search (bm25), bash/use_tool, edit, glob, grep, LSP (async-lsp), memory (sqlite), reminders, research (web), computer use, task_output, skills, retry/attribution/taxonomy. **Compat setleri**: `codex/` (apply_patch), `opencode/` (bash/edit/glob/grep/read/skill/todowrite/write), `grok_build_hashline/` (anchor/edit/mutate/grep/read_file + scheme/config/benchmark) |
| **xai-grok-agent** | 0.1.0 | ~22K | Taşınabilir `Agent` tipi: tanım parse (Markdown+YAML frontmatter, `.grok/agents/`), discovery, system prompt assembly (minijinja), compaction, system_reminder, repo, plugins. Host-bağımsız — shell'den temizce ayrılmış |
| **xai-grok-sampler** | 0.1.0 | ~15K | Actor-tabanlı 3 katmanlı sampling: client (ham chunk) → stream transform → SamplerHandle (retry/cancel). Router engine, routing_config, doom_loop collector, Auth401 attribution, metrics, sampling_log. **Shell bağlantısı yok** — en temiz katman |
| **xai-grok-workspace** | 0.1.220-alpha.4 | ~88K | Host-local workspace: file_system, worktree (git2), permission/folder_trust/trust, discovery, session, foreign_sessions, hub + hub_server (preview proxy, axum), diag_server, envrc, export_github, channel/rpc_envelope, preview_supervisor, daemonize, recovery, fs_notify. İçinde `pub(crate) mod telemetry` kalıntısı var |
| **xai-grok-mcp** | 0.1.0 | ~11K | MCP entegrasyonu: rmcp 2.1 + reqwest 0.13'ü **karantinada** tutar (workspace reqwest 0.12 ile çakışma çözümü), credential store (`mcp_credentials.json`), browser OAuth (cross-process dedup), servers (streamable-http + child process), backoff wrapper, liveness, acp_transport |
| **xai-grok-config** | 0.1.0 | ~11K | Katmanlı config: managed > user > requirements (Ed25519 imzalı, fail-closed), macOS MDM, campaigns, version_overrides, global_hook_sources, fs_atomic, validation |
| **xai-omni-keychain** | 0.1.0 | ~5K | **Omnitrix'e özgü**: AES-256-GCM şifreli key deposu (`~/.grok/keychain.omx`), Argon2id master password KDF, zeroize, OS keyring (keyring crate), key tipi algılama, stack import/export/sync, TTL master-key cache, Türkçe dokümantasyon |
| **xai-grok-sandbox** | 0.1.0 | ~6.4K | OS-level sandbox: nono (Landlock/Seatbelt) =0.53.0 pinned (bypass riski gerekçeli), profiller, deny globs, child seccomp network, hook_write_deny, deny_paths_e2e testi (CI'da self-skip) |

## 2. EN İYİ feature'lar (TUT listesi) — korunması/taşınması gereken üstün parçalar

1. **Pager mimarisi**: Elm tarzı Action/Effect ayrımı, `xai-grok-pager-render`'a ayrılmış presentation-primitives katmanı (appearance/clipboard/theme/syntax/terminal), PTY güvenliği (dunce + xai-tty-utils), minimal mod + `-p` headless mod, mermaid render, vim modu. `omni_bridge` dikişi sayesinde omnitrix servisleri TUI'ye ikinci proses olmadan bağlanıyor — bu desen korunmalı.
2. **Sampler'ın 3 katmanlı actor tasarımı**: sampling mantığının shell'den tamamen koparılması (client/stream/handle) + doom-loop sinyal kolektörü + router engine + 401 attribution. Yeniden yazılamaz kalitede; artifact olarak dondur.
3. **xai-grok-tools compat setleri**: `grok_build_hashline` (omnitrix'in kendi FFS/hashline ailesi — anchor/edit/mutate/grep) ve `opencode`/`codex` tool setleri. Tool registry + taxonomy + retry/attribution altyapısı sağlam.
4. **xai-grok-mcp karantina stratejisi**: rmcp 2.1'i izole crate'te tutup `pub use rmcp` ile model tiplerini dışarı verme — versiyon çakışmasını monolitik bump'a gerek kalmadan çözüyor. OAuth dedup (cross-process) + credential store düzgün.
5. **xai-grok-config**: imzalı requirements (Ed25519) + fail-closed startup + MDM katmanı + TOML deep-merge sıralaması. Kurumsal dayanıklılık, omnitrix'e doğrudan taşınabilir.
6. **xai-grok-sandbox**: nono versiyon pin'i + neden gerekçesi (Seatbelt rule-order), glob doğrulama, child network seccomp, e2e test. Küçük, testli, bağımsız.
7. **xai-omni-keychain**: zaten omnitrix'e ait; Argon2id + AES-GCM + zeroize + OS keyring + stack sync. Temel katmanın "kasa" parçası.
8. **Session persistence**: rusqlite bundled (>=3.50.2, CVE-2025-29087/3277/6965 patched) + FTS5 + sqlite-vec. SQLite journal ve hunk-tracker katmanları da ayrılmış.
9. **Profile sertleştirmesi**: `release`'de overflow-checks + debug-assertions açık (bellek güvenliği), `release-dist` thin LTO + CGU=1, panic=abort.
10. **Agent crate'i**: Agent tanımı/compaction/system-prompt'un host-bağımsız taşınması — ACP/headless/omnitrix tarafı için doğru soyutlama.

## 3. Dezavantajlar (LOBOTOMİ listesi)

1. **İki dev monolit**: pager (~473K) + shell (~367K) = ~840K satır iki crates'te. Derleme maliyeti korkunç; değişiklik darboğazı. Cargo.lock 1327 crate — her ikisi de bunu tetikliyor.
2. **Telemetry kalıntıları (no-telemetry patch'leri "nötralize" ediyor, silmiyor)**: `xai-grok-telemetry` (Mixpanel + Sentry + tracing-chrome), `xai-mixpanel`, `xai-grok-secrets` (yalnızca outbound scrubber), workspace'te hâlâ `opentelemetry`/`opentelemetry-otlp`/`fastrace*`/`tonic`/`prometheus`/`pprof`/`tracing-opentelemetry` bağımlılıkları, `xai-grok-workspace::telemetry`, `obfstr` (string obfuscation), `xai-grok-update`, `xai-grok-announcements`, `campaigns` (ürün kampanyası!). Patch yaklaşımı CI'ı kırıyor ve ağırlık kalıyor — kod silinmeli.
3. **Ağır bağımlılıklar**: `syntect` (pager), `resvg`+`fontdb`+tiny-skia (mermaid → 100MB+ statik render), `pdf_oxide` (tools), `git2` vendored (5 crate) **ve** `gix` (3 crate) — iki ayrı git implementasyonu, `gcloud-storage` (shell'de Google Cloud!), `alacritty_terminal` (ptyctl/pty-harness), `termwiz` (ratatui-inline), `tikv-jemallocator`, `gzip`/`zstd`/`flate2` yığını. Bunların çoğu desktop/telemetry/upload ekseni için; CLI çekirdeğine girmiyor ama workspace derlemesini şişiriyor.
4. **Versiyon kaosu**: pager/shell 0.2.112 vs tools/workspace 0.1.220-alpha.4 vs diğerleri 0.1.0 — tek kaynak yerine üç versiyon hattı. `alpha.4` etiketi "fork sonrası yaşayan" parçaları ele veriyor.
5. **Dual reqwest (0.12 + 0.13)**: karantina bilinçli ama yine de ~200 crate'lik ikinci bir HTTP yığını derleniyor.
6. **Shell içi eski sampling kopyası**: `xai-grok-shell/src/sampling/` hâlâ duruyor — xai-grok-sampler'a taşınmış katmanın eski ikizi, kod çiftlemesi.
7. **Server-kıyafeti ağırlık**: shell'de `axum` (ws+multipart), `tower-http` (cors), `gcloud-storage`, `tokio-tungstenite`, `xai-grok-workspace`'te `hub_server`/`diag_server`/`preview_supervisor`/`daemonize` — bir CLI için kocaman yan yüzey. `upload/` + `export_github` telemetry/exfil ekseniyle birlikte değerlendirilmeli.
8. **Vendored third_party**: `dagre_rust`, `graphlib_rust`, `mermaid-to-svg` — bakım yükü omnitrix'e kalıyor; resvg+mermaid hattı düşürülürse bunlar da düşer.
9. **Test dağılımı eşitsiz**: sampler/mcp/sandbox/testleri var; `xai-grok-workspace`'in test klasörü yok, shell/pager testleri bazı modüllerde self-skip (ör. `deny_paths_e2e` CI'da çalışmıyor).
10. **Dev araçları**: `dhat`, `criterion`, `pprof`, `wiremock`, `mockito`, `insta`, `serial_test` — bazıları feature-gated değil; clean workspace'te kırpılmalı.

## 4. omnitrix için öneri (dondur / yeniden yaz)

**A. Artifact olarak dondur (build-once, dokunma):**
- `xai-grok-sampler` + `xai-grok-sampling-types` — 3 katmanlı tasarım mükemmel, shell'den ayrık.
- `xai-grok-mcp` — rmcp karantinası + OAuth/credential store; yeni MCP work'ü bu crate'in üstüne yazılır.
- `xai-grok-config` — imzalı/fail-closed katman; omnitrix config şeması üstüne bindirilir.
- `xai-grok-sandbox` — nono pin'i korunur; yeni profil eklemek dışında dokunulmaz.
- `xai-omni-keychain` — zaten omnitrix malı; `crates/common`'a taşınabilir.
- `xai-tool-types` / `xai-tool-protocol` / `xai-tool-runtime` / `xai-tool-protocol` (common ailesi) — wire/tip katmanı olarak dondur.
- `xai-grok-agent` — küçük ve temiz; dondur.

**B. Yeniden yaz / lobotomize (özünü al, etini at):**
- `xai-grok-shell` → **en büyük operasyon**: session actor + ACP stdio + `-p` headless + auth çekirdeği + claude_import kalır; `upload/`, `telemetry/`, `campaigns`/`announcements`/`update`/`marketplace` ağ ekseni, `gcloud-storage`, eski `sampling/` ikizi silinir. Hedef: ~367K → ~150K.
- `xai-grok-workspace` → `file_system`, `worktree`, `permission`/`folder_trust`, `discovery`, `fs_notify`, `recovery` kalır; `hub_server`, `diag_server`, `preview_supervisor`, `daemonize`, `upload`, `export_github`, `telemetry` omnitrix'in kendi runtime'ına devredilir ya da silinir.
- `xai-grok-tools` → registry + hashline/opencode/codex compat + LSP + memory kalır; `pdf_oxide` (PDF) ve `computer` (computer-use) isteğe bağlı feature'a çekilir, `research_tool` web ağı omnitrix search'e bağlanır.
- `xai-grok-pager` → **yeniden yazma, katmanla**: `xai-grok-pager-render` + `omni_bridge` zaten doğru iskelet; telemetry/announcements/marketplace/voice/isimsiz ağırlıklar feature-gate'lenir. pager'ın kendisi artifact, omnitrix UI'ı onun üstünde.

**C. Workspace temizliği (tümünde):**
- `xai-grok-telemetry`, `xai-mixpanel`, `xai-grok-secrets`, `xai-grok-update`, `xai-grok-announcements`, `xai-mixpanel` silinir; `opentelemetry*`/`fastrace*`/`tonic`/`prometheus`/`pprof`/`obfstr`/`tracing-opentelemetry` workspace'ten çıkar. no-telemetry patch'leri gereksizleşir (kod zaten yok).
- Git ikiliği: tek implementasyon seç (git2 vendored, zaten 5 crate kullanıyor) — `gix` hattı (fast-worktree/gix-status/hunk-tracker) gix'te kalırsa o üç crate dondurulur; çift git taşınmaz.
- Versiyon tekilleştirme: tüm omnitrix crates'i tek versiyon hattına (ör. 0.1.0 + git describe) alınır.
- Mermaid/resvg hattı kararı: TUI'de mermaid yoksa `resvg`/`fontdb`/`tiny-skia`/`dagre_rust`/`graphlib_rust`/`mermaid-to-svg` birlikte düşer (~en pahalı render zinciri). Varsa bütün halinde dondur, ayrıca feature-gate yap.

**Sonuç özeti:** Dondurulacak çekirdek ≈ sampler + mcp + config + sandbox + keychain + tool-protocol + agent (~90K LOC). Yeniden yazılacak gövde = shell + workspace + tools + pager katmanları (~1.05M LOC'dan ~400K'ya iner). Nihai omnitrix CLI: omnitrix runtime (ork-router/ork-provider) + xai temel katmanı (dondurulmuş) + ince bir kabuk.
