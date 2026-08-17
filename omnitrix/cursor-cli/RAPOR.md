# Cursor CLI — Feature Ekstraksiyon Raporu (RE tabanlı)

Kaynaklar: 0xlane/reverse-cursor-agent (docs/00-overview, cursor-agent 2026.04.17-787b533, 7.6 MB index.js), nhan665/cursor-opencode-auth (RESEARCH.md, Connect-RPC/protobuf RE), pleaseai/shunt adapters/cursor/agent.rs (wire format), sperixlabs.org egress analizi (2026-04-11: `agent-cli 2026.04.08`), m24927605/agentic-spendguard RECON.md (framing), Cursor resmi docs (via agent-cli davranışı).

## 1. Kimlik (bilinen mimari)

- **Dağıtım:** Node.js tabanlı `cursor-agent` CLI; tek `index.js` bundle (~7.6 MB), Electron IDE'den bağımsız. IDE sürümlerinden ayrı `cli-YYYY.MM.DD-<hash>` sürüm takvimi.
- **Ağ protokolü:** Connect-RPC (Buf) over HTTP/2 — gRPC-Web değil. Endpoint: `agentn.global.api5.cursor.sh/agent.v1.AgentService/Run` (privacy modda `agent.api5.cursor.sh`). Eski `api2.cursor.sh` artık 464/`invalid x-api-key` döndürüyor (ALB protokol uyumsuzluğu). Protobuf şeması: 603 `agent.v1` + 2.357 `aiserver.v1` message. Auth: JWT (macOS Keychain `cursor-access-token`), header'lar `x-cursor-client-type: cli`, `x-cursor-client-version` (sürüm doğru değilse `permission_denied`), `x-ghost-mode`, `connect-protocol-version: 1`.
- **Mimari rol:** **Saf tool-call rölesi** — istemcide HİÇ model inference yok. Tüm LLM inference, context assembly, tool seçimi sunucuda. İstemci sadece: run request gönder → `exec_server_message` (tool call) al → yerelde çalıştır → sonucu geri gönder → `turnEnded`'a kadar döngü.
- **Stream içi mimari (stream splitter):** tek BiDi gRPC stream'i 6 alt akışa ayrılır: StreamSplitter, ClientExecController (tool döngüsü), ClientInteractionController (UI güncellemeleri), CheckpointController (durum snapshot'ları), ControlledKvManager (SHA-256 content-addressed blob store), ConversationActionManager. Hepsi `Promise.allSettled` ile eşzamanlı çalışır.
- **State:** `conversation_checkpoint_update` → sunucu periyodik olarak `ConversationStateStructure` gönderir (turns blob ID listesi, summary, subagentStates); istemci blob'ları KV alt protokolüyle çeker — büyük objeler ana stream'i bloklamaz. Kesinti sonrası `resumeAction` + en son checkpoint ile otomatik kurtarma (max ~10 retry).
- **Telemetry/egress:** `api2.cursor.sh/v1/traces` (OTLP protobuf), `repo42.cursor.sh` (Repo Merkle sync: FastRepoInitHandshakeV2, SyncMerkleSubtreeV2), `localhost` indexing daemon (`localhost/getRepositoryInfo`).

## 2. EN İYİ feature'lar (ÇAL listesi)

1. **Thin-client röle mimarisi:** istemci sadece protokol + tool execution — inference yükü sıfır. Yerel CPU/RAM maliyeti minimal, tüm zeka sunucuda. Claude Code'un 400 MB baseline'ına karşılık Cursor CLI bu sayede düşük footprint'te kalıyor.
2. **Checkpoint/resume protokolü:** `ConversationStateStructure` + SHA-256 content-addressed blob store + `resumeAction` — bağlantı kopunca (30s stall tespiti) otomatik yeniden bağlanıp son checkpoint'ten devam eder. Kesintisiz session sürekliliği, RE'de belgelenmiş tek tam uygulama.
3. **Bidirectional stall/heartbeat protokolü:** sunucu → istemci interaction heartbeat; istemci → sunucu tool exec sırasında her 3 sn'de exec heartbeat; streamClose (başarı) / throw (hata + stack trace) / abort (sunucu iptali) kontrolleri. Streaming güvenilirliği protokol seviyesinde çözülmüş.
4. **InteractionQuery side-channel'ı:** sunucu tek stream içinde senkron onay isteyebilir: `PreToolUseRequestQuery`/`PostToolUseRequestQuery` (tool onayı), `AskQuestionInteractionQuery`, `WebFetch/WebSearch` onayı, `SubagentStart/StopRequestQuery`, `McpAuthRequestQuery`. Permission akışı protokolün birinci sınıf vatandaşı — ayrı bir HTTP çağrısı değil.
5. **44 tool'luk protokol ailesi + subagent'lar:** `ToolCall` oneof'unda 44 tool (dosya, shell, arama, MCP, sub-task); ayrı subagent protokolü ve subagent billing şeması (11-subagent-billing.md). TurnEnded tek event'te agrege token kullanımı raporluyor.
6. **Sunucu-taraflı context summarization:** otomatik + manuel tetiklenebilir özetleme protokolü (09-context-summarization.md) — istemci tarafında compaction kodu yok, sunucu yönetiyor.
7. **Repo Merkle sync:** çalışma ağacı `repo42.cursor.sh`'a Merkle ağacı olarak hash'lenip gönderiliyor → sunucu context'i repo yapısıyla zenginleştiriyor (context feature'ları için).
8. **Ghost mode (privacy toggle):** `x-ghost-mode: true` — prompt/completion içeriği sunucuya gitmez (kapsamı için Bkz. §3).

