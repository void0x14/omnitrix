# Sağlayıcılar ve Anahtarlar

Bu kılavuz, omnitrix'in anahtar (API key) altyapısını A'dan Z'ye anlatır: şifreli **keychain**, **sistem anahtarlığı (OS keyring)** entegrasyonu, `/import` ve `/export` komutları, `/keys` anahtar yöneticisi, `/connect` sağlayıcı sihirbazı ve `grok keys` CLI komutları.

Anahtar akışı üç yüzeyde çalışır:

- **`/keys`** — şifreli omnitrix keychain'ini yönetir (ekleme, düzenleme, silme, arşiv export/import, harici araç senkronu).
- **`/import <araç>` / `/export <araç>`** — desteklenen harici kodlama araçlarının credential dosyalarıyla çift yönlü, çakışma-güvenli senkron yapar.
- **`/connect`** — sağlayıcı/model seçer ve keychain'deki bir anahtarla oturumu bağlar.

Ayrıca tüm bu işlemler `grok keys …` CLI komutlarıyla da yapılabilir.

---

## Keychain nedir?

Keychain, tüm API key'lerinizi saklayan **şifreli** bir deposudur. Dosya konumu:

```
~/.grok/keychain.omx
```

(`GROK_HOME` ayarlanmışsa onun altında.)

Güvenlik modeli:

- **Şifreleme:** AES-256-GCM (nonce + ciphertext + tag, base64). Key'ler diskte asla düz metin bulunmaz.
- **Anahtar türetme:** KDF **Argon2id** (64 MiB parametreleri) — master password'ünüzden 32 byte'lık şifreleme anahtarı türetilir; salt dosyada saklanır.
- **Metadata düz metin, sırlar şifreli:** kategori, provider adı, maskelenmiş key, model, son kullanım tarihi ve ID gibi bilgiler okunabilir; **ham key yalnızca şifreli payload içindedir**.
- **RAM sıfırlama:** Bellekteki ham key kopyaları `zeroize` ile sıfırlanır (Reveal kapanınca, oturum bitince).
- **Gösterim kuralı:** Tüm listeler, önizlemeler ve özetler key'leri **maskeli** gösterir — örn. `sk-abc…wxyz` (ilk 3 + `…` + son 4 karakter). Ham key yalnızca `/keys` içindeki **Reveal** modunda ve `grok keys show <id>` ile görünür.

> [!TIP]
> Keychain kilidi, oturumdaki keychain açılışından ibarettir: `grok keys …` her alt komutta keychain'i açar; TUI'de ise `/keys`, `/import` ve `/export` kilitliyken master password ister.

## Master password ve sistem anahtarlığı (keyring)

İlk kullanımda keychain dosyası yoktur; sizden yeni bir **master password** belirlemeniz istenir (tek seferlik giriş — onay tekrarı yoktur):

```
yeni keychain: master password belirle:
```

Bu şifre girdiğinizde terminal echo'su kapatılır; yazdıklarınız ekrana yansımaz. Sonraki açılışlarda prompt şu şekildedir:

```
keychain master password:
```

**Sistem anahtarlığı (OS keyring):** Master password'ü yalnızca **bir kez** girersiniz. Başarılı açılışta şifre otomatik olarak işletim sisteminin anahtarlığına yazılır:

- Linux: **Secret Service** (GNOME Keyring / KWallet — dbus üzerinden libsecret); Secret Service daemon'ı yoksa **keyutils** çekirdek anahtarlığına düşer.
- macOS: Apple anahtarlık (Keychain).
- Windows: Windows Credential Manager.

Kayıt `omnitrix-keychain` servisi altında `master` kullanıcısı olarak saklanır. Kayıt mevcutsa sonraki tüm açılışlar (CLI ve TUI) şifreyi anahtarlıktan **sessizce** okur — yeniden prompt gösterilmez (sessiz otomatik açılış / auto-unlock). Yeniden başlatma (reboot) anahtarlıktaki kaydı **etkilemez**; sistem anahtarlığı kalıcıdır, dolayısıyla yeniden başlatma sonrası da otomatik açılış çalışır.

