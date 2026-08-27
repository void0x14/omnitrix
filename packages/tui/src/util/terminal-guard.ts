import * as fs from "fs"
import * as path from "path"

let installed = false
let originals: {
  log?: typeof console.log
  error?: typeof console.error
  warn?: typeof console.warn
  debug?: typeof console.debug
  info?: typeof console.info
  stdoutWrite?: typeof process.stdout.write
  stderrWrite?: typeof process.stderr.write
} = {}
let logFilePath = ""
let cleanupFns: Array<() => void> = []

function getLogFile(): string {
  // Use Global.Path.log if available, else fallback to ~/.local/share/opencode/log/app-debug.log
  try {
    // Dynamic import to avoid cycle
    // eslint-disable-next-line @typescript-eslint/no-require-imports
    const { Global } = require("@opencode-ai/core/global")
    if (Global?.Path?.log) return path.join(Global.Path.log, "app-debug.log")
  } catch {}
  const home = process.env.HOME || process.env.USERPROFILE || "/tmp"
  return path.join(home, ".local", "share", "opencode", "log", "app-debug.log")
}

function writeLog(level: string, args: any[]) {
  const ts = new Date().toISOString()
  // Detect AI_APICallError and sanitize
  const isAIError = args.some(
    (a) =>
      typeof a === "string" &&
      (a.includes("AI_APICallError") || a.includes("RegionError") || a.includes("stream error")),
  )
  // For AI errors, write a compact version to file, not full JSON dump
  let line: string
  if (isAIError) {
    const msg = args
      .map((a) => {
        if (typeof a === "string") {
          // Truncate huge base64 or responseBody
          if (a.length > 2000) return a.slice(0, 2000) + "… [truncated]"
          return a
        }
        try {
          const s = JSON.stringify(a)
          return s.length > 2000 ? s.slice(0, 2000) + "… [truncated]" : s
        } catch {
          return String(a)
        }
      })
      .join(" ")
    line = `[${ts}] ${level} AI_ERROR ${msg}\n`
  } else {
    const msg = args
      .map((a) => {
        if (typeof a === "string") return a
        try {
          return JSON.stringify(a)
        } catch {
          return String(a)
        }
      })
      .join(" ")
    line = `[${ts}] ${level} ${msg}\n`
  }
  try {
    fs.mkdirSync(path.dirname(logFilePath), { recursive: true })
    fs.appendFileSync(logFilePath, line)
  } catch {}
}

