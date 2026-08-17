# 09 — Manevra Teknolojileri: DERİN Extraction

> **Amaç:** omnitrix'in "manevra hızı"nı (tarama, arama, render, gezinme gecikmeleri) belirleyecek üç teknolojinin veri yapıları, algoritmaları, karmaşıklık sınıfları, bellek profili ve omnitrix'e port notları.
>
> **Kaynaklar:**
> - Warp: `warpdotdev/warp` açık kaynak klonu (AGPL-3.0) → `warp/warp-terminal/` + warp.dev blog "The Block Model Behind Warp's Agentic Development Environment" (2026-04)
> - pi TUI: `pi/packages/tui/` yerel kaynak
> - oh-my-pi: `oh-my-pi/crates/pi-walker`, `pi-walker` + `pi-natives` yerel Rust kaynakları
>
> Tarih: 2026-08-14

---

## 1. Warp SumTree / BlockList Veri Modeli

### 1.1 Mimari özet

Warp'un terminali tek bir karakter ızgarası değil; **tiplenmiş blokların sıralı listesi** (`BlockList`). Her blok bir komut + çıktısı (terminal bloğu) ya da keyfi UI view (zengin içerik bloğu — agent konuşması). Geleneksel scrollback yerine **adreslenebilir, semantik çıktı birimleri**.

```
TerminalModel
 └─ BlockList
     ├─ blocks: Vec<Block>                          ← gerçek veri (sıralı)
     ├─ block_heights: SumTree<BlockHeightItem>     ← yükseklik indeksi
     ├─ block_id_to_block_index: HashMap<BlockId, BlockIndex>
     └─ removable_blocklist_item_positions: HashMap<RemovableBlocklistItem, TotalIndex>
```

Kaynak: `app/src/terminal/model/blocks.rs` (4262 satır), `app/src/terminal/model/block.rs`.

### 1.2 SumTree (crates/sum_tree)

İmza: `SumTree<T: Item>(Arc<Node<T>>)` — **kalıcı (persistent), paylaşılan yaprak yapısı**.

```rust
pub trait Item: Clone + fmt::Debug {
    type Summary: AddAssign<&'a Self::Summary> + Default + Clone + fmt::Debug;
    fn summary(&self) -> Self::Summary;
}
```

**Düğüm modeli** (`Node<T>`):
- `Leaf`: `items: ArrayVec<T, {2*TREE_BASE}>` + `item_summaries`
- `Internal`: `child_trees: ArrayVec<SumTree<T>, {2*TREE_BASE}>` + `child_summaries`

**TREE_BASE = 6** (üretim), 2 (test-util). Yükseklik ≤ 2·BASE eleman/alt-ağaç → dengeli, sığ ağaç.

**Kritik tasarım kararları:**
- `Arc` paylaşımı: ekleme sırasında `Arc::make_mut` ile **path-copying** (yalnız etkilenen yol kopyalanır) — yaprak üretiminde çift kopya yok, `push` amortize O(1) yaprak işi.
- `push_tree_recursive`: yükseklik farkı 0 → alt-ağaçları birleştir; fark 1 ve alt ağaç underflow değilse direkt ekle; aksi halde sağdaki çocuğa recursive. Alt-ağaç yüksekliği büyükse `from_child_trees` ile **yeni kök** kurulur (amortize O(log n)).
- `Dimension` trait'i: `extent::<Lines>()` (yükseklik toplamı), `TotalIndex` (eleman sayısı), `BlockIndex` (blok sayısı) gibi farklı toplam eksenleri tek yapıdan türetilir — özet ekseninden bağımsız genel arama.
- `Cursor` (cursor.rs, 1010 satır): istiflenmiş (stack-of-path) gezgin; `seek(&pos, SeekBias)` — **alt ağaç özetleri üzerinde toplama ile log(n) adımda iniş**. Zaten seek edilmiş durumda sağa kaymalar için "unwind stack" hızlı yolu. `slice`/`suffix`/`summary` operasyonları tek geçişte toplanan özetleri ayrıştırarak döner.
- `Edit<T>` (index + replacement): birden çok bloğu tek `edit` çağrısında toplu güncelleme.

### 1.3 BlockList ve BlockHeightItem

