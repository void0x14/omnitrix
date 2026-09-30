import { describe, expect, test } from "bun:test"
import { runPreflight } from "../script/echo-preflight"

describe("Echo product preflight", () => {
  test("accepts the selected artifact only under the required runtime contract", () => {
    const result = runPreflight()

    expect(result.status).toBe("pass")
    expect(result.failures).toEqual([])
    expect(result.checks.artifact_sha256).toBe(true)
    expect(result.checks.runtime_quantization).toBe(true)
    expect(result.checks.runtime_backend).toBe(true)
    expect(result.checks.runtime_device).toBe(true)
    expect(result.checks.single_system_message_contract).toBe(true)
    expect(result.checks.cpu_fallback_disabled).toBe(true)
    expect(result.checks.reasoning_enabled).toBe(true)
  })
})
