import * as fs from "fs"
import * as path from "path"
import yargs from "yargs"
import { hideBin } from "yargs/helpers"
import { RunCommand } from "./cli/cmd/run"
import { GenerateCommand } from "./cli/cmd/generate"
import { ConsoleCommand } from "./cli/cmd/account"
import { ProvidersCommand } from "./cli/cmd/providers"
import { AgentCommand } from "./cli/cmd/agent"
import { UpgradeCommand } from "./cli/cmd/upgrade"
import { UninstallCommand } from "./cli/cmd/uninstall"
import { ModelsCommand } from "./cli/cmd/models"
import { UI } from "./cli/ui"
import { InstallationVersion } from "@opencode-ai/core/installation/version"
import { FormatError } from "./cli/error"
import { ServeCommand } from "./cli/cmd/serve"
import { DebugCommand } from "./cli/cmd/debug"
import { StatsCommand } from "./cli/cmd/stats"
import { McpCommand } from "./cli/cmd/mcp"
import { GithubCommand } from "./cli/cmd/github"
import { ExportCommand } from "./cli/cmd/export"
import { ImportCommand } from "./cli/cmd/import"
import { AttachCommand } from "./cli/cmd/attach"
import { TuiThreadCommand } from "./cli/cmd/tui"
import { AcpCommand } from "./cli/cmd/acp"
import { EOL } from "os"
import { WebCommand } from "./cli/cmd/web"
import { PrCommand } from "./cli/cmd/pr"
import { SessionCommand } from "./cli/cmd/session"
import { DbCommand } from "./cli/cmd/db"
import { errorMessage } from "./util/error"
import { PluginCommand } from "./cli/cmd/plug"

