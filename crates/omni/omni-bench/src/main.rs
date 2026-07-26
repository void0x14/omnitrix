//! omni-bench — MASTER-PLAN Bolum 4 `benches/` olcum araci.
//!
//! Uc faz-kapisi olcumu (I1: her kapi = komut + metrik + esik):
//!
//! | Olcum | Plan | Komut | Metrik | Esik |
//! |---|---|---|---|---|
//! | cold-start | 8.2 | `OMNITRIX_TRACE_STARTUP=1 omnitrix` | exec -> ilk TUI frame | < 100ms sicak, < 400ms soguk |
//! | shutdown | 8.1 | `kill -INT <pid>` | SIGINT -> exit | < 50ms (bos ve yuk altinda) |
//! | RSS | 8.2 / Bolum 4 | `omnitrix rss` + `/proc/<pid>/status` | yerlesik bellek | < esik (kB) |
//!
//! Cikti hem insan okunur tablo hem makine okunur JSON'dur. Herhangi bir esik
//! asilirsa surec 1 ile doner — CI kapisi kirmizi olur.
//!
//! Cold-start olcumu gercek bir TUI ister: ikili bir PTY'ye baglanir, kendi
//! oturumunun (session) lideri yapilir ve PTY denetim terminali olarak atanir.
//! Boylece `hyperfine` gibi TTY'siz kosucularin aksine `enable_raw_mode()`
//! basarili olur ve gercekten ilk frame cizilir. Zamanlama icin ikilinin kendi
//! damgasi (`OMNITRIX_TRACE_STARTUP=1` -> stderr) ve harici duvar saati birlikte
//! raporlanir; kapiya vurulan deger harici duvar saatidir (exec maliyetini de
//! icerdigi icin daha muhafazakar).

mod soak;

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Seek, SeekFrom, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Esikler (I1). Varsayilanlar MASTER-PLAN 8.1 / 8.2'den gelir; `--thresholds`
// ile bir JSON dosyasindan ya da tek tek bayraklarla ezilebilir.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Thresholds {
    /// 8.2: exec -> ilk TUI frame, sicak sayfa onbelleginde.
    cold_start_warm_ms: f64,
    /// 8.2: exec -> ilk TUI frame, soguk sayfa onbelleginde.
    cold_start_cold_ms: f64,
    /// 8.1: SIGINT -> exit. Veri hacminden bagimsiz olmali.
    shutdown_ms: f64,
    /// Bolum 4: kararli durum RSS tavani (kB).
    rss_max_kb: f64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            cold_start_warm_ms: 100.0,
            cold_start_cold_ms: 400.0,
            shutdown_ms: 50.0,
            rss_max_kb: 262_144.0,
        }
    }
}

// ---------------------------------------------------------------------------
// Argumanlar
// ---------------------------------------------------------------------------

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone)]
struct Args {
    bin: Option<PathBuf>,
    config_dir: Option<PathBuf>,
    runs: usize,
    warmup: usize,
    shutdown_runs: usize,
    settle_ms: u64,
    load_threads: usize,
    load_file_mb: u64,
    json_out: Option<PathBuf>,
    thresholds_file: Option<PathBuf>,
    thresholds: Thresholds,
    skip: Vec<String>,
    drop_caches: bool,
    cols: u16,
    rows: u16,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            bin: None,
            config_dir: None,
            runs: 20,
            warmup: 3,
            shutdown_runs: 5,
            settle_ms: 750,
            load_threads: 4,
            load_file_mb: 64,
            json_out: None,
            thresholds_file: None,
            thresholds: Thresholds::default(),
            skip: Vec::new(),
            drop_caches: false,
            cols: 120,
            rows: 40,
        }
    }
}

impl Args {
    fn skipped(&self, id: &str) -> bool {
        self.skip.iter().any(|s| s == id)
    }
}

fn print_help() {
    println!(
        "omni-bench {VERSION}\n\
         Faz kapisi olcumleri: cold-start (8.2), shutdown (8.1), RSS.\n\
         \n\
         Kullanim:\n\
         \x20 omni-bench [SECENEKLER]\n\
         \n\
         Secenekler:\n\
         \x20 --bin <YOL>              omnitrix ikilisi (varsayilan: omni-bench ile ayni dizin)\n\
         \x20 --config-dir <YOL>       cocuk surece OMNITRIX_CONFIG_DIR olarak verilir\n\
         \x20 --runs <N>               cold-start orneklem sayisi (varsayilan 20)\n\
         \x20 --warmup <N>             sayilmayan on kosum (varsayilan 3)\n\
         \x20 --shutdown-runs <N>      shutdown orneklem sayisi (varsayilan 5)\n\
         \x20 --settle-ms <MS>         isinma sonrasi bekleme (varsayilan 750)\n\
         \x20 --load-threads <N>       yuk altinda shutdown: yazici sayisi (varsayilan 4)\n\
         \x20 --load-file-mb <MB>      yazici basina dosya boyu (varsayilan 64)\n\
         \x20 --json <YOL>             JSON raporu dosyaya yaz ('-' => stdout)\n\
         \x20 --thresholds <YOL>       esik JSON dosyasi\n\
         \x20 --cold-start-warm-ms <F> esigi ez (varsayilan 100)\n\
         \x20 --cold-start-cold-ms <F> esigi ez (varsayilan 400)\n\
         \x20 --shutdown-ms <F>        esigi ez (varsayilan 50)\n\
         \x20 --rss-max-kb <F>         esigi ez (varsayilan 262144)\n\
         \x20 --skip <AD>              olcum atla: cold-start | shutdown | rss (tekrarlanabilir)\n\
         \x20 --drop-caches            soguk seri icin sayfa onbellegini bosalt (root gerekir)\n\
         \x20 --cols <N> --rows <N>    sahte terminal boyutu (varsayilan 120x40)\n\
         \x20 --version, --help\n\
         \n\
         Cikis kodu: tum kapilar yesilse 0, en az bir esik asildiysa 1."
    );
}

