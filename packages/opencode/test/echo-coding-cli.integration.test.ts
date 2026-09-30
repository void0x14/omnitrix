import { expect } from "bun:test"
import { mkdir, readFile, writeFile } from "node:fs/promises"
import path from "node:path"
import { Effect } from "effect"
import { cliIt } from "./lib/cli-process"
import { tmpdir } from "./fixture/fixture"

const task = JSON.stringify({
  schema: "echo.coding-task.v1",
  task_id: "echo-cli-integration-20260831",
  target_file: "src/slugify.ts",
  acceptance_command: ["bun", "test", "test"],
})
const source = 'export function slugify(value: string): string {\n  return value.trim()\n}\n'
const testSource = [
  'import { expect, test } from "bun:test"',
  'import { slugify } from "../src/slugify"',
  "",
  'test("slugify normalizes punctuation", () => {',
  '  expect(slugify("  Echo / Agent  ")).toBe("echo-agent")',
  "})",
  "",
].join("\n")
const replacement = 'export function slugify(value: string): string {\n  return value.trim().toLocaleLowerCase("tr-TR").replace(/[^\\p{L}\\p{N}]+/gu, "-").replace(/^-|-$/g, "")\n}\n'

cliIt.live("runs a real CLI coding task through the harness tool registry", ({ llm, opencode }) =>
  Effect.gen(function* () {
    const fixture = yield* Effect.promise(() => tmpdir())
    yield* Effect.addFinalizer(() => Effect.promise(() => fixture[Symbol.asyncDispose]()))
    yield* Effect.promise(() => mkdir(path.join(fixture.path, "src"), { recursive: true }))
    yield* Effect.promise(() => mkdir(path.join(fixture.path, "test"), { recursive: true }))
    yield* Effect.promise(() => writeFile(path.join(fixture.path, "task.json"), task + "\n"))
    yield* Effect.promise(() => writeFile(path.join(fixture.path, "src/slugify.ts"), source))
    yield* Effect.promise(() => writeFile(path.join(fixture.path, "test/slugify.test.ts"), testSource))

    yield* llm.tool("read", { filePath: path.join(fixture.path, "task.json") })
    yield* llm.tool("read", { filePath: path.join(fixture.path, "src/slugify.ts") })
    yield* llm.tool("read", { filePath: path.join(fixture.path, "test/slugify.test.ts") })
    yield* llm.tool("edit", {
      filePath: path.join(fixture.path, "src/slugify.ts"),
      oldString: source.trimEnd(),
      newString: replacement.trimEnd(),
    })
    yield* llm.tool("bash", { command: "bun test test", workdir: fixture.path })
    yield* llm.text("Görev tamamlandı; test geçti ve değişiklik doğrulandı.")

    const result = yield* opencode.run("task.json içindeki coding taskı uygula ve gerçek testi geçir.", {
      format: "json",
      extraArgs: ["--auto", "--dir", fixture.path],
      timeoutMs: 30_000,
    })
    opencode.expectExit(result, 0, "echo coding CLI integration")

    const finalSource = yield* Effect.promise(() => readFile(path.join(fixture.path, "src/slugify.ts"), "utf8"))
    expect(finalSource).toBe(replacement)
    const events = opencode.parseJsonEvents(result.stdout)
    const toolParts = events
      .filter((event) => event.type === "tool_use")
      .map((event) => event.part as { tool?: string; state?: { status?: string } })
    expect(toolParts.map((part) => part.tool)).toEqual(["read", "read", "read", "edit", "bash"])
    expect(toolParts.some((part) => part.tool === "bash" && part.state?.status === "completed")).toBe(true)

    const modelRequests = yield* llm.inputs
    const advertisedToolNames = modelRequests.flatMap((body) => {
      if (!Array.isArray(body.tools)) return []
      return body.tools.flatMap((entry) => {
        if (!entry || typeof entry !== "object") return []
        const fn = "function" in entry && entry.function && typeof entry.function === "object" ? entry.function : undefined
        return fn && "name" in fn && typeof fn.name === "string" ? [fn.name] : []
      })
    })
    expect(advertisedToolNames).not.toContain("list_mcp_resources")
    expect(advertisedToolNames).not.toContain("list_mcp_resource_templates")
    expect(advertisedToolNames).not.toContain("read_mcp_resource")
  }),
  { timeout: 30_000 },
)
