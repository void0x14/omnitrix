import { describe, expect } from "bun:test"
import { createHash } from "node:crypto"
import { execFileSync } from "node:child_process"
import path from "node:path"
import { Effect, Layer } from "effect"
import { LayerNode } from "@opencode-ai/core/effect/layer-node"
import { ProviderV2 } from "@opencode-ai/core/provider"
import { ModelV2 } from "@opencode-ai/core/model"
import { Agent } from "../src/agent/agent"
import { Config } from "@/config/config"
import { RuntimeFlags } from "@/effect/runtime-flags"
import { ToolJsonSchema } from "@/tool/json-schema"
import { ToolRegistry } from "@/tool/registry"
import { TestConfig } from "./fixture/config"
import { testEffect } from "./lib/effect"

const configLayer = TestConfig.layer({ directories: () => Effect.succeed([]) })
const layer = LayerNode.compile(LayerNode.group([ToolRegistry.node, Agent.node]), [
  [Config.node, configLayer],
  [RuntimeFlags.node, RuntimeFlags.layer()],
])
const it = testEffect(layer)

describe("Echo runtime tool schema", () => {
  it.instance("exports the live ToolRegistry schema without model execution", () =>
    Effect.gen(function* () {
      const registry = yield* ToolRegistry.Service
      const agents = yield* Agent.Service
      const build = yield* agents.get("build")
      if (!build) throw new Error("build agent not found")
      const tools = yield* registry.tools({
        providerID: ProviderV2.ID.make("echo-local"),
        modelID: ModelV2.ID.make("echo-fsociety-h200-20260828"),
        agent: build,
      })
      const document = {
        schema: "echo.open-code-runtime-tool-schema.v1",
        harness_root: path.resolve(import.meta.dir, "../../.."),
        harness_revision: execFileSync("git", ["rev-parse", "HEAD"], {
          cwd: path.resolve(import.meta.dir, "../../.."),
          encoding: "utf8",
        }).trim(),
        model_context: { provider_id: "echo-local", model_id: "echo-fsociety-h200-20260828" },
        tools: tools
          .map((tool) => ({
            name: tool.id,
            description_sha256: createHash("sha256").update(tool.description).digest("hex"),
            input_schema: ToolJsonSchema.fromTool(tool),
          }))
          .sort((a, b) => a.name.localeCompare(b.name)),
      }
      const schemaSha256 = createHash("sha256").update(JSON.stringify(document)).digest("hex")
      const output = { ...document, schema_sha256: schemaSha256 }
      const destination = process.env.ECHO_RUNTIME_TOOL_SCHEMA_OUT
      if (destination) yield* Effect.promise(() => Bun.write(destination, JSON.stringify(output, null, 2) + "\n"))

      expect(output.tools.some((tool) => tool.name === "task")).toBe(true)
      expect(output.tools.some((tool) => tool.name === "read")).toBe(true)
      expect(output.tools.every((tool) => tool.input_schema && typeof tool.input_schema === "object")).toBe(true)
      expect(output.schema_sha256).toMatch(/^[a-f0-9]{64}$/)
    }),
    { git: true },
  )
})