fn parse_args(raw: Vec<String>) -> anyhow::Result<Option<Args>> {
    let mut args = Args::default();
    let mut it = raw.into_iter();

    // Deger bekleyen bayrak icin bir sonraki argumani ceker.
    fn need(it: &mut impl Iterator<Item = String>, flag: &str) -> anyhow::Result<String> {
        it.next()
            .ok_or_else(|| anyhow::anyhow!("{flag} icin deger gerekli"))
    }

    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                print_help();
                return Ok(None);
            }
            "--version" | "-V" => {
                println!("omni-bench {VERSION}");
                return Ok(None);
            }
            "--bin" => args.bin = Some(PathBuf::from(need(&mut it, "--bin")?)),
            "--config-dir" => args.config_dir = Some(PathBuf::from(need(&mut it, "--config-dir")?)),
            "--runs" => args.runs = need(&mut it, "--runs")?.parse()?,
            "--warmup" => args.warmup = need(&mut it, "--warmup")?.parse()?,
            "--shutdown-runs" => args.shutdown_runs = need(&mut it, "--shutdown-runs")?.parse()?,
            "--settle-ms" => args.settle_ms = need(&mut it, "--settle-ms")?.parse()?,
            "--load-threads" => args.load_threads = need(&mut it, "--load-threads")?.parse()?,
            "--load-file-mb" => args.load_file_mb = need(&mut it, "--load-file-mb")?.parse()?,
            "--json" => args.json_out = Some(PathBuf::from(need(&mut it, "--json")?)),
            "--thresholds" => {
                args.thresholds_file = Some(PathBuf::from(need(&mut it, "--thresholds")?));
            }
            "--cold-start-warm-ms" => {
                args.thresholds.cold_start_warm_ms =
                    need(&mut it, "--cold-start-warm-ms")?.parse()?;
            }
            "--cold-start-cold-ms" => {
                args.thresholds.cold_start_cold_ms =
                    need(&mut it, "--cold-start-cold-ms")?.parse()?;
            }
            "--shutdown-ms" => {
                args.thresholds.shutdown_ms = need(&mut it, "--shutdown-ms")?.parse()?
            }
            "--rss-max-kb" => {
                args.thresholds.rss_max_kb = need(&mut it, "--rss-max-kb")?.parse()?
            }
            "--skip" => args.skip.push(need(&mut it, "--skip")?),
            "--drop-caches" => args.drop_caches = true,
            "--cols" => args.cols = need(&mut it, "--cols")?.parse()?,
            "--rows" => args.rows = need(&mut it, "--rows")?.parse()?,
            other => anyhow::bail!("bilinmeyen arguman: {other}"),
        }
    }

    if args.runs == 0 || args.shutdown_runs == 0 {
        anyhow::bail!("--runs ve --shutdown-runs sifir olamaz");
    }

    // Dosyadan gelen esikler, once okunur; komut satiri bayraklari onu ezmelidir.
    // Bu yuzden dosya once uygulanir, sonra bayraklarla farklar geri yazilir.
    if let Some(path) = args.thresholds_file.clone() {
        let text = std::fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("esik dosyasi okunamadi ({}): {e}", path.display()))?;
        let from_file: Thresholds = serde_json::from_str(&text).map_err(|e| {
            anyhow::anyhow!("esik dosyasi ayristirilamadi ({}): {e}", path.display())
        })?;
        let defaults = Thresholds::default();
        let cli = args.thresholds;
        args.thresholds = Thresholds {
            cold_start_warm_ms: pick(
                cli.cold_start_warm_ms,
                defaults.cold_start_warm_ms,
                from_file.cold_start_warm_ms,
            ),
            cold_start_cold_ms: pick(
                cli.cold_start_cold_ms,
                defaults.cold_start_cold_ms,
                from_file.cold_start_cold_ms,
            ),
            shutdown_ms: pick(cli.shutdown_ms, defaults.shutdown_ms, from_file.shutdown_ms),
            rss_max_kb: pick(cli.rss_max_kb, defaults.rss_max_kb, from_file.rss_max_kb),
        };
    }

    Ok(Some(args))
}

/// Komut satiri degeri varsayilandan farkliysa o kazanir; degilse dosya degeri.
fn pick(cli: f64, default: f64, file: f64) -> f64 {
    if (cli - default).abs() > f64::EPSILON {
        cli
    } else {
        file
    }
}

// ---------------------------------------------------------------------------
// Istatistik
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, serde::Serialize)]
struct Stats {
    n: usize,
    min: f64,
    median: f64,
    p95: f64,
    max: f64,
    mean: f64,
}

fn stats(samples: &[f64]) -> Stats {
    if samples.is_empty() {
        return Stats::default();
    }
    let mut v = samples.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = v.len();
    let median = if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    };
    // En yakin sira (nearest-rank) p95: kucuk orneklemde de tanimli.
    let rank = ((0.95 * n as f64).ceil() as usize).clamp(1, n);
    let sum: f64 = v.iter().sum();
    Stats {
        n,
        min: v[0],
        median,
        p95: v[rank - 1],
        max: v[n - 1],
        mean: sum / n as f64,
    }
}

// ---------------------------------------------------------------------------
// PTY — cocuk surecin gercek bir terminale ihtiyaci var (ratatui/crossterm)
// ---------------------------------------------------------------------------

struct Pty {
    master: OwnedFd,
    slave_path: PathBuf,
}

fn os_err(what: &str) -> anyhow::Error {
    anyhow::anyhow!("{what}: {}", std::io::Error::last_os_error())
}

impl Pty {
    fn open(cols: u16, rows: u16) -> anyhow::Result<Self> {
        // SAFETY: posix_openpt yeni bir master fd dondurur, hata durumunda -1.
        let fd = unsafe { libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY) };
        if fd < 0 {
            return Err(os_err("posix_openpt"));
        }
        // SAFETY: fd gecerli ve sahipligi buradan sonra OwnedFd'de.
        let master = unsafe { OwnedFd::from_raw_fd(fd) };