```rust
pub enum BlockHeightItem {
    Block(Lines),                    // terminal bloğu yüksekliği
    Gap(Lines),                      // bloklar arası boşluk (ctrl-l sonrası)
    RestoredBlockSeparator { ... },
    InlineBanner { ... },
    SubshellSeparator { ... },
    RichContent(RichContentItem),    // agent view — kendi yüksekliğini raporlar
}
```

**Model/görünüm ayrımı (zero-height gizleme):** Veri `Vec<Block>` içinde; görünüm SumTree'de `BlockHeightItem`. Gizli blok (filtrelenmiş agent komutu, tamamlama bloğu, view dışı konuşma) SumTree'de **yüksekliği 0** girdi olarak tutulur → renderer özel kod gerektirmeden üzerinden atlar. `BlockFilter { include_hidden, include_background }` görünürlüğü sorgular.

**Karmaşıklık tablosu:**

| Operasyon | Naif (Vec) | Warp (SumTree) |
|---|---|---|
| "Hangi bloklar A..B satırlarını kesiyor?" | O(n) | **O(log n)** |
| Aktif blok yüksekliği güncelleme (`update_active_block_height`) | O(n) | O(log n) |
| Görünüm noktası → blok & offset (`clamp_to_grid_points`) | O(n) | O(log n) |
| Append / push | amortize O(1) | amortize O(log n) |
| Araya blok ekleme / toplu düzenleme (`edit`) | O(n) | O(log n) |

### 1.4 GridStorage → FlatStorage (çift depolama)

Her terminal bloğu iki ızgara tutar: komut tarafı + çıktı tarafı. İki fiziksel temsil:

**1. `GridStorage`** (canlı satırlar): per-cell matris — `Vec<Row>`; satır/kolon koordinatlı hücre + stil. Cursor'ın erişebildiği (yazılabilir) bölge. `app/src/terminal/model/grid/grid_handler.rs`.

**2. `FlatStorage`** (`crates/warp_terminal/src/model/grid/flat_storage/mod.rs`) — scrollback için paketlenmiş buffer:
- `content: Content` — bayt dizisi, **chunk'lara bölünmüş** (BTreeMap<ByteOffset, Chunk>). Önden kesme (truncate_front) kopyasız — "dairesel olmayan dairesel buffer"; offsetler asla sıfırlanmaz, kuyruk işaretçisi yalnız ileri gider.
- `index: Index` — `VecDeque<Entry>` satır→bayt-ofset haritası. `Entry` **24 bayt** (64-bit), cache-line'a 2-3 sığar (static_assert ile sabitlenmiş). `content_offset`, `has_trailing_newline`, `ends_with_leading_wide_char_spacer` (yumuşak sarmada geniş karakter). Yalnızca `rows` ofsetlere anahtarlanır — **ofsetler sabit, satırlar düşse bile metadata taşınmaz**; resize'da yalnızca satır indeksi yeniden kurulur, stil verisi dokunulmaz.
- `fg_color_map`, `bg_and_style_map`, `hyperlink_id_map` — **AttributeMap = BTreeMap<ByteOffset, A> aralık (RLE) haritaları**; "biten ofset → değer" biçiminde. Değişimin olmadığı uzun koşular tek kayıt. Bitiş-ofsetine göre BTreeMap: nokta sorgusu O(log r) (r = renk değişim sayısı), ileri tarama O(log r + k). Stil verisi koordinat değil **bayt-ofset anahtarlı** olduğu için resize stil verisini taşımaz.
- `max_rows` sınırı: taşan satırlar önden düşer (`num_truncated_rows`).

**Desteklenen operasyonlar:** Index, Scan/Iterate, Push, Pop. **Insert yok** — orta dize ekleme flat buffer'da pahalıdır; scrollback zaten değişmez olduğu için sorun değil.

**Hibrit kullanım** (`grid_handler.rs:397`): `enum StorageRow { GridStorage(usize), FlatStorage(usize) }` — canlı bölge GridStorage, yukarı kayan her satır FlatStorage'a taşınır. Uzun sunucu loglarında canlı grid maliyeti yalnızca görünür son satırlar için ödenir. `MaximizeFlatStorage` feature flag ile sınır ayarlanır.

