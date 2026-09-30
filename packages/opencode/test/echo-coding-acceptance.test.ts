import { describe, expect, test } from "bun:test"
import { cpSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import os from "node:os"
import path from "node:path"
import { runAcceptance } from "../script/echo-coding-acceptance"

const FIXTURE_ROOT = path.resolve(import.meta.dir, "../fixtures/echo-coding-task")
const MANIFEST_PATH = "/home/void0x14/Documents/echo/eval/artifact-provenance-20260830.json"

describe("Echo coding acceptance", () => {
  test("grades a changed disposable fixture with a passing acceptance command", () => {
    const manifest = JSON.parse(readFileSync(MANIFEST_PATH, "utf8"))
    const tempRoot = mkdtempSync(path.join(os.tmpdir(), "echo-coding-acceptance-"))
    const evidenceDir = path.join(tempRoot, "evidence")
    const fixture = path.join(tempRoot, "fixture")
    cpSync(FIXTURE_ROOT, fixture, { recursive: true })
    writeFileSync(
      path.join(fixture, "src/slugify.ts"),
      'export function slugify(value: string): string {\n  return value.trim().toLocaleLowerCase("tr-TR").replace(/[^\\p{L}\\p{N}]+/gu, "-").replace(/^-|-$/g, "")\n}\n',
    )

    try {
      const result = runAcceptance({
        fixtureRoot: fixture,
        evidenceDir,
        modelPath: manifest.selected_for_future_measurement.path,
        modelSha256: manifest.selected_for_future_measurement.sha256,
        harnessRoot: path.resolve(import.meta.dir, "../../.."),
        expectedHarnessRevision: "5fc72e9d11",
        executionID: "test-execution-echo-coding",
        attempt: 1,
        reasoning: true,
      })

      expect(result.status).toBe("pass")
      expect(result.failures).toEqual([])
      expect(result.evidence.verified).toBe(true)
      expect(result.evidence.target_changed).toBe(true)
      expect(result.evidence.acceptance_exit).toBe(0)
      expect(result.evidence.terminal_state).toBe("succeeded")
    } finally {
      rmSync(tempRoot, { recursive: true, force: true })
    }
  })

  test("rejects a task specification changed after the baseline", () => {
    const manifest = JSON.parse(readFileSync(MANIFEST_PATH, "utf8"))
    const tempRoot = mkdtempSync(path.join(os.tmpdir(), "echo-coding-acceptance-task-lock-"))
    const fixture = path.join(tempRoot, "fixture")
    cpSync(FIXTURE_ROOT, fixture, { recursive: true })
    const taskPath = path.join(fixture, "task.json")
    const task = JSON.parse(readFileSync(taskPath, "utf8"))
    task.acceptance_command = ["bun", "test"]
    writeFileSync(taskPath, JSON.stringify(task) + "\n")

    try {
      const result = runAcceptance({
        fixtureRoot: fixture,
        evidenceDir: path.join(tempRoot, "evidence"),
        modelPath: manifest.selected_for_future_measurement.path,
        modelSha256: manifest.selected_for_future_measurement.sha256,
        harnessRoot: path.resolve(import.meta.dir, "../../.."),
        expectedHarnessRevision: "5fc72e9d11",
        executionID: "test-execution-task-lock",
        attempt: 1,
        reasoning: true,
      })

      expect(result.status).toBe("fail")
      expect(result.failures).toContain("task specification changed since baseline")
      expect(result.failures).toContain("task acceptance command differs from baseline")
      expect(result.evidence.acceptance_exit).toBeNull()
      expect(result.evidence.verified).toBe(false)
    } finally {
      rmSync(tempRoot, { recursive: true, force: true })
    }
  })
})
