import { createHash } from "node:crypto"
import { execFileSync } from "node:child_process"
import { mkdir, readFile, writeFile } from "node:fs/promises"
import path from "node:path"
import { Effect } from "effect"
import { withCliFixture } from "../test/lib/cli-process"
import { tmpdir } from "../test/fixture/fixture"

const echoRoot = "/home/void0x14/Documents/echo"
const harnessRoot = path.resolve(import.meta.dir, "../../..")
const outputDirectory = path.join(echoRoot, "eval/real-harness-cli-mock-20260831")
const task = JSON.stringify({
  schema: "echo.coding-task.v1",
  task_id: "echo-cli-evidence-20260831",
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

async function sha256(file: string) {
  return createHash("sha256").update(await readFile(file)).digest("hex")
}

const result = await Effect.runPromise(
  Effect.scoped(
    withCliFixture(({ llm, opencode }) =>
      Effect.gen(function* () {
        const fixture = yield* Effect.promise(() => tmpdir())
        yield* Effect.addFinalizer(() => Effect.promise(() => fixture[Symbol.asyncDispose]()))
        yield* Effect.promise(() => mkdir(path.join(fixture.path, "src"), { recursive: true }))
        yield* Effect.promise(() => mkdir(path.join(fixture.path, "test"), { recursive: true }))
        yield* Effect.promise(() => writeFile(path.join(fixture.path, "task.json"), task + "\n"))
        yield* Effect.promise(() => writeFile(path.join(fixture.path, "src/slugify.ts"), source))
        yield* Effect.promise(() => writeFile(path.join(fixture.path, "test/slugify.test.ts"), testSource))
        const beforeSha256 = yield* Effect.promise(() => sha256(path.join(fixture.path, "src/slugify.ts")))

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

        const run = yield* opencode.run("task.json içindeki coding taskı uygula ve gerçek testi geçir.", {
          format: "json",
          extraArgs: ["--auto", "--dir", fixture.path],
          timeoutMs: 30_000,
        })
        const afterSha256 = yield* Effect.promise(() => sha256(path.join(fixture.path, "src/slugify.ts")))
        const events = opencode.parseJsonEvents(run.stdout)
        const toolParts = events
          .filter((event) => event.type === "tool_use")
          .map((event) => event.part as { tool?: string; state?: { status?: string } })
        return {
          fixturePath: fixture.path,
          beforeSha256,
          afterSha256,
          targetChanged: beforeSha256 !== afterSha256,
          acceptanceExit: run.exitCode,
          toolSequence: toolParts.map((part) => ({ tool: part.tool, status: part.state?.status })),
          cliStdout: run.stdout,
          cliStderr: run.stderr,
          durationMs: run.durationMs,
        }
      }),
    ),
  ),
)

const evidencePath = path.join(outputDirectory, "evidence.json")
const eventsPath = path.join(outputDirectory, "events.jsonl")
const harnessRevision = execFileSync("git", ["rev-parse", "HEAD"], { cwd: harnessRoot, encoding: "utf8" }).trim()
const passed = result.acceptanceExit === 0 && result.targetChanged && result.toolSequence.map((item) => item.tool).join(",") === "read,read,read,edit,bash"
const evidence = {
  schema: "echo.real-harness-cli-evidence.v1",
  status: passed ? "pass" : "fail",
  provider: "deterministic-TestLLMServer",
  not_q4_model_result: true,
  harness_root: harnessRoot,
  harness_revision: harnessRevision,
  fixture_root: result.fixturePath,
  task_id: "echo-cli-evidence-20260831",
  target_file: "src/slugify.ts",
  target_sha256_before: result.beforeSha256,
  target_sha256_after: result.afterSha256,
  target_changed: result.targetChanged,
  acceptance_command: ["bun", "test", "test"],
  acceptance_exit: result.acceptanceExit,
  tool_sequence: result.toolSequence,
  terminal_state: passed ? "verified_success" : "failed_or_unverified",
  verified: passed,
  side_effect_verified: passed,
  duration_ms: result.durationMs,
  raw_cli_stdout: { path: eventsPath, bytes: Buffer.byteLength(result.cliStdout), sha256: createHash("sha256").update(result.cliStdout).digest("hex") },
  stderr_bytes: Buffer.byteLength(result.cliStderr),
  failures: passed ? [] : ["deterministic CLI coding acceptance did not satisfy target-change/tool-sequence/exit contract"],
}
await mkdir(outputDirectory, { recursive: true })
await writeFile(eventsPath, result.cliStdout, "utf8")
await writeFile(evidencePath, JSON.stringify(evidence, null, 2) + "\n", "utf8")
console.log(JSON.stringify({ status: evidence.status, verified: evidence.verified, targetChanged: evidence.target_changed, acceptanceExit: evidence.acceptance_exit, evidencePath, eventsPath }))
if (!passed) process.exitCode = 1
