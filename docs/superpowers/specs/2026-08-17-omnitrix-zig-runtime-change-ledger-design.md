# Omnitrix Zig Runtime ve Degisiklik Ledger Tasarimi

**Durum:** Taslak, kullanici onayi bekliyor  
**Tarih:** 2026-08-17  
**Kapsam:** Omnitrix runtime, TUI, dosya degisikligi gorunurlugu ve dil/altyapi sinirlari

## 1. Karar Ozeti

Omnitrix tek bir ana surec olarak tasarlanir. Runtime, scheduler, permission
broker, storage, network/async altyapisi, degisiklik ledger'i ve TUI ayni
calisma alaninin parcalaridir. Bu parcalar arasinda worker sureci, JSON/Protobuf
mesajlasmasi veya token basina serialization yoktur.

Ana uygulama dili Zig'dir. Bu, tum gelecekteki kodun ideolojik olarak Zig olmak
zorunda oldugu anlamina gelmez; her modul icin en uygun dil arastirilir. Ancak
farkli bir dili runtime'a sokmak, process veya haberlesme bloat'i getiriyorsa
bu tercih reddedilir. Dil secimi performans, bellek, baslangic, manevra
kabiliyeti, platform kapsami ve bakim borcu olcumleriyle verilir.

Mevcut xai-* Rust agaci kaynak kodu olarak kopyalanmaz. Davranis, algoritma,
durum makinesi, hata senaryosu ve test fikri havuzu olarak incelenir; hedef
uygulama Omnitrix'in secilen dilinde yeniden yazilir.

Urun adi her yerde **Omnitrix** olarak kullanilir. Kisa urun veya modul adi
kullanilmaz. Gelecekteki gelismis urun formu **Ultimatrix** olarak adlandirilir;
bu bir surum numarasi degildir.

## 2. Kesin Sinirlar

### 2.1 Temel mimaride reddedilenler

- Rust'u cekirdek temel yapmak
- Go provider worker'lari
- Python veya TypeScript worker'larini ana akisa sokmak
- C ABI'yi modul haberlesme sistemi yapmak
- cgo veya benzeri runtime kopruleri
- Her token icin JSON, Protobuf veya IPC
- Sadece Git diff'ine bakarak agent degisikligi cikarmak
- TUI ile runtime arasinda ikinci bir durum kaynagi tutmak
- Dil eksigi sebebiyle tasarimdan vazgecmek

### 2.2 C kodu icin kural

C, Zig'in eksigini kapatmak icin otomatik bir iletisim katmani olarak
kullanilmaz. Bir algoritmanin C olmasi gercekten gerekiyorsa, o algoritma
ayri, kucuk, statik bir leaf modul olarak yazilir ve kendi testleriyle
dogrulanir. Ana mimari C'ye baglanmaz.

Derleme seviyesinde farkli diller arasinda bir makine ABI'si bulunmasi teknik
olarak kacirilamaz; ancak bu, Omnitrix'in runtime haberlesme modeli degildir.
C leaf gerekli degilse hic C kullanilmaz.

## 3. Omnitrix Zig Altyapisi

Upstream Zig'in mevcut eksikleri kabul edilmis bir sinir degildir. Omnitrix,
gerekli altyapiyi kendi kaynak koduyla gelistirir.

### 3.1 omnitrix-io

Platform event loop ve dosya/socket IO katmanidir.

- Linux: epoll ve ilgili syscall katmani
- macOS/BSD: kqueue
- Windows: IOCP
- explicit allocator kullanimi
- bounded queue ve backpressure
- monotonic deadline
- cancellation token
- shutdown sirasinda bekleyen operasyonlarin deterministik bosaltilmasi

Bu modul upstream `std.Io.Evented` durumuna bagimli degildir. Gerekli kisimlar
Omnitrix icinde gelistirilir ve benchmark edilir. Ortak API platform farklarini
gizler; platform backend'leri gereksiz katman eklemez.

### 3.2 omnitrix-task

Ajan, tool, scheduler ve network operasyonlarinin task yasam dongusudur.

- task state: created, queued, running, waiting, cancelling, completed,
  failed
- bounded active task sayisi
- uyuyan ajanlar icin bosuna task/stack tutulmamasi
- cancellation'in yield veya IO noktasinda garanti edilmesi
- task leak, orphan child ve deadline asimi icin watchdog
- her task icin kaynak ve sahiplik kaydi

### 3.3 omnitrix-net

Provider ve streaming altyapisidir.

- socket ve DNS
- TLS
- HTTP request/response
- SSE streaming
- WebSocket gerektiginde
- provider timeout ve retry
- 401, 429, balance ve transport hatasi ayrimi
- response chunk'lari icin bounded buffer