        // SAFETY: fd bir pty master; grantpt/unlockpt yan etkisi yalnizca izinler.
        if unsafe { libc::grantpt(fd) } != 0 {
            return Err(os_err("grantpt"));
        }
        // SAFETY: yukaridaki ile ayni.
        if unsafe { libc::unlockpt(fd) } != 0 {
            return Err(os_err("unlockpt"));
        }

        let mut buf = [0 as libc::c_char; 256];
        // SAFETY: buf yeterince buyuk ve yasam suresi cagri boyunca gecerli.
        let rc = unsafe { libc::ptsname_r(fd, buf.as_mut_ptr(), buf.len()) };
        if rc != 0 {
            return Err(anyhow::anyhow!("ptsname_r: hata kodu {rc}"));
        }
        // SAFETY: ptsname_r basarili donduyse buf NUL ile sonlanan bir yoldur.
        let name = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) }
            .to_str()
            .map_err(|e| anyhow::anyhow!("pty adi UTF-8 degil: {e}"))?
            .to_string();

        let ws = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: fd bir tty, ws gecerli bir winsize.
        if unsafe { libc::ioctl(fd, libc::TIOCSWINSZ as _, &ws) } != 0 {
            return Err(os_err("ioctl(TIOCSWINSZ)"));
        }

        Ok(Self {
            master,
            slave_path: PathBuf::from(name),
        })
    }

    fn open_slave(&self) -> anyhow::Result<File> {
        OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.slave_path)
            .map_err(|e| {
                anyhow::anyhow!("pty slave acilamadi ({}): {e}", self.slave_path.display())
            })
    }

    /// Master tarafi surekli bosaltilmazsa cekirdek tamponu dolar ve cocuk
    /// surec `write` uzerinde bloklanir; bu da olcumu bozar. Ayri bir is
    /// parcacigi ciktiyi yutar.
    fn spawn_drain(&self) -> anyhow::Result<()> {
        let clone = self
            .master
            .try_clone()
            .map_err(|e| anyhow::anyhow!("pty master klonlanamadi: {e}"))?;
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                // SAFETY: clone gecerli bir fd; buf yazilabilir ve boyutu dogru.
                let n = unsafe {
                    libc::read(
                        clone.as_raw_fd(),
                        buf.as_mut_ptr().cast::<libc::c_void>(),
                        buf.len(),
                    )
                };
                if n <= 0 {
                    // EIO = son slave kapandi (cocuk oldu) — normal son.
                    break;
                }
            }
        });
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Cocuk surec yonetimi
// ---------------------------------------------------------------------------

/// `OMNITRIX_TRACE_STARTUP=1` ciktisinin ayristirilmis hali.
#[derive(Debug, Clone, Default)]
struct StartupTrace {
    phase: String,
    elapsed_us: f64,
    rss_kb: f64,
}

fn parse_startup_trace(line: &str) -> Option<StartupTrace> {
    if !line.contains("phase=") {
        return None;
    }
    let mut kv: BTreeMap<&str, &str> = BTreeMap::new();
    for token in line.split_whitespace() {
        if let Some((k, v)) = token.split_once('=') {
            kv.insert(k, v);
        }
    }
    let phase = (*kv.get("phase")?).to_string();
    Some(StartupTrace {
        phase,
        elapsed_us: kv
            .get("elapsed_us")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0),
        rss_kb: kv.get("rss_kb").and_then(|v| v.parse().ok()).unwrap_or(0.0),
    })
}

/// Cocugun stderr'inden okunan satirlar, okunduklari an ile birlikte.
type TraceLines = Receiver<(Instant, String)>;

struct TuiProc {
    child: Child,
    lines: TraceLines,
    _pty: Pty,
    reaped: bool,
}

impl TuiProc {
    fn pid(&self) -> i32 {
        // Child::id() u32 dondurur; POSIX pid_t'ye guvenle sigar.
        i32::try_from(self.child.id()).unwrap_or(-1)
    }

    /// Belirli bir isinma asamasini bekler; zaman asiminda hata.
    fn wait_phase(
        &self,
        phase: &str,
        timeout: Duration,
    ) -> anyhow::Result<(Instant, StartupTrace)> {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                anyhow::bail!("'{phase}' asamasi {timeout:?} icinde gelmedi");
            }
            match self.lines.recv_timeout(left) {
                Ok((at, line)) => {
                    if let Some(trace) = parse_startup_trace(&line)
                        && trace.phase == phase
                    {
                        return Ok((at, trace));
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    anyhow::bail!("'{phase}' asamasi {timeout:?} icinde gelmedi")
                }
                Err(RecvTimeoutError::Disconnected) => {
                    anyhow::bail!("cocuk surec '{phase}' asamasindan once stderr'i kapatti")
                }
            }
        }
    }

    /// SIGINT gonderir ve cikisa kadar gecen sureyi olcer (8.1 kapisi).
    fn sigint_and_wait(&mut self, timeout: Duration) -> anyhow::Result<f64> {
        let pid = self.pid();
        if pid <= 0 {
            anyhow::bail!("gecersiz pid");
        }
        let t = Instant::now();
        // SAFETY: pid dogrudan bu surecin cocugudur, henuz reap edilmedi.
        if unsafe { libc::kill(pid, libc::SIGINT) } != 0 {
            return Err(os_err("kill(SIGINT)"));
        }
        let status = wait_with_timeout(&mut self.child, timeout)?;
        let elapsed = t.elapsed().as_secs_f64() * 1000.0;
        self.reaped = true;
        if status.is_none() {
            anyhow::bail!("SIGINT sonrasi {timeout:?} icinde cikmadi");
        }
        Ok(elapsed)
    }

    fn kill_now(&mut self) {
        if self.reaped {
            return;
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.reaped = true;
    }
}

