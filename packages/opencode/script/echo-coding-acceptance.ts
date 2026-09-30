import { createHash } from "node:crypto"
import { execFileSync } from "node:child_process"
import { mkdirSync, readFileSync, statSync, writeFileSync } from "node:fs"
import path from "node:path"

type TaskSpec = {
  task_id: string
  target_file: string
  acceptance_command: string[]
}

type Baseline = {
  task_id: string
  target_file: string
  target_sha256_before: string
  task_sha256_before: string
  acceptance_command: string[]
}

export type CodingAcceptanceInput = {
  fixtureRoot: string
  evidenceDir: string
  modelPath: string
  modelSha256: string
  harnessRoot?: string
  expectedHarnessRevision?: string
  executionID: string
  attempt: number
  reasoning: boolean
}

export type CodingAcceptanceResult = {
  status: "pass" | "fail"
  failures: string[]
  evidencePath: string
  evidence: Record<string, unknown>
}

export function sha256(file: string) {
  return createHash("sha256").update(readFileSync(file)).digest("hex")
}

function text(value: Uint8Array | string | null | undefined) {
  if (value === null || value === undefined) return ""
  return typeof value === "string" ? value : new TextDecoder().decode(value)
}

function revision(root: string) {
  return execFileSync("git", ["rev-parse", "HEAD"], { cwd: root, encoding: "utf8" }).trim()
}

function validate(input: CodingAcceptanceInput, task: TaskSpec, baseline: Baseline, harnessRoot: string) {
  const failures: string[] = []
  if (!input.executionID) failures.push("executionID is required")
  if (!Number.isInteger(input.attempt) || input.attempt < 1) failures.push("attempt must be a positive integer")
  if (!input.reasoning) failures.push("reasoning must be enabled")
  if (!/^[a-f0-9]{64}$/i.test(input.modelSha256)) failures.push("modelSha256 must be a SHA-256")
  if (task.task_id !== baseline.task_id) failures.push("task and baseline task_id differ")
  if (task.target_file !== baseline.target_file) failures.push("task and baseline target_file differ")
  if (!Array.isArray(task.acceptance_command) || task.acceptance_command.length === 0)
    failures.push("acceptance command is missing")
  if (!path.isAbsolute(input.modelPath)) failures.push("modelPath must be absolute")
  if (!path.isAbsolute(harnessRoot)) failures.push("harnessRoot must be absolute")
  try {
    if (!statSync(input.modelPath).isFile()) failures.push("modelPath is not a regular file")
    else if (sha256(input.modelPath) !== input.modelSha256) failures.push("model SHA-256 mismatch")
  } catch (error) {
    failures.push(`model inspection failed: ${error instanceof Error ? error.message : String(error)}`)
  }
  try {
    const actualRevision = revision(harnessRoot)
    if (input.expectedHarnessRevision && !actualRevision.startsWith(input.expectedHarnessRevision))
      failures.push("harness revision mismatch")
  } catch (error) {
    failures.push(`harness revision lookup failed: ${error instanceof Error ? error.message : String(error)}`)
  }
  return failures
}