Network stack'in eksik kisimlari dis worker'a atilmaz. Omnitrix icinde
gelistirilir veya olcumle daha uygun bir leaf modul olarak yeniden yazilir.

### 3.4 omnitrix-stream

Provider stream'i, tool output'u, markdown parcasi ve agent event'lerini tek
bir bounded stream modelinde birlestirir. TUI'ye giden her event'in revision'i
vardir; her event'te tum transcript yeniden serialize edilmez.

### 3.5 omnitrix-zig upstream politikasi

- Zig toolchain ve gerekli std degisiklikleri kontrollu fork'ta tutulur.
- Upstream otomatik merge edilmez.
- Her upstream commit inceleme, compile, unit test ve benchmark sonrasinda
  secilir.
- Sadece mantikli ve Omnitrix'e deger katan degisiklikler cherry-pick edilir.
- Upstream API degisirse Omnitrix kodu otomatik olarak kirilmaz; adapter ve
  fork politikasi tarafindan kontrol edilir.

## 4. Tek-Surec Durum Modeli

Tek authoritative state Omnitrix runtime icindedir.

```text
Omnitrix process
  StateStore
    AgentRuntime
    Scheduler
    PermissionBroker
    Storage
    ProviderNetwork
    FileMutationLedger
    TUI Renderer
```

Moduller arasinda dogrudan typed call, ortak immutable snapshot veya kontrollu
mutable state kullanilir. TUI, runtime'in durumunu kopyalayan ikinci bir
backend degildir; runtime tarafindan uretilen snapshot/event verisini gorur ve
kullanici aksiyonlarini dogrudan control katmanina verir.

## 5. Zig TUI

TUI Zig ile yazilir. Terminal altyapisi icin dusuk seviyeli, degistirilebilir
bir backend kullanilir; ustune Omnitrix'in kendi renderer'i kurulur.

### 5.1 Goruntu modeli

- conversation block listesi
- collapsed/expanded tool block'lari
- streaming markdown safe-tail render
- diff hunk goruntuleme
- changed-files paneli
- status, model, token ve task durumlari
- dar terminalde fullscreen diff fallback'i
- resize sonrasi secili dosya/hunk korunmasi

Hazir bir UI framework'u Omnitrix state modelinin sahibi olamaz. Terminal
backend'i degisse bile block, diff ve ledger modelleri degismez.

### 5.2 Degisiklik paneli

Panel, TUI'nin calistirildigi proje kokunu izler. Varsayilan davranis Git'e
bagli degildir; Git ek bir gorunum kaynagidir. `.git/` gibi VCS ic metadata
klasorleri, Omnitrix'in kendi cache/build klasorleri ve symlink ile proje
kokunun disina cikan yollar degisiklik paneline dahil edilmez. Proje icindeki
normal ignored dosyalar ise dahil edilir.

```text
Agent Changes
  M  src/main.zig             +12 -4   agent: executor
  A  notes/debug.txt           +8 -0   agent: juryrigg
  D  src/old.zig               +0 -31  agent: quick_fix

Project Changes
  M  README.md                         actor: user
  ?  ignored/cache.dat                 actor: external
```

Kayitli durumlar:

- added
- modified
- deleted
- renamed
- copied
- type_changed
- binary
- conflict
- mixed_actor
- stale
- unavailable

Git gorunumleri ayrica staged, unstaged, untracked, branch ve uncommitted
olarak sunulur. Git status'teki X/Y durumu kaybolmaz. Git'e ait olmayan
degisiklikler agent ledger'inda yine gorunur.

## 6. FileMutationLedger

Ledger, proje kokunun altindaki tum gercek dosya degisikliklerini izler.

```text
MutationRecord {
  revision
  path
  old_path?
  before_hash?
  after_hash?
  kind
  actor
  agent_id?
  session_id?
  turn_id?
  operation_id?
  additions?
  deletions?
  hunk_index?
  observed_at
  attribution_confidence
}
```

### 6.1 Mutasyon kaynaklari

- Omnitrix edit tool'u: islemden once ve sonra kesin hash
- Omnitrix write/create/delete tool'u: operation id ile dogrudan sahiplik
- shell komutu: islem sinirinda bounded pre/post snapshot
- filesystem watcher: degisiklik sinyali, authoritative sonuc degil
- kullanici veya dis surec: aktif agent islemi disinda gorulen degisiklik
- Git index ve HEAD: repository state icin ayri tarama

