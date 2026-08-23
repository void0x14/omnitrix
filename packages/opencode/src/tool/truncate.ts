import { LayerNode } from "@opencode-ai/core/effect/layer-node"
import { NodePath } from "@effect/platform-node"
import { Cause, Duration, Effect, Layer, Option, Schedule, Context } from "effect"
import path from "path"
import os from "node:os"
import { randomUUID } from "node:crypto"
import type { Agent } from "../agent/agent"
import { FSUtil } from "@opencode-ai/core/fs-util"
import { Config } from "@/config/config"
import { ToolID } from "./schema"
import { TRUNCATION_DIR } from "./truncation-dir"


const SPILL_DIR = path.join(os.tmpdir(), "opencode-spill")
const PREVIEW_LINES = 20

export interface SpillOptions {
  maxLines?: number
  maxBytes?: number
  direction?: "head" | "tail"
}

export interface SpillResult {
  spilled: boolean
  /** Full text when within limits; otherwise a bounded preview envelope (head/tail + saved path + counts). */
  content: string
  outputPath?: string
  bytes: number
  lines: number
}

/**
 * Codex-style auto-spill. Within the byte/line budget the text is returned unchanged
 * (no data loss, normal usage unaffected). Above the budget the FULL text is written to
 * `os.tmpdir()/opencode-spill/<uuid>.txt` and only a bounded preview envelope enters context.
 */
export async function spillOrInline(text: string, opts: SpillOptions = {}): Promise<SpillResult> {
  const maxLines = opts.maxLines ?? MAX_LINES
  const maxBytes = opts.maxBytes ?? MAX_BYTES
  const lines = text.split("\n")
  const bytes = Buffer.byteLength(text, "utf-8")

  if (lines.length <= maxLines && bytes <= maxBytes) {
    return { spilled: false, content: text, bytes, lines: lines.length }
  }

  const head = lines.slice(0, PREVIEW_LINES)
  const tail = lines.length > PREVIEW_LINES * 2 ? lines.slice(-PREVIEW_LINES) : []
  const file = path.join(SPILL_DIR, `${randomUUID()}.txt`)

  await Bun.write(file, text)

  const note = `Tam cikti (${bytes} bayt, ${lines.length} satir) kayitli: ${file}`
  const envelope = [
    ...head,
    ...(tail.length ? ["", "... (ortadan kesildi) ...", "", ...tail] : []),
    "",
    note,
  ].join("\n")

  return { spilled: true, content: envelope, outputPath: file, bytes, lines: lines.length }
}
const RETENTION = Duration.days(7)

export const MAX_LINES = 2000
export const MAX_BYTES = 50 * 1024
export const DIR = TRUNCATION_DIR
export const GLOB = path.join(TRUNCATION_DIR, "*")

export type Result =
  | { content: string; truncated: false; outputPath?: string }
  | { content: string; truncated: true; outputPath: string }

export interface Options {
  maxLines?: number
  maxBytes?: number
  direction?: "head" | "tail"
}


export interface Interface {
  readonly cleanup: () => Effect.Effect<void>
  readonly write: (text: string) => Effect.Effect<string>
  /**
   * Returns output unchanged when it fits within the limits, otherwise spills the full
   * text to a temp file and returns a bounded preview envelope plus the saved path.
   */
  readonly output: (text: string, options?: Options, agent?: Agent.Info) => Effect.Effect<Result>
  /**
   * Resolved truncation limits: values from `tool_output` in opencode config, or MAX_LINES / MAX_BYTES if unset.
   */
  readonly limits: () => Effect.Effect<{ maxLines: number; maxBytes: number }>
}

export class Service extends Context.Service<Service, Interface>()("@opencode/Truncate") {}

const layer = Layer.effect(
  Service,
  Effect.gen(function* () {
    const fs = yield* FSUtil.Service

    const cleanup = Effect.fn("Truncate.cleanup")(function* () {
      const cutoff = Date.now() - Duration.toMillis(RETENTION)
      for (const dir of [TRUNCATION_DIR, SPILL_DIR]) {
        const entries = yield* fs.readDirectory(dir).pipe(
          Effect.map((all) => all.filter((name) => name.endsWith(".txt"))),
          Effect.catch(() => Effect.succeed([])),
        )
        for (const entry of entries) {
          const file = path.join(dir, entry)
          const info = yield* fs.stat(file).pipe(Effect.catch(() => Effect.succeed(undefined)))
          const mtime = info && Option.getOrUndefined(info.mtime)
          if (!mtime || mtime.getTime() >= cutoff) continue
          yield* fs.remove(file).pipe(Effect.catch(() => Effect.void))
        }
      }
    })

    const write = Effect.fn("Truncate.write")(function* (text: string) {
      const file = path.join(TRUNCATION_DIR, ToolID.ascending())
      yield* fs.ensureDir(TRUNCATION_DIR).pipe(Effect.orDie)
      yield* fs.writeFileString(file, text).pipe(Effect.orDie)
      return file
    })

    const limits = Effect.fn("Truncate.limits")(function* () {
      const configSvc = yield* Effect.serviceOption(Config.Service)
      if (Option.isNone(configSvc)) return { maxLines: MAX_LINES, maxBytes: MAX_BYTES }
      const cfg = yield* configSvc.value.get().pipe(Effect.catch(() => Effect.succeed(undefined)))
      return {
        maxLines: cfg?.tool_output?.max_lines ?? MAX_LINES,
        maxBytes: cfg?.tool_output?.max_bytes ?? MAX_BYTES,
      }
    })

    const output = Effect.fn("Truncate.output")(function* (text: string, options: Options = {}, _agent?: Agent.Info) {
      const resolved = yield* limits()
      const maxLines = options.maxLines ?? resolved.maxLines
      const maxBytes = options.maxBytes ?? resolved.maxBytes
      const spill = yield* Effect.promise(() =>
        spillOrInline(text, { maxLines, maxBytes, direction: options.direction ?? "head" }),
      )
      if (!spill.spilled) return { content: text, truncated: false } as const
      return { content: spill.content, truncated: true, outputPath: spill.outputPath! } as const
    })

    yield* cleanup().pipe(
      Effect.catchCause((cause) => Effect.logError("truncation cleanup failed", { cause: Cause.pretty(cause) })),
      Effect.repeat(Schedule.spaced(Duration.hours(1))),
      Effect.delay(Duration.minutes(1)),
      Effect.forkScoped,
    )

    return Service.of({ cleanup, write, output, limits })
  }),
)

export const node = LayerNode.make({ service: Service, layer: layer, deps: [FSUtil.node] })

export * as Truncate from "./truncate"
