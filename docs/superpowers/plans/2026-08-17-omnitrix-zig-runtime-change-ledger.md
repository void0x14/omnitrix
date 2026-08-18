# Omnitrix Zig Runtime ve Değişiklik Ledger — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.
>
> **Kaynak tasarım:** [`docs/superpowers/specs/2026-08-17-omnitrix-zig-runtime-change-ledger-design.md`](../specs/2026-08-17-omnitrix-zig-runtime-change-ledger-design.md) — onaylandı (2026-08-17).
> **Dil:** Tasarımın dili (Türkçe) korunur; plan checkbox sözdizimini kullanır.

**Goal:** Omnitrix'i Zig'de tek-süreç runtime olarak inşa etmeye başlamak: omnitrix-io,
omnitrix-task, omnitrix-net, omnitrix-stream, FileMutationLedger ve TUI; eski Rust xai-*
ağacı kaynak olarak kopyalanmaz (fikir/test havuzu), worker/JSON/Protobuf IPC yok.

**Architecture:** Tek ana süreç. Authoritative state runtime içindedir
(`StateStore` → AgentRuntime, Scheduler, PermissionBroker, Storage, ProviderNetwork,
FileMutationLedger, TUI Renderer). Modüller arasında doğrudan typed call, ortak immutable
snapshot veya kontrollü mutable state kullanılır; TUI ikinci durum kaynağı değildir
(tasarım Bölüm 4). Üç paralel iş kolu (Bölüm 7) ortak sözleşmeleri paylaşır:
MutationRecord, task state, stream/revision modeli, TUI görüntü modeli.