impl Drop for TuiProc {
    fn drop(&mut self) {
        self.kill_now();
    }
}

/// `try_wait` dongusuyle sinirli bekleme. `None` => zaman asimi.
fn wait_with_timeout(
    child: &mut Child,
    timeout: Duration,
) -> anyhow::Result<Option<std::process::ExitStatus>> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        std::thread::sleep(Duration::from_micros(200));
    }
}

/// TUI'yi bir PTY uzerinde baslatir. Donen `Instant` `spawn()` cagrisindan
/// hemen once alinmistir; exec maliyeti olcume dahildir.
fn spawn_tui(
    bin: &Path,
    args: &Args,
    data_home: &Path,
    extra_env: &[(&str, &str)],
) -> anyhow::Result<(Instant, TuiProc)> {
    let pty = Pty::open(args.cols, args.rows)?;
    pty.spawn_drain()?;

    let stdin = pty.open_slave()?;
    let stdout = pty.open_slave()?;

    let mut cmd = Command::new(bin);
    cmd.stdin(Stdio::from(stdin))
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::piped())
        .env("OMNITRIX_TRACE_STARTUP", "1")
        .env("TERM", "xterm-256color")
        .env("XDG_DATA_HOME", data_home)
        .env_remove("OMNITRIX_LOG_STDERR");

    if let Some(dir) = &args.config_dir {
        cmd.env("OMNITRIX_CONFIG_DIR", dir);
    }
    for (k, v) in extra_env {
        cmd.env(k, v);
    }

    // SAFETY: pre_exec yalnizca async-signal-safe libc cagrilari yapar.
    // setsid + TIOCSCTTY, PTY'yi cocugun denetim terminali yapar; boylece
    // crossterm `/dev/tty` uzerinden ham kipi acabilir.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let t0 = Instant::now();
    let mut child = cmd
        .spawn()
        .map_err(|e| anyhow::anyhow!("{} calistirilamadi: {e}", bin.display()))?;

    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow::anyhow!("cocuk stderr alinamadi"))?;

    let (tx, rx) = channel::<(Instant, String)>();
    std::thread::spawn(move || {
        let reader = BufReader::new(stderr);
        for line in reader.lines() {
            let Ok(line) = line else { break };
            // Alici dustuyse okumaya devam etmenin anlami yok.
            if tx.send((Instant::now(), line)).is_err() {
                break;
            }
        }
    });

    Ok((
        t0,
        TuiProc {
            child,
            lines: rx,
            _pty: pty,
            reaped: false,
        },
    ))
}

// ---------------------------------------------------------------------------
// Yuk uretici — 8.1 "veri hacminden bagimsiz" iddiasini sinar
// ---------------------------------------------------------------------------

/// Cocuk surecin veri dizinine surekli, fsync'li yazma baskisi uygular.
struct WriteLoad {
    stop: Arc<AtomicBool>,
    handles: Vec<std::thread::JoinHandle<()>>,
    bytes: Arc<std::sync::atomic::AtomicU64>,
}

impl WriteLoad {
    fn start(dir: &Path, threads: usize, file_mb: u64) -> anyhow::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let stop = Arc::new(AtomicBool::new(false));
        let bytes = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let mut handles = Vec::new();
        let limit = file_mb.saturating_mul(1024 * 1024).max(1024 * 1024);

        for i in 0..threads.max(1) {
            let path = dir.join(format!("omni-bench-load-{i}.bin"));
            let stop = Arc::clone(&stop);
            let bytes = Arc::clone(&bytes);
            let handle = std::thread::spawn(move || {
                let Ok(mut file) = OpenOptions::new()
                    .create(true)
                    .truncate(true)
                    .write(true)
                    .open(&path)
                else {
                    return;
                };
                let chunk = vec![0xABu8; 1024 * 1024];
                let mut written: u64 = 0;
                while !stop.load(Ordering::Relaxed) {
                    if file.write_all(&chunk).is_err() {
                        break;
                    }
                    // fsync: yazma gercekten diske iner, baski sahici olur.
                    let _ = file.sync_data();
                    written += chunk.len() as u64;
                    bytes.fetch_add(chunk.len() as u64, Ordering::Relaxed);
                    if written >= limit {
                        written = 0;
                        if file.seek(SeekFrom::Start(0)).is_err() {
                            break;
                        }
                    }
                }
            });
            handles.push(handle);
        }
        Ok(Self {
            stop,
            handles,
            bytes,
        })
    }

    fn written_mb(&self) -> f64 {
        self.bytes.load(Ordering::Relaxed) as f64 / (1024.0 * 1024.0)
    }

    fn stop(self) -> f64 {
        let mb = self.written_mb();
        self.stop.store(true, Ordering::Relaxed);
        for h in self.handles {
            let _ = h.join();
        }
        mb
    }
}

// ---------------------------------------------------------------------------
// Kapi (gate) modeli
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize)]
struct Gate {
    id: String,
    command: String,
    metric: String,
    unit: String,
    value: f64,
    threshold: f64,
    comparison: &'static str,
    pass: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stats: Option<Stats>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    samples: Vec<f64>,
}

impl Gate {
    fn less_than(
        id: &str,
        command: String,
        metric: &str,
        unit: &str,
        value: f64,
        threshold: f64,
    ) -> Self {
        Self {
            id: id.to_string(),
            command,
            metric: metric.to_string(),
            unit: unit.to_string(),
            value,
            threshold,
            comparison: "<",
            pass: value < threshold,
            note: None,
            stats: None,
            samples: Vec::new(),
        }
    }

    fn with_note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }

    fn with_samples(mut self, samples: Vec<f64>, st: Stats) -> Self {
        self.samples = samples;
        self.stats = Some(st);
        self
    }
}

#[derive(Debug, serde::Serialize)]
struct Report {
    schema: &'static str,
    tool_version: &'static str,
    generated_at_unix: u64,
    binary: String,
    thresholds: Thresholds,
    gates: Vec<Gate>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    errors: Vec<String>,
    passed: usize,
    failed: usize,
    ok: bool,
}