Watcher olaylari burst halinde debounce edilir. Ardindan dosya tekrar taranir;
yalnizca watcher olayina dayanarak liste gosterilmez. Dosya degisikligi
olmadigi kesinlesmeden `No changes` yazilmaz.

### 6.2 Attribution

Onceden kirli dosya agent tarafindan sahiplenilmis sayilmaz. Ayni dosyada iki
actor farkli hunk'lara dokunursa hunk sahipligi tutulur. Hunk ayrimi
guvenilir degilse dosya `mixed_actor` olur ve geri alma islemi sessizce
uygulanmaz.

Agent'in bir dosyayi okumasiyla dosyayi degistirmesi ayridir. Okuma `access`
aktivitesi olarak kaydedilebilir; changed-files paneline yalnizca content hash'i
degisen kayitlar girer.

### 6.3 Revert guvenligi

Bir dosya veya hunk geri alinmadan once:

1. Secilen scope kesinlestirilir.
2. Mevcut hash beklenen postimage ile karsilastirilir.
3. Araya giren user/external degisiklik varsa islem durur.
4. Recovery backup olusturulur.
5. Yalnizca secilen dosya/hunk uygulanir.
6. Sonuc tekrar okunur ve hash'lenir.
7. Ledger ve Git durumu yeniden taranir.

Hash uyusmazligi basarisizligi acikca gosterir; Omnitrix genis kapsamli
`reset --hard` veya butun worktree restore kullanmaz.

## 7. Paralel Is Kolları

Surum veya milestone isimleri kullanilmaz. Asagidaki kollar ayni anda
ilerler:

### Kol A: TUI ve changed files

Terminal backend, block renderer, diff viewer, FileMutationLedger gorunumu,
resize, Unicode ve PTY testleri.

### Kol B: Agent runtime ve Omnitrix altyapisi

omnitrix-io, omnitrix-task, omnitrix-net, streaming, provider baglantisi,
agent state machine ve scheduler.

### Kol C: Permission ve gerceklik kapisi

Tool broker, capability kurallari, deny/gizleme, cancellation, doom-loop,
operation sahipligi ve ledger attribution.

Kollar ortak contract ve test fixture'lari kullanir. Bir kol digerinin
tamamlanmasini beklemez; ancak authoritative state ve dosya ledger'i ortak
sozlesme olarak korunur.

## 8. Hata ve Kaynak Sinirlari

- Kuyruklar bounded olur; limitsiz event veya transcript buffer tutulmaz.
- Her task'in deadline ve cancellation yolu vardir.
- IO, task veya renderer panic/abort durumunda terminal state temizlenir.
- Provider, filesystem, Git ve parser hatalari ayri siniflandirilir.
- Stale veri current gibi gosterilmez.
- Buyuk patch'ler lazy hunk olarak tutulur.
- Dosya icerigi yerine hash, metadata ve bounded patch tutulur.
- Her uzun kosu RSS, allocation, event lag, first-frame ve shutdown ile
  olculur.

## 9. Dogrulama Kosullari

Tasarim uygulanirken su kosullar test edilmeden basarili kabul edilmez:

1. Agent, Gitignored dosyayi degistirir; panelde gorunur.
2. Agent, untracked dosya olusturur; path, hash ve hunk gorunur.
3. User ve agent ayni dosyanin farkli hunk'larini degistirir; actor ayrimi
   veya `mixed_actor` gorunur.
4. Dis surec dosya degistirir; agent degisikligi olarak yanlis etiketlenmez.
5. Agent shell komutuyla dosya degistirir; operation sonrasi ledger kaydi gelir.
6. Dosya commit edilir; repository uncommitted gorunumu guncellenir, session
   history kaydi korunur.
7. Staged ve unstaged ayni dosyada birlikte bulunur; iki durum kaybolmaz.
8. Rename ve delete durumlari path bilgisiyle gorunur.
9. Buyuk/binary/conflict dosyada sahte `0/0` gosterilmez.
10. Eski hash ile revert denenir; islem non-destructive sekilde reddedilir.
11. Terminal resize, Unicode ve uzun streaming transcript sonrasinda state
    ve secim korunur.
12. 24/7 soak testinde bounded queue, memory growth ve orphan task kapilari
    gecilir.

## 10. Sonraki Is

Bu belge onaylandiktan sonra:

1. Rust-merkezli dil politikasini ve worker/protocol onerilerini eski vizyon
   belgelerinden cikaran dokuman revizyonu yapilir.
2. Bu tasarimdan implementation plan uretilir.
3. Omnitrix kodu yazilmaya baslanir.

Bu belge, urun surumu tanimlamaz ve `v0.1` benzeri bir yapi kullanmaz.