// --- Terminal Guard: monkey-patch console/stdout/stderr to prevent TUI corruption ---
const __guardLogFile = (() => {
  try {
    const home = process.env.HOME || process.env.USERPROFILE || "/tmp"
    return path.join(home, ".local", "share", "opencode", "log", "app-debug.log")
  } catch { return "/tmp/app-debug.log" }
})()
try { fs.mkdirSync(path.dirname(__guardLogFile), { recursive: true }) } catch {}
const __guardWrite = (lvl: string, args: any[]) => {
  const msg = args.map(a => typeof a === "string" ? a : (() => { try { return JSON.stringify(a) } catch { return String(a) } })()).join(" ")
  if (msg.includes("AI_APICallError") || msg.includes("RegionError")) {
    const compact = msg.length > 2000 ? msg.slice(0, 2000) + "… [truncated]" : msg
    try { fs.appendFileSync(__guardLogFile, `[${new Date().toISOString()}] ${lvl} AI_ERROR ${compact}\n`) } catch {}
    return
  }
  try { fs.appendFileSync(__guardLogFile, `[${new Date().toISOString()}] ${lvl} ${msg}\n`) } catch {}
}
const __origLog = console.log
const __origError = console.error
const __origWarn = console.warn
const __origDebug = console.debug
const __origInfo = console.info
console.log = (...a: any[]) => __guardWrite("LOG", a)
console.error = (...a: any[]) => __guardWrite("ERROR", a)
console.warn = (...a: any[]) => __guardWrite("WARN", a)
console.debug = (...a: any[]) => __guardWrite("DEBUG", a)
console.info = (...a: any[]) => __guardWrite("INFO", a)
const __origStdoutWrite = process.stdout.write.bind(process.stdout) as any
const __origStderrWrite = process.stderr.write.bind(process.stderr) as any
const __isLogLine = (s: string) => /^\[\d{4}-\d{2}-\d{2}T/.test(s) || s.includes("] INFO") || s.includes("] WARN") || s.includes("] ERROR") || s.includes("] DEBUG")
const __isRendererFrame = (s: string) => s.includes("\x1b[?1049") || s.includes("\x1b[H") || (s.includes("\x1b[") && s.length > 100)
process.stdout.write = ((chunk: any, enc?: any, cb?: any) => {
  const str = typeof chunk === "string" ? chunk : chunk?.toString?.("utf8") ?? String(chunk)
  if (__isRendererFrame(str)) return __origStdoutWrite(chunk, enc, cb)
  if (__isLogLine(str)) { __guardWrite("STDOUT_LOG", [str.trim().slice(0, 2000)]); if (typeof cb === "function") cb(); return true }
  return __origStdoutWrite(chunk, enc, cb)
}) as any
process.stderr.write = ((chunk: any, enc?: any, cb?: any) => {
  const str = typeof chunk === "string" ? chunk : chunk?.toString?.("utf8") ?? String(chunk)
  if (__isRendererFrame(str)) return __origStderrWrite(chunk, enc, cb)
  if (__isLogLine(str) || str.includes("AI_APICallError") || str.includes("RegionError")) {
    __guardWrite("STDERR_LOG", [str.trim().slice(0, 2000)])
    if (typeof cb === "function") cb()
    return true
  }
  return __origStderrWrite(chunk, enc, cb)
}) as any
const __restoreScreen = () => { try { __origStdoutWrite("\x1b[?1049l"); __origStdoutWrite("\x1b[?25h"); __origStdoutWrite("\x1b[0m") } catch {} }
process.on("uncaughtException", (err) => { __guardWrite("FATAL_UNCAUGHT", [err instanceof Error ? err.stack || err.message : String(err)]); __restoreScreen(); try { __origStderrWrite(`[FATAL] ${err instanceof Error ? err.message : String(err)}\n`) } catch {} ; process.exit(1) })
process.on("unhandledRejection", (reason) => {
  const msg = String(reason instanceof Error ? reason.stack || reason.message : String(reason))
  if (msg.includes("AI_APICallError") || msg.includes("RegionError")) { __guardWrite("HANDLED_AI", [msg.slice(0,2000)]); return }
  __guardWrite("FATAL_UNHANDLED", [msg]); __restoreScreen(); try { __origStderrWrite(`[UNHANDLED] ${msg}\n`) } catch {}
})
process.on("SIGINT", () => { __guardWrite("SIGNAL", ["SIGINT"]); __restoreScreen(); process.exit(0) })
process.on("SIGTERM", () => { __guardWrite("SIGNAL", ["SIGTERM"]); __restoreScreen(); process.exit(0) })
{
  const idx = process.argv.indexOf("--log-level")
  if (idx !== -1 && process.argv[idx+1]) process.env.OPENCODE_LOG_LEVEL = process.argv[idx+1]
  if (process.argv.includes("--print-logs")) process.env.OPENCODE_PRINT_LOGS = "1"
}
// --- End Guard ---

const args = hideBin(process.argv)

function show(out: string) {
  const text = out.trimStart()
  if (!text.startsWith("opencode ")) {
    process.stderr.write(UI.logo() + EOL + EOL)
    process.stderr.write(text + EOL)
    return
  }
  process.stderr.write(out)
}

const cli = yargs(args)
  .parserConfiguration({ "populate--": true })
  .scriptName("opencode")
  .wrap(100)
  .help("help", "show help")
  .alias("help", "h")
  .version("version", "show version number", InstallationVersion)
  .alias("version", "v")
  .option("print-logs", {
    describe: "print logs to stderr",
    type: "boolean",
  })
  .option("log-level", {
    describe: "log level",
    type: "string",
    choices: ["DEBUG", "INFO", "WARN", "ERROR"],
  })
  .option("pure", {
    describe: "run without external plugins",
    type: "boolean",
  })
  .middleware(async (opts) => {
    if (opts.printLogs) process.env.OPENCODE_PRINT_LOGS = "1"
    if (opts.logLevel) process.env.OPENCODE_LOG_LEVEL = opts.logLevel
    if (opts.pure) {
      process.env.OPENCODE_PURE = "1"
    }

    process.env.AGENT = "1"
    process.env.OPENCODE = "1"
    process.env.OPENCODE_PID = String(process.pid)
  })
  .usage("")
  .completion("completion", "generate shell completion script")
  .command(AcpCommand)
  .command(McpCommand)
  .command(TuiThreadCommand)
  .command(AttachCommand)
  .command(RunCommand)
  .command(GenerateCommand)
  .command(DebugCommand)
  .command(ConsoleCommand)
  .command(ProvidersCommand)
  .command(AgentCommand)
  .command(UpgradeCommand)
  .command(UninstallCommand)
  .command(ServeCommand)
  .command(WebCommand)
  .command(ModelsCommand)
  .command(StatsCommand)
  .command(ExportCommand)
  .command(ImportCommand)
  .command(GithubCommand)
  .command(PrCommand)
  .command(SessionCommand)
  .command(PluginCommand)
  .command(DbCommand)
  .fail((msg, err) => {
    if (
      msg?.startsWith("Unknown argument") ||
      msg?.startsWith("Not enough non-option arguments") ||
      msg?.startsWith("Invalid values:")
    ) {
      if (err) throw err
      cli.showHelp(show)
    }
    if (err) throw err
    process.exit(1)
  })
  .strict()

try {
  if (args.includes("-h") || args.includes("--help")) {
    await cli.parse(args, (err: Error | undefined, _argv: unknown, out: string) => {
      if (err) throw err
      if (!out) return
      show(out)
    })
  } else {
    await cli.parse()
  }
} catch (e) {
  const formatted = FormatError(e)
  if (formatted) UI.error(formatted)
  if (formatted === undefined) {
    UI.error("Unexpected error" + EOL)
    process.stderr.write(errorMessage(e) + EOL)
  }
  process.exitCode = 1
} finally {
  // Some subprocesses don't react properly to SIGTERM and similar signals.
  // Most notably, some docker-container-based MCP servers don't handle such signals unless
  // run using `docker run --init`.
  // Explicitly exit to avoid any hanging subprocesses.
  process.exit()
}
