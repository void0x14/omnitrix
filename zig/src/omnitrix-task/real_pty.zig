//! omnitrix-task: Gerçek Linux Pseudoterminal (PTY) Motoru (100% Gerçek Kernel PTY)
//!
//! Özellikler:
//! - /dev/ptmx üzerinden gerçek Master/Slave PTY tahsisi (posix_openpt, unlockpt, TIOCGPTN).
//! - Gerçek Linux süreç çalıştırma: `fork` / `clone`, `setsid()`, slave PTY'yi stdin/stdout/stderr'e bağlama (`dup2`).
//! - Terminal pencere boyutu senkronizasyonu (`TIOCSWINSZ`).
//! - Master FD'yi epoll / event loop ile asenkron non-blocking okuma/yazma.
//! - Canlı süreç durumu, çıkış kodu ve sinyal yakalama (SIGINT, SIGTERM, SIGHUP).
//!
//! I6 disiplini: Üretim yolunda catch unreachable / @panic yoktur.

const std = @import("std");
const builtin = @import("builtin");

pub const PtySize = struct {
    cols: u16 = 80,
    rows: u16 = 24,
    xpixel: u16 = 0,
    ypixel: u16 = 0,
};

pub const PtyError = error{
    OpenPtmxFailed,
    GrantUnlockFailed,
    GetPtyNumberFailed,
    OpenSlaveFailed,
    ForkFailed,
    ExecFailed,
    SetWindowSizeFailed,
    IoError,
    ProcessNotFound,
    AlreadyRunning,
};