**Bellek profili:** GridStorage per-hücre (hücre × stil × atıf); FlatStorage paketli bayt + 24B/satır indeksi + RLE aralık haritaları. Aynı mantıksal içerik için GridStorage'ın **küçük bir kesri** (log çıktılarında tipik olarak 10-100×).

### 1.5 İki seviyeli sanallaştırılmış render

1. **BlockList seviyesi:** viewport ile kesişen bloklara sorulur (SumTree üzerinden O(log n)).
2. **Blok seviyesi:** görünür bloğun yalnızca viewport ile kesişen satırları render edilir.

BlockList içeriğe kayıtsızdır — yalnızca yükseklik ister. `BlockHeightItem` enum'u terminal ve zengin blokları tek listede birleştirir; renderer/indeks makinesi "konuşma"nın ne olduğunu bilmez.

### 1.6 omnitrix'e port notları

- **Nerede kullanılır:** omnitrix'in TUI'sinde (pi TUI tabanlıysa) komut/çıktı geçmişi listesi; agent geçmişi + terminal çıktısını tek scrollable akışta birleştirmek; uzun seanslarda blok yükseklik indeksleme.
- **Port maliyeti:** SumTree ~1.5k satır Rust — doğrudan vendor edilebilir (AGPL-3.0; omnitrix AGPL uyumluysa). FlatStorage da bağımsız crate (`warp_terminal` içinde modül olarak — ayrıca alınmalı).
- **Kritik dersler:**
  1. Özet eksenini (`Dimension`) genelleştir — yükseklik, eleman sayısı, blok sayısı tek yapıda.
  2. `Arc` + `make_mut` path-copying — blok güncellemelerinde kopya maliyetini sınırlar.
  3. Stili koordinattan değil **kalıcı ofsetten** anahtarla — resize'da sıfır taşıma.
  4. Gizlemeyi "sil" değil **zero-height** yap — renderer özel durum koduna gerek kalmaz.
  5. Model (Vec) ile indeks (SumTree) ayrımı — view filtreleme/collapse veriyi bozmaz.

---

## 2. pi Diferansiyel TUI Render

Kaynak: `pi/packages/tui/src/` — `tui.ts` (1263), `tui-main-screen.ts` (586), `tui-alt-screen.ts` (1314), `layout.ts` (410), `terminal.ts` (559).

### 2.1 Render akışı (üç aşama)

```
Component ağacı
  │ render(width)  ← her bileşen genişliğe göre ANSI'li string[] üretir
  ▼
LayoutFrame: layout pass + paint pass
  ├─ layoutComponent()  → LayoutBox ağacı (x,y,w,h + clip kesişimi)
  └─ paintBox()         → screen: string[] (satır başına bir ANSI'li satır)
  ▼
doRender() (TuiMainScreen)
  └─ önceki satırlarla fark → yalnız değişen satırlar yazılır
```