export function runAcceptance(input: CodingAcceptanceInput): CodingAcceptanceResult {
  const fixtureRoot = path.resolve(input.fixtureRoot)
  const evidenceDir = path.resolve(input.evidenceDir)
  const harnessRoot = path.resolve(input.harnessRoot ?? path.resolve(import.meta.dir, "../../.."))
  const evidencePath = path.join(evidenceDir, "evidence.json")
  const taskPath = path.join(fixtureRoot, "task.json")
  const baselinePath = path.join(fixtureRoot, "baseline.json")
  const task = JSON.parse(readFileSync(taskPath, "utf8")) as TaskSpec
  const baseline = JSON.parse(readFileSync(baselinePath, "utf8")) as Baseline
  const targetPath = path.resolve(fixtureRoot, task.target_file)
  const targetPrefix = fixtureRoot.endsWith(path.sep) ? fixtureRoot : fixtureRoot + path.sep
  const failures = validate(input, task, baseline, harnessRoot)
  try {
    if (!/^[a-f0-9]{64}$/i.test(baseline.task_sha256_before)) failures.push("task_sha256_before must be a SHA-256")
    else if (sha256(taskPath) !== baseline.task_sha256_before) failures.push("task specification changed since baseline")
  } catch (error) {
    failures.push(`task inspection failed: ${error instanceof Error ? error.message : String(error)}`)
  }
  if (JSON.stringify(task.acceptance_command) !== JSON.stringify(baseline.acceptance_command))
    failures.push("task acceptance command differs from baseline")
  if (!targetPath.startsWith(targetPrefix)) failures.push("target file escapes fixture root")

  let targetAfterHash: string | undefined
  try {
    if (!statSync(targetPath).isFile()) failures.push("target file is missing")
    else targetAfterHash = sha256(targetPath)
  } catch (error) {
    failures.push(`target inspection failed: ${error instanceof Error ? error.message : String(error)}`)
  }

  let acceptanceExit: number | null = null
  let acceptanceOutput = ""
  if (failures.length === 0) {
    const command = baseline.acceptance_command
    const run = Bun.spawnSync({ cmd: command, cwd: fixtureRoot, stdout: "pipe", stderr: "pipe" })
    acceptanceExit = run.exitCode
    acceptanceOutput = text(run.stdout) + text(run.stderr)
  }

  const targetChanged = targetAfterHash !== undefined && targetAfterHash !== baseline.target_sha256_before
  const passed = failures.length === 0 && targetChanged && acceptanceExit === 0
  const evidence: Record<string, unknown> = {
    schema: "echo.coding-acceptance-evidence.v1",
    status: passed ? "pass" : "fail",
    task_id: task.task_id,
    fixture_root: fixtureRoot,
    target_file: task.target_file,
    target_sha256_before: baseline.target_sha256_before,
    target_sha256_after: targetAfterHash ?? null,
    target_changed: targetChanged,
    acceptance_command: baseline.acceptance_command,
    task_acceptance_command: task.acceptance_command,
    task_sha256_baseline: baseline.task_sha256_before ?? null,
    task_sha256_observed: (() => {
      try {
        return sha256(taskPath)
      } catch {
        return null
      }
    })(),
    acceptance_exit: acceptanceExit,
    acceptance_output_bytes: Buffer.byteLength(acceptanceOutput, "utf8"),
    model_path: input.modelPath,
    model_sha256: input.modelSha256,
    harness_root: harnessRoot,
    harness_revision: (() => {
      try {
        return revision(harnessRoot)
      } catch {
        return null
      }
    })(),
    execution_id: input.executionID,
    attempt: input.attempt,
    reasoning: input.reasoning,
    terminal_state: passed ? "succeeded" : acceptanceExit === null ? "blocked" : "failed",
    verified: passed,
    failures,
  }
  mkdirSync(evidenceDir, { recursive: true })
  writeFileSync(path.join(evidenceDir, "acceptance.log"), acceptanceOutput)
  writeFileSync(evidencePath, JSON.stringify(evidence, null, 2) + "\n")
  return { status: passed ? "pass" : "fail", failures, evidencePath, evidence }
}

function cli(argv: string[]) {
  const values: Record<string, string> = {}
  for (let i = 0; i < argv.length; i++) {
    const key = argv[i]
    if (!key?.startsWith("--")) continue
    values[key.slice(2)] = argv[++i] ?? ""
  }
  return values
}

if (import.meta.main) {
  const values = cli(process.argv.slice(2))
  const required = ["fixture", "evidence", "model", "model-sha", "execution-id", "attempt"]
  const missing = required.filter((key) => !values[key])
  if (missing.length > 0) {
    console.error(`missing arguments: ${missing.join(", ")}`)
    process.exit(2)
  }
  const result = runAcceptance({
    fixtureRoot: values.fixture,
    evidenceDir: values.evidence,
    modelPath: values.model,
    modelSha256: values["model-sha"],
    harnessRoot: values["harness-root"],
    expectedHarnessRevision: values["harness-revision"],
    executionID: values["execution-id"],
    attempt: Number(values.attempt),
    reasoning: values.reasoning !== "false",
  })
  console.log(JSON.stringify(result, null, 2))
  process.exit(result.status === "pass" ? 0 : 1)
}