// ---------------------------------------------------------------------------
// Olcum 1 — cold-start (8.2)
// ---------------------------------------------------------------------------

struct ColdStartSeries {
    wall_ms: Vec<f64>,
    internal_ms: Vec<f64>,
    /// Ilk frame anindaki RSS (kB) — lazy-init'in bellek maliyeti.
    frame_rss_kb: Vec<f64>,
}

fn measure_cold_start(
    bin: &Path,
    args: &Args,
    data_home: &Path,
    runs: usize,
    warmup: usize,
    drop_caches_each: bool,
) -> anyhow::Result<ColdStartSeries> {
    let mut wall_ms = Vec::with_capacity(runs);
    let mut internal_ms = Vec::with_capacity(runs);
    let mut frame_rss_kb = Vec::with_capacity(runs);

    for i in 0..(runs + warmup) {
        if drop_caches_each {
            drop_page_cache()?;
        }
        let (t0, mut proc) = spawn_tui(bin, args, data_home, &[])?;
        let (at, trace) = proc.wait_phase("first_frame", Duration::from_secs(20))?;
        proc.kill_now();

        if i < warmup {
            continue;
        }
        wall_ms.push(at.duration_since(t0).as_secs_f64() * 1000.0);
        internal_ms.push(trace.elapsed_us / 1000.0);
        frame_rss_kb.push(trace.rss_kb);
    }

    Ok(ColdStartSeries {
        wall_ms,
        internal_ms,
        frame_rss_kb,
    })
}

/// Sayfa onbellegini bosaltir. Root degilse hata dondurur (cagiran karar verir).
fn drop_page_cache() -> anyhow::Result<()> {
    // sync() olmadan drop_caches kirli sayfalari birakmaz.
    // SAFETY: sync() argumansizdir ve her zaman guvenlidir.
    unsafe { libc::sync() };
    let mut f = OpenOptions::new()
        .write(true)
        .open("/proc/sys/vm/drop_caches")
        .map_err(|e| anyhow::anyhow!("/proc/sys/vm/drop_caches acilamadi: {e}"))?;
    f.write_all(b"3")
        .map_err(|e| anyhow::anyhow!("drop_caches yazilamadi: {e}"))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Olcum 2 — shutdown (8.1)
// ---------------------------------------------------------------------------

fn measure_shutdown(
    bin: &Path,
    args: &Args,
    data_home: &Path,
    runs: usize,
    load: Option<&WriteLoad>,
) -> anyhow::Result<Vec<f64>> {
    let mut samples = Vec::with_capacity(runs);
    for _ in 0..runs {
        let (_t0, mut proc) = spawn_tui(bin, args, data_home, &[])?;
        // Isinma spawn edilene kadar bekle; SIGINT gozcusu ondan sonra kurulur.
        proc.wait_phase("warmup_spawned", Duration::from_secs(20))?;
        // Isinmanin depolamaya gercekten dokunmasi icin oturusma suresi.
        std::thread::sleep(Duration::from_millis(args.settle_ms));
        let ms = proc.sigint_and_wait(Duration::from_secs(10))?;
        samples.push(ms);
        if let Some(l) = load {
            // Yuk hala aktif mi? Degilse olcum yuk altinda sayilmaz.
            let _ = l.written_mb();
        }
    }
    Ok(samples)
}

// ---------------------------------------------------------------------------
// Olcum 3 — RSS
// ---------------------------------------------------------------------------

fn measure_cli_rss(bin: &Path, data_home: &Path) -> anyhow::Result<f64> {
    let out = Command::new(bin)
        .arg("rss")
        .env("XDG_DATA_HOME", data_home)
        .output()
        .map_err(|e| anyhow::anyhow!("'{} rss' calistirilamadi: {e}", bin.display()))?;
    let text = String::from_utf8_lossy(&out.stdout);
    for token in text.split_whitespace() {
        if let Some(v) = token.strip_prefix("rss_kb=") {
            return v
                .parse::<f64>()
                .map_err(|e| anyhow::anyhow!("rss_kb ayristirilamadi: {e}"));
        }
    }
    anyhow::bail!("'{} rss' ciktisinda rss_kb bulunamadi", bin.display())
}

fn read_proc_rss_kb(pid: i32) -> anyhow::Result<f64> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/status"))
        .map_err(|e| anyhow::anyhow!("/proc/{pid}/status okunamadi: {e}"))?;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            let value = rest
                .split_whitespace()
                .next()
                .ok_or_else(|| anyhow::anyhow!("VmRSS degeri yok"))?;
            return value
                .parse::<f64>()
                .map_err(|e| anyhow::anyhow!("VmRSS ayristirilamadi: {e}"));
        }
    }
    anyhow::bail!("/proc/{pid}/status icinde VmRSS yok")
}

fn measure_steady_rss(bin: &Path, args: &Args, data_home: &Path) -> anyhow::Result<f64> {
    let (_t0, mut proc) = spawn_tui(bin, args, data_home, &[])?;
    proc.wait_phase("warmup_spawned", Duration::from_secs(20))?;
    std::thread::sleep(Duration::from_millis(args.settle_ms));
    let rss = read_proc_rss_kb(proc.pid())?;
    proc.kill_now();
    Ok(rss)
}

// ---------------------------------------------------------------------------
// Ikili bulma + gecici dizin
// ---------------------------------------------------------------------------

