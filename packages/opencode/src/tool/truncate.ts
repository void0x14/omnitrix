import { LayerNode } from "@opencode-ai/core/effect/layer-node"
import { NodePath } from "@effect/platform-node"
import { Cause, Duration, Effect, Layer, Option, Schedule, Context } from "effect"
import path from "path"
import os from "node:os"
import { randomUUID } from "node:crypto"
import { mkdir } from "node:fs/promises"
import type { Agent } from "../agent/agent"
import { FSUtil } from "@opencode-ai/core/fs-util"
import { Config } from "@/config/config"
import { evaluate } from "@/permission/evaluate"
import { ToolID } from "./schema"
import { TRUNCATION_DIR } from "./truncation-dir"


const SPILL_DIR = path.join(os.tmpdir(), "opencode-spill")

export interface SpillOptions {
  maxLines?: number
  maxBytes?: number
  direction?: "head" | "tail"
  agent?: Agent.Info
}

export interface SpillResult {
  spilled: boolean
  /** Full text when within limits; otherwise a bounded preview envelope (head/tail + saved path + counts). */
  content: string
  outputPath?: string
  bytes: number
  lines: number
  retainedBytes: number
  retainedLines: number
  direction: "head" | "tail"
}

function hasTaskTool(agent?: Agent.Info) {
  if (!agent?.permission) return false
  return evaluate("task", "*", agent.permission).action !== "deny"
}

function sliceBytes(text: string, maxBytes: number, direction: "head" | "tail") {
  const bytes = Buffer.from(text, "utf-8")
  if (bytes.length <= maxBytes) return text
  const slice = direction === "head" ? bytes.subarray(0, maxBytes) : bytes.subarray(bytes.length - maxBytes)
  let start = 0
  let end = slice.length
  while (start < end && (slice[start]! & 0xc0) === 0x80) start++
  while (end > start && (slice[end - 1]! & 0xc0) === 0x80) end--
  return slice.subarray(start, end).toString("utf-8")
}

function boundedPreview(text: string, maxLines: number, maxBytes: number, direction: "head" | "tail") {
  const lines = text.split("\n")
  const selected: string[] = []
  let selectedBytes = 0
  const indexes = direction === "head" ? lines.keys() : Array.from(lines.keys()).reverse().values()

  for (const index of indexes) {
    if (selected.length >= maxLines) break
    const line = lines[index] ?? ""
    const separator = selected.length > 0 ? 1 : 0
    const lineBytes = Buffer.byteLength(line, "utf-8")
    const available = maxBytes - selectedBytes - separator
    if (available <= 0) break
    if (lineBytes > available) {
      const partial = sliceBytes(line, available, direction)
      if (direction === "head") selected.push(partial)
      else selected.unshift(partial)
      selectedBytes += Buffer.byteLength(partial, "utf-8") + separator
      break
    }
    if (direction === "head") selected.push(line)
    else selected.unshift(line)
    selectedBytes += lineBytes + separator
  }

  const content = selected.join("\n")
  return {
    content,
    bytes: Buffer.byteLength(content, "utf-8"),
    lines: selected.length,
  }
}

function truncationContent(input: {
  preview: string
  totalBytes: number
  totalLines: number
  keptBytes: number
  keptLines: number
  maxBytes: number
  maxLines: number
  direction: "head" | "tail"
  outputPath: string
  agent?: Agent.Info
}) {
  const byteLimited = input.totalBytes > input.maxBytes
  const removed = byteLimited
    ? Math.max(0, input.totalBytes - input.keptBytes)
    : Math.max(0, input.totalLines - input.keptLines)
  const unit = byteLimited ? "bytes" : "lines"
  const marker = `...${removed} ${unit} truncated...`
  const hint = hasTaskTool(input.agent)
    ? `The tool call succeeded but the output was truncated. Full output saved to: ${input.outputPath}\nUse the Task tool to have an explore agent process this file with Grep and Read (with offset/limit). Do NOT read the full file yourself - delegate to save context.`
    : `The tool call succeeded but the output was truncated. Full output saved to: ${input.outputPath}\nUse Grep to search the full content or Read with offset/limit to view specific sections.`
  return input.direction === "head"
    ? `${input.preview}\n\n${marker}\n\n${hint}`
    : `${marker}\n\n${hint}\n\n${input.preview}`
}

