# Persona semasi (MASTER-PLAN 11.1)

Her persona **diskte bagimsiz bir dosya**dir: `config/personas/<ad>.toml`.
Yeni persona = yeni dosya. **Derleme yoktur**; `omni-core`'un dosya izleyicisi
(`xai-fsnotify`) dizini izler ve dosyayi surec calisirken yukler (Faz 5).

MASTER-PLAN yalnizca semayi ve yukleyiciyi kurar; **katalogu doldurmaz** (11.2).
Bu dizinde yalnizca iki referans persona vardir (`planner`, `executor`); geri
kalanini kullanici ekler.

## Yukleme kurallari

- Dosya adi persona adidir: `planner.toml` -> `name = "planner"`. Uyusmazlik
  yukleme hatasidir.
- `_` ile baslayan dosyalar (bu dosya dahil) taramanin disindadir; sablon ve
  taslaklar icin kullanilir.
- Alan kumesi **bu dosyadaki ilk `toml` blogundan** okunur. Blokta olmayan bir
  alani kullanan persona reddedilir; bloga yukleyicinin bilmedigi bir alan
  eklenirse yukleme sema-kaymasi hatasiyla durur (sessiz kayma yok).
- `name` ve `description` zorunludur; kalan alanlar opsiyoneldir.
- Tarama basarisiz olursa **onceki defter korunur**: yarim yazilmis bir dosya
  calisan sistemi durdurmaz, yalnizca uyari uretir.

## Alanlar

| Alan | Tip | Anlam |
|---|---|---|
| `name` | string | Persona adi. Kucuk harf, rakam, `_`, `-`. Dosya adiyla ayni. **Zorunlu.** |
| `description` | string | Bir cumlelik amac. **Zorunlu.** |
| `system_prompt` | string | Sistem promptu **dosya referansi**, bu dizine gore cozulur. |
| `role` | string | JEP rolu (10.1): `planner` · `executor` · `judge`. Model bu roldan **katalog** uzerinden secilir (AS7). |
| `temperature` | float | Ornekleme sicakligi, `0.0 ..= 2.0`. |
| `tools` | string dizisi | K3 **allowlist**: modele acilan tool adlari. Listede olmayan tool broker'da reddedilir (9.1). |
| `disallowed` | string dizisi | Acikca kapatilan tool adlari. `tools` ile kesismesi hatadir. |
| `budget` | tablo | `{ mode, max_cost }` — butce zarfi (AS4). `max_cost` verilmezse mod kendi zarfini belirler. |
| `routing` | string | Yonlendirme politikasi (6.5): `round_robin` · `weighted` · `fallback` · `jep`. |
| `max_depth` | integer | Gorev agaci derinlik tavani override'i (AS3). En az `1`. Verilmezse global tavan gecerlidir. |
| `recording` | string | Kayit modu (AS6). Event-log her zaman acik; medya tetiklemelidir. |
| `model` | string | Model kimligi. **Normalde bos birakilir**: model `role` uzerinden katalogdan cozulur (AS7, I5). Yalnizca bir personayi tek bir modele sabitlemek gerektiginde doldurulur ve deger **yapilandirmadan** gelir, koda gomulu degildir. |

## Alan kumesi (yukleyici bunu okur)

```toml
name          = "ornek"
description   = "Sema referansi; bu blok yalnizca ALAN KUMESINI tanimlar"
system_prompt = "prompts/ornek.md"
role          = "executor"
temperature   = 0.2
tools         = []
disallowed    = []
budget        = { mode = "user_focused", max_cost = 5.0 }
routing       = "jep"
max_depth     = 3
recording     = "event_log"
model         = ""
```

## Tool adlari

`tools`/`disallowed` icindeki adlar **modelin gordugu** adlardir (broker karari
bu adlara gore verilir, `omni-tools::broker`), tool kayit defterinin ic
id'leri degil. Faz 1 dikey diliminde acik set: `hashline_read`, `hashline_grep`,
`write`, `hashline_edit` (9.4/9.5).