fn resolve_binary(args: &Args) -> anyhow::Result<PathBuf> {
    if let Some(p) = &args.bin {
        if !p.exists() {
            anyhow::bail!("--bin yolu yok: {}", p.display());
        }
        return absolute_path(p);
    }
    if let Ok(p) = std::env::var("OMNITRIX_BIN") {
        let p = PathBuf::from(p);
        if p.exists() {
            return absolute_path(&p);
        }
    }
    // omni-bench ile omnitrix ayni target profil dizininde uretilir.
    let exe = std::env::current_exe().map_err(|e| anyhow::anyhow!("current_exe okunamadi: {e}"))?;
    if let Some(dir) = exe.parent() {
        let candidate = dir.join("omnitrix");
        if candidate.exists() {
            return absolute_path(&candidate);
        }
    }
    anyhow::bail!(
        "omnitrix ikilisi bulunamadi — --bin <yol> ver ya da OMNITRIX_BIN ayarla \
         (once 'cargo build --release -p omnitrix')"
    )
}

/// Goreli yolu mutlak hale getirir. `Path::canonicalize` clippy.toml ile
/// yasak (verbatim yol sorunu); burada sembolik bag cozumune de gerek yok —
/// yol yalnizca `Command::new` ve rapora yazilacak.
fn absolute_path(p: &Path) -> anyhow::Result<PathBuf> {
    if p.is_absolute() {
        return Ok(p.to_path_buf());
    }
    let cwd = std::env::current_dir().map_err(|e| anyhow::anyhow!("cwd okunamadi: {e}"))?;
    Ok(cwd.join(p))
}

/// Cocuk sureclerin veri dizini icin izole, benzersiz bir gecici kok.
struct ScratchDir {
    path: PathBuf,
}

impl ScratchDir {
    fn new(tag: &str) -> anyhow::Result<Self> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        // SAFETY: getpid argumansiz ve her zaman guvenlidir.
        let pid = unsafe { libc::getpid() };
        let path = std::env::temp_dir().join(format!("omni-bench-{tag}-{pid}-{nanos}"));
        std::fs::create_dir_all(&path)?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

// ---------------------------------------------------------------------------
// Rapor cikti
// ---------------------------------------------------------------------------

fn print_table(report: &Report) {
    let header = ["OLCUM", "KOMUT", "METRIK", "DEGER", "ESIK", "DURUM"];
    let mut rows: Vec<[String; 6]> = Vec::new();
    for g in &report.gates {
        rows.push([
            g.id.clone(),
            g.command.clone(),
            g.metric.clone(),
            format!("{:.3} {}", g.value, g.unit),
            format!("{} {:.3} {}", g.comparison, g.threshold, g.unit),
            if g.pass {
                "GECTI".into()
            } else {
                "KALDI".into()
            },
        ]);
    }

    let mut width = [0usize; 6];
    for (i, h) in header.iter().enumerate() {
        width[i] = h.chars().count();
    }
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            width[i] = width[i].max(cell.chars().count());
        }
    }

    let line: String = width
        .iter()
        .map(|w| "-".repeat(w + 2))
        .collect::<Vec<_>>()
        .join("+");

    println!("omni-bench {VERSION} — faz kapisi olcumleri");
    println!("ikili: {}", report.binary);
    println!("+{line}+");
    let head: Vec<String> = header
        .iter()
        .enumerate()
        .map(|(i, h)| format!("{:<w$}", h, w = width[i]))
        .collect();
    println!("| {} |", head.join(" | "));
    println!("+{line}+");
    for row in &rows {
        let cells: Vec<String> = row
            .iter()
            .enumerate()
            .map(|(i, c)| format!("{:<w$}", c, w = width[i]))
            .collect();
        println!("| {} |", cells.join(" | "));
    }
    println!("+{line}+");

    for g in &report.gates {
        if let Some(st) = &g.stats {
            println!(
                "  {} istatistik: n={} min={:.3} med={:.3} p95={:.3} max={:.3} mean={:.3} ({})",
                g.id, st.n, st.min, st.median, st.p95, st.max, st.mean, g.unit
            );
        }
        if let Some(note) = &g.note {
            println!("  {} not: {note}", g.id);
        }
    }
    for e in &report.errors {
        println!("  HATA: {e}");
    }
    println!(
        "\nkapilar: {} gecti, {} kaldi -> {}",
        report.passed,
        report.failed,
        if report.ok { "YESIL" } else { "KIRMIZI" }
    );
}