function isRendererFrame(chunk: any): boolean {
  const str = typeof chunk === "string" ? chunk : chunk?.toString?.("utf8") ?? ""
  // Renderer frames are large ANSI sequences, typically >200 chars and contain CSI
  // We must allow them through, otherwise TUI won't render
  if (str.length === 0) return false
  // If it contains alternate screen or cursor movement, it's renderer
  if (str.includes("\x1b[?1049") || str.includes("\x1b[?25") || str.includes("\x1b[H") || str.includes("\x1b[2J")) {
    return true
  }
  // Heuristic: renderer writes are typically >100 chars with many ESC
  const escCount = (str.match(/\x1b\[/g) || []).length
  if (escCount >= 3 && str.length > 80) return true
  // Small writes like single char input should also pass if they are ANSI
  if (escCount >= 1 && str.length > 20) return true
  return false
}

export function installTerminalGuard(): () => void {
  if (installed) return () => {}
  installed = true
  logFilePath = getLogFile()
  try {
    fs.mkdirSync(path.dirname(logFilePath), { recursive: true })
  } catch {}

  // Save originals
  originals.log = console.log
  originals.error = console.error
  originals.warn = console.warn
  originals.debug = console.debug
  originals.info = console.info
  originals.stdoutWrite = process.stdout.write.bind(process.stdout) as any
  originals.stderrWrite = process.stderr.write.bind(process.stderr) as any

  // 1) Monkey-patch console.* -> file only
  console.log = (...args: any[]) => writeLog("LOG", args)
  console.error = (...args: any[]) => writeLog("ERROR", args)
  console.warn = (...args: any[]) => writeLog("WARN", args)
  console.debug = (...args: any[]) => writeLog("DEBUG", args)
  console.info = (...args: any[]) => writeLog("INFO", args)

  // 1b) Monkey-patch stdout/stderr -> file, except renderer frames
  process.stdout.write = ((chunk: any, encoding?: any, cb?: any) => {
    if (isRendererFrame(chunk)) {
      return (originals.stdoutWrite as any)(chunk, encoding, cb)
    }
    // For non-renderer plain text (background logs), pipe to file only
    const str = typeof chunk === "string" ? chunk : chunk?.toString?.("utf8") ?? String(chunk)
    // Filter out the debug JSON that was corrupting TUI (write:0, time:{...})
    if (str.trim().length > 0) {
      writeLog("STDOUT", [str.trim().slice(0, 2000)])
    }
    if (typeof cb === "function") cb()
    return true
  }) as any

  process.stderr.write = ((chunk: any, encoding?: any, cb?: any) => {
    if (isRendererFrame(chunk)) {
      return (originals.stderrWrite as any)(chunk, encoding, cb)
    }
    const str = typeof chunk === "string" ? chunk : chunk?.toString?.("utf8") ?? String(chunk)
    if (str.trim().length > 0) {
      // Suppress AI_APICallError raw dump from reaching terminal
      if (str.includes("AI_APICallError") || str.includes("RegionError") || str.includes("stream error")) {
        writeLog("STDERR_AI", [str.slice(0, 2000)])
        if (typeof cb === "function") cb()
        return true
      }
      writeLog("STDERR", [str.trim().slice(0, 2000)])
    }
    if (typeof cb === "function") cb()
    return true
  }) as any

  // 2) Guarantee Alternate Screen Buffer enter
  try {
    ;(originals.stdoutWrite as any)("\x1b[?1049h")
  } catch {}
  try {
    ;(originals.stdoutWrite as any)("\x1b[?25l")
  } catch {}

  const restoreTerminal = () => {
    try {
      ;(originals.stdoutWrite as any)("\x1b[?1049l")
    } catch {}
    try {
      ;(originals.stdoutWrite as any)("\x1b[?25h")
    } catch {}
    try {
      ;(originals.stdoutWrite as any)("\x1b[0m")
    } catch {}
  }

  const restoreAll = () => {
    if (!installed) return
    installed = false
    restoreTerminal()
    try {
      process.stdout.write = originals.stdoutWrite as any
    } catch {}
    try {
      process.stderr.write = originals.stderrWrite as any
    } catch {}
    try {
      console.log = originals.log as any
    } catch {}
    try {
      console.error = originals.error as any
    } catch {}
    try {
      console.warn = originals.warn as any
    } catch {}
    try {
      console.debug = originals.debug as any
    } catch {}
    try {
      console.info = originals.info as any
    } catch {}
    for (const fn of cleanupFns) {
      try {
        fn()
      } catch {}
    }
    cleanupFns = []
  }

  // 3) Signal handlers with clean close before logging
  const handleUncaught = (err: unknown) => {
    const msg = err instanceof Error ? err.stack || err.message : String(err)
    writeLog("FATAL_UNCAUGHT", [msg])
    restoreAll()
    // Use original stderr to ensure message is visible after restore (in normal terminal, not TUI)
    try {
      originals.stderrWrite?.call(process.stderr, `[FATAL] ${msg}\n`)
    } catch {}
    process.exit(1)
  }
  const handleUnhandled = (reason: unknown) => {
    const msg = reason instanceof Error ? reason.stack || reason.message : String(reason)
    // Suppress AI errors from crashing - just log to file
    if (String(msg).includes("AI_APICallError") || String(msg).includes("RegionError")) {
      writeLog("HANDLED_AI_REJECTION", [msg.slice(0, 2000)])
      return
    }
    writeLog("FATAL_UNHANDLED", [msg])
    restoreAll()
    try {
      originals.stderrWrite?.call(process.stderr, `[UNHANDLED] ${msg}\n`)
    } catch {}
  }
  const handleSigInt = () => {
    writeLog("SIGNAL", ["SIGINT"])
    restoreAll()
    process.exit(0)
  }
  const handleSigTerm = () => {
    writeLog("SIGNAL", ["SIGTERM"])
    restoreAll()
    process.exit(0)
  }

  process.on("uncaughtException", handleUncaught)
  process.on("unhandledRejection", handleUnhandled)
  process.on("SIGINT", handleSigInt)
  process.on("SIGTERM", handleSigTerm)
  cleanupFns.push(() => process.off("uncaughtException", handleUncaught))
  cleanupFns.push(() => process.off("unhandledRejection", handleUnhandled))
  cleanupFns.push(() => process.off("SIGINT", handleSigInt))
  cleanupFns.push(() => process.off("SIGTERM", handleSigTerm))

  // Return restore function for normal exit
  return restoreAll
}

export function isAIAPICallError(error: unknown): boolean {
  if (!error || typeof error !== "object") return false
  const anyErr = error as any
  return (
    anyErr.name === "AI_APICallError" ||
    anyErr.type === "AI_APICallError" ||
    String(anyErr.message || "").includes("AI_APICallError") ||
    String(anyErr.message || "").includes("RegionError") ||
    String(anyErr.code || "").includes("RegionError")
  )
}
