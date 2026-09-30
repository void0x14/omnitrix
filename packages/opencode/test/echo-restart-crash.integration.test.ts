import { expect } from "bun:test"
import { Database } from "bun:sqlite"
import { createHash } from "node:crypto"
import { Effect } from "effect"
import { mkdirSync, readFileSync, writeFileSync } from "node:fs"
import path from "node:path"
import { cliIt } from "./lib/cli-process"
import { reply } from "./lib/llm-server"
import { tmpdir } from "./fixture/fixture"

function serializedBody(body: Record<string, unknown>) {
  return JSON.stringify(body)
}

function sessionRows(dbPath: string) {
  const db = new Database(dbPath, { readonly: true })
  try {
    return db
      .query("SELECT id, parent_id, metadata FROM session WHERE parent_id IS NOT NULL ORDER BY time_created DESC")
      .all() as Array<{ id: string; parent_id: string; metadata: string | null }>
  } finally {
    db.close()
  }
}

function sha256(file: string) {
  return createHash("sha256").update(readFileSync(file)).digest("hex")
}

cliIt.live("reconciles an orphaned child after the owning CLI is force-killed", ({ llm, opencode, home }) =>
  Effect.gen(function* () {
    const fixture = yield* Effect.promise(() => tmpdir())
    yield* Effect.addFinalizer(() => Effect.promise(() => fixture[Symbol.asyncDispose]()))
    const dbPath = path.join(home, "echo-restart.db")

    const launchPrompt = "Launch a background child and keep the parent running."
    const childPrompt = "hold child open"
    const launchMatch = (hit: { body: Record<string, unknown> }) => serializedBody(hit.body).includes(launchPrompt)
    const childMatch = (hit: { body: Record<string, unknown> }) => serializedBody(hit.body).includes(childPrompt)

    yield* llm.toolMatch(launchMatch, "task", {
      description: "hold child",
      prompt: childPrompt,
      subagent_type: "general",
      background: true,
    })
    yield* llm.pushMatch(childMatch, reply().hang().item())
    yield* llm.pushMatch(launchMatch, reply().hang().item())

    const first = yield* opencode.startRun(launchPrompt, {
      format: "json",
      extraArgs: ["--auto", "--dir", fixture.path],
      env: { OPENCODE_DB: dbPath, OPENCODE_EXPERIMENTAL_BACKGROUND_SUBAGENTS: "true" },
      timeoutMs: 30_000,
    })
    yield* llm.wait(3)
    first.kill("SIGKILL")
    const firstResult = yield* first.result
    expect(firstResult.exitCode).not.toBe(0)

    const rows = sessionRows(dbPath)
    const child = rows.find((row) => {
      if (!row.metadata) return false
      const metadata = JSON.parse(row.metadata) as Record<string, unknown>
      const execution = metadata["opencode.execution"]
      return execution && typeof execution === "object"
    })
    expect(child).toBeDefined()
    const childMetadata = JSON.parse(child!.metadata!) as Record<string, any>
    expect(childMetadata["opencode.execution"].state).toBe("running")
    expect(childMetadata["opencode.execution"].ownerID).toBeString()

    yield* llm.reset
    const statusPrompt = `Inspect orphaned child ${child!.id}`
    yield* llm.toolMatch(
      (hit) => serializedBody(hit.body).includes(statusPrompt),
      "task",
      { action: "status", task_id: child!.id },
    )
    const second = yield* opencode.run(statusPrompt, {
      format: "json",
      extraArgs: ["--auto", "--dir", fixture.path],
      env: { OPENCODE_DB: dbPath },
      timeoutMs: 30_000,
    })
    opencode.expectExit(second, 0, "restart crash reconciliation")
    const events = opencode.parseJsonEvents(second.stdout)
    const taskEvent = events.find(
      (event) =>
        event.type === "tool_use" &&
        typeof event.part === "object" &&
        event.part !== null &&
        (event.part as { tool?: string }).tool === "task",
    )
    const output = (taskEvent?.part as { state?: { output?: string } } | undefined)?.state?.output ?? ""
    expect(output).toContain("[orphaned]")

    const evidencePath = process.env.ECHO_RESTART_CRASH_EVIDENCE
    if (evidencePath) {
      mkdirSync(path.dirname(evidencePath), { recursive: true })
      writeFileSync(
        evidencePath,
        JSON.stringify(
          {
            schema: "echo.real-harness-restart-crash-evidence.v1",
            status: "pass",
            provider: "deterministic-TestLLMServer",
            not_q4_model_result: true,
            harness_root: path.resolve(import.meta.dir, "../../.."),
            database_path: dbPath,
            database_sha256: sha256(dbPath),
            first_process: {
              exit_code: firstResult.exitCode,
              forced_signal: "SIGKILL",
              duration_ms: firstResult.durationMs,
            },
            durable_child: {
              session_id: child!.id,
              state_before_restart: childMetadata["opencode.execution"].state,
              owner_id_present: typeof childMetadata["opencode.execution"].ownerID === "string",
            },
            second_process: {
              exit_code: second.exitCode,
              duration_ms: second.durationMs,
              orphaned_status_observed: output.includes("[orphaned]"),
            },
            verified: true,
            side_effect_verified: true,
          },
          null,
          2,
        ) + "\n",
      )
    }
  }),
)