## 3. Dezavantajlar (LOBOTOMİ listesi — kanıtlı)

1. **Ghost mode zarfı değil sadece içeriği kapatıyor (sperixlabs Finding 2):** flag açıkken bile OTLP trace `api2.cursor.sh/v1/traces`'e gidiyor ve şunları taşıyor: `host.name` (yerel hostname), `os.version`, `client.arch`, runtime versiyonu, **tam RPC span ağacı** (`hydrateSummaryArchives`, `rpc.run`, `ClientInteractionController.run`, `ControlledKvManager.run`, `ClientExecController.run`, `CheckpointController.run`, `AgentConnectClient.run`, `networkPhase`, `chat.queued`, `chat.request`) ve `event.token_counts_visible`. Kullanıcı zihnindeki "ghost mode = hiçbir şey kaydedilmiyor" modeli yanlış.
2. **Repo yapı metadata'sı dışarı gidiyor:** `repo42.cursor.sh`'a Merkle subtree hash'leri itiliyor — hangi dosyaların var olduğu (içerik değilse bile) yapısal olarak Cursor altyapısına sızıyor. Bloklanırsa "Breaks Cursor context features" (sperixlabs tablosu).
3. **Tam sunucu bağımlılığı — offline yok:** istemci saf röle; sunucu yoksa hiçbir şey çalışmaz. Model, context yönetimi, hatta context özetlemesi bile sunucuda. BYOK/özel endpoint yok. CLI'nin kendisi bir oturum dosyası üretmeden çalışamaz.
4. **Sürüm tabanı kilidi:** `x-cursor-client-version` eski kalırsa sunucu 464/`permission_denied` ile reddediyor. Satıcı istediği an eski CLI'ları fonksiyonel olarak öldürebilir; wire format değişince üçüncü taraf uyumluluğu (jcode, shunt gibi) kırılıyor.
5. **İstemcide limit/jeton yok:** rate limit, bütçe, ödeme kontrolü tamamen sunucuda (RE bulgusu 8.4). Kullanıcının hiçbir yerel kontrolü yok; kullanımı görmek için sunucuya güvenmek zorunda.
6. **Proprietary protobuf, dokümante değil:** şema ancak MITM + hex analiziyle çözülüyor; frame cap 4 MiB/8 MiB, compression gzip/zstd/br karışık. Enterprise denetim/güvenlik aracı yazmak isteyen herkes her sürümde kırılan RE'ye mahkum.
7. **Protocol erişimi protokole bağlı:** `interactionQuery` permission onayları ağ turu gerektirir — ağ kesintisinde tool çalıştırma kilitlenir (Claude Code'un yerel permission cache'i gibi bir düşme yok).
8. **Server-side model yönlendirme:** kullanıcının seçtiği modelle gerçekte kullanılan model farklı olabilir; istemci tarafında doğrulama yok.

## 4. omnitrix için öneri

- **Checkpoint/resume protokolünü al:** SHA-256 content-addressed session blob'ları + resumeAction + stall tespiti (30s) + sınırlı retry (10) — omnitrix'in bağlantı dayanıklılığı için referans tasarım. Tool exec sırasında 3 sn'lik heartbeat + streamClose/throw/abort üçlüsünü uygula.
- **InteractionQuery side-channel modelini al:** PreToolUse/PostToolUse onayı, web erişim onayı, MCP auth onayı — hepsi tek stream'de, ayrı HTTP çağrısı yok. omnitrix permission akışını stream içinde protokol seviyesinde kur.
- **Thin-client felsefesini yarıya kadar al:** omnitrix'te inference yerel kalabilir (offline çalışma şart), ama **state serialization'ı merkezi bir şemaya** (content-addressed blob) bağla — büyük context parçalarını ana mesaj akışından ayır.
- **YAPMA:** "saf röle" olma — sunucusuz/offline modu koru; sunucu ölünce (ya da satıcı sürüm tabanını yükseltince) tool'un tamamen ölmesini miras alma.
- **YAPMA:** ghost mode'un yalanını yapma — "içerik kapalı ama envelope açık" ayrımı kullanıcıyı aldatır. omnitrix'te telemetry tek anahtarla tamamen kapanabilmeli; hostname/OS/RPC ağacı gibi envelope verisi bile gönderilmemeli (veya en azından net belgelenmeli).
- **YAPMA:** repo yapısını (Merkle hash) satıcı sunucusuna itme — yerel indeksleme + yerel context zenginleştirme yeterli; repo yapısı dışarı çıkacaksa kullanıcı onayı şart.
- **Stall/heartbeat'ı watchdog ile birleştir:** Claude Code'un ölü watchdog'undan ders al; heartbeat'ları 0. fazdan itibaren başlat.
