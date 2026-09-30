import { existsSync, readFileSync, writeFileSync } from "fs"
import path from "path"

export async function autoSyncEchoModel(config: any) {
  try {
    const res = await fetch("http://127.0.0.1:8305/v1/models", { signal: AbortSignal.timeout(1500) })
    if (!res.ok) return config
    const data = await res.json()
    const models = data?.data || []
    if (!models.length) return config
    const activeModelPath = models[0].id
    const modelId = path.basename(activeModelPath).replace(".gguf", "")

    if (!config.provider) config.provider = {}
    if (!config.provider["echo-local"]) {
      config.provider["echo-local"] = {
        name: "Echo Ulaç — Canlı Model",
        npm: "@ai-sdk/openai-compatible",
        options: {
          apiKey: "local",
          baseURL: "http://127.0.0.1:8305/v1"
        },
        models: {}
      }
    }

    const echoLocal = config.provider["echo-local"]
    if (!echoLocal.models) echoLocal.models = {}

    echoLocal.models[modelId] = {
      id: modelId,
      name: "Echo — " + modelId,
      attachment: false,
      reasoning: true,
      temperature: false,
      tool_call: true,
      limit: {
        context: 32768,
        output: 4096
      },
      options: {
        systemMessageMode: "single"
      }
    }

    config.small_model = "echo-local/" + modelId
    config.model = "echo-local/" + modelId

    // OpenCode yapılandırma dosyasını diskte kalıcı güncelle
    const configPath = path.resolve(process.cwd(), ".opencode/opencode.jsonc")
    if (existsSync(configPath)) {
      try {
        let text = readFileSync(configPath, "utf-8")
        if (!text.includes(modelId)) {
          text = text.replace(/"model":\s*"[^"]*"/, '"model": "echo-local/' + modelId + '"')
          text = text.replace(/"small_model":\s*"[^"]*"/, '"small_model": "echo-local/' + modelId + '"')
          writeFileSync(configPath, text, "utf-8")
        }
      } catch {}
    }

    return config
  } catch {
    return config
  }
}
