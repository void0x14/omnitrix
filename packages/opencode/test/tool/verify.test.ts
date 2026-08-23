import { afterEach, describe, expect } from "bun:test"
import { LayerNode } from "@opencode-ai/core/effect/layer-node"
import { Effect } from "effect"
import path from "path"
import { AppProcess } from "@opencode-ai/core/process"
import { VerifyTool } from "../../src/tool/verify"
import { FSUtil } from "@opencode-ai/core/fs-util"
import { Git } from "../../src/git"
import { Truncate } from "@/tool/truncate"
import { Tool } from "@/tool/tool"
import { Agent } from "../../src/agent/agent"
import { SessionID, MessageID } from "../../src/session/schema"
import { CrossSpawnSpawner } from "@opencode-ai/core/cross-spawn-spawner"
import { disposeAllInstances, TestInstance } from "../fixture/fixture"
import { testEffect } from "../lib/effect"

const ctx = {
  sessionID: SessionID.make("ses_test-verify-session"),
  messageID: MessageID.make("msg_test"),
  callID: "",
  agent: "build",
  abort: AbortSignal.any([]),
  messages: [],
  metadata: () => Effect.void,
  ask: () => Effect.void,
}

afterEach(async () => {
  await disposeAllInstances()
})

const it = testEffect(
  LayerNode.compile(
    LayerNode.group([
      Git.node,
      FSUtil.node,
      Truncate.node,
      Agent.node,
      CrossSpawnSpawner.node,
      AppProcess.node,
    ]),
  ),
)

const init = Effect.fn("VerifyToolTest.init")(function* () {
  const info = yield* VerifyTool
  return yield* info.init()
})

const run = Effect.fn("VerifyToolTest.run")(function* (
  args: Tool.InferParameters<typeof VerifyTool>,
  next: Tool.Context = ctx,
) {
  const tool = yield* init()
  return yield* tool.execute(args, next)
})

const yaz = (directory: string, dosya: string, icerik: string) =>
  Effect.promise(async () => {
    const hedef = path.join(directory, dosya)
    await Bun.write(hedef, icerik)
  })

describe("tool.verify", () => {
  it.instance(
    "python sozdizimi hatasinda OLMADI der",
    () =>
      Effect.gen(function* () {
        const test = yield* TestInstance
        yield* yaz(test.directory, "bad.py", "def bozuk(:\n    return 1\n")
        const result = yield* run({ eylem: "kod derlensin" })
        expect(result.output).toBe("OLMADI: sozdizimi hatasi: bad.py:1 — SyntaxError: invalid syntax")
      }),
    { git: true },
  )

  it.instance(
    "gecerli python ve kosul varsa OLDU der",
    () =>
      Effect.gen(function* () {
        const test = yield* TestInstance
        yield* yaz(test.directory, "ok.py", "from asyncio import TaskGroup\n\ndef f():\n    return 1\n")
        const result = yield* run({ eylem: "kod derlensin", kosul: "TaskGroup" })
        expect(result.output).toBe("OLDU")
      }),
    { git: true },
  )

  it.instance(
    "kosul degisen dosyalarda yoksa OLMADI der",
    () =>
      Effect.gen(function* () {
        const test = yield* TestInstance
        yield* yaz(test.directory, "ok.py", "def f():\n    return 1\n")
        const result = yield* run({ eylem: "kod derlensin", kosul: "TaskGroup" })
        expect(result.output).toBe("OLMADI: istenen TaskGroup bulunamadi (degisen dosyalarda yok)")
      }),
    { git: true },
  )

  it.instance(
    "bahsedilen dosya degismemisse OLMADI der",
    () =>
      Effect.gen(function* () {
        const result = yield* run({ eylem: "src/main.zig degismeli" })
        expect(result.output).toBe("OLMADI: beklenen dosya degismemis: src/main.zig")
      }),
    { git: true },
  )

  it.instance(
    "bahsedilen dosya degistiyse dosya sarti gecer",
    () =>
      Effect.gen(function* () {
        const test = yield* TestInstance
        yield* yaz(test.directory, "not.md", "icerik\n")
        const result = yield* run({ eylem: "not.md degismeli" })
        expect(result.output).toBe("OLDU")
      }),
    { git: true },
  )

  it.instance(
    "komut basarisizsa OLMADI der",
    () =>
      Effect.gen(function* () {
        const result = yield* run({ eylem: "calistir", komut: "exit 3" })
        expect(result.output).toStartWith("OLMADI: komut basarisiz (exit 3)")
      }),
    { git: false },
  )

  it.instance(
    "komut basariliysa OLDU der",
    () =>
      Effect.gen(function* () {
        const result = yield* run({ eylem: "calistir", komut: "echo merhaba" })
        expect(result.output).toBe("OLDU")
      }),
    { git: false },
  )

  it.instance(
    "test istegi varken test dosyasi yoksa OLMADI der",
    () =>
      Effect.gen(function* () {
        const test = yield* TestInstance
        yield* yaz(test.directory, "ok.py", "def f():\n    return 1\n")
        const result = yield* run({ eylem: "testler gecsin" })
        expect(result.output).toBe("OLMADI: test dosyasi bulunamadi")
      }),
    { git: true },
  )

  it.instance(
    "zig derleme basariliysa OLDU der",
    () =>
      Effect.gen(function* () {
        if (!Bun.which("zig")) return
        const test = yield* TestInstance
        yield* yaz(
          test.directory,
          "build.zig",
          'const std = @import("std");\n\npub fn build(b: *std.Build) void {\n    const mod = b.createModule(.{ .root_source_file = b.path("src/main.zig"), .target = b.resolveTargetQuery(.{}) });\n    const exe = b.addExecutable(.{ .name = "main", .root_module = mod });\n    b.installArtifact(exe);\n}\n',
        )
        yield* yaz(test.directory, "src/main.zig", "pub fn main() void {}\n")
        const result = yield* run({ eylem: "derle" })
        expect(result.output).toBe("OLDU")
      }),
    { git: true },
    30000,
  )

  it.instance(
    "zig sozdizimi hatasinda OLMADI der",
    () =>
      Effect.gen(function* () {
        if (!Bun.which("zig")) return
        const test = yield* TestInstance
        yield* yaz(test.directory, "build.zig.zon", "")
        yield* yaz(test.directory, "src/main.zig", "pub fn main() void {\n")
        const result = yield* run({ eylem: "derle" })
        expect(result.output).toStartWith("OLMADI:")
      }),
    { git: true },
    30000,
  )

  it.instance(
    "bos dizinde katman calismazsa OLMADI der",
    () =>
      Effect.gen(function* () {
        const result = yield* run({ eylem: "kod derlensin" })
        expect(result.output).toBe("OLMADI: dogrulanacak bir is bulunamadi (katman calismadi)")
      }),
    { git: false },
  )
})
