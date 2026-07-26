# benches/ — faz kapisi olcumleri

MASTER-PLAN Bolum 4'teki `benches/` girdisi. Olcum araci
[`crates/omni/omni-bench`](../crates/omni/omni-bench) binary crate'idir; bu klasor
kosucu scripti, esik verisini ve cikti dosyalarini tutar.

I1: her faz kapisi = **komut + metrik + esik**.

| Kapi | Plan | Komut | Metrik | Esik |
|---|---|---|---|---|
| `cold_start.warm_cache` | 8.2 | `OMNITRIX_TRACE_STARTUP=1 omnitrix` (pty) | `exec -> ilk TUI frame`, duvar saati medyani | < 100 ms |
| `cold_start.cold_cache` | 8.2 | ayni + `drop_caches=3` | ayni, soguk sayfa onbelleginde | < 400 ms |
| `shutdown.idle` | 8.1 | `kill -INT <pid>` | `SIGINT -> exit`, en kotu ornek | < 50 ms |
| `shutdown.under_load` | 8.1 | ayni + fsync'li yazicilar | ayni, veri hacminden bagimsiz olmali | < 50 ms |
| `rss.cli` | Bolum 4 | `omnitrix rss` | lazy-init taban cizgisi (kB) | < `rss_max_kb` |
| `rss.tui_steady` | Bolum 4 | `/proc/<pid>/status` (isinma sonrasi) | `VmRSS` (kB) | < `rss_max_kb` |

## Kosum

```sh
benches/run.sh                      # release build + olcum + JSON
benches/run.sh --runs 50            # ek bayraklar dogrudan omni-bench'e gider
sudo -E benches/run.sh --drop-caches   # gercek soguk-onbellek serisi (root)
```

Cikis kodu: `0` tum kapilar yesil, `1` en az bir esik asildi (CI kirmizi),
`2` olcum yurutulemedi.

**Release zorunlu.** Debug ikilisi ile cold-start olcumu anlamsizdir.

## Esikler

[`thresholds.json`](thresholds.json) tek kaynaktir. Tek tek bayraklar
(`--shutdown-ms`, `--rss-max-kb`, ...) dosyayi ezer; boylece CI'da gecici bir
tavan denemesi dosyayi degistirmeden yapilabilir.

## Cikti

`--json <yol>` makine okunur raporu yazar (`schema: "omni-bench/1"`). `run.sh`
her kosumu `benches/results/<UTC-damga>.json` altina koyar ve
`benches/results/latest.json` sembolik bagini gunceller. CI bu dosyadaki
`ok` alanini ya da surecin cikis kodunu okur.

```json
{
  "schema": "omni-bench/1",
  "gates": [
    {
      "id": "shutdown.idle",
      "command": "kill -INT <omnitrix pid> (5 kosum)",
      "metric": "SIGINT->exit max",
      "unit": "ms",
      "value": 3.1,
      "threshold": 50.0,
      "comparison": "<",
      "pass": true
    }
  ],
  "ok": true
}
```

## Olcum yontemi — neden PTY?

Ilk TUI frame gercekten cizilmeden cold-start olculemez, `ratatui`/`crossterm`
ise ham kip (raw mode) icin denetim terminali ister. `hyperfine` gibi genel
kosucular TTY vermez; o yuzden `omni-bench` bir PTY acar, cocuk sureci `setsid`
ile kendi oturumunun lideri yapar ve PTY'yi `TIOCSCTTY` ile denetim terminali
olarak baglar. Master tarafi ayri bir is parcaciginda surekli bosaltilir, aksi
halde cekirdek tamponu dolunca cocuk `write` uzerinde bloklanir ve olcum bozulur.

Iki zaman raporlanir:

- **duvar saati** — `spawn()` oncesinden `phase=first_frame` satiri okunana
  kadar. Kapiya vurulan deger budur; exec + dinamik baglama maliyetini icerir.
- **ikili-ici damga** — `omnitrix`'in kendi `Instant` damgasi (`elapsed_us`),
  nota yazilir.

`shutdown.under_load` olcumu cocugun veri dizinine (`XDG_DATA_HOME`) N adet
fsync'li yazici surer; 8.1'in "kapanis O(1), veri hacminden bagimsiz" iddiasi
budur. Yazilan MB miktari raporun notunda yer alir.

## results/

Cikti dosyalari surum kontrolune girmez (bkz. `results/.gitignore`).
