# Provider Bağlantısı — Kullanım Kılavuzu

Omnitrix'in provider bağlama özelliği üç şeyi birleştirir:

1. **Provider seçimi** — [models.dev](https://models.dev) canlı kataloğundan
   (24 saat TTL'li cache) ya da custom endpoint olarak.
2. **API key saklama** — şifreli keychain (`~/.grok/keychain.omx`), master
   password ile korunur; key `config.toml`'a **düz metin olarak asla**
   yazılmaz.
3. **Config yazımı** — `~/.grok/config.toml`'a `[model_providers.<id>]` +
   `[model.<key>]` bölümleri ve `[models] default` yazılır, oturum başlatılır.

Üç giriş noktası aynı akışı kullanır:

| Giriş | Komut / Kısayol | Ne zaman |
|---|---|---|
| TUI wizard | `/connect` (veya Ctrl+P → **Connect Provider**, girişsiz ekranda menünün ilk satırı) | İnteraktif, adım adım |
| Headless CLI | `grok connect --provider ... --api-key ...` | Script / tek seferlik |
| Keychain yönetimi | `/keys` (Ctrl+P → **API Keys (Keychain)**) veya `grok keys ...` | Kayıtları yönetme, export/import |

---

## 1. İlk kullanım: master password belirleme

Keychain dosyası (`~/.grok/keychain.omx`) ilk keychain komutunda
otomatik oluşturulur:

```sh
$ grok keys list
yeni keychain: master password belirle:  ████
```

- Girdi **gizlidir** (terminal echo'su kapatılır, yalnızca Unix).
- Boş şifre reddedilir: `master password boş olamaz`.
- Sonraki açılışlarda prompt değişir:
  `keychain master password: ` — her açılışta girilir.
- **Şifre hiçbir yerde saklanmaz.** Her açılışta Argon2id ile 32 byte'lık
  anahtar türetilir; anahtar RAM'de **15 dakika TTL** ile yaşar, TTL dolunca
  `zeroize` ile sıfırlanır ve keychain "kilitli" duruma döner
  (`keychain kilitli; master password yeniden girilmeli`).
- TTY olmayan ortamda (pipe) şifre stdin'den düz okunabilir
  (echo kapatma yoktur).

İpucu: master password ile export şifresi **farklı kavramlardır**
(bkz. [Export/import](#8-exportimport)).

## 2. TUI wizard (`/connect`) — adım adım

Wizard'ı açmanın üç yolu:

- Slash komutu: `/connect`
- Komut paleti (Ctrl+P) → **Connect Provider** (kısayol `/connect`)
- Giriş yapılmamış welcome ekranında menünün ilk satırı
  (0 = Connect Provider, 1 = Login, 2 = Quit)

Akış adımları:

| # | Adım | Açıklama |
|---|---|---|
| 1 | **Provider** | models.dev kataloğundan canlı liste (fuzzy arama). Rozetler: `[key]` = keychain'de kayıt var, env değişken adı = ortamda set. Sonda iki sabit satır: **Custom provider (OpenAI compatible)** ve **Custom provider (Anthropic compatible)** |
| 2 | **Base URL** | Yalnızca custom provider seçilirse gösterilir (varsayılan `https://api.openai.com/v1` / `https://api.anthropic.com/v1` düzenlenebilir). Katalog provider'larında bu adım atlanır |
| 3 | **Key** | Üç kaynak: **yeni key** (maskeli giriş), **keychain kaydı** (id ile seçim), **env değişkeni** (ad ile) |
| 4 | **Kategori** | Opsiyonel; keychain kategorilerinden seçim ya da yeni kategori adı. Atlanırsa varsayılan kategori (`personal`) kullanılır |
| 5 | **Model** | Katalog model listesi; custom/offline durumda manuel model ID girişi ve OpenAI-compatible `/models` fetch fallback'i (sorgu, key ile birlikte gider) |
| 6 | **Apply** | Config yazımı + switch (async). Sonuç **Done** ya da **Error** (geri dönüşte wizard kapanır, hata mesajı gösterilir) |

Keychain kapalıysa (TTL doldu) wizard key adımlarında kilit uyarısı verir;
master password ile açılmadan devam edilemez.

`/keys` komutu aynı amaçla keychain yöneticisini açar: `Kategori | Provider |
Maskeli | Model | Son Kullanim` tablosu, add/edit formları, reveal (tam key,
master password ister), export/import ve kategori listesi/varsayılan seçimi.

## 3. `grok connect` — headless bağlama

Flag verilmeden TTY'de çalışırsa wizard'ın TUI'a taşındığını hatırlatır;
TTY değilse en az `--provider + --api-key` (veya `--keychain-id`) ister.

```sh
grok connect --provider openai --api-key sk-... --model gpt-4o
```

| Flag | Etki |
|---|---|
| `--auto` | API key'i canlı probe ile otomatik provider tespiti (aşağıda §3.1). `--api-key` zorunlu; `--provider`/`--base-url`/`--keychain-id` ile çakışır |
| `--provider <id>` | models.dev provider id (`openai`, `anthropic`, `deepseek`, ...) |
| `--api-key <key>` | Keychain'e şifreli kaydedilir |
| `--base-url <url>` | Custom endpoint. Katalogda olmayan id için **zorunlu** |
| `--model <id>` | Seçilecek model. Verilmezse katalogdan tek model; çokluysa ilki seçilir ve stderr'e not düşülür |
| `--keychain-id <id>` | Mevcut keychain kaydını kullanır; provider/model/base_url o kayıttan çözülür |
| `--category <ad>` | Keychain kategorisi (varsayılan: keychain default, `personal`) |
| `--no-session` | Oturum başlatmaz; yalnızca config/keychain yazımı |

Çözüm öncelikleri: **provider**: flag > keychain kaydı; **base URL**: flag >
keychain kaydı > katalog; **model**: flag > keychain kaydı > katalog.

Yazılan config (key asla dahil değildir):

```toml
[model_providers.openai]
base_url = "https://api.openai.com/v1"
api_backend = "responses"          # chat_completions | responses | messages

[model.omni-openai-gpt-4o]
model = "gpt-4o"
model_provider = "openai"
```

`api_backend`, katalog bilgisinden gelir (anthropic → `messages`, openai/xai →
`responses`, diğerleri → `chat_completions`); custom endpoint'ler
`chat_completions` varsayar.

TTY + `--no-session` yoksa bağlama sonrası normal TUI oturumu başlar; key
keychain'den **borrow** edilip process runtime store'a itilir
(config.toml'da `api_key` alanı yoktur). `--no-session` ile ise şu not basılır:

```sh
bağlandı: openai (https://api.openai.com/v1)
model: gpt-4o → [omni-openai-gpt-4o] (varsayılan)
keychain kaydı: k_1 (kategori: personal)
Ajan oturumu şöyle başlatılır: grok --model omni-openai-gpt-4o
```

## 3.1 `grok connect --auto` — otomatik provider tespiti

`--provider` bilmeden, yalnızca API key ile bağlanır. Key önce keychain
detect'inden geçer (prefix tabanlı adaylar), adaylar models.dev katalogundan
çözülür, her adaya **canlı probe** atılır (kısa timeout) ve kazanan provider
+ base URL seçilir:

```sh
grok connect --auto --api-key sk-...
grok connect --auto --api-key sk-... --model gpt-5
grok connect --auto --api-key sk-... --no-session
```

| Kural | Davranış |
|---|---|
| `--api-key` | **Zorunlu** — `--auto` tek başına parse-time hatası verir |
| `--provider` / `--base-url` / `--keychain-id` | `--auto` ile **çakışır**: her iki kaynak birlikte verilirse hiçbir yan etki olmadan (config/keychain yazılmadan) deterministik hata |
| `--model` | Opsiyonel; winner'ın model listesine karşı doğrulanır. Verilmezse tek model, çokluysa ilki seçilir |
| `--category` / `--no-session` | Manuel akışla aynı anlamda |

Seçim davranışı:

- Key'den birden fazla provider tespit edilir ve probe'lar **eşit** skor
  üretirse sonuç **ambiguous** hatasıdır (hangi provider listesi hataya
  yazılır; key hiçbir hata mesajına girmez).
- Tüm aday probe'ları bağlantı/timeout ile başarısız olursa canlı winner
  yoktur → hata.
- Winner kazanır: config'e `[model_providers.<id>]` + `[model.omni-<id>-<model>]`
  + `[models] default` yazılır; key keychain'de şifreli kalır, `config.toml`'a
  asla düz metin yazılmaz. TTY + `--no-session` yoksa bağlama sonrası normal
  TUI oturumu başlar (manuel akışla aynı).

Örnek çıktı (`--no-session` ile):

```sh
keychain'e eklendi: openai (personal) [k_2]
bağlandı: openai (https://api.openai.com/v1)
model: gpt-5 → [omni-openai-gpt-5] (varsayılan)
keychain kaydı: k_2 (kategori: personal)
Ajan oturumu şöyle başlatılır: grok --model omni-openai-gpt-5
```

## 4. `grok keys` — keychain komutları

| Komut | Etki |
|---|---|
| `grok keys list` | Maskeli liste. Başlık: `KATEGORI  PROVIDER  MASKELI  MODEL  SON_KULLANIM  ID` |
| `grok keys show <id>` | Tam key — Provider, Category, API Key, Model, ID basar (master password ister) |
| `grok keys add --provider <id> [--api-key <key>] [--category <ad>] [--model <id>] [--base-url <url>]` | Yeni kayıt. `--api-key` verilmezse gizli prompt |
| `grok keys edit <id> [--model <id>] [--base-url <url>] [--api-key <key>]` | Kayıt düzenle. Kategori taşıma **desteklenmiyor** (`--category` uyarıyla atlanır) |
| `grok keys remove <id>` | Kayıt sil (onaysız) |
| `grok keys export [YOL] [--category <ad> ...]` | Şifreli `.omx` dışa aktarım (tekrarlanabilir `--category` ile kapsam daraltılır) |
| `grok keys import YOL [--overwrite]` | `.omx` içe aktarım |
| `grok keys categories` | Kategori listesi; varsayılan `(varsayilan)` işaretli |

Örnekler:

```sh
grok keys add --provider deepseek --api-key sk-... --model deepseek-chat
grok keys show k_1
grok keys edit k_1 --model deepseek-reasoner
grok keys export --category work --category personal
grok keys import keychain-export-1723456789.omx --overwrite
```

## 5. Custom provider

Katalogda olmayan herhangi bir id + `--base-url` custom endpoint sayılır
(id config'e aynı şekilde yazılır, `custom` da dahil). Backend varsayılanı
OpenAI-compatible (`chat_completions`)'tır.

### OpenAI-compatible (örnekler)

```sh
# OpenRouter
grok connect --provider openrouter --api-key sk-or-v1-... --model openai/gpt-4o \
  --base-url https://openrouter.ai/api/v1

# DeepSeek
grok connect --provider deepseek --api-key sk-... --model deepseek-chat \
  --base-url https://api.deepseek.com/v1

# Ollama (yerel; key zorunludur ama boşta değil — herhangi bir değer yeter)
grok connect --provider ollama --api-key sk-ollama --model llama3.1 \
  --base-url http://localhost:11434/v1
```

TUI'da bu provider'lar **Custom provider (OpenAI compatible)** satırından
seçilir: Base URL girilir, model adımı `/models` fetch'ini dener
(OpenAI-compatible `GET {base_url}/models`), başarısızsa manuel model ID
girilebilir.

### Anthropic-compatible

```sh
grok connect --provider custom --api-key sk-ant-... --model claude-sonnet-4-5 \
  --base-url https://api.anthropic.com/v1
```

Backend `chat_completions` varsayılır; katalogda `anthropic` id'si zaten varsa
`messages` backend'iyle otomatik gelir. TUI'da **Custom provider (Anthropic
compatible)** satırı kullanılır.

## 6. Headless tek seferlik (`grok -p`)

Tek tur sorusu için aynı provider flag'leri `grok -p` ile birleşir:

```sh
grok -p "şunu açıkla: ..." --provider openai --api-key sk-... --model gpt-5
```

- TTY ise: `--api-key` keychain'e **kalıcı** yazılır, config de yazılır.
- TTY değilse: master password sorulamaz → `--api-key` yalnızca **runtime
  store'a** gider (oturum-scoped, diske yazılmaz).
- TTY değilken `--keychain-id` **hata verir** (fail-closed: borrow imkânsız);
  önce bir TTY'de `grok keys` ile keychain'i açın.
- `--api-key` ve `--keychain-id` ikisi de yoksa keychain'e hiç dokunulmaz
  (master password prompt'u tetiklenmez); yalnızca provider/model/base-url
  config akışı çalışır.
- `--model` provider akışında config anahtarına çevrilir:
  `-m gpt-5 --provider openai` → `omni-openai-gpt-5`.

## 7. Export/import

- Varsayılan export yolu: `~/.grok/keychain-export-<unix_ts>.omx`.
- Export **kendi şifresini** ister (iki kez girilir, master password'den
  **bağımsız**: kendi salt + Argon2id + AES-256-GCM). Metadata düz metin,
  key'ler şifreli.
- Kategori kapsamı: `--category` tekrarlanabilir; verilmezse hepsi.
- Import aynı export şifresini ister; çakışan `(kategori, provider)` kayıtları
  yalnızca `--overwrite` ile üzerine yazılır, aksi halde atlanır:

```sh
grok keys export --category work          # ~/.grok/keychain-export-<ts>.omx
grok keys import ~/.grok/keychain-export-1723456789.omx --overwrite
# import edildi: 3 key (üzerine yazılan: 1, atlanan: 0)
```

## 8. Güvenlik modeli

| Bileşen | Değer |
|---|---|
| Dosya | `~/.grok/keychain.omx` (`GROK_HOME`'a duyarlı) |
| KDF | **Argon2id** (64 MiB parametreleri) — master password'den 32 byte anahtar |
| Şifreleme | **AES-256-GCM** (nonce + ciphertext + tag, base64) |
| Anahtar nerede | **Asla diskte değil** — her açılışta yeniden türetilir, RAM'de `MasterKeyCache`'te |
| RAM TTL | 15 dakika (varsayılan); dolunca anahtar `zeroize` ile sıfırlanır, keychain kilitlenir |
| Master password | Kullanıcıdadır; şifre olarak hiçbir yerde saklanmaz |
| Ajan erişimi | `borrow()`: key RAM'e çözülür, `BorrowedKey` guard'ı döner — drop'ta `zeroize`, `is_expired` TTL takibi |
| Runtime key store | Connect sonrası key process runtime store'a itilir; `config.toml`'a `api_key` **asla** yazılmaz |
| Export şifresi | Master'dan bağımsız, kendi salt + Argon2id ile türetilir |

## 9. models.dev katalog cache

- Kaynak: `GET https://models.dev/api.json`.
- Cache: `~/.grok/models.dev.json`, **24 saat TTL**.
- Taze cache → diskten kullanılır (ağ isteği yok).
- Bayat/yok → ağ istenir; ağ başarısız + bayat cache varsa **fallback olarak
  bayat cache** sunulur; hiçbiri yoksa hata
  (`models.dev kataloğu çözümlenemedi`).
- Katalogdaki her provider `api_backend` ve `base_url` eşlemesini de taşır
  (npm SDK paketine göre: anthropic → `messages`, openai/xai → `responses`,
  openai-compatible → `chat_completions` + `/v1` eklenmiş `api`).

## 10. Sorun giderme

| Belirti | Neden / Çözüm |
|---|---|
| `keychain kilitli; master password yeniden girilmeli` | RAM'deki anahtar TTL'si (15 dk) doldu; master password'ü yeniden girin |
| `yanlış master password; keychain açılamadı` | GCM kimlik doğrulaması başarısız — şifre hatalı (dosya bozuksa `Corrupted` ayrı mesaj verir) |
| `'X' models.dev kataloğunda yok; --base-url ile custom endpoint kullanın` | Provider id katalogda değil ve `--base-url` verilmedi; custom endpoint olarak bağlayın |
| `models.dev kataloğu çözümlenemedi` | Ağ yok + cache de yok (veya TTL'li cache bozuk). Bağlantıyı düzeltin ya da cache'i elle silin (`~/.grok/models.dev.json`) |
| Custom `/models` fetch başarısız | Wizard Model adımında hata rozeti gösterilir; **manuel model ID** girişiyle devam edebilirsiniz |
| `config.toml geçerli TOML değil; düzeltmeden yazılamaz` | Bozuk config üzerine yazılmaz; TOML'u düzeltin, tekrar deneyin |
| `--category` ile keychain kategorisi değişmiyor | `grok keys edit` kategori taşımayı desteklemiyor (uyarı basar); kaydı silip `grok keys add --category` ile yeniden ekleyin |
| `keychain'de {id} yok (grok keys list ile bakın)` | `--keychain-id` kaydı bulunamadı; `grok keys list` ile id'yi doğrulayın |
