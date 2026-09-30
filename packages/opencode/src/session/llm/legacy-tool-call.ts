import type {
  LanguageModelV3Middleware,
  LanguageModelV3StreamPart,
} from "@ai-sdk/provider"

type ParsedCall = {
  name: string
  arguments: Record<string, unknown>
}

type ParsedText = {
  calls: ParsedCall[]
  residual: string
}

const TOOL_CALL_BLOCK = /<tool_call>\s*(?:<\s*)?<function=([^>\s]+)>([\s\S]*?)<\/function>\s*<\/tool_call>/g
const PARAMETER_BLOCK = /<parameter=([^>\s]+)>\s*([\s\S]*?)\s*<\/parameter>/g

function parseFunction(name: string, body: string): ParsedCall | undefined {
  const trimmed = body.trim()
  if (trimmed.startsWith("{") && trimmed.endsWith("}")) {
    try {
      const parsed = JSON.parse(trimmed)
      if (typeof parsed === "object" && parsed !== null) {
        return { name, arguments: parsed }
      }
    } catch {}
  }

  const args: Record<string, unknown> = {}
  let cursor = 0
  let match: RegExpExecArray | null
  while ((match = PARAMETER_BLOCK.exec(body)) !== null) {
    if (body.slice(cursor, match.index).trim() !== "") return undefined
    const parameter = match[1]
    if (parameter in args) {
      cursor = match.index + match[0].length
      continue
    }
    const raw = match[2].trim()
    try {
      args[parameter] = JSON.parse(raw)
    } catch {
      args[parameter] = raw
    }
    cursor = match.index + match[0].length
  }
  PARAMETER_BLOCK.lastIndex = 0
  if (body.slice(cursor).trim() !== "") return undefined
  return { name, arguments: args }
}

function parseLegacyText(text: string): ParsedText | undefined {
  TOOL_CALL_BLOCK.lastIndex = 0
  const calls: ParsedCall[] = []
  let residual = ""
  let cursor = 0
  let match: RegExpExecArray | null
  while ((match = TOOL_CALL_BLOCK.exec(text)) !== null) {
    const call = parseFunction(match[1], match[2])
    if (!call) {
      TOOL_CALL_BLOCK.lastIndex = 0
      return undefined
    }
    calls.push(call)
    residual += text.slice(cursor, match.index)
    cursor = match.index + match[0].length
  }
  TOOL_CALL_BLOCK.lastIndex = 0
  if (calls.length === 0) return undefined
  residual += text.slice(cursor)
  if (residual.includes("<tool_call")) return undefined
  return { calls, residual }
}

type PendingText = {
  start: Extract<LanguageModelV3StreamPart, { type: "text-start" }>
  delta: string
  end: Extract<LanguageModelV3StreamPart, { type: "text-end" }>
}

export function legacyToolCallMiddleware(): LanguageModelV3Middleware {
  return {
    specificationVersion: "v3",
    wrapStream: async ({ doStream }) => {
      const result = await doStream()
      let pending: PendingText | undefined
      let nextCallID = 0
      let emittedToolCall = false

      const stream = result.stream.pipeThrough(
        new TransformStream<LanguageModelV3StreamPart, LanguageModelV3StreamPart>({
          transform(part, controller) {
            if (part.type === "text-start") {
              if (pending) flushPending(pending, controller)
              pending = { start: part, delta: "", end: { type: "text-end", id: part.id } }
              return
            }
            if (part.type === "text-delta" && pending?.start.id === part.id) {
              pending.delta += part.delta
              return
            }
            if (part.type === "text-end" && pending?.start.id === part.id) {
              pending.end = part
              flushPending(pending, controller, () => {
                emittedToolCall = true
                return `legacy-${nextCallID++}`
              })
              pending = undefined
              return
            }
            if (pending) {
              flushPending(pending, controller)
              pending = undefined
            }
            if (part.type === "finish" && emittedToolCall) {
              controller.enqueue({
                ...part,
                finishReason: { ...part.finishReason, unified: "tool-calls" as const },
              })
              return
            }
            controller.enqueue(part)
          },
          flush(controller) {
            if (pending) flushPending(pending, controller)
          },
        }),
      )
      return { ...result, stream }
    },
  }
}

function flushPending(
  pending: PendingText,
  controller: TransformStreamDefaultController<LanguageModelV3StreamPart>,
  allocateCallID?: () => string,
) {
  let delta = pending.delta
  const thinkMatch = delta.match(/<think>([\s\S]*?)<\/think>/)
  if (thinkMatch) {
    const thought = thinkMatch[1].trim()
    if (thought) {
      controller.enqueue({ type: "reasoning-start", id: `think-${pending.start.id}` })
      controller.enqueue({ type: "reasoning-delta", id: `think-${pending.start.id}`, delta: thought })
      controller.enqueue({ type: "reasoning-end", id: `think-${pending.start.id}` })
    }
    delta = delta.replace(/<think>[\s\S]*?<\/think>\s*/g, "")
  }

  const parsed = parseLegacyText(delta)
  if (!parsed) {
    controller.enqueue(pending.start)
    if (delta) controller.enqueue({ type: "text-delta", id: pending.start.id, delta })
    controller.enqueue(pending.end)
    return
  }
  for (const call of parsed.calls) {
    controller.enqueue({
      type: "tool-call",
      toolCallId: allocateCallID?.() ?? `legacy-${call.name}`,
      toolName: call.name,
      input: JSON.stringify(call.arguments),
      providerExecuted: false,
    })
  }
}