> [!WARNING]
> Anahtarlık erişilemezse (Secret Service daemon'u yok, dbus kapalı) master password her seferinde sorulur; anahtarlığa yazma başarısızlığı sessizce atlanır. Şifreyi unutursanız keychain **kurtarılamaz** — şifreli dosyayı açmanın başka yolu yoktur.

## /import `<araç>`

Harici bir kodlama aracının credential dosyasındaki API key'lerini şifreli omnitrix keychain'ine çeker.

```
/import <stack>
```

**Argümansız çağrı** desteklenen tüm stack'leri ve yön kapasitelerini listeler:

```
kullanım: /import <stack> — desteklenen stack'ler:
  opencode         import ✓  OpenCode
  kilo             import ✓  Kilo CLI
  …
```

`/import` yazarken `<Tab>` argüman tamamlama, yalnızca içe aktarma destekleyen araçları önerir.

**Akış:**

1. Keychain açıksa (veya sistem anahtarlığındaki şifreyle sessizce açılabiliyorsa) senkron **anında** çalışır.
2. Kilitliyse `/keys` anahtar yöneticisi unlock modunda açılır (`/import <araç> — keychain şifresi` başlığıyla); şifre girilince işlem otomatik sürer.
3. Sonuç, aktif oturumun scrollback'ine **system bloğu** olarak yazılır; oturum yoksa toast olarak gösterilir. Modal (açıldıysa) kapanır.
4. Birleştirme **çakışma-güvenli**dir: keychain'de aynı kategori/sağlayıcıda kayıt zaten varsa gelen kopya **atlanır**, mevcut kayıt asla ezilmez.

**Örnek çıktı:**

```
stack → omnitrix [opencode]: 3 key · conflict atlandı: 1 · diğer atlanan: 0 · üzerine yazılan: 0 · /home/user/.local/share/opencode/auth.json
```

Tüm adaylar zaten kayıtlıysa:

```
stack → omnitrix [opencode]: güncel — 3 key zaten kayıtlı (üzerine yazılmadı) · /home/user/.local/share/opencode/auth.json
```

### Desteklenen stack'ler

| ID | Araç | Okunan dosya / kaynak | İçe aktar | Dışa aktar |
|---|---|---|---|---|
| `opencode` | OpenCode | `auth.json` (provider → api key haritası) | ✓ | ✓ |
| `kilo` | Kilo CLI | `kilo/auth.json` (OpenCode-uyumlu harita) | ✓ | ✓ |
| `pi` | pi coding-agent | `~/.pi/agent/auth.json` | ✓ | ✓ |
| `codex` | OpenAI Codex CLI | `~/.codex/auth.json` (`OPENAI_API_KEY` + token'lar) | ✓ | ✓ |
| `claude-code` | Claude Code | `~/.claude/.credentials.json` + `settings.json` env | ✓ | ✓ |
| `gemini-cli` | Gemini CLI | `~/.gemini/settings.json` / `oauth_creds.json` | ✓ | — |
| `hermes` | Hermes Agent | `~/.hermes/auth.json` (`credential_pool`) | ✓ | ✓ |
| `aider` | Aider | `.aider.conf.yml` | ✓ | ✓ |
| `continue` | Continue.dev | `~/.continue/config.json` | ✓ | ✓ |
| `copilot-cli` | GitHub Copilot CLI | `~/.copilot` + `github-copilot` config | ✓ | — |
| `cursor` | Cursor | Cursor auth/mcp (okunabilir dosyalar) | ✓ | — |
| `windsurf` | Windsurf | Windsurf/Codeium config | ✓ | — |
| `cline` | Cline | VS Code/Cursor Cline globalStorage | ✓ | — |
| `roo` | Roo Code | Roo Code globalStorage | ✓ | — |
| `kilocode` | Kilo Code (VS Code) | Kilo Code extension storage | ✓ | — |
| `qwen` | Qwen Code | `~/.qwen` auth | ✓ | ✓ |
| `kimi` | Kimi CLI | `~/.kimi` auth | ✓ | ✓ |
| `crush` | Crush | `~/.crush` auth | ✓ | ✓ |
| `antigravity` | Google Antigravity | Antigravity auth | ✓ | — |
| `kiro` | Kiro | AWS Kiro auth | ✓ | — |
| `amp` | Amp | Amp auth | ✓ | ✓ |
| `factory` | Factory Droid | Factory auth | ✓ | ✓ |
| `poolside` | Poolside | `~/.config/poolside/credentials.json` | ✓ | ✓ |
| `higgsfield` | Higgsfield | `~/.config/higgsfield/credentials.json` | ✓ | ✓ |
| `context7` | Context7 | `~/.context7/credentials.json` | ✓ | ✓ |
| `dotenv` | .env dosyası | Proje/home `.env` (`OPENAI_API_KEY=…`) | ✓ | ✓ |
| `env` | Süreç ortamı | Çalışan shell ortam değişkenleri | ✓ | — |
| `json-scan` | Özel JSON (tara) | Herhangi bir auth/credentials JSON — alan tarama | ✓ | — |

> [!TIP]
> `env` (süreç ortamı) için dosya yoktur — key'ler doğrudan shell ortam değişkenlerinden okunur (`/import env`). `json-scan` için CLI'da `--path` ile hedef dosyayı belirtmeniz gerekir; TUI'de credential path adayı olmadığından "path bulunamadı" hatası döner.

## /export `<araç>`

Omnitrix keychain'indeki API key'lerini harici bir kodlama aracının credential dosyasına yazar (ör. `auth.json`, `settings.json`, `.env`). `/import`'ün ters yönüdür: omnitrix → araç.

```
/export <stack>
```

**Akış** `/import` ile aynıdır: keychain açık veya anahtarlıkla açılabilir durumdaysa anında çalışır; kilitliyse önce unlock akışı açılır; sonuç scrollback'e system bloğu olarak yazılır. Birleştirme yine **çakışma-güvenli**dir — hedef dosyada aynı provider/key zaten varsa üzerine **yazılmaz** (mevcut kayıt kazanır).

Hedef dosya bulunamazsa aracın varsayılan yoluna **yeni dosya oluşturularak** yazılır. Yalnızca dışa aktarma destekleyen formatlar yazılır; salt-okuma stack'ler (`env`, `json-scan`, `cursor`, …) "export desteklemiyor" hatası verir.

**Araç başına yazım biçimleri:**

| ID | Dışa aktarma davranışı |
|---|---|
| `opencode` / `kilo` / `pi` | `auth.json` içine provider haritası yazar: `{"openai": {"type": "api", "key": "…"}}` |
| `claude-code` | `settings.json` içine `env` bloğu yazar: `"env": {"ANTHROPIC_API_KEY": "…", "OPENAI_API_KEY": "…"}` |
| `codex` | `auth.json` içine `OPENAI_API_KEY` alanı yazar |
| `dotenv` | `OPENAI_API_KEY=…` satırları yazar (provider → env adı eşlemesi) |
| `aider` | `.aider.conf.yml` içine `openai-api-key:` / `api-key:` listesine ekler |
| `continue` | `config.json` provider `apiKey` alanlarına ekler |
| `hermes` | `auth.json` `credential_pool` yapısına ekler |

**Örnek çıktı:**

```
omnitrix → stack [dotenv]: 2 key · conflict atlandı: 0 · diğer atlanan: 0 · üzerine yazılan: 0 · /home/user/.env
```

## /keys

Şifreli keychain anahtar yöneticisi. `/keys` ile açılır.

- Keychain **kilitliyse** önce master password istenir (Unlock modu); yanlış şifrede "yanlış master password (veya bozuk dosya)" gösterilir. Başarılı açılışta şifre sistem anahtarlığına kaydedilir, OpenCode taraması arka planda başlar ve bakiye sorguları tetiklenir.
- Keychain **açıksa** `/keys` açılır açılmaz arka planda OpenCode credential taraması çalışır: kurulu `auth.json` bulunur, yalnızca API key kayıtları (OAuth ve bilinen key-olmayan kayıtlar yok sayılır) şifreli keychain'e içe aktarılır, mevcut omnitrix kayıtları korunur, tek `save` ile kalıcılaştırılır ve katalogdaki ilk kullanılabilir provider/model otomatik etkinleştirilir.

**Tuş haritası:**

| Tuş | Eylem |
|---|---|
| `↑` / `↓`, `j` / `k` | Seçimi hareket ettir. |
| `r` | Seçili key'i **Reveal** modunda göster (keychain erişimi açıkken). |
| `a` | Sağlayıcı key'i ekle. |
| `e` | Seçili kaydın modelini, base URL'ini veya key'ini düzenle. |
| `x` | Onaydan sonra seçili key'i sil. |
| `X` | Onaydan sonra seçili **kategoriyi** (içindeki tüm key'lerle) sil. |
| `c` | Kategorileri gez ve aktif/varsayılan kategoriyi ayarla. |
| `E` | Tüm key'leri veya bir kategoriyi şifreli arşive dışa aktar. |
| `I` | Şifreli omnitrix arşivini içe aktar. |
| `S` | Desteklenen harici araçlar için stack import/export akışını açar. |
| `Tab` / oklar | Form alanları arasında gezin. |
| `Ctrl+T` | Geçerli password/key editörünü göster veya maskele. |
| `Enter` | Odaklanan eylemi çalıştır. |
| `Esc` | Geri dön veya kapat. |

Fare ile satır seçimi ve footer tıklamaları aynı tuş eylemlerine çevrilir; böylece işaretçi ve klavye yolları aynı doğrulamayı ve yan etkileri paylaşır.

**Kilit/açık akışı:** Unlock modunda girilen master password `Keychain::open` ile doğrulanır; başarıda Browse moduna geçilir, satırlar yüklenir, anahtarlığa yazılır ve arka plan taramaları başlar. Hata durumunda `Unlock` modunda hata mesajı gösterilir.

**Export (E):** Kapsam *tüm key'ler* veya *tek kategori* olabilir. **Ayrı bir export şifresi** sorulur (şifre + tekrar; boş veya eşleşmeyen şifre reddedilir). Çıktı varsayılan olarak `~/.grok/keychain-export-<unix_zaman_damgası>.omx` yoluna yazılır. Arşiv, master password'den bağımsız kendi salt + Argon2id + AES-256-GCM ile şifrelenir; metadata düz metin, key'ler şifrelidir.

**Import (I):** Dosya yolu ve arşiv şifresi istenir; içerik mevcut keychain ile birleştirilir (modal akışında çakışan kayıtlar üzerine yazılır) ve özet gösterilir.

**Stack senkronu (S):** Bir stack seçildiğinde önizleme ekranı çıkar — yön (`stack → omnitrix` veya `omnitrix → stack`), dosya yolu, aday sayısı ve conflict sayısı, her aday için `[yeni]`/`[CONFLICT]` işaretli maskeli satırlar (ilk 12, sonrası `… +N daha`). `Enter`: çalıştır (merge, conflict atlanır) · `Esc`: geri.

## /connect

Sağlayıcı bağlama sihirbazı. Terminalin ortasında kompakt bir seçici açar:

1. Sağlayıcı modunu seç.
2. Bir sağlayıcı seç.
3. Sağlayıcı birden fazla model sunuyorsa model seç.
4. Gerekirse credential sağla veya seç.
5. Bağlan.

Satır tıklamak, seçip `Enter` basmakla aynıdır; footer tıklamaları ilgili klavye eylemini çalıştırır; `Esc` önceki adıma döner veya seçiciyi kapatır. Sağlayıcı keşfi models.dev kataloğundan gelir; model ID'leri ve kimlik doğrulama gereksinimleri görünüme gömülü değildir.

**Key modları:**

- **Keychain kaydı** (`Keychain(id)`) — seçilen kaydı keychain'den borçlanır (RAM; config'e düz metin yazılmaz).
- **Yeni key** (`New(key)`) — keychain açıksa kaydı ekler, kaydeder ve borçlanır; kilitliyse key yalnızca o oturum için kullanılır (`key yalnızca bu oturumda — keychain'e yazılmadı`).
- **Ortam değişkeni** (`Env(name)`) — belirtilen ortam değişkenini okur (oturumluk).

Başarılı bağlantı toast ile bildirilir:

```
bağlandı: openai / gpt-5 (keychain: openai) [abc123]
```

Grok tarayıcı girişi ve OAuth hâlâ kullanılabilir. Kapasite/kota hataları nötr hatalar olarak gösterilir. CLI karşılığı: `grok connect --provider <id> --api-key <key> [--model <id>] [--base-url <url>] [--category <kategori>] [--keychain-id <id>]`.

## CLI komutları

`grok keys` — şifreli keychain yönetimi. Her alt komut keychain'i açar; anahtarlıkta saklı master password varsa sessizce kullanılır, yoksa şifre gizli okunur.

| Komut | Açıklama | Örnek çıktı |
|---|---|---|
| `grok keys list` | Kayıtları maskeli listeler | `openai  openai  sk-abc…wxyz  gpt-5  2026-08-09  abc123` + `3 kayıt (grok keys show <id> ile tam key)` |
| `grok keys show <id>` | Tek kaydın tam key dahil detayını gösterir | `Provider: openai` … `API Key: sk-…` … `ID: abc123` |
| `grok keys add --provider <id> [--api-key <key>] [--model <id>] [--base-url <url>]` | Key ekler (key verilmezse gizli sorulur) | `keychain'e eklendi: openai (openai) [abc123]` |
| `grok keys edit <id> [--model <id>] [--base-url <url>] [--api-key <key>]` | Kaydı günceller | `güncellendi: abc123` |
| `grok keys remove <id>` | Kaydı siler | `silindi: abc123` |
| `grok keys export [PATH] [--category <kategori>]` | Şifreli arşive dışa aktarır (kapsam: tümü veya kategori) | `export edildi: ~/.grok/keychain-export-1750000000.omx (3 key)` |
| `grok keys import <PATH> [--overwrite]` | Şifreli arşivi içe aktarır | `import edildi: 2 key (üzerine yazılan: 0, atlanan: 1)` |
| `grok keys categories` | Kategorileri listeler (varsayılan işaretli) | `openai (varsayilan)` |
| `grok keys stacks [--detected]` | Stack kataloğunu veya kurulu araçları listeler | `opencode  evet  evet  OpenCode  OpenCode auth.json (provider→api key haritası)` |
| `grok keys sync-from <stack> [--path PATH] [--dry-run] [--overwrite]` | Araçtan keychain'e içe aktarır | `stack → omnitrix [opencode]: 3 key · conflict atlandı: 1 · …` |
| `grok keys sync-to <stack> [--path PATH] [--dry-run] [--overwrite]` | Keychain'den araca dışa aktarır | `omnitrix → stack [dotenv]: 2 key · …` |

**Önizleme (`--dry-run`):**

```
önizleme OpenCode → omnitrix  (/home/user/.local/share/opencode/auth.json)
aday: 4  conflict: 1
  [yeni] openai  sk-abc…wxyz  (openai.key)
  [CONFLICT] anthropic  sk-abc…wxyz  (anthropic.key)
not: conflict'ler atlanacak (varsayılan merge); --overwrite ile üzerine yazılır
```

Varsayılan merge politikası **SkipConflicts**'tır; `--overwrite` verilirse mevcut kayıtlar üzerine yazılır. `--path` ile dosya yolunu açıkça belirtebilirsiniz (ör. `json-scan` için zorunludur). Gizli girdiler (master password, export şifresi, API key) terminalde yankılanmaz; TTY yoksa stdin'den okunur.

## Güvenlik garantileri

- **Her yerde maskeli:** Listeler, önizlemeler, özetler ve kullanım satırları yalnızca maskeli key gösterir (ilk 3 + `…` + son 4 karakter). Ham key hiçbir listeleme çıktısına girmez.
- **Reveal yalnızca bilinçli istekte:** Tam key yalnızca `/keys` içinde `r` (Reveal modu) veya `grok keys show <id>` ile bellekten çözülür; Reveal kapanınca `zeroize` ile sıfırlanır.
- **config.toml'a asla düz metin yazılmaz:** Keychain'den çözülen key'ler yalnızca oturum süresince RAM'de yaşar; config yazımı yalnızca provider/model/base_url bilgilerini kapsar.
- **Loglara asla yazılmaz:** Ham key'ler `Zeroizing` sarmalıyla taşınır; hata ve bilgi logları yalnızca provider ID'si ve hata mesajı içerir.
- **Arşiv export'u ayrı şifrelidir:** `.omx` arşivleri master password'den bağımsız, kendi salt + Argon2id + AES-256-GCM kombinasyonuyla korunur; metadata düz metin, key'ler şifrelidir.
- **Anahtarlık kaydı işletim sistemine aittir:** Sistem anahtarlığı, keychain dosyasından ayrı bir güvenlik katmanıdır.

## Sorun giderme

**"güncel — N key zaten kayıtlı (üzerine yazılmadı)" ne demek?**
Bu bir hata değil. İçe aktarma sırasında tüm aday key'lerin keychain'de aynı kategori/sağlayıcıda karşılığı zaten vardı; çakışma-güvenli birleştirme bunları atladı ve hiçbir şeyin üzerine yazmadı. Kayıtları görmek için `/keys` veya `grok keys list` kullanın.

**Yanlış master password:**
- CLI: `yanlış master password; keychain açılamadı`
- TUI `/keys`: `yanlış master password (veya bozuk dosya)`
Şifreniz sistem anahtarlığında saklanıyorsa ve değiştiyse, anahtarlıktaki eski kayıt otomatik açılışı deneyecek ve başarısız olacaktır; manuel prompt akışına düşersiniz.

**Sistem anahtarlığı yoksa (keyring yok):**
Secret Service daemon'u çalışmıyorsa (dbus kapalı, headless ortam) anahtarlığa yazma sessizce atlanır ve master password her açılışta sorulur. Bu güvenli ama konforsuzdur; masaüstü oturumunda GNOME Keyring/KWallet'in çalıştığından emin olun.

**Stack path bulunamadı:**
`{stack} için credential path bulunamadı` — araç kurulu değil veya dosyası beklenen konumda değil. Aracı kurun veya CLI'da `grok keys sync-from <stack> --path <dosya>` ile yolu açıkça verin. `json-scan` her zaman açık bir `--path` ister.

**Bilinmeyen stack:**
`bilinmeyen stack: <id> — /import ya da /export argümansız çağırarak listeyi görün`. Desteklenen id'ler yukarıdaki tablodadır.

**"`<id>` stack'i bu yönde desteklenmiyor":**
Stack var ama istenen yön desteklenmiyor (ör. `export cursor`, `import env` dışında `export env`). Tablodaki ✓/— sütunlarını kontrol edin.

**OpenCode taraması aktive edemedi:**
- `OpenCode: API key kayıtları güncel · provider/model doğrulanıyor…` — kayıtlar zaten günceldi; katalog aktivasyonu yine çalışır.
- `katalogda etkinleştirilebilir model bulunamadı` — key'ler içe aktarıldı ama katalogda eşleşen provider/model yok; key'ler saklı kalır.
- OpenCode `auth.json` yoksa manuel keychain kullanımı engellenmez.
- Sağlayıcı bağlanıyor ama istekler başarısızsa `/keys` içinde provider'ın base URL'ini, model ID'sini ve key kapsamını doğrulayıp `/connect` ile yeniden bağlanın.