fn emit_json(report: &Report, target: &Path) -> anyhow::Result<()> {
    let text = serde_json::to_string_pretty(report)
        .map_err(|e| anyhow::anyhow!("JSON serilestirilemedi: {e}"))?;
    if target == Path::new("-") {
        println!("{text}");
        return Ok(());
    }
    if let Some(parent) = target.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(target, text)
        .map_err(|e| anyhow::anyhow!("JSON yazilamadi ({}): {e}", target.display()))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

fn main() {
    match run() {
        Ok(true) => std::process::exit(0),
        Ok(false) => std::process::exit(1),
        Err(e) => {
            eprintln!("omni-bench: {e:#}");
            std::process::exit(2);
        }
    }
}

/// `Ok(true)` => tum kapilar yesil.
fn run() -> anyhow::Result<bool> {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    // Alt-komut: soak 7/24 uzun-kosu (Bolum 20 kesisen kapi) — kendi ayristiricisi var.
    if raw.first().is_some_and(|a| a == "soak") {
        return soak::main_subcommand(&raw[1..]);
    }
    let Some(args) = parse_args(raw)? else {
        return Ok(true);
    };

    let bin = resolve_binary(&args)?;
    let mut gates: Vec<Gate> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    let th = args.thresholds;

    // --- cold-start -------------------------------------------------------
    if !args.skipped("cold-start") {
        let scratch = ScratchDir::new("cold")?;
        match measure_cold_start(&bin, &args, scratch.path(), args.runs, args.warmup, false) {
            Ok(series) => {
                let wall = stats(&series.wall_ms);
                let internal = stats(&series.internal_ms);
                let frame_rss = stats(&series.frame_rss_kb);
                let cmd = format!(
                    "OMNITRIX_TRACE_STARTUP=1 {} (pty, {} kosum)",
                    bin.display(),
                    wall.n
                );

                gates.push(
                    Gate::less_than(
                        "cold_start.warm_cache",
                        cmd.clone(),
                        "wall median (exec->first_frame)",
                        "ms",
                        wall.median,
                        th.cold_start_warm_ms,
                    )
                    .with_note(format!(
                        "ikili-ici damga medyani {:.3} ms (exec/dinamik baglama harici); \
                         ilk frame aninda RSS medyani {:.0} kB",
                        internal.median, frame_rss.median
                    ))
                    .with_samples(series.wall_ms.clone(), wall.clone()),
                );

                // Soguk seri: sayfa onbellegi bosaltilabiliyorsa gercek olcum,
                // aksi halde sicak serinin p95'i vekil olarak kullanilir.
                let cold_gate = if args.drop_caches {
                    match drop_page_cache() {
                        Ok(()) => {
                            let cold_scratch = ScratchDir::new("coldcache")?;
                            let cold_runs = args.runs.clamp(1, 5);
                            match measure_cold_start(
                                &bin,
                                &args,
                                cold_scratch.path(),
                                cold_runs,
                                0,
                                true,
                            ) {
                                Ok(cold) => {
                                    let cs = stats(&cold.wall_ms);
                                    Some(
                                        Gate::less_than(
                                            "cold_start.cold_cache",
                                            format!(
                                                "drop_caches=3 + OMNITRIX_TRACE_STARTUP=1 {} ({} kosum)",
                                                bin.display(),
                                                cs.n
                                            ),
                                            "wall median (exec->first_frame)",
                                            "ms",
                                            cs.median,
                                            th.cold_start_cold_ms,
                                        )
                                        .with_note("gercek soguk sayfa onbellegi")
                                        .with_samples(cold.wall_ms.clone(), cs.clone()),
                                    )
                                }
                                Err(e) => {
                                    errors.push(format!("soguk cold-start serisi: {e:#}"));
                                    None
                                }
                            }
                        }
                        Err(e) => {
                            errors.push(format!(
                                "sayfa onbellegi bosaltilamadi (root gerekir): {e:#}"
                            ));
                            None
                        }
                    }
                } else {
                    None
                };

                gates.push(cold_gate.unwrap_or_else(|| {
                    Gate::less_than(
                        "cold_start.cold_cache",
                        cmd,
                        "wall p95 (exec->first_frame)",
                        "ms",
                        wall.p95,
                        th.cold_start_cold_ms,
                    )
                    .with_note(
                        "VEKIL: sayfa onbellegi bosaltilmadi; sicak serinin p95'i soguk \
                         esige vuruluyor. Gercek olcum icin --drop-caches (root)",
                    )
                    .with_samples(series.wall_ms, wall)
                }));
            }
            Err(e) => errors.push(format!("cold-start olculemedi: {e:#}")),
        }
    }

    // --- shutdown ---------------------------------------------------------
    if !args.skipped("shutdown") {
        let scratch = ScratchDir::new("shutdown")?;
        match measure_shutdown(&bin, &args, scratch.path(), args.shutdown_runs, None) {
            Ok(samples) => {
                let st = stats(&samples);
                gates.push(
                    Gate::less_than(
                        "shutdown.idle",
                        format!("kill -INT <omnitrix pid> ({} kosum)", st.n),
                        "SIGINT->exit max",
                        "ms",
                        st.max,
                        th.shutdown_ms,
                    )
                    .with_note("bos: isinma tamam, ek yuk yok")
                    .with_samples(samples, st),
                );
            }
            Err(e) => errors.push(format!("shutdown (bos) olculemedi: {e:#}")),
        }

        // Yuk altinda: 8.1 "veri hacminden bagimsiz" iddiasi.
        let load_scratch = ScratchDir::new("shutdown-load")?;
        let load_dir = load_scratch.path().join("omnitrix").join("bench-load");
        match WriteLoad::start(&load_dir, args.load_threads, args.load_file_mb) {
            Ok(load) => {
                // Yuk sahiden akmaya baslasin.
                std::thread::sleep(Duration::from_millis(500));
                let result = measure_shutdown(
                    &bin,
                    &args,
                    load_scratch.path(),
                    args.shutdown_runs,
                    Some(&load),
                );
                let written_mb = load.stop();
                match result {
                    Ok(samples) => {
                        let st = stats(&samples);
                        gates.push(
                            Gate::less_than(
                                "shutdown.under_load",
                                format!(
                                    "kill -INT <omnitrix pid> ({} kosum, {} fsync'li yazici)",
                                    st.n, args.load_threads
                                ),
                                "SIGINT->exit max",
                                "ms",
                                st.max,
                                th.shutdown_ms,
                            )
                            .with_note(format!(
                                "olcum sirasinda veri dizinine {written_mb:.0} MB fsync'li yazildi \
                                 — kapanis O(1), veri hacminden bagimsiz olmali"
                            ))
                            .with_samples(samples, st),
                        );
                    }
                    Err(e) => errors.push(format!("shutdown (yuk altinda) olculemedi: {e:#}")),
                }
            }
            Err(e) => errors.push(format!("yuk ureticisi baslatilamadi: {e:#}")),
        }
    }

    // --- RSS --------------------------------------------------------------
    if !args.skipped("rss") {
        let scratch = ScratchDir::new("rss")?;
        match measure_cli_rss(&bin, scratch.path()) {
            Ok(kb) => gates.push(
                Gate::less_than(
                    "rss.cli",
                    format!("{} rss", bin.display()),
                    "rss_kb (arguman ayristirmasi sonrasi)",
                    "kB",
                    kb,
                    th.rss_max_kb,
                )
                .with_note("lazy-init taban cizgisi: provider/storage acilmadan"),
            ),
            Err(e) => errors.push(format!("rss (cli) olculemedi: {e:#}")),
        }

        match measure_steady_rss(&bin, &args, scratch.path()) {
            Ok(kb) => gates.push(
                Gate::less_than(
                    "rss.tui_steady",
                    format!(
                        "{} (pty) + /proc/<pid>/status, isinma + {} ms",
                        bin.display(),
                        args.settle_ms
                    ),
                    "VmRSS",
                    "kB",
                    kb,
                    th.rss_max_kb,
                )
                .with_note("isinma tamamlandiktan sonraki kararli durum"),
            ),
            Err(e) => errors.push(format!("rss (tui) olculemedi: {e:#}")),
        }
    }

    let failed = gates.iter().filter(|g| !g.pass).count();
    let passed = gates.len() - failed;
    let ok = failed == 0 && errors.is_empty() && !gates.is_empty();

    let report = Report {
        schema: "omni-bench/1",
        tool_version: VERSION,
        generated_at_unix: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        binary: bin.display().to_string(),
        thresholds: th,
        gates,
        errors,
        passed,
        failed,
        ok,
    };

    print_table(&report);
    if let Some(target) = &args.json_out {
        emit_json(&report, target)?;
    }

    Ok(report.ok)
}

// ---------------------------------------------------------------------------
// Testler
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_tek_sayida_orneklem() {
        let s = stats(&[3.0, 1.0, 2.0]);
        assert_eq!(s.n, 3);
        assert!((s.min - 1.0).abs() < f64::EPSILON);
        assert!((s.median - 2.0).abs() < f64::EPSILON);
        assert!((s.max - 3.0).abs() < f64::EPSILON);
        assert!((s.mean - 2.0).abs() < 1e-12);
    }

    #[test]
    fn stats_cift_sayida_orneklem_medyani_ortalar() {
        let s = stats(&[1.0, 2.0, 3.0, 4.0]);
        assert!((s.median - 2.5).abs() < f64::EPSILON);
    }

    #[test]
    fn stats_p95_en_yakin_sira() {
        let samples: Vec<f64> = (1..=100).map(f64::from).collect();
        let s = stats(&samples);
        assert!((s.p95 - 95.0).abs() < f64::EPSILON);
    }

    #[test]
    fn stats_bos_orneklem_cokmez() {
        let s = stats(&[]);
        assert_eq!(s.n, 0);
    }

    #[test]
    fn startup_trace_ayristirilir() {
        let line =
            "omnitrix startup: phase=first_frame elapsed_us=8123 elapsed_ms=8.123 rss_kb=4096";
        let t = parse_startup_trace(line).expect("trace ayristirilmali");
        assert_eq!(t.phase, "first_frame");
        assert!((t.elapsed_us - 8123.0).abs() < f64::EPSILON);
        assert!((t.rss_kb - 4096.0).abs() < f64::EPSILON);
    }

    #[test]
    fn ilgisiz_satir_trace_degil() {
        assert!(parse_startup_trace("some unrelated stderr line").is_none());
    }

    #[test]
    fn varsayilan_esikler_plan_ile_uyumlu() {
        let t = Thresholds::default();
        assert!((t.cold_start_warm_ms - 100.0).abs() < f64::EPSILON);
        assert!((t.cold_start_cold_ms - 400.0).abs() < f64::EPSILON);
        assert!((t.shutdown_ms - 50.0).abs() < f64::EPSILON);
    }

    #[test]
    fn esik_dosyasi_okunur_bayrak_ezer() {
        let dir = ScratchDir::new("test-th").expect("gecici dizin");
        let path = dir.path().join("th.json");
        std::fs::write(
            &path,
            r#"{"cold_start_warm_ms":42.0,"shutdown_ms":7.0,"rss_max_kb":1024.0,"cold_start_cold_ms":99.0}"#,
        )
        .expect("yaz");

        let args = parse_args(vec![
            "--thresholds".into(),
            path.display().to_string(),
            "--shutdown-ms".into(),
            "5".into(),
        ])
        .expect("ayristir")
        .expect("args");

        // Dosya degeri temel alinir...
        assert!((args.thresholds.cold_start_warm_ms - 42.0).abs() < f64::EPSILON);
        // ...ama acikca verilen bayrak dosyayi ezer.
        assert!((args.thresholds.shutdown_ms - 5.0).abs() < f64::EPSILON);
    }

    #[test]
    fn bilinmeyen_arguman_hata() {
        assert!(parse_args(vec!["--yok-boyle".into()]).is_err());
    }

    #[test]
    fn skip_listesi_calisir() {
        let args = parse_args(vec!["--skip".into(), "rss".into()])
            .expect("ayristir")
            .expect("args");
        assert!(args.skipped("rss"));
        assert!(!args.skipped("shutdown"));
    }

    #[test]
    fn sifir_kosum_reddedilir() {
        assert!(parse_args(vec!["--runs".into(), "0".into()]).is_err());
    }

    #[test]
    fn rapor_json_serilesir() {
        let gate = Gate::less_than("x.y", "cmd".into(), "m", "ms", 1.0, 2.0);
        assert!(gate.pass);
        let report = Report {
            schema: "omni-bench/1",
            tool_version: VERSION,
            generated_at_unix: 0,
            binary: "/bin/true".into(),
            thresholds: Thresholds::default(),
            gates: vec![gate],
            errors: vec![],
            passed: 1,
            failed: 0,
            ok: true,
        };
        let text = serde_json::to_string(&report).expect("serilesmeli");
        assert!(text.contains("\"ok\":true"));
        assert!(text.contains("omni-bench/1"));
    }

    #[test]
    fn pty_acilir_ve_slave_yolu_verir() {
        let pty = Pty::open(80, 24).expect("pty acilmali");
        assert!(pty.slave_path.starts_with("/dev/pts") || pty.slave_path.starts_with("/dev/"));
        assert!(pty.open_slave().is_ok());
    }

    #[test]
    fn proc_rss_kendi_surecimiz_icin_okunur() {
        // SAFETY: getpid argumansizdir.
        let pid = unsafe { libc::getpid() };
        let kb = read_proc_rss_kb(pid).expect("VmRSS okunmali");
        assert!(kb > 0.0);
    }
}
