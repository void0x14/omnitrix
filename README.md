<div align="center">

<h1>Omnitrix (<code>omnitrix</code>)</h1>

**Omnitrix bir orkestrasyon katmanidir.** Tek-ajan calisma zamanini, LLM
tasimasini ve tool sistemini vendored `xai-*` agaci saglar; omnitrix bunlarin
uzerine **cok-ajan planlama, yonlendirme, persona, saglayici-besleme, uzak
erisim ve dayaniklilik** ekler.

[Kurulum](#kurulum) ·
[Kullanim](#kullanim) ·
[Mimari](#mimari) ·
[Config](#config-as8) ·
[Kapilar](#kapilar) ·
[Depo duzeni](#depo-duzeni) ·
[Lisans](#katki-ve-lisans)

</div>

---

## Omnitrix ne, ne degil

MASTER-PLAN 1.1'in merkezi kurali:

> Omnitrix = orkestrasyon katmanidir. Tek-ajan calisma zamanini, LLM tasimasini
> ve tool sistemini `xai-*` saglar; omnitrix bunlarin uzerine cok-ajan planlama,
> yonlendirme, persona, saglayici-besleme, uzak erisim ve dayaniklilik ekler.

Bunun iki yonlu sinir sonucu:

- **`omni-*` → `xai-*`'a bagimlidir**, tam tersi degil.
- **`xai-*` duzenlenmez.** `crates/common/` ve `crates/codegen/` altindaki
  `xai-*` agaci monorepo'dan senkronlanan **vendored** koddur (surum kokteki
  `SOURCE_REV` dosyasinda kayitlidir). Omnitrix onu bir kutuphane gibi tuketir,
  fork etmez — bu invariant `I2`'dir; CI, `crates/common` + `crates/codegen`
  altinda diff gorurse kapiyi kirmiziya cevirir.

Omnitrix **sifirdan bir ajan yazmaz**: tek-ajan dongusu `xai-grok-agent`,
ornekleme/transport `xai-grok-sampler`, temel tool ailesi `xai-grok-tools`
tarafindan saglanir. `omni-*` crate'leri bu imzalari baglar ve ustune
orkestrasyon koyar.

## Kurulum

Gereksinimler:

- **Rust** — surum [`rust-toolchain.toml`](rust-toolchain.toml) ile sabitlenmistir
  (`1.92.0`, `rustfmt` + `clippy` bilesenleri). `rustup` ilk derlemede otomatik
  kurar. Hedefler: `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`.
- **protoc** — proto codegen icin. `$PROTOC` gosterilebilir (CI `/usr/bin/protoc`
  kullanir), yoksa DotSlash uzerinden [`bin/protoc`](bin/protoc) cozulur.

```sh
git clone <depo> omnitrix && cd omnitrix
cargo build -p omnitrix                  # tek binary
cargo build --workspace                  # tum agac (xai-* dahil)
cargo run -p omnitrix -- --version
```

Dagitim derlemesi icin sertlestirilmis profil:
`cargo build -p omnitrix --profile release-dist`.

> [!IMPORTANT]
> Kok `Cargo.toml` (workspace uyeleri, bagimlilik surumleri, lint'ler, profiller)
> **uretilmistir** — salt okunur muamelesi gorur. Degisiklikleri crate basina
> `Cargo.toml` dosyalarinda yapin.

## Kullanim

Tek binary, alt-komutlarla. Arguman ayristirmasi **her turlu init'ten once**
yapilir (MASTER-PLAN 8.2 lazy-init): `--version` hicbir saglayiciya baglanmaz,
storage acmaz, tokio runtime kurmaz.

```sh
omnitrix                       # TUI panosunu baslat (arguman yoksa)
omnitrix --version             # surum
omnitrix --help                # yardim
omnitrix key add [ANAHTAR]     # API anahtari ekle: onek -> saglayici tespiti + canli dogrulama
omnitrix task <metin>          # gorev yaz (WAL niyet kaydi, op_id doner)
omnitrix run <gorev>           # dikey dilim: tek gorev, tek ajan, uctan uca
omnitrix rss                   # surecin RSS degerini yaz (kB)
```

Notlar:

- `key add` **yalnizca dogrulanan anahtari** saklar; onekten saglayici tespiti
  `omni-provider::detection`, saklama `omni-provider::keyring` ile yapilir.
- `task` islemi once WAL niyet kaydina yazar (invariant `I7`) ve `op_id` basar.
- `run` zinciri: saglayici cozumu → **katalogdan** model cozumu (kodda literal
  model adi yoktur, `I5`) → CAS destekli diff-stream fs-shim → tool broker'dan
  turetilen izin listesi → tek ajan oturumu.
- TUI ilk frame'i cizmeden once **yalnizca** terminali kurar; isinma ilk
  frame'den sonra arka planda baslar ve TUI onu beklemez (8.2).

Ortam degiskenleri (secme):

| Degisken | Etki |
|---|---|
| `OMNITRIX_PROFILE` | Aktif kaynak profili (`config/profiles/`, varsayilan `mid`) |
| `OMNITRIX_CONFIG_DIR` | `config/` dizinini ez |
| `OMNITRIX_TRACE_STARTUP=1` | exec → ilk frame suresini stderr'e yaz |
| `OMNITRIX_LOG_STDERR=1` | TUI modunda tracing loglarini stderr'e ac |
| `OMNITRIX_<YOL__ALTYOL>` | Herhangi bir config anahtarini ez (bkz. [Config](#config-as8)) |

Durum dizini `dirs::data_dir()/omnitrix`; SQLite dosyasi
`.../omnitrix/omnitrix.sqlite` — WAL ve `write_journal` niyet tablosu orada yasar.

## Mimari

Dort katman. Ayrintili diyagram ve crate haritasi:
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md). Kaynak otorite `MASTER-PLAN.md`
Bolum 1-3'tur.

```
Yuzler           omni-webui · uzak kanallar (omni-notify)
Kontrol duzlemi  omni-control  (tek API, auth zorunlu, SSE/WS yayini)
Cekirdek         omni-proto/omni-core · omni-scheduler · omni-router · omni-tools · omni-agent
Temel            xai-*  (VENDORED — duzenlenmez)
Dayaniklilik     omni-storage · omni-provider
```

Yuzler asla cekirdek durumu tutmaz; kontrol duzleminden okur/yazar. Cekirdek tek
surecte yasar. Dayaniklilik her yazimi once WAL'a gecirir.

19 `omni-*` crate ve sorumluluklari (MASTER-PLAN 3.2):

| Crate | Sorumluluk |
|---|---|
| `omni-proto` | Kanonik ortak durum modeli, olay ve komut tipleri — tek kaynak (K7/I3) |
| `omni-core` | Domain tipleri, ajan/gorev durum makinesi, orkestrasyon cekirdegi |
| `omni-config` | Katmanli config: env > DB (`config_kv`) > dosya (AS8) |
| `omni-control` | Tek kontrol duzlemi API'si: token auth, SSE/WS durum yayini, komut girisi |
| `omni-scheduler` | Cok-ajan zamanlayici: `SubagentBackend` implementasyonu, tier/hiyerarsi, kaynak valisi, interrupt, butce |
| `omni-router` | Istek yonlendirme: round/fallback/JEP stratejileri, judge, grounding |
| `omni-tools` | Tool broker (K3), diff-stream fs-shim, tool kablolamasi |
| `omni-agent` | `xai_grok_agent::Agent` sarmalayicisi; persona → `AgentDefinition` |
| `omni-storage` | Kalicilik: CAS, WAL, SQLite/redb, tek-writer aktor, checkpoint |
| `omni-provider` | Saglayici tespiti, keyring, health, anahtar besleme (ingestion) |
| `omni-notify` | Bildirim/webhook dagitimi: kanallar, dedup, politika, tetikleyici |
| `omni-research` | Saglayici-degistirilebilir arastirma motoru (surface/deep/ocean) |
| `omni-record` | Oturum kaydi, olay-log ve tekrar oynatma |
| `omni-backup` | Yedekleme/geri yukleme: 3-2-1, SigV4 imzalama + sifreleme |
| `omni-webui` | Sunucu-render web yuzu: maud SSR + SSE/WS, JS derleme zinciri yok |
| `omni-tests` | Entegrasyon/kapi test kosum takimi; kokteki `tests/*.rs` hedeflerini barindirir |
| `omni-bench` | Faz kapisi olcum araci: cold-start, shutdown, RSS — komut + metrik + esik (I1) |
| `omnitrix` | Binary: lazy-init entrypoint + alt-komutlar |

**Ayrim kurali:** `crates/omni/` yazilir ve duzenlenir; `crates/common/` +
`crates/codegen/` (`xai-*`) **salt okunur** temeldir (I2).

## Config (AS8)

Uc katman, oncelik **yuksekten dusuge**:

1. **Env override** (en yuksek) — `OMNITRIX_*`. Yol ayraci `__`:
   `OMNITRIX_RUNTIME__MAX_ACTIVE_AGENTS` → `runtime.max_active_agents`.
2. **DB runtime** — `config_kv` tablosu. WebUI'dan degisen ayarlar buraya yazilir
   ve **calisirken** etkili olur; istege bagli olarak dosyaya disa aktarilir.
3. **Dosya varsayilan** (en dusuk) — `config/*.toml` + aktif profil dosyasi.

Cakisma onceligi: **env > DB > dosya**. Persona/prompt gibi git-izlenen seyler
dosyada, calisma-zamani anahtarlari DB'de yasar. Yukleyici `xai-grok-config` +
`xai-grok-config-types` (`omni-config` uzerinden).

Dosya katmani:

```
config/
├── models.toml        # model katalogu override'i (AS7 — kodda literal model adi yok)
├── routing.toml       # rol -> model esleme + yonlendirme stratejisi
├── profiles/          # kaynak profilleri: low.toml · mid.toml · high.toml
└── personas/          # her persona bir dosya; _schema.md sema referansi
prompts/               # rol sistem promptlari
migrations/            # SQLite semasi: 0001-0009
```

## Kapilar

Her faz kapisi = **komut + metrik + esik** (I1). Asagidakilerin hepsi CI'da
bloklayicidir.

```sh
bash bin/ci-gates.sh          # invariant taramalari (I1/I2a/I2b/I4/I5/I7/IP + I6 raporu)
bash bin/ci-gates.sh --build  # ustune cargo check + clippy -D warnings + omnitrix --version
cargo test --workspace        # birim + entegrasyon + kapi testleri
cargo run -p omni-bench       # cold-start / shutdown / RSS esikleri; esik asilirsa cikis 1
```

`omni-bench` esikleri `--thresholds <JSON>` ya da `--cold-start-warm-ms`,
`--cold-start-cold-ms`, `--shutdown-ms`, `--rss-max-kb` bayraklariyla ezilebilir;
`--json -` makine okunur rapor basar, `--skip <ad>` tek bir olcumu atlar.

Zorlanan invariantlar:

| # | Invariant | Zorlama noktasi |
|---|---|---|
| I1 | Her faz kapisi = komut + metrik + esik | CI + `omni-bench` + `tests/*.rs` varligi |
| I2 | `omni-*` → `xai-*` tek yon; `xai-*` duzenlenmez | `crates/common` + `crates/codegen` diff = 0; deklare edilen her `xai-*` bagimliligi fiilen kullanilir |
| I3 | Tek ortak durum kaynagi; iki yuz ondan turer | `omni-proto` tek tanim |
| I4 | Yetki tek noktada: tool broker | Yasak sembol taramasi |
| I5 | Model ismi/fiyati gomulu degil | Literal model-adi taramasi (`config/models.toml` muaf) |
| I6 | Uretim yolunda `unwrap`/`expect`/`panic!` = 0 | `clippy -D warnings` + ci-gates sayimi |
| I7 | Her yan etkili islem once WAL niyet kaydi | `write_journal` semasi migrations'ta tanimli |
| IP | `Path::canonicalize` yasak, `dunce::canonicalize` kullanilir | `clippy.toml` disallowed-methods + agac taramasi |

Kapi testleri (kokteki `tests/`, `omni-tests` hedefleri olarak kosar):
`crash_recovery` · `diff_visibility` · `ui_parity` · `multiagent_fanout` ·
`provider_fallback` · `grounding_redteam` · `tool_allowlist_redteam` ·
`interrupt_granularity`.

Gunluk gelistirme:

```sh
cargo check -p <crate> --all-targets   # tam workspace derlemesi yavastir, crate hedefleyin
cargo clippy -p <crate> --no-deps -- -D warnings
cargo fmt --all
```

## Depo duzeni

| Yol | Icerik |
|---|---|
| `MASTER-PLAN.md` · `ALPHA-PLAN.md` | Mimari plan (otorite) ve onu doguran kararlar |
| `docs/ARCHITECTURE.md` | Plana atifli katman + crate haritasi ozeti |
| `crates/omni/` | Orkestrasyon crate'leri — **yazilir ve duzenlenir** |
| `crates/common/` · `crates/codegen/` · `crates/build/` | `xai-*` VENDORED temel — **duzenlenmez** (I2) |
| `config/` · `prompts/` | Dosya config katmani, persona ve rol promptlari |
| `migrations/` | SQLite semasi (0001-0009) |
| `tests/` | Entegrasyon + faz kapisi testleri |
| `benches/` | `omni-bench` olcum ciktilari |
| `bin/ci-gates.sh` | Invariant kapilari — CI'nin tek kaynagi |
| `third_party/` · `prod/` | Vendored ucuncu taraf kaynak |
| `SOURCE_REV` | Vendored `xai-*` agacinin monorepo commit SHA'si |

## Katki ve lisans

- Katki kurallari: [`CONTRIBUTING.md`](CONTRIBUTING.md).
- Guvenlik bildirimi: [`SECURITY.md`](SECURITY.md).
- Lisans: birinci taraf kod **Apache-2.0** — [`LICENSE`](LICENSE).
- Ucuncu taraf ve vendored kod kendi lisanslari altindadir:
  [`THIRD-PARTY-NOTICES`](THIRD-PARTY-NOTICES) ve
  [`third_party/NOTICE`](third_party/NOTICE).

Mimari kararlarda **`MASTER-PLAN.md` otoritedir**; bu README onun gerceklesmis
durumunu ozetler.
