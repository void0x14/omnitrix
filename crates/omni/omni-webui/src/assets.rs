//! Gomulu varliklar: stil ve istemci betigi.
//!
//! K8: **JS build zinciri yok**. Burada duran her sey elle yazilmis, derlemeye
//! gomulu, tek dosyalik metindir; harici CDN, font ya da paketleyici yoktur.
//! Sayfa tek istekte gelir — telefonda ve dar hatta calisir (Bolum 5 gerekcesi).
//!
//! Betik **ilerlemeli zenginlestirmedir**: `EventSource` yoksa ya da JS kapaliysa
//! sunucu-taraflı HTML tek basina calisir; form gonderimleri klasik POST ile
//! `omni-control` komut ucuna duser.

/// Sayfa stili. Karanlik varsayilan, `prefers-color-scheme` ile aydinlik;
/// dar ekranda tablolar karta doner (`data-label` basliklariyla).
pub const STYLE: &str = r#"
:root {
  color-scheme: dark light;
  --bg: #0e1116; --panel: #161b22; --line: #262d36; --raise: #1c2330;
  --fg: #e6edf3; --dim: #8b949e;
  --ok: #3fb950; --warn: #d29922; --err: #f85149; --accent: #58a6ff;
  --radius: 10px;
}
@media (prefers-color-scheme: light) {
  :root { --bg:#f6f8fa; --panel:#ffffff; --line:#d8dee4; --raise:#eef1f5;
          --fg:#1f2328; --dim:#636c76; }
}
* { box-sizing: border-box; }
html, body { margin: 0; padding: 0; }
body {
  background: var(--bg); color: var(--fg);
  font: 15px/1.45 ui-sans-serif, system-ui, -apple-system, "Segoe UI", Roboto, Arial, sans-serif;
  -webkit-text-size-adjust: 100%;
}
a { color: var(--accent); }
h1 { font-size: 1.05rem; margin: 0; letter-spacing: .02em; }
h2 { font-size: .85rem; margin: 0 0 .55rem; text-transform: uppercase;
     letter-spacing: .08em; color: var(--dim); }
header.top {
  position: sticky; top: 0; z-index: 5;
  display: flex; align-items: center; gap: .6rem; flex-wrap: wrap;
  padding: .65rem .9rem; background: var(--panel);
  border-bottom: 1px solid var(--line);
}
header.top .grow { flex: 1 1 auto; }
main { display: grid; gap: .8rem; padding: .8rem; max-width: 1200px; margin: 0 auto; }
section.panel {
  background: var(--panel); border: 1px solid var(--line);
  border-radius: var(--radius); padding: .75rem .8rem;
}
.scroll { overflow-x: auto; }
table { width: 100%; border-collapse: collapse; font-size: .88rem; }
th, td { text-align: left; padding: .38rem .45rem; border-bottom: 1px solid var(--line); }
th { color: var(--dim); font-weight: 600; font-size: .72rem;
     text-transform: uppercase; letter-spacing: .06em; }
td.num { text-align: right; font-variant-numeric: tabular-nums; }
tr.placeholder td, li.placeholder { color: var(--dim); font-style: italic; }
ul.feed { list-style: none; margin: 0; padding: 0; display: grid; gap: .3rem;
          max-height: 15rem; overflow-y: auto; }
ul.feed li { padding: .3rem .4rem; border-radius: 6px; background: var(--raise);
             font-size: .82rem; display: flex; gap: .45rem; flex-wrap: wrap; }
.gauge { display: grid; gap: .5rem;
         grid-template-columns: repeat(auto-fit, minmax(8.5rem, 1fr)); }
.cell { background: var(--raise); border-radius: 8px; padding: .45rem .55rem; }
.cell .k { font-size: .68rem; text-transform: uppercase; letter-spacing: .06em;
           color: var(--dim); display: block; }
.cell .v { font-size: 1.05rem; font-variant-numeric: tabular-nums; }
.bar { height: 5px; border-radius: 3px; background: var(--line); margin-top: .35rem; }
.bar > span { display: block; height: 100%; border-radius: 3px; background: var(--accent); }
.tag { display: inline-block; padding: .05rem .38rem; border-radius: 999px;
       font-size: .7rem; letter-spacing: .03em; background: var(--raise); color: var(--dim); }
.tag.active, .tag.healthy, .tag.done { color: var(--ok); }
.tag.queued, .tag.degraded, .tag.blocked, .tag.interrupted { color: var(--warn); }
.tag.failed, .tag.down, .tag.quota_exhausted { color: var(--err); }
.mono { font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
        font-size: .8rem; }
.dim { color: var(--dim); }
.plus { color: var(--ok); }
.minus { color: var(--err); }
.link { font-size: .7rem; padding: .1rem .45rem; border-radius: 999px;
        background: var(--raise); color: var(--dim); }
.link.up { color: var(--ok); }
.link.down { color: var(--err); }
tr.fresh, li.fresh { animation: fresh 1.2s ease-out; }
@keyframes fresh { from { background: rgba(88,166,255,.28); } to { background: transparent; } }
form.stack { display: grid; gap: .45rem; }
form.row { display: flex; gap: .4rem; flex-wrap: wrap; align-items: center; }
label { display: grid; gap: .15rem; font-size: .74rem; color: var(--dim); }
input, select, textarea, button {
  font: inherit; color: var(--fg); background: var(--bg);
  border: 1px solid var(--line); border-radius: 8px; padding: .4rem .5rem;
  min-height: 2.2rem; max-width: 100%;
}
textarea { min-height: 4rem; resize: vertical; }
button { background: var(--accent); color: #06121f; border-color: transparent;
         font-weight: 600; cursor: pointer; }
button.ghost { background: var(--raise); color: var(--fg); font-weight: 500; }
details > summary { cursor: pointer; color: var(--dim); font-size: .8rem;
                    padding: .2rem 0; }
.grid2 { display: grid; gap: .5rem;
         grid-template-columns: repeat(auto-fit, minmax(13rem, 1fr)); }
.err { color: var(--err); font-size: .82rem; }
@media (max-width: 660px) {
  main { padding: .55rem; gap: .55rem; }
  table thead { display: none; }
  table, tbody, tr, td { display: block; width: 100%; }
  tr { border: 1px solid var(--line); border-radius: 8px;
       margin-bottom: .45rem; padding: .2rem .35rem; }
  td { border: 0; display: flex; justify-content: space-between; gap: .6rem;
       padding: .18rem .1rem; }
  td.num { text-align: right; }
  td::before { content: attr(data-label); color: var(--dim); font-size: .7rem;
               text-transform: uppercase; letter-spacing: .05em; }
  tr.placeholder td::before { content: ""; }
}
"#;

/// Istemci betigi: SSE akisindan gelen HTML parcalarini DOM'a uygular.
///
/// Sozlesme `live::Fragment` ile birebirdir: `target` bulunursa `outerHTML`
/// degistirilir (idempotent upsert), bulunamazsa `container` icine `swap`
/// konumuna eklenir. Akista bosluk olursa sunucu `reload` olayi gonderir ve
/// sayfa yeniden SSR edilir (Bolum 6.2 crash-only kurtarma).
pub const SCRIPT: &str = r#"
(function () {
  var url = document.body.getAttribute('data-stream');
  if (!url || typeof window.EventSource !== 'function') { return; }
  var LIMIT = 60;
  var dot = document.getElementById('link-state');
  var src = new EventSource(url);

  function link(cls, text) {
    if (!dot) { return; }
    dot.className = 'link ' + cls;
    dot.textContent = text;
  }
  function mark(id) {
    if (!id) { return; }
    var el = document.getElementById(id);
    if (!el || !el.classList) { return; }
    el.classList.add('fresh');
    setTimeout(function () { el.classList.remove('fresh'); }, 1200);
  }
  function apply(f) {
    if (!f || typeof f.html !== 'string') { return; }
    var el = f.target ? document.getElementById(f.target) : null;
    if (el) { el.outerHTML = f.html; mark(f.target); return; }
    var box = f.container ? document.getElementById(f.container) : null;
    if (!box) { return; }
    var ph = box.querySelector('.placeholder');
    if (ph && ph.parentNode) { ph.parentNode.removeChild(ph); }
    box.insertAdjacentHTML(f.swap === 'prepend' ? 'afterbegin' : 'beforeend', f.html);
    while (box.children.length > LIMIT && box.lastElementChild) {
      box.removeChild(box.lastElementChild);
    }
    mark(f.target);
  }

  src.addEventListener('open', function () { link('up', 'canli'); });
  src.addEventListener('error', function () { link('down', 'kopuk'); });
  src.addEventListener('fragment', function (ev) {
    var f = null;
    try { f = JSON.parse(ev.data); } catch (e) { return; }
    apply(f);
  });
  src.addEventListener('reload', function () {
    src.close();
    window.location.reload();
  });
  window.addEventListener('beforeunload', function () { src.close(); });
})();
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varliklar_kapanis_etiketi_icermez() {
        // `<style>`/`<script>` govdesine gomulu kapanis etiketi HTML'i bozar.
        assert!(!STYLE.contains("</style"));
        assert!(!SCRIPT.contains("</script"));
    }

    #[test]
    fn betik_harici_kaynak_cekmez() {
        // K8: CDN/paketleyici yok; ag erisimi yalnizca kendi SSE ucumuza.
        for banned in ["http://", "https://", "cdn", "import "] {
            assert!(!SCRIPT.contains(banned), "yasak parca: {banned}");
            assert!(!STYLE.contains(banned), "yasak parca: {banned}");
        }
    }

    #[test]
    fn stil_dar_ekrani_kapsar() {
        assert!(STYLE.contains("@media (max-width: 660px)"));
        assert!(STYLE.contains("attr(data-label)"));
    }
}