**Dirty tracking — renderCache:** `LayoutContext.renderCache: Map<Component, Map<width, string[]>>`. `renderCached()` aynı bileşeni aynı genişlikte ikinci kez render etmez. Bileşenler de kendi iç cache tutar (ör. `Text.cachedLines`; `invalidate()` → cache temizleme → bir sonraki frame'de zorunlu yeniden render). `TuiBase.invalidate()` ağacı dolaşıp tüm alt bileşenleri invalidate eder. Ölçüm (`measureHeight`/`measureWidth`) cache'e dokunur, bileşeni tekrar çalıştırmaz — **intrinsic boyut ölçümü render'ın kendisidir, ikinci kez yapılmaz.**

**ScrollView:** içerik tam boyutla layout edilir, `translateBox(childBox, deltaY)` ile scrollTop'a göre kaydırılır; `clip` (rect kesişimi) ekran dışını otomatik keser. `updateLayout(contentHeight, viewportHeight)` → viewport yetersizse `requestRender()` (kaydırma talebi render döngüsüne işaret gönderir).

**Paint:** `compositeTuiLine` ile overlay kompozisyonu; fast path — tam genişlik blok + boş hedef satır → string referansını doğrudan kopyala (ANSI segmentasyonu atlanır). OSC 133 zone prefix temizlenir; Kitty görüntüleri crop edilir.

### 2.2 Diferansiyel yazım (tui-main-screen.ts doRender)

Durum: `previousLines: string[]`, `hardwareCursorRow`, `previousViewportTop`, `maxLinesRendered`.

1. **Kıyas:** `firstChanged`/`lastChanged` — iki satır dizisi tek geçişte string-eşitlik ile karşılaştırılır O(H) (H = satır sayısı). `appendedLines` ise lastChanged'i sona iter.
2. **Büyük kırılımlar → fullRender:** ilk render, genişlik değişimi (wrap değişir), yükseklik değişimi (Termux hariç — yazılım klavyesi titremesini önlemek için), shrink + clearOnShrink, `firstChanged < prevViewportTop` (görünmeyen bölgeye dokunulamaz → tamamen temizle-yeniden çiz).
3. **Sadece değişen satırlar:** `\x1b[?2026h` (synchronized output başla) → değişen Kitty görüntülerini sil → `\x1b[{n}A/B` ile donanım cursor'unu `moveTargetRow`'a taşı → `firstChanged..lastChanged` aralığındaki satırları yaz (renderEnd sınırı — spinner'da tek satır flicker'sız güncellenir) → `\x1b[?2026l`.
4. **Silinen satırlar:** ekran kuyruğu aşağı kaydırılmadan `\x1b[2K` (erase-line) + `\x1b[{n}B/A` ile temizlenir; `extraLines > height` ise fullRender'a düş.
5. **Cursor yerleşimi:** `CURSOR_MARKER` bileşenlerin render'ında satıra gömülür; `extractCursorPosition` marker'ı bulur, `positionHardwareCursor` delta-move + `\x1b[{col}G` ile mutlak kolon — her frame'de cursor'ı son yazılan satıra taşımak yerine **donanım konumu takip edilir** ve yalnız fark yazılır.

### 2.3 Render zamanlama (tui.ts)

- `requestRender()` → `renderRequested` bayrağı ile **coalesce** (aynı tick'te N istem = 1 render), `process.nextTick` → `scheduleRender`.
- **16ms minimum aralık** (`MIN_RENDER_INTERVAL_MS`) throttle; `lastRenderAt` bakiye hesaplar (`max(0, 16 - elapsed)`).
- **Input yolu ayrıcalıklı:** klavye girişi `requestImmediateRender()` → throttle'dan geçmez (Windows'ta setTimeout(0) bile 16ms tick alabildiği için) — gecikme duyarlı giriş doğrudan render edilir.
- `renderNow(force)` → `resetRenderState()` (önceki buffer'ı sıfırla) + anında fullRender.
- Overlay stack modal katmanı: render öncesi `compositeOverlays` ile ana satırlara gömülür (diferansiyel kıyas **öncesinde** — overlay değişimi normal satır farkı gibi işlenir).
- Kitty görüntüleri: `collectKittyImageIds`/`deleteChangedKittyImages` — değişen aralıktaki görüntüler önce `\x1b_G...\x1b\\` ile silinir, `expandChangedRangeForKittyImages` çok satırlı görüntü bloklarını değişim aralığına katar.

### 2.4 Bellek ve karmaşıklık profili

| Öğe | Maliyet |
|---|---|
| Önceki ekran tamponu | H satır × (genişlik + ANSI segmentleri) — tam ekran kopyası |
| Render cache | bileşen × genişlik başına satır dizisi (layout pass içinde ölçeklenir) |
| Fark hesaplama | O(H) string karşılaştırma; satır başına ~100B-2KB |
| Yazım | yalnız değişen satırlar + O(1) cursor move — full write yok |
| Throttle | 16ms kap / input'da 0 |

**Amaç:** büyük ekranlarda bile frame başına yazılan bayt sayısını değişimle orantılı tutmak; `clearOnShrink=false` (PI_CLEAR_ON_SHRINK=0) ile yavaş terminallerde (SSH) tam temizliği kapatma seçeneği.

### 2.5 omnitrix'e port notları

- **Doğrudan taşınabilir:** paket `@earendil-works/pi-tui` — TypeScript + Node (process.stdin raw mode, ANSI). omnitrix'te zaten Node/TS ise değişikliksiz.
- **Alınacak dersler:**
  1. **Render cache'i ölçümle birleştir** — layout iki kez render ettirmez; ölçüm render'ın kendisidir.
  2. **İki aşamalı layout (ölç → yerleştir → boya)** vs. saf satır üretimi: klip kesişimi + scroll kaydırma tek yerde.
  3. **Diferansiyel karşılaştırmayı satır dizisi üzerinde** yap — hücre ızgarası taşıma; ANSI'li satır string eşitliği ucuz ve güvenli (stil değişimi satır değişikliği demektir).
  4. **Synchronized output** (`\x1b[?2026h/l`) — kısmi güncellemelerde yarım kare flaşını önler; her yazım bloğu sarılmalı.
  5. **Donanım cursor takibi** — cursor'u "bir yere koy, sonra geri al" yerine delta-move; her frame fazladan 2-3 sekans tasarrufu.
  6. **Input'u throttle'dan geçirme**; animasyon/spinner'ı 16ms'de tut — kullanıcı etkileşimi her zaman öncelikli.
  7. Termux/SIGWINCH ayrımları ve Kitty image yaşam döngüsü (id setleri) port sırasında korunmalı.

---

## 3. oh-my-pi: In-Process Natives + Walker

### 3.1 Katman yapısı

```
JS (packages/natives) → N-API (napi-rs) → Rust
  ├─ pi-natives   (grep/glob/fd/text/diff/clipboard/shell/pty/keys/...)
  ├─ pi-walker    (dizin tarama çekirdeği + ortak scan cache) ← pi-natives'in bağımlılığı
  ├─ pi-shell     (brush bash embeds + cancel + minimizer)
  └─ pi-builtins  (shell builtin'leri)
```

Fork/exec'siz tasarımın omurgası: tarayıcı + regex + glob **aynı süreçte, libuv iş parçacığı havuzunda (Task trait) çalışır**. Yalnızca gerçek komut çalıştırma PTY (portable-pty) üzerinden alt süreç açar.

### 3.2 pi-walker — paralel, ignore-aware tarayıcı

`crates/pi-walker/src/lib.rs` (141k) + `cache.rs` (19k).

**API:** `WalkRequest` builder → `collect()` / `collect_ranked()` / `stream()` / `for_each_entry()` / `collect_file_candidates()`. Yürüyüşçü kalıbı: ziyaretçi + heartbeat (128 kayıtta bir) + `WalkDecision { Include, Skip, SkipDescend, Stop }`.

**Veri yapıları:**
- `WalkOptions` (bitflag'li, `Hash + Eq` — **cache key'in parçası**): `include_hidden`, `use_gitignore`, `skip_git`, `skip_node_modules`, `follow_links` (Never/Roots/Always — `follow_at_depth`), `min_depth/max_depth`, `same_file_system`, `contents_first`, `detail` (Minimal/Full).
- `CollectedEntry` → `FileCandidate` (yol + type + mtime + size hint) — grep'in kuyruğuna giren paket.
- `FastIgnore` + `IgnoreState`: üç `ignore::gitignore::Gitignore` (`.ignore`, `.gitignore`, git exclude) katmanı; `state_from_entries` her dizin için ebeveynden türetilmiş durum kurar — `.gitignore`'lar **iniş sırasında kademeli derlenir**, kökler yalnız bir kez.
- `DirScratch` havuzu: `scratch_pool` — per-dizin kayıt buffer'ları geri dönüşümle yeniden kullanılır (`take_scratch`/`recycle_scratch`) → derin ağaçlarda allokasyon amortize edilir.
- `SymlinkAncestorStack` (dir identity): follow_links=Always iken döngü tespiti.

**Paralellik (cache.rs):**
- `WALK_POOL`: rayon `ThreadPool` — `PI_WALK_WORKERS` (default 4; 0=auto, 1=serial). İş parçacığı adları `pi-walker-N`.
- `parallel_for_each`: `should_parallelize(item_count)` → **eşik 256 dosya** altında serial. Böylece küçük ağaçlarda thread ek yükü sıfır.
- `parallel_for_each_init`: per-worker state (SearchWorker) ile `par_iter().try_for_each_init` — grep regex durumu worker başına bir kez kurulur.

**Yürüyüşçü:** DFS, seri (tek iş parçacığı), ama her dizin için tek `read_dir` syscall + buffer'da toplu işleme. İgnore kararı ve prune iniş öncesi; `WalkOrder::Path` için `scratch.sort_by_name()`.

**Scan cache (cache.rs):**
- `SCAN_CACHE: LazyLock<DashMap<CacheKey, CacheEntry>>` — **süreç içi, iş parçacığı güvenli** paylaşımlı cache.
- `CacheKey { root: PathBuf, options: WalkOptions }` — tüm karar bayrakları anahtarın parçası (farklı istekler çakışmaz).
- TTL: `FS_SCAN_CACHE_TTL_MS` (default **1000ms**); `FS_SCAN_EMPTY_RECHECK_MS` (200ms) — boş sonuç daha sık yeniden taranır (yanlış boş riski).
- `MAX_CACHE_ENTRIES` (16): LRU yerine **yaş bazlı eviction** (`evict_oldest`).
- `invalidate_path(path)` / `invalidate_all()`: JS tarafından dosya mutasyonundan sonra çağrılır (`invalidate_fs_scan_cache` N-API) — cache key root-of-path eşleşmesiyle kapsamlı geçersiz kılma.
- `collect_entries` → `get_or_scan` → TTL içindeyse `entries.clone()` (bellek kopyası — yine de sıfır syscall).

**Karmaşıklık:** DFS O(N) düğüm, syscall sayısı = dizin sayısı; cache hit → O(1) lookup + O(k) kopya (k = sonuç). Boş recheck sayesinde "yeni dosya oluştu" gecikmesi ≤ 200ms.

### 3.3 pi-natives — in-process ripgrep / glob / find

**grep.rs (95k — en büyük modül):**
- **İki katman:** `search()` (bellek içi içerik) + `grep()` (dosya sistemi). Motor: `grep-regex` (rust regex) ve `grep-pcre2` (PCRE2, JIT — macOS'ta JIT kapalı: SLJIT allocator hatası issue #7399; `OMP_PCRE2_JIT` ile zorla).
- Searcher: `grep-searcher` SearcherBuilder (multiline, context, BinaryDetection).
- **Köşe taşı:** `collect_grep_candidates` → `pi_walker::WalkRequest` (files_only + glob filter + gitignore + hidden + SkipSkippable) → `collect_file_candidates_with_heartbeat` — **kandidat keşfi walker'dan, regex pi-natives'te; ikisi de aynı süreç**.
- `MAX_FILE_BYTES = 4 MiB`: normal dosyalar tam okunur; **iri dosyalar iki geçişli plan** (`process_candidates`): pass 1 normal + hint'li irileri öne `deferred`'a al; pass 2 yalnız irilerin ilk 4MiB penceresi (mmap) — match bütçesi pass 1'de dolduysa pass 2 tamamen atlanır.
- Paralellik: `pi_walker::execute_candidates_init` + `should_parallelize(256)` eşiği; `SearchWorker` (per-worker regex state).
- Paylaşılan durum: `PassState { emitted: AtomicU64, results: Mutex<Vec<...>>, deferred: Mutex<Vec<...>> }` — erken çıkış bütçesi (`stop_after_matches`) atomik sayaçla.
- `GrepOutputMode`: content / count / filesWithMatches.

**glob.rs:** `GlobOptions` (pattern, recursive, hidden, gitignore, cache, sort_by_mtime, signal, timeout) → `run_glob`: `CompiledWalkGlob` (globset) + walker cache; mtime sıralı sonuçlar `compare_matches_by_rank`. `apply_file_type_filter` symlink hedefi çözer.

**fd.rs — fuzzy find:** `fuzzy_find` → walker kandidatları üzerinde **fuzzy subsequence skoru** (`fuzzy_subsequence_score`) + `TopMatches` (kapasiteli yığın) + `ranked` sıralama; path derinliği skora katar (`path_depth`).

**Tüm ağır işler `task.rs` üzerinden:** napi `Task` trait → libuv thread pool; `CancelToken` (timeout + AbortSignal) — `heartbeat()` periyodik kontrol; `profile_region` ile örnekleme (döngüsel buffer, `get_work_profile`). CPU-işleri libuv ana döngüsünü asla bloklamaz.

### 3.4 pi-shell — brush bash in-process

- `pi-shell` → `brush_core` (Rust ile yazılmış bash — **fork/exec'siz gömülü kabuk**).
- `execute_shell` / `execute_shell_streams` / `StreamSinks` — komut metni parse edilir (brush AST), segmentli çalıştırılır; **çıktı minimizer** (`MinimizerResult`, TOML ayarları + xxHash64 trust gate): örn. `git`, `find`, `grep`, `cat` çıktıları akıllıca küçültülür (kaynak dosya özetleme `source_outline_level`).
- `pi-builtins`: `utility_builtins`, `process_builtins` — brush'un builtin'lerini pi'ninkilerle değiştirir (pi üzerinden bildirim).
- PTY akışı (`pi-natives/pty.rs`): `portable_pty` + child pgroup; gerçek interaktif komutlar için.
- `cancel.rs`: `CancelToken` çekirdeği hem pi-natives hem pi-walker heartbeat'inin kaynağı.

### 3.5 Bellek profili

| Bileşen | Profil |
|---|---|
| Scan cache | 16 root × sonuç vektörü; TTL 1s — mutasyon sonrası invalidate |
| Grep pass 1 | dosya başına anlık buffer (≤4MiB okuma) — seri; kandidat listesi tamamı bellekte |
| Grep pass 2 | iri dosyaların ilk 4MiB'ı mmap — dosya başına sabit |
| Walk | DFS + DirScratch havuzu (per-dizin buffer geri dönüşümü) |
| Parallel | rayon 4 thread; <256 öğede 0 thread |

### 3.6 omnitrix'e port notları

- **En yüksek kaldıraç:** `pi-walker` cache + `parallel_for_each` deseni — "her arama taze syscall" yerine TTL'li ortak tarama; omnitrix'in her yerinde (file picker, grep, symbol index, workspace taraması) **tek ortak cache** paylaşılır.
- **Eşik 256 dosya** kuralı: küçük iş yükünde thread havuzu tamamen atlanır — omnitrix'te mikro-girişimler (tek dosya glob) serial kalmalı.
- **İki geçişli grep** (normal → iri): büyük repo aramalarında ilk sonuç gecikmesini (TTFB) düşürür; budget erken dolunca iri geçişi sıfır maliyet.
- **CancelToken heartbeat** her 128 kayıtta: agent kullanıcı arayüzünde iptal duyarlılığı (uzun taramayı Ctrl+C ile durdurma) — omnitrix'te zorunlu desen.
- **brush bash:** komut çıktısı minimizer (git/grep/find/cat) — omnitrix'in shell execution katmanında token tasarrufu: çıktıyı model bağlamına sokmadan önce küçült.
- N-API katmanı omnitrix'te yoksa: `pi-walker` ve `pi-shell` saf Rust — doğrudan bağımlılık; yalnız `pi-natives` N-API'ye bağlı.

---

## 4. Özet Karşılaştırma ve omnitrix Eylem Planı

| Teknoloji | Çekirdek yapı | Karmaşıklık | omnitrix kazancı |
|---|---|---|---|
| Warp SumTree | kalıcı B-tree, özet aksları | viewport sorgusu O(log n) | binlerce blokluk agent+terminal geçmişinde sıfır gezinme takılması |
| Warp FlatStorage | paketli buffer + ofset indeks + RLE stil | append O(1) amortize, stil sorgusu O(log r) | uzun çıktıların bellek ayak izi 10-100× |
| pi TUI diff render | satır-dizi farkı + render cache + 16ms koalesans | fark O(H), yazım O(değişim) | çerçeve başına sabit maliyet, flicker yok |
| pi-walker | TTL'li DashMap cache + rayon eşikli | cache hit O(1), tarama O(N) | tekrarlı taramalar syscall'sız |
| pi-natives grep | grep-searcher + 2 geçişli plan | paralel, budget erken çıkış | büyük repo aramasında ilk sonuç gecikmesi düşük |
| brush bash | gömülü Rust kabuk + minimizer | parse + segmentli run | komut çıktısı bağlam öncesi küçültülür |

**Önerilen port sırası:**
1. pi-walker (cache + eşikli paralellik) — en hızlı kazanç, bağımsız crate.
2. pi TUI diff render desenleri (render cache, synchronized output, cursor delta, input önceliği) — mevcut TUI'ye aşılanabilir.
3. SumTree → omnitrix geçmiş/çıktı listesi; FlatStorage → uzun çıktı tamponlama (AGPL-3.0 lisans uyumu kontrol edilmeli).
4. brush minimizer → agent komut çıktı hattı.
