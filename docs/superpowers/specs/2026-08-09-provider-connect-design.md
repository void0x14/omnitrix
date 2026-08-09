# Provider Connect — Tasarım Dokümanı

Tarih: 2026-08-09
Durum: Onaylandı (kullanıcı)

## Amaç

Omnitrix CLI'sına opencode / crush / cline tarzı bir **provider bağlantı sistemi** ekle: kullanıcı
TUI içinden (`/connect`, Ctrl+P palette, welcome menüsü) veya headless olarak (`grok connect`,
CLI flag'leri) bir LLM provider'ı seçebilsin, API key'ini şifreli keychain'e kaydedebilsin ve
gerçek zamanlı model listesinden model seçebilsin. Custom provider desteği (OpenAI-compatible /
Anthropic-compatible) yerleşik olsun.

## Kullanıcı Gereksinimleri (sohbetten derlendi)

1. Provider seçimi hem TUI hem headless çalışmada mümkün olmalı.
2. Custom provider (OpenAI compatible / Anthropic compatible) kolayca eklenebilmeli.
3. Akış: provider seç → (gerekirse) API key gir → model seç.
4. `/connect` yazınca picker açılmalı; Ctrl+P palette'inde görünmeli.
5. API key'ler **şifreli** saklanmalı; şifre çözme anahtarı **kullanıcının belirlediği** master
   password (rastgele üretilmiş sistem anahtarı değil).
6. Kullanıcı key'lerini istediği zaman görebilmeli (reveal), kontrol edebilmeli, ekleme/çıkarma/
   değiştirme yapabilmeli. Key'ler kullanıcıya şifresiz görünür; başkaları (ajanlar dahil) yalnızca
   verilen süre kadar ve belli şekilde erişebilir.
7. Key'ler kategorili olarak tek tıkla export/import edilebilmeli.
8. Güvenlik hedefi: dışarıdan diske sızma vektöründe key'ler ele geçirilememesi. Aşırı güvenlik
   (sandbox vb.) istenmiyor.
9. Builtin provider listesi **models.dev** kataloğundan canlı çekilmeli (statik liste yok).
10. Model listeleri **gerçek zamanlı** çekilmeli; key girildiği an o anki en güncel modeller
    gösterilmeli. Statik model listesi istenmiyor (güncelleme yükü).
11. Her provider'ın bir model kataloğu vardır; fallback olarak `/models` fetch ve manuel model ID
    girişi kullanılabilir ama kolaya kaçılmaz.
12. Headless interactive komut adı: `grok connect`.

## Mimari

### Bileşen 1 — Şifreli Keychain (`xai-omni-keychain` crate)

- **Dosya**: `~/.grok/keychain.omx`
- **Şifreleme**: AES-256-GCM. Anahtar = `Argon2id(master_password, salt, ...)`; salt dosyada,
  anahtar asla diskte saklanmaz.
- **Format** (serde JSON, şifreli gövde):
  ```json
  {
    "version": 1,
    "salt_b64": "...",
    "kdf_params": { "m_cost": 65536, "t_cost": 3, "p_cost": 1 },
    "ciphertext_b64": "...",   // AES-256-GCM: nonce || tag || payload
    "payload": {
      "categories": {
        "personal": {
          "providers": {
            "openai": {
              "api_key": "sk-...",
              "model_id": "gpt-5",
              "base_url": null,
              "created_at": "...",
              "last_used": "..."
            }
          }
        }
      },
      "default_category": "personal"
    }
  }
  ```
- **Master password akışı**: ilk kullanımda kullanıcı belirler; sonraki oturumlarda istenir.
  Çözülen anahtar RAM'de `Arc<SecretString>` olarak TTL'li (varsayılan 15 dk) tutulur; TTL bitince
  veya işlem bitince sıfırlanır. Disk'e şifresiz asla yazılmaz.
- **API** (`lib.rs`):
  - `Keychain::open(grok_home) -> Result<KeychainHandle>` (master password callback)
  - `list_keys() -> Vec<KeyEntry>` (masked: son 4 + provider + kategori)
  - `reveal(key_id) -> Result<SecretString>`
  - `add_key(category, provider_id, api_key, model_id?, base_url?)`
  - `update_key(key_id, ...)`, `remove_key(key_id)`
  - `categories() -> Vec<String>`, `set_default_category()`
  - `export(categories: Option<Vec<String>>, export_password) -> Result<Vec<u8>>`
  - `import(bytes, export_password) -> Result<ImportSummary>`
  - `borrow(key_id) -> Result<BorrowedKey>` — RAM'de tutulan, `Drop` ile sıfırlanan key (ajan erişimi)
- **Bağımlılıklar**: `aes-gcm`, `argon2`, `rand`, `zeroize`, `serde`, `serde_json`, `base64`,
  `chrono` (veya benzeri). `zeroize` ile RAM'deki key'ler sıfırlanır.
- **Ajan erişimi (süreli)**: `borrow()` dönen `BorrowedKey` bir `ArcSwap`/`Drop` guard üzerinden
  yaşar; oturum TTL'si (örn. 30 dk) sonrası ve process çıkışında sıfırlanır. Başka süreçler key'i
  okuyamaz (dosya şifreli, anahtar RAM'de).
- **Export formatı**: bağımsız `.omx` dosyası, ayrı export şifresiyle AES-256-GCM; metadata
  (provider, kategori, tarih) düz metin, key'ler şifreli. Import ederken mevcut kategorilere
  birleştirilir, çakışan provider'lar için üzerine yazma onayı sorulur.

### Bileşen 2 — Canlı Provider Kataloğu (models.dev client)

- **Kaynak**: `GET https://models.dev/api.json`
- **Cache**: `~/.grok/models.dev.json` + `last_fetched_at`; TTL 24 saat. `force_refresh` seçeneği
  (pickera "yenile").
- **Veri modeli** (modeller.dev API'sinden, doğrulanmış):
  - `ProviderCatalog { id, name, env: Vec<String>, npm: String, api: Option<String>, doc, models: IndexMap<model_id, ModelInfo> }`
  - `ModelInfo { id, name, description, reasoning, tool_call, temperature, limit: { context, output }, cost: { input, output, cache_read }, modalities }`
- **API tipi eşlemesi** (npm'den):
  - `@ai-sdk/openai-compatible` → `ApiBackend::ChatCompletions` (base_url = `api` alanı + `/v1` gerekirse)
  - `@ai-sdk/anthropic` → `ApiBackend::Messages` (base_url = `https://api.anthropic.com/v1`)
  - `@ai-sdk/openai` / `@ai-sdk/xai` / diğer native → `ApiBackend::Responses` veya mevcut eşleme
- **Yer**: `xai-grok-shell` içinde `util/models_dev.rs` (shell'in HTTP altyapısını kullanır) — ayrı
  crate açılmaz, shell zaten tüm config/ağ altyapısına sahip.
- **Fallback zinciri** (model listesi için):
  1. models.dev kataloğundan provider'ın modelleri (canlı, TTL'li cache)
  2. models.dev'de yoksa → provider'ın `/models` endpoint'i (OpenAI-compatible)
  3. İkisi de başarısız → manuel model ID girişi (free-text)
- **Yenileme politikası**: model listesi picker açıldığında cache'ten; "yenile" kısayolu ile
  force fetch. TTL bitince otomatik arka planda fetch.

### Bileşen 3 — TUI Akışı (`/connect`)

Yeni view: `crates/codegen/xai-grok-pager/src/views/provider_picker/` — mevcut `picker.rs`
altyapısını kullanır. Durum makinesi (`ProviderConnectFlow`):

1. **Provider seç** — models.dev kataloğu (182 provider), fuzzy arama (`PickerState` type-to-find),
   satır başına durum rozeti:
   - `keychain` — keychain'de kayıtlı key var
   - `env` — `env[0]` ortam değişkeni set
   - `new` — key yok
   - Satır altı: kategori, fiyat/context özeti (varsa)
   - Alt satırlar: "Custom provider (OpenAI compatible)", "Custom provider (Anthropic compatible)",
     "Ollama / localhost"
2. **Key sağlama** — seçenekler:
   - keychain'de kayıtlı key varsa: listeden seç (masked)
   - yeni key: masked input (mevcut text input altyapısı) + kategori seçimi (varsayılan kategori)
   - env var kullan (keychain'e kaydetmeden)
   - Custom provider için önce base URL girişi (ön doldurma: models.dev `api` alanı veya varsayılan)
3. **Model seç** — models.dev canlı listesi (reasoning/context/fiyat rozetleri), fallback
   `/models` fetch, son çare manuel ID. Seçili model öne gelir.
4. **Uygula** — `[model_providers.<id>]` + `[model.<id>]` + `[models] default` config.toml'a
   yazılır (mevcut shell persist altyapısı `update_config`), model switch edilir, oturum yeni
   modelle devam eder.

**Giriş noktaları**:
- `/connect` slash komutu (`slash/commands/connect.rs`)
- Ctrl+P palette → "Connect Provider" entry (`views/modal.rs` `default_palette_entries` +
  `PaletteCommand::ConnectProvider`)
- Welcome ekranı → menü satırı "Connect Provider" (login menüsünün üstünde)
- `/keys` slash komutu → key yönetim ekranı (listele, reveal, add, edit, remove, export, import)

**Key yönetim ekranı** (`/keys`): tablolu liste (kategori, provider, maskeli key, son kullanım),
action satırları: reveal / add / edit / remove / export / import / category manage.

### Bileşen 4 — Headless / CLI

- `grok connect` — TTY'de aynı wizard (interactive); TTY yoksa flag'lere dayanır.
- Yeni flag'ler (`AgentArgs`):
  - `--provider <id>` — models.dev id (örn. `openai`, `anthropic`, `deepseek`)
  - `--api-key <key>` — doğrudan key (keychain'e kaydedilir)
  - `--base-url <url>` — custom provider endpoint (openai-compatible için zorunlu)
  - `--model <id>` — model id
  - `--keychain-id <keyId>` — keychain'den hazır key kullan
  - `--category <name>` — keychain kategorisi (varsayılan: default)
- `grok keys` subcommand'leri: `list`, `show <id>`, `add`, `edit`, `remove`, `export [kategori]`,
  `import <file>`, `categories`.
- Mevcut `XAI_API_KEY` env yolu korunur; `--api-key` onu ezmez (env önceliği korunur, dokümante
  edilir).

## Akış Diyagramı (TUI)

```
/connect (veya Ctrl+P, welcome)
  └─ Provider picker (models.dev, fuzzy, rozetler)
       ├─ custom-openai → base URL → key → model
       ├─ custom-anthropic → base URL → key → model
       └─ builtin → key (keychain/env/yeni) → model
            └─ Uygula: config.toml [model_providers.X] + [model.Y] + default
                 └─ SwitchModel + oturum devam
```

## Hata Yönetimi

- **models.dev erişilemez**: cache yoksa + ağ yoksa → offline durumda provider picker'ı boş
  gösterir, hata satırı + "yeniden dene". Mevcut config'teki `[model_providers.*]` her zaman
  listelenir (offline çalışma).
- **Keychain master password yanlış**: GCM auth fail → "yanlış şifre" mesajı, 3 deneme hakkı,
  sonra kilitlenme (process içi).
- **Keychain dosyası bozuk**: `.omx.corrupt.<timestamp>` yedeği, sıfırdan başlatma teklifi.
- **Export şifresi yanlış**: import sırasında auth fail → temiz hata.
- **Provider /models fetch hatası**: fallback manuel ID girişi; hata satırı gösterilir.
- **Config yazma çakışması**: mevcut `update_config` lock mekanizması kullanılır; çakışmada
  retry.

## Test Stratejisi

Rust kısıtı nedeniyle cargo çalıştırılamaz; testler yazılır ama **derlenmez/çalıştırılmaz**.
Testler mevcut kod tabanındaki pattern'lere uyar (birim testler modül içinde `#[cfg(test)]`,
mevcut fixture'lar):

- **Keychain**: round-trip (add→reveal), yanlış şifre, export/import, kategori birleştirme,
  zeroize davranışı, TTL expiry.
- **models.dev client**: fixture JSON ile parse, TTL cache davranışı, fallback sırası,
  npm→ApiBackend eşlemesi.
- **Connect flow**: provider→key→model→apply durum geçişleri (fake catalog, fake keychain),
  config.toml yazımı (tmpdir).
- **CLI**: flag parsing, `grok keys` komutları (tmpdir GROK_HOME).

Doğrulama: manuel review + kullanıcı tarafından `cargo check` (kullanıcı derleyiciyi çalıştırır).

## Kapsam Dışı

- OAuth/device-code login akışları değişmez (mevcut grok.com/oidc yolları korunur).
- Sandbox izolasyonu — kullanıcı istemiyor.
- Telemetri, paylaşım, marketplace.
- Fiyat takibi (cost display sadece rozet olarak).