**Tech Stack:** Zig (kontrollü fork politikası, Bölüm 3.5) · platform IO: Linux epoll,
macOS/BSD kqueue, Windows IOCP (kendi katmanı — upstream `std.Io.Evented`'e bağımlı değil)
· explicit allocator · git yalnızca ek görünüm kaynağı (Bölüm 5.2).

**KRİTİK KISIT (tüm task'lar için):** Üretim yolunda `catch unreachable` / `@panic` /
unwrap yok (I6 disiplini, Bölüm 8). Kuyruklar bounded; limitsiz buffer tutulmaz. Her
task, Bölüm 9 doğrulama koşullarından en az birine karşılık gelir.

---

## Dosya Yapısı

```
zig/
├── build.zig                  # exe + test hedefleri (0.17 Build API)
├── build.zig.zon
└── src/
    ├── root.zig                # modül kökü
    ├── omnitrix-io/            # Bölüm 3.1 — event loop + deadline/cancel/queue
    ├── omnitrix-task/          # Bölüm 3.2 — task state makinesi + scheduler
    ├── omnitrix-net/           # Bölüm 3.3 — provider hata taksonomisi + HTTP/SSE (sonra)
    ├── omnitrix-stream/        # Bölüm 3.4 — bounded stream + revision
    ├── omnitrix-ledger/        # Bölüm 6 — FileMutationLedger + MutationRecord + scan
    └── omnitrix-tui/           # Bölüm 5 — Kol A (sonraki faz)
```

---

## Faz 0 — Ortak sözleşmeler ve iskelet

- [x] **T0.1** `zig/build.zig` + `build.zig.zon`: `zig build` exe, `zig build test` test
  hedefi üretir (0.17 Build API: `addExecutable`/`addTest`/`addRunArtifact`).
- [x] **T0.2** `zig/src/root.zig`: tüm alt modülleri dışa verir; her modül `test` bloğu
  içerir.
- [x] **T0.3** Ortak tip iskeletleri: `MutationKind`, `Actor`, `TaskState` enum'ları
  (tasarım 5.2, 3.2) + hash yardımcısı (tasarım 6.0).

---

## Kol A — TUI ve changed files (Bölüm 5)

- [x] **A1** Terminal backend katmanı (düşük seviye, değiştirilebilir); Omnitrix renderer'ı
  backend'e bağlanmaz (5.1).
- [x] **A2** Block renderer: conversation block listesi, collapsed/expanded tool block'ları,
  streaming markdown safe-tail render (5.1).
- [x] **A3** Diff hunk görüntüleme + dar terminalde fullscreen diff fallback'i (5.1).
- [x] **A4** Changed-files paneli: Agent Changes / Project Changes ayrımı, git X/Y durumu
  korunur, `.git/` ve Omnitrix cache/build klasörleri hariç, normal ignored dosyalar dahil
  (5.2). **Kapanır: Doğrulama 1, 2, 6, 7, 8, 9.**
- [x] **A5** Resize sonrası seçili dosya/hunk korunması + Unicode testleri (5.1).
  **Kapanır: Doğrulama 11.**
- [x] **A6** PTY testleri (5.1; pager-pty-harness dersi).

## Kol B — Agent runtime ve Omnitrix altyapısı (Bölüm 3)

- [x] **B1** omnitrix-io: monotonic deadline, cancellation token, bounded queue +
  backpressure (3.1). **Kapanır: Doğrulama 12 (bounded queue).**
- [x] **B2** omnitrix-io: platform event loop (Linux epoll) + ortak API; platform
  backend'leri gereksiz katman eklemez (3.1).
- [x] **B3** omnitrix-io: kqueue (macOS/BSD) ve IOCP (Windows) backend'leri + shutdown'da
  bekleyen operasyonların deterministik boşaltılması (3.1).
- [x] **B4** omnitrix-task: task state makinesi
  (created/queued/running/waiting/cancelling/completed/failed) + bounded aktif sayı +
  cancellation'ın yield/IO noktasında garantisi (3.2).
- [x] **B5** omnitrix-task: watchdog (task leak, orphan child, deadline aşımı) + kaynak/
  sahiplik kaydı (3.2). **Kapanır: Doğrulama 12.**
- [x] **B6** omnitrix-net: socket+DNS, HTTP request/response, SSE streaming, bounded
  response buffer; provider timeout/retry; 401/429/balance/transport ayrımı (3.3).
  **Kapanır: Doğrulama 5 (hata ayrımı).**
- [x] **B7** omnitrix-stream: provider stream, tool output, markdown parçası, agent event'i
  tek bounded stream modelinde; her event'in revision'ı, transcript yeniden serialize
  edilmez (3.4). **Kapanır: Doğrulama 11.**
- [x] **B8** Agent state machine + scheduler (StateStore içinde), provider bağlantısı
  (3.2, 4).

## Kol C — Permission ve gerçeklik kapısı (Bölüm 3.2, 5.2, 6)

- [x] **C1** Tool broker + capability kuralları + deny → gizleme (model yasak tool'u
  görmez); doom-loop sayacı (3.2 dersi, 2.2). **Kapanır: Doğrulama 4.**
- [x] **C2** Operation sahipliği: Omnitrix edit/write/create/delete tool'ları
  operation_id ile doğrudan sahiplik; edit öncesi/sonrası kesin hash (6.1).
  **Kapanır: Doğrulama 1, 2.**
- [x] **C3** Shell komutu değişikliği: işlem sınırında bounded pre/post snapshot (6.1).
  **Kapanır: Doğrulama 5.**
- [x] **C4** Attribution: önceden kirli dosya sahiplenilmez; hunk sahipliği tutulur;
  güvenilmez ayrımda `mixed_actor`; okuma ≠ değiştirme (6.2). **Kapanır: Doğrulama 3, 4.**
- [x] **C5** Revert güvenliği: scope kesinleştir → hash karşılaştır → araya giren
  user/external değişiklikte dur → recovery backup → yalnızca seçili uygula → hash'le →
  ledger/git yeniden tara; `reset --hard` / whole-worktree restore YOK (6.3).
  **Kapanır: Doğrulama 10.**
- [x] **C6** Filesystem watcher: burst debounce + ardından dosya yeniden taranır;
  watcher'a dayanarak liste gösterilmez; `No changes` ancak kesinleşince (6.1).
  **Kapanır: Doğrulama 9.**

---

## Bölüm 9 Doğrulama Koşulları → Task Eşlemesi

| # | Koşul | Task |
|---|---|---|
| 1 | Gitignored dosya değişir; panelde görünür | A4, C2 |
| 2 | Untracked dosya oluşur; path/hash/hunk görünür | A4, C2 |
| 3 | User+agent farklı hunk; actor ayrımı / mixed_actor | C4 |
| 4 | Dış süreç değişikliği agent olarak etiketlenmez | C1, C4 |
| 5 | Shell komutu değişikliği; operation sonrası ledger kaydı | B6, C3 |
| 6 | Dosya commit edilir; repository uncommitted görünümü güncellenir, history korunur | A4 |
| 7 | Staged+unstaged birlikte; iki durum kaybolmaz | A4 |
| 8 | Rename/delete path bilgisiyle görünür | A4 |
| 9 | Büyük/binary/conflict dosyada sahte 0/0 yok | C6, A4 |
| 10 | Eski hash ile revert non-destructive reddedilir | C5 |
| 11 | Resize/Unicode/uzun streaming sonrası state+seçim korunur | A5, B7 |
| 12 | 24/7 soak: bounded queue, memory, orphan task kapıları | B1, B5 |

## Bölüm 8 Hata/Kaynak Sınırları → Zorlama Noktası

- Kuyruklar bounded → B1, B7 · task deadline/cancellation → B4, B5 · IO/task/renderer
  panic'inde terminal temizliği → A1 · hata sınıflandırması ayrı → B6, C1 · stale data
  current gösterilmez → C6 · büyük patch lazy hunk → A3 · dosya içeriği yerine hash +
  metadata + bounded patch → C2, C5 · uzun koşu ölçümleri (RSS, event lag, first-frame,
  shutdown) → Faz 9 (aşağıda) + B1-B5.

---

## Faz 9 — Ölçüm ve Soak (Bölüm 8, Doğrulama 12)

- [x] **F9.1** Her uzun koşu: RSS, allocation, event lag, first-frame, shutdown ölçümü.
- [x] **F9.2** 24/7 soak kapısı: bounded queue, memory growth, orphan task — müdahale
  gerektiren hata = 0.

## Notlar

- Bu plan Bölüm 10'un üç maddesinden ikincisidir; (1) doküman revizyonu tamamlandı
  (Task 1), (3) kod başlangıcı Faz 0 + ilk çekirdek modüllerle başladı (Task 3).
- Kollar birbirini beklemez; authoritative state + dosya ledger'ı ortak sözleşmedir (Bölüm 7).
