import { createHash } from "node:crypto"
import { execFileSync } from "node:child_process"
import { readFileSync, statSync } from "node:fs"
import path from "node:path"

type JsonObject = Record<string, any>

export type PreflightResult = {
  status: "pass" | "fail"
  failures: string[]
  checks: Record<string, boolean>
  harnessRevision?: string
  selectedArtifact?: {
    path: string
    sha256: string
    sizeBytes: number
  }
}

const REPO_ROOT = path.resolve(import.meta.dir, "../../..")
const DEFAULT_MANIFEST = "/home/void0x14/Documents/echo/eval/artifact-provenance-20260830.json"
const DEFAULT_INTEGRATION = "/home/void0x14/Documents/echo/eval/harness-integration-20260830.json"

function sha256(file: string) {
  return createHash("sha256").update(readFileSync(file)).digest("hex")
}

function readJson(file: string): JsonObject {
  return JSON.parse(readFileSync(file, "utf8")) as JsonObject
}

function gitRevision(root: string) {
  return execFileSync("git", ["rev-parse", "HEAD"], { cwd: root, encoding: "utf8" }).trim()
}

function check(result: PreflightResult, name: string, condition: boolean, failure: string) {
  result.checks[name] = condition
  if (!condition) result.failures.push(failure)
}

export function runPreflight(input: { manifestPath?: string; integrationPath?: string } = {}): PreflightResult {
  const result: PreflightResult = { status: "fail", failures: [], checks: {} }
  const manifestPath = input.manifestPath ?? process.env.ECHO_PROVENANCE_MANIFEST ?? DEFAULT_MANIFEST
  const integrationPath = input.integrationPath ?? process.env.ECHO_HARNESS_INTEGRATION ?? DEFAULT_INTEGRATION

  let manifest: JsonObject
  let integration: JsonObject
  try {
    manifest = readJson(manifestPath)
    integration = readJson(integrationPath)
  } catch (error) {
    result.failures.push(`manifest/integration read failed: ${error instanceof Error ? error.message : String(error)}`)
    return result
  }

  let revision: string | undefined
  try {
    revision = gitRevision(REPO_ROOT)
    result.harnessRevision = revision
  } catch (error) {
    result.failures.push(`harness revision lookup failed: ${error instanceof Error ? error.message : String(error)}`)
  }

  const selected = manifest.selected_for_future_measurement
  const provider = integration.provider
  const runtime = integration.runtime
  const harness = integration.harness
  const manifestRuntime = manifest.measurement_constraints

  check(result, "manifest_schema", manifest.schema === "echo.artifact-provenance.v1", "unexpected provenance manifest schema")
  check(
    result,
    "selected_candidate_is_independent",
    selected?.selection_status === "independently_verified_local_candidate",
    "selected model is not independently verified local candidate",
  )
  check(result, "selected_not_b300_claim", selected?.not_claimed_as === "20260830 B300 final", "selected model provenance does not explicitly exclude the B300 final claim")
  check(result, "harness_root_matches", harness?.root === REPO_ROOT, "integration descriptor points at a different harness root")
  check(result, "harness_revision_matches", !revision || !harness?.expected_revision || revision.startsWith(harness.expected_revision), "harness revision mismatch")
  check(result, "provider_is_local_openai_compatible", provider?.id === "echo-local" && provider?.npm === "@ai-sdk/openai-compatible", "unexpected provider")
  check(result, "provider_endpoint_is_explicit", provider?.base_url === "http://127.0.0.1:8080/v1", "unexpected provider endpoint")
  check(result, "provider_model_is_explicit", typeof provider?.model_id === "string" && provider.model_id.length > 0, "model id is missing")
  check(result, "single_system_message_contract", provider?.system_message_mode === "single", "model requires a single system message contract")
  check(result, "no_provider_fallback", provider?.fallback_provider === false && provider?.fallback_model === false, "provider/model fallback is enabled")
  check(result, "runtime_quantization", runtime?.quantization === "Q4_K_M" && manifestRuntime?.quantization === "Q4_K_M", "runtime is not Q4_K_M")
  check(result, "runtime_backend", runtime?.backend === "Vulkan" && manifestRuntime?.backend === "Vulkan", "runtime backend is not Vulkan")
  check(result, "runtime_device", typeof runtime?.device === "string" && /RX\s*460/i.test(runtime.device), "runtime device is not RX460")
  check(result, "cpu_fallback_disabled", runtime?.cpu_fallback === false && manifestRuntime?.cpu_fallback === false, "CPU fallback is enabled")
  check(result, "reasoning_enabled", runtime?.reasoning === true && provider?.reasoning === true && manifestRuntime?.reasoning === true, "reasoning is not enabled")
  check(result, "artifact_path_matches", provider?.artifact_path === selected?.path, "integration artifact path differs from selected provenance artifact")
  check(result, "artifact_sha_matches_manifest", provider?.artifact_sha256 === selected?.sha256, "integration artifact SHA differs from selected provenance SHA")

  if (typeof selected?.path === "string" && typeof selected?.sha256 === "string") {
    try {
      const stat = statSync(selected.path)
      const actual = sha256(selected.path)
      result.selectedArtifact = { path: selected.path, sha256: actual, sizeBytes: stat.size }
      check(result, "artifact_exists", stat.isFile(), "selected artifact is not a regular file")
      check(result, "artifact_sha256", actual === selected.sha256, "selected artifact SHA-256 mismatch")
    } catch (error) {
      result.failures.push(`selected artifact inspection failed: ${error instanceof Error ? error.message : String(error)}`)
    }
  } else {
    result.failures.push("selected artifact path/SHA is missing")
  }

  result.status = result.failures.length === 0 ? "pass" : "fail"
  return result
}

function args(argv: string[]) {
  const parsed: { manifestPath?: string; integrationPath?: string } = {}
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === "--manifest") parsed.manifestPath = argv[++i]
    if (argv[i] === "--integration") parsed.integrationPath = argv[++i]
  }
  return parsed
}

if (import.meta.main) {
  const result = runPreflight(args(process.argv.slice(2)))
  console.log(JSON.stringify(result, null, 2))
  process.exit(result.status === "pass" ? 0 : 1)
}
