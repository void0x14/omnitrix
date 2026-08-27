import { Server } from "@/server/server"
import { InstanceRuntime } from "@/project/instance-runtime"
import { Rpc } from "@/util/rpc"
import { upgrade } from "@/cli/upgrade"
import { Config } from "@/config/config"
import { GlobalBus } from "@/bus/global"
import { ServerAuth } from "@/server/auth"
import { writeHeapSnapshot } from "node:v8"
import { AppRuntime } from "@/effect/app-runtime"
import { Effect } from "effect"
import { disposeAllInstancesAndEmitGlobalDisposed } from "@/server/global-lifecycle"
import * as fs from "fs"
import * as path from "path"
// --- Worker Guard: prevent shared-fd TUI corruption (main thread is in alternate buffer) ---
const __wGuardFile = (() => { try { const h = process.env.HOME || process.env.USERPROFILE || "/tmp"; return path.join(h, ".local", "share", "opencode", "log", "worker-debug.log") } catch { return "/tmp/worker-debug.log" } })()
try { fs.mkdirSync(path.dirname(__wGuardFile), { recursive: true }) } catch {}
const __wWrite = (lvl: string, args: any[]) => {
  const msg = args.map(a => typeof a === "string" ? a : (() => { try { return JSON.stringify(a) } catch { return String(a) } })()).join(" ")
  try { fs.appendFileSync(__wGuardFile, `[${new Date().toISOString()}] ${lvl} ${msg.slice(0, 2000)}\n`) } catch {}
}
const __wOrigLog = console.log
const __wOrigErr = console.error
const __wOrigWarn = console.warn
const __wOrigDbg = console.debug
const __wOrigInfo = console.info
console.log = (...a: any[]) => __wWrite("LOG", a)
console.error = (...a: any[]) => __wWrite("ERROR", a)
console.warn = (...a: any[]) => __wWrite("WARN", a)
console.debug = (...a: any[]) => __wWrite("DEBUG", a)
console.info = (...a: any[]) => __wWrite("INFO", a)
;(console as any).table = (...a: any[]) => __wWrite("TABLE", a)
;(console as any).trace = (...a: any[]) => __wWrite("TRACE", a)
;(console as any).dir = (...a: any[]) => __wWrite("DIR", a)
const __wOrigStdout = process.stdout.write.bind(process.stdout) as any
const __wOrigStderr = process.stderr.write.bind(process.stderr) as any
const __wIsRenderer = (s: string) => s.includes("\x1b[?1049") || s.includes("\x1b[H") || (s.includes("\x1b[") && s.length > 100)
process.stdout.write = ((c: any, e?: any, cb?: any) => {
  const s = typeof c === "string" ? c : c?.toString?.("utf8") ?? String(c)
  if (__wIsRenderer(s)) return __wOrigStdout(c, e, cb)
  if (s.trim().length > 0) __wWrite("STDOUT", [s.trim().slice(0, 2000)])
  if (typeof cb === "function") cb()
  return true
}) as any
process.stderr.write = ((c: any, e?: any, cb?: any) => {
  const s = typeof c === "string" ? c : c?.toString?.("utf8") ?? String(c)
  if (__wIsRenderer(s)) return __wOrigStderr(c, e, cb)
  if (s.trim().length > 0) __wWrite("STDERR", [s.trim().slice(0, 2000)])
  if (typeof cb === "function") cb()
  return true
}) as any
// --- End Worker Guard ---

const onUnhandledRejection = (_error: unknown) => {}

const onUncaughtException = (_error: Error) => {}

process.on("unhandledRejection", onUnhandledRejection)
process.on("uncaughtException", onUncaughtException)

// Subscribe to global events and forward them via RPC
GlobalBus.on("event", (event) => {
  Rpc.emit("global.event", event)
})

let server: Awaited<ReturnType<typeof Server.listen>> | undefined

export const rpc = {
  async fetch(input: { url: string; method: string; headers: Record<string, string>; body?: string }) {
    const headers = { ...input.headers }
    const auth = ServerAuth.header()
    if (auth && !headers["authorization"] && !headers["Authorization"]) {
      headers["Authorization"] = auth
    }
    const request = new Request(input.url, {
      method: input.method,
      headers,
      body: input.body,
    })
    const response = await Server.Default().app.fetch(request)
    const body = await response.text()
    return {
      status: response.status,
      headers: Object.fromEntries(response.headers.entries()),
      body,
    }
  },
  snapshot() {
    const result = writeHeapSnapshot("server.heapsnapshot")
    return result
  },
  async server(input: { port: number; hostname: string; mdns?: boolean; cors?: string[] }) {
    if (server) await server.stop(true)
    server = await Server.listen(input)
    return { url: server.url.toString() }
  },
  async checkUpgrade(input: { directory: string }) {
    await InstanceRuntime.load({ directory: input.directory })
    await upgrade().catch(() => {})
  },
  async reload() {
    await AppRuntime.runPromise(
      Effect.gen(function* () {
        const cfg = yield* Config.Service
        yield* cfg.invalidate()
        yield* disposeAllInstancesAndEmitGlobalDisposed({ swallowErrors: true })
      }),
    )
  },
  async shutdown() {
    await InstanceRuntime.disposeAllInstances()
    if (server) await server.stop(true)
    process.off("unhandledRejection", onUnhandledRejection)
    process.off("uncaughtException", onUncaughtException)
  },
}

Rpc.listen(rpc)