/**
 * Codex-style auto-spill. Within the byte/line budget the text is returned unchanged
 * (no data loss, normal usage unaffected). Above the budget the FULL text is written to
 * `os.tmpdir()/opencode-spill/<uuid>.txt` and only a bounded preview envelope enters context.
 */
export async function spillOrInline(text: string, opts: SpillOptions = {}): Promise<SpillResult> {
  const maxLines = opts.maxLines ?? MAX_LINES
  const maxBytes = opts.maxBytes ?? MAX_BYTES
  const direction = opts.direction ?? "head"
  const lines = text.split("\n")
  const bytes = Buffer.byteLength(text, "utf-8")

  if (lines.length <= maxLines && bytes <= maxBytes) {
    return {
      spilled: false,
      content: text,
      bytes,
      lines: lines.length,
      retainedBytes: bytes,
      retainedLines: lines.length,
      direction,
    }
  }

  const file = path.join(SPILL_DIR, `${randomUUID()}.txt`)

  await mkdir(SPILL_DIR, { recursive: true })
  await Bun.write(file, text)

  const preview = boundedPreview(text, maxLines, maxBytes, direction)
  const envelope = truncationContent({
    preview: preview.content,
    totalBytes: bytes,
    totalLines: lines.length,
    keptBytes: preview.bytes,
    keptLines: preview.lines,
    maxBytes,
    maxLines,
    direction,
    outputPath: file,
    agent: opts.agent,
  })

  return {
    spilled: true,
    content: envelope,
    outputPath: file,
    bytes,
    lines: lines.length,
    retainedBytes: preview.bytes,
    retainedLines: preview.lines,
    direction,
  }
}
const RETENTION = Duration.days(7)

export const MAX_LINES = 2000
export const MAX_BYTES = 50 * 1024
export const DIR = TRUNCATION_DIR
export const GLOB = path.join(TRUNCATION_DIR, "*")

export type Result =
  | { content: string; truncated: false; outputPath?: string }
  | {
      content: string
      truncated: true
      outputPath: string
      originalBytes: number
      originalLines: number
      retainedBytes: number
      retainedLines: number
      retainedDirection: "head" | "tail"
    }

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
          Effect.map((all) =>
            all.filter((name) => dir === TRUNCATION_DIR ? name.startsWith("tool_") : name.endsWith(".txt")),
          ),
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

    const output = Effect.fn("Truncate.output")(function* (text: string, options: Options = {}, agent?: Agent.Info) {
      const resolved = yield* limits()
      const maxLines = options.maxLines ?? resolved.maxLines
      const maxBytes = options.maxBytes ?? resolved.maxBytes
      const direction = options.direction ?? "head"
      const lines = text.split("\n")
      const bytes = Buffer.byteLength(text, "utf-8")
      if (lines.length <= maxLines && bytes <= maxBytes) return { content: text, truncated: false } as const

      const preview = boundedPreview(text, maxLines, maxBytes, direction)
      const outputPath = yield* write(text)
      return {
        content: truncationContent({
          preview: preview.content,
          totalBytes: bytes,
          totalLines: lines.length,
          keptBytes: preview.bytes,
          keptLines: preview.lines,
          maxBytes,
          maxLines,
          direction,
          outputPath,
          agent,
        }),
        truncated: true,
        outputPath,
        originalBytes: bytes,
        originalLines: lines.length,
        retainedBytes: preview.bytes,
        retainedLines: preview.lines,
        retainedDirection: direction,
      } as const
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