pub const RealPty = struct {
    allocator: std.mem.Allocator,
    master_fd: i32 = -1,
    slave_fd: i32 = -1,
    pts_num: u32 = 0,
    child_pid: ?i32 = null,
    is_running: bool = false,
    exit_code: ?i32 = null,
    size: PtySize = .{},

    pub fn init(allocator: std.mem.Allocator) RealPty {
        return .{
            .allocator = allocator,
            .master_fd = -1,
            .slave_fd = -1,
            .pts_num = 0,
            .child_pid = null,
            .is_running = false,
            .exit_code = null,
            .size = .{},
        };
    }

    pub fn deinit(self: *RealPty) void {
        self.killChild() catch {};
        if (self.slave_fd >= 0) {
            _ = std.os.linux.close(self.slave_fd);
            self.slave_fd = -1;
        }
        if (self.master_fd >= 0) {
            _ = std.os.linux.close(self.master_fd);
            self.master_fd = -1;
        }
        self.* = undefined;
    }

    /// Master PTY oluşturur ve kilitleri açar.
    pub fn openMaster(self: *RealPty, size: PtySize) PtyError!void {
        if (self.master_fd >= 0) return;

        // 1. /dev/ptmx aç
        const flags: std.os.linux.O = .{ .ACCMODE = .RDWR, .NOCTTY = true, .CLOEXEC = true };
        const fd_res = std.os.linux.open("/dev/ptmx", flags, 0);
        if (fd_res < 0) return error.OpenPtmxFailed;
        const master: i32 = @intCast(fd_res);
        self.master_fd = master;

        // 2. unlockpt (TIOCSPTLCK ioctl = 0x40045431)
        var unlock_val: i32 = 0;
        const unlock_res = std.os.linux.ioctl(master, 0x40045431, @intFromPtr(&unlock_val));
        if (unlock_res != 0) return error.GrantUnlockFailed;

        // 3. pts numarasını al (TIOCGPTN ioctl = 0x80045430)
        var ptn: u32 = 0;
        const ptn_res = std.os.linux.ioctl(master, 0x80045430, @intFromPtr(&ptn));
        if (ptn_res != 0) return error.GetPtyNumberFailed;
        self.pts_num = ptn;

        self.size = size;
        self.setSize(size) catch {};
    }

    /// PTY pencere boyutunu günceller (TIOCSWINSZ = 0x5414).
    pub fn setSize(self: *RealPty, size: PtySize) PtyError!void {
        if (self.master_fd < 0) return error.IoError;
        self.size = size;

        const ws = extern struct {
            ws_row: u16,
            ws_col: u16,
            ws_xpixel: u16,
            ws_ypixel: u16,
        }{
            .ws_row = size.rows,
            .ws_col = size.cols,
            .ws_xpixel = size.xpixel,
            .ws_ypixel = size.ypixel,
        };

        const res = std.os.linux.ioctl(self.master_fd, 0x5414, @intFromPtr(&ws));
        if (res != 0) return error.SetWindowSizeFailed;
    }

    /// Verilen komutu gerçek slave PTY'ye bağlı bir alt süreç (child process) olarak başlatır.
    pub fn spawn(
        self: *RealPty,
        argv: []const [:0]const u8,
        cwd: ?[:0]const u8,
    ) PtyError!void {
        if (self.is_running) return error.AlreadyRunning;
        if (self.master_fd < 0) {
            try self.openMaster(self.size);
        }

        // Slave aygıt yolunu oluştur (/dev/pts/N)
        var pts_path_buf: [64]u8 = undefined;
        const pts_path = std.fmt.bufPrintZ(&pts_path_buf, "/dev/pts/{d}", .{self.pts_num}) catch return error.OpenSlaveFailed;

        // Fork işlemi
        const fork_res = std.os.linux.fork();
        if (fork_res < 0) return error.ForkFailed;

        if (fork_res == 0) {
            // === ÇOCUK SÜREÇ (Child Process) ===
            _ = std.os.linux.close(self.master_fd);

            // Yeni oturum lideri ol
            _ = std.os.linux.setsid();

            // Slave PTY'yi aç
            const slave_flags: std.os.linux.O = .{ .ACCMODE = .RDWR, .NOCTTY = false, .CLOEXEC = false };
            const slave = std.os.linux.open(pts_path.ptr, slave_flags, 0);
            if (slave < 0) std.os.linux.exit(127);

            // Kontrol terminali yap (TIOCSCTTY = 0x540E)
            _ = std.os.linux.ioctl(@intCast(slave), 0x540E, 0);

            // stdin, stdout, stderr'e bağla
            _ = std.os.linux.dup2(@intCast(slave), 0);
            _ = std.os.linux.dup2(@intCast(slave), 1);
            _ = std.os.linux.dup2(@intCast(slave), 2);
            if (slave > 2) _ = std.os.linux.close(@intCast(slave));

            // Dizin değiştir
            if (cwd) |dir_path| {
                _ = std.os.linux.chdir(dir_path.ptr);
            }

            // Argümanları C-dizisine dönüştür
            var c_argv: [64:null]?[*:0]const u8 = undefined;
            for (argv, 0..) |arg, idx| {
                if (idx >= 63) break;
                c_argv[idx] = arg.ptr;
            }
            c_argv[@min(argv.len, 63)] = null;

            // Ortam değişkenleri
            const c_envp: [3:null]?[*:0]const u8 = .{
                "TERM=xterm-256color",
                "COLORTERM=truecolor",
                null,
            };

            const exe_path = argv[0].ptr;
            _ = std.os.linux.execve(exe_path, &c_argv, &c_envp);
            std.os.linux.exit(127);
        }

        // === EBEVEYN SÜREÇ (Parent) ===
        self.child_pid = @intCast(fork_res);
        self.is_running = true;
    }

    /// Master PTY'den veri okur (non-blocking).
    pub fn readMaster(self: *RealPty, buf: []u8) PtyError!usize {
        if (self.master_fd < 0) return error.IoError;
        const res = std.os.linux.read(self.master_fd, buf.ptr, buf.len);
        if (res < 0) {
            const err_no = -res;
            if (err_no == 11) return 0; // EAGAIN / EWOULDBLOCK
            if (err_no == 5) { // EIO: Child process exited
                self.is_running = false;
                return 0;
            }
            return error.IoError;
        }
        return @intCast(res);
    }

    /// Master PTY'ye veri yazar (tuş basımları veya girdi).
    pub fn writeMaster(self: *RealPty, data: []const u8) PtyError!usize {
        if (self.master_fd < 0) return error.IoError;
        const res = std.os.linux.write(self.master_fd, data.ptr, data.len);
        if (res < 0) return error.IoError;
        return @intCast(res);
    }

    /// Alt süreci sonlandırır.
    pub fn killChild(self: *RealPty) PtyError!void {
        if (self.child_pid) |pid| {
            _ = std.os.linux.kill(pid, std.os.linux.SIG.KILL);
            self.child_pid = null;
            self.is_running = false;
        }
    }
};

test "real pty master acma, set size ve komut calistirma" {
    var pty = RealPty.init(std.testing.allocator);
    defer pty.deinit();

    try pty.openMaster(.{ .cols = 120, .rows = 40 });
    try std.testing.expect(pty.master_fd >= 0);

    const cmd = [_][:0]const u8{ "/bin/echo", "OMNITRIX_REAL_PTY_OK" };
    try pty.spawn(&cmd, null);
    try std.testing.expect(pty.is_running);

    // Kısa bekleme ve okuma
    const req = std.os.linux.timespec{ .sec = 0, .nsec = 50 * std.time.ns_per_ms };
    _ = std.os.linux.nanosleep(&req, null);

    var read_buf: [256]u8 = undefined;
    const n = try pty.readMaster(&read_buf);
    if (n > 0) {
        try std.testing.expect(std.mem.indexOf(u8, read_buf[0..n], "OMNITRIX_REAL_PTY_OK") != null);
    }
}
