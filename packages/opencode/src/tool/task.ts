import * as Tool from "./tool"
import DESCRIPTION from "./task.txt"
import { ToolJsonSchema } from "./json-schema"
import { SessionV1 } from "@opencode-ai/core/v1/session"
import { BackgroundJob } from "@/background/job"
import { SubagentState } from "@/session/subagent-state"
import { Session } from "@/session/session"
import { SessionID, MessageID } from "../session/schema"
import { MessageV2 } from "../session/message-v2"
import { Agent } from "../agent/agent"
import { deriveSubagentSessionPermission } from "../agent/subagent-permissions"
import type { SessionPrompt } from "../session/prompt"
import { Config } from "@/config/config"
import { Effect, Exit, Schema, Scope } from "effect"
import { EffectBridge } from "@/effect/bridge"
import { RuntimeFlags } from "@/effect/runtime-flags"
import { Database } from "@opencode-ai/core/database/database"

export interface TaskPromptOps {
  cancel(sessionID: SessionID): Effect.Effect<void>
  resolvePromptParts(template: string): Effect.Effect<SessionPrompt.PromptInput["parts"]>
  prompt(input: SessionPrompt.PromptInput): Effect.Effect<SessionV1.WithParts>
}

const id = "task"
const BACKGROUND_DESCRIPTION = [
  "Background mode: background=true launches the subagent asynchronously and returns immediately.",
  "Foreground is the default; use it when you need the result before continuing. Both foreground and background tasks are tracked in real-time by a deterministic state hook.",
  "Use background only for independent work that can run while you continue elsewhere.",
  "While any subagent is running you will see a real-time [SUBAGENT STATE] injection before each turn. You can also explicitly query via task status, cancel via task cancel, or update via task update. You are stateful and must remain aware and in control.",
].join(" ")
const BACKGROUND_STARTED = [
  "The task is running. You can monitor it in real-time: use task with action:\"status\" (optionally task_id) to see live output, action:\"cancel\" to stop it, or action:\"update\" to send more context.",
  "A deterministic hook will inject [SUBAGENT STATE] before your next turn so you remain aware of all running subagents — you are not stateless.",
  "You will also be notified automatically when it finishes. While it runs, you may work on other independent tasks that do not overlap in files/topics, or wait and poll via status.",
].join("\n")
const BACKGROUND_UPDATED = [
  "Additional context sent to the running background task.",
  "The task is still running. Continue to monitor via task status if needed; the [SUBAGENT STATE] hook will keep you aware and in control.",
  "You will be notified automatically when it finishes. While it runs, avoid duplicating its work on overlapping files/topics.",
].join("\n")

const BaseParameterFields = {
  description: Schema.optional(Schema.String).annotate({ description: "A short (3-5 words) description of the task (required for launch, optional for status/cancel/update with task_id)" }),
  prompt: Schema.optional(Schema.String).annotate({ description: "The task for the agent to perform (required for launch, optional for status/cancel/update)" }),
  subagent_type: Schema.optional(Schema.String).annotate({ description: "The type of specialized agent to use for this task (required for launch, optional for status/cancel/update)" }),
  task_id: Schema.optional(Schema.String).annotate({
    description:
      "This should only be set if you mean to resume a previous task (you can pass a prior task_id and the task will continue the same subagent session as before instead of creating a fresh one)",
  }),
  command: Schema.optional(Schema.String).annotate({ description: "The command that triggered this task" }),
  action: Schema.optional(
    Schema.Union([Schema.Literal("launch"), Schema.Literal("status"), Schema.Literal("cancel"), Schema.Literal("update")]),
  ).annotate({
    description:
      "Controls what the tool does: launch starts a new task (default), status reports on a running task, cancel stops a running task, update sends additional context to a running task",
  }),
}

const BaseParameters = Schema.Struct(BaseParameterFields)

export const Parameters = Schema.Struct({
  ...BaseParameterFields,
  background: Schema.optional(Schema.Boolean).annotate({
    description: "Run the agent in the background. You will be notified when it completes.",
  }),
}) as unknown as any

function renderOutput(input: {
  sessionID: SessionID
  state: "running" | "completed" | "error"
  summary?: string
  text: string
}) {
  const tag = input.state === "error" ? "task_error" : "task_result"
  return [
    `<task id="${input.sessionID}" state="${input.state}">`,
    ...(input.summary ? [`<summary>${input.summary}</summary>`] : []),
    `<${tag}>`,
    input.text,
    `</${tag}>`,
    "</task>",
  ].join("\n")
}
type TaskExecuteMetadata = {
  parentSessionId?: SessionID
  sessionId: SessionID
  model?: { modelID: string; providerID: string }
  background?: boolean
  jobId?: string
}
type TaskToolRequirements =
  | Agent.Service
  | BackgroundJob.Service
  | Config.Service
  | Session.Service
  | Scope.Scope
  | RuntimeFlags.Service
  | Database.Service

export const TaskTool = Tool.define<typeof Parameters, TaskExecuteMetadata, TaskToolRequirements>(
  id,
  Effect.gen(function* () {
    const agent = yield* Agent.Service
    const background = yield* BackgroundJob.Service
    const config = yield* Config.Service
    const sessions = yield* Session.Service
    const scope = yield* Scope.Scope
    const flags = yield* RuntimeFlags.Service
    const database = yield* Database.Service

    const run = Effect.fn("TaskTool.execute")(function* (
      params: Schema.Schema.Type<typeof Parameters>,
      ctx: Tool.Context,
    ) {
      const cfg = yield* config.get()
      const runInBackground = params.background === true
      if (runInBackground && !flags.experimentalBackgroundSubagents) {
        return yield* Effect.fail(
          new Error("Background subagents require OPENCODE_EXPERIMENTAL_BACKGROUND_SUBAGENTS=true"),
        )
      }

      const action = params.action ?? "launch"
      if (action === "launch" && (!params.description || !params.prompt || !params.subagent_type)) {
        return {
          title: "task",
          metadata: { sessionId: ctx.sessionID } as any,
          output: "description, prompt, and subagent_type are required for launch (task_id optional for resume)",
        }
      }
      if (action === "status") {
        const all = yield* SubagentState.hybridList(ctx.sessionID)
        const filtered = all.filter(
          (job) => !params.task_id || job.id === params.task_id || (job.sessionID as string) === params.task_id,
        )
        if (filtered.length === 0 && params.task_id) {
          // Fallback: standalone session not parented? try direct lookup
          const direct = yield* sessions
            .get(SessionID.make(params.task_id))
            .pipe(Effect.catch(() => Effect.succeed(undefined as unknown as Session.Info)))
          if (direct) {
            const msgs = yield* MessageV2.filterCompactedEffect(SessionID.make(params.task_id)).pipe(
              Effect.provideService(Database.Service, database),
              Effect.catch(() => Effect.succeed([] as any)),
            )
            const tail = direct.title ?? ""
            const hasPending = (msgs as any[]).some((m: any) =>
              m.parts.some((p: any) => p.type === "tool" && (p.state.status === "pending" || p.state.status === "running")),
            )
            const status = hasPending ? "running" : "completed"
            return {
              title: params.description ?? tail ?? "task",
              metadata: { sessionId: SessionID.make(params.task_id) } as any,
              output: `${params.task_id} [${status}] source=durable standalone ${tail ? `\n${tail}` : ""}`,
            }
          }
          return {
            title: params.description ?? "task",
            metadata: { sessionId: SessionID.make(params.task_id) } as any,
            output: `Task ${params.task_id} not found`,
          }
        }
        if (filtered.length === 0)
          return {
            title: params.description ?? "task",
            metadata: { sessionId: ctx.sessionID } as any,
            output: "No tasks found for this parent (hybrid durable+memory). SQL fallback confirms no child sessions.",
          }
        const text = filtered
          .map((job) => {
            const tail = job.tail ?? ""
            return `${job.id} [${job.status}] source=${job.source}${job.title ? ` ${job.title}` : ""}${tail ? `\n${tail}` : ""}`
          })
          .join("\n\n")
        return {
          title: params.description ?? filtered[0]?.title ?? "task",
          metadata: { sessionId: SessionID.make(params.task_id ?? filtered[0]!.id) } as any,
          output: text,
        }
      }

      if (action === "cancel") {
        if (!params.task_id) return yield* Effect.fail(new Error("cancel requires a task_id"))
        const all = yield* SubagentState.hybridList(ctx.sessionID)
        const target = all.find((j) => j.id === params.task_id || (j.sessionID as string) === params.task_id)
        const job = yield* background.get(params.task_id).pipe(Effect.catch(() => Effect.succeed(undefined as any)))
        const isRunning = target?.status === "running" || job?.status === "running"
        if (!target && (!job || job.metadata?.parentSessionId !== ctx.sessionID))
          return {
            title: params.description ?? "task",
            metadata: { sessionId: SessionID.make(params.task_id) } as any,
            output: `Task ${params.task_id} not found or not running (hybrid check: no durable child nor job)`,
          }
        if (!isRunning)
          return {
            title: params.description ?? "task",
            metadata: { sessionId: SessionID.make(params.task_id) } as any,
            output: `Task ${params.task_id} is not running (status: ${target?.status ?? job?.status ?? "unknown"}). Cannot cancel.`,
          }
        const cancelled = yield* background.cancel(params.task_id).pipe(Effect.catch(() => Effect.succeed(undefined as any)))
        // Even if no job to cancel (durable-only orphan), still cancel the session runner
        const cancelOps = ctx.extra?.promptOps as TaskPromptOps | undefined
        if (cancelOps) yield* cancelOps.cancel(SessionID.make(params.task_id)).pipe(Effect.catch(() => Effect.void))
        if (cancelled) {
          return {
            title: params.description ?? "task",
            metadata: { sessionId: SessionID.make(params.task_id), parentSessionId: ctx.sessionID } as any,
            output: `Task ${params.task_id} cancelled (hybrid)`,
          }
        }
        // Durable-only path: cancelled via session
        return {
          title: params.description ?? "task",
          metadata: { sessionId: SessionID.make(params.task_id), parentSessionId: ctx.sessionID } as any,
          output: `Task ${params.task_id} cancelled (durable session)`,
        }
      }

      if (action === "update" && !params.task_id) {
        return {
          title: "task",
          metadata: { sessionId: ctx.sessionID } as any,
          output: "update requires a task_id",
        }
      }
      if (action === "update" && !params.prompt) {
        return {
          title: "task",
          metadata: { sessionId: SessionID.make(params.task_id!) } as any,
          output: "prompt is required for update",
        }
      }
      if (action === "update") {
        const all = yield* SubagentState.hybridList(ctx.sessionID)
        const target = all.find((j) => j.id === params.task_id || (j.sessionID as string) === params.task_id)
        const job = yield* background.get(params.task_id!).pipe(Effect.catch(() => Effect.succeed(undefined as any)))
        const status = target?.status ?? (job?.status as string | undefined)
        if (!target && !job) {
          return {
            title: params.description ?? "task",
            metadata: { sessionId: SessionID.make(params.task_id!) } as any,
            output: `Task ${params.task_id} is not running (status: not found). To resume from where it left off, launch a new task with the same task_id and a new prompt (e.g., task with task_id:"${params.task_id}", prompt:"Continue..." ). The update action only works for running tasks.`,
          }
        }
        if (status !== "running") {
          return {
            title: params.description ?? "task",
            metadata: { sessionId: SessionID.make(params.task_id!) } as any,
            output: `Task ${params.task_id} is not running (status: ${status ?? "unknown"}). To resume from where it left off, launch a new task with the same task_id and a new prompt (e.g., task with task_id:"${params.task_id}", prompt:"Continue..." ). The update action only works for running tasks.`,
          }
        }
      }
      if (action === "update" && !params.prompt) {
        return yield* Effect.fail(new Error("prompt is required for update"))
      }

      const parent = yield* sessions.get(ctx.sessionID)
      let current = parent
      let depth = 0
      while (current.parentID) {
        depth++
        current = yield* sessions.get(current.parentID)
      }
      if (depth >= (cfg.subagent_depth ?? 1)) {
        return yield* Effect.fail(
          new Error(
            `Subagent depth limit reached (${cfg.subagent_depth ?? 1}). Increase "subagent_depth" to allow nested subagents.`,
          ),
        )
      }

      if (!ctx.extra?.bypassAgentCheck) {
        yield* ctx.ask({
          permission: id,
          patterns: [params.subagent_type!],
          always: ["*"],
          metadata: {
            description: params.description!,
            subagent_type: params.subagent_type!,
          },
        })
      }

      const next = yield* agent.get(params.subagent_type!)
      if (!next) {
        return yield* Effect.fail(new Error(`Unknown agent type: ${params.subagent_type!} is not a valid agent type`))
      }

      const session = params.task_id
        ? yield* sessions.get(SessionID.make(params.task_id)).pipe(Effect.catchCause(() => Effect.succeed(undefined)))
        : undefined
      const childPermission = deriveSubagentSessionPermission({
        parentSessionPermission: parent.permission ?? [],
        subagent: next,
      })
      const childToolDenies = [
        ...(next.permission.some((rule) => rule.permission === "todowrite")
          ? []
          : [{ permission: "todowrite" as const, pattern: "*" as const, action: "deny" as const }]),
        ...(next.permission.some((rule) => rule.permission === id)
          ? []
          : [{ permission: id, pattern: "*" as const, action: "deny" as const }]),
        ...(cfg.experimental?.primary_tools?.map((permission) => ({
          permission,
          pattern: "*" as const,
          action: "deny" as const,
        })) ?? []),
      ]
      const nextSession =
        session ??
        (yield* sessions.create({
          parentID: ctx.sessionID,
          title: params.description + ` (@${next.name} subagent)`,
          agent: next.name,
          permission: [
            ...childPermission,
            ...childToolDenies.filter(
              (deny) =>
                !childPermission.some(
                  (rule) =>
                    rule.permission === deny.permission && rule.pattern === deny.pattern && rule.action === deny.action,
                ),
            ),
          ],
        }))

      const msg = yield* MessageV2.get({ sessionID: ctx.sessionID, messageID: ctx.messageID }).pipe(
        Effect.provideService(Database.Service, database),
        Effect.orDie,
      )
      if (msg.info.role !== "assistant") return yield* Effect.fail(new Error("Not an assistant message"))
      const variant = msg.info.variant

      const model = next.model ?? {
        modelID: msg.info.modelID,
        providerID: msg.info.providerID,
      }
      const metadata = {
        parentSessionId: ctx.sessionID,
        sessionId: nextSession.id,
        model,
        ...(runInBackground ? { background: true } : {}),
      }

      yield* ctx.metadata({
        title: params.description,
        metadata,
      })

      const ops = ctx.extra?.promptOps as TaskPromptOps
      if (!ops) return yield* Effect.fail(new Error("TaskTool requires promptOps in ctx.extra"))

      const runTask = Effect.fn("TaskTool.runTask")(function* () {
        const parts = yield* ops.resolvePromptParts(params.prompt!)
        const result = yield* ops.prompt({
          messageID: MessageID.ascending(),
          sessionID: nextSession.id,
          model: {
            modelID: model.modelID,
            providerID: model.providerID,
          },
          variant: next.model ? undefined : variant,
          agent: next.name,
          parts,
        })
        if (result.info.role === "assistant" && result.info.error) {
          const message =
            "message" in result.info.error.data && typeof result.info.error.data.message === "string"
              ? result.info.error.data.message
              : result.info.error.name
          return yield* Effect.fail(new Error(`Subagent failed (task_id: ${nextSession.id}): ${message}`))
        }
        const failed = result.parts.findLast((item) => item.type === "tool" && item.state.status === "error")
        if (failed?.type === "tool" && failed.state.status === "error") {
          return yield* Effect.fail(new Error(`Subagent failed (task_id: ${nextSession.id}): ${failed.state.error}`))
        }
        return result.parts.findLast((item) => item.type === "text")?.text ?? ""
      })

      const inject = Effect.fn("TaskTool.injectBackgroundResult")(function* (
        state: "completed" | "error",
        text: string,
      ) {
        const currentParent = yield* sessions.get(ctx.sessionID)
        yield* ops
          .prompt({
            sessionID: ctx.sessionID,
            agent: currentParent.agent ?? ctx.agent,
            variant,
            parts: [
              {
                type: "text",
                synthetic: true,
                text: renderOutput({
                  sessionID: nextSession.id,
                  state,
                  summary:
                    state === "completed"
                      ? `Background task completed: ${params.description}`
                      : `Background task failed: ${params.description}`,
                  text,
                }),
              },
            ],
          })
          .pipe(Effect.ignore, Effect.forkIn(scope, { startImmediately: true }))
      })

      const notify = Effect.fn("TaskTool.notifyBackgroundResult")(function* (jobID: string) {
        yield* background.wait({ id: jobID }).pipe(
          Effect.flatMap((result) => {
            if (result.info?.status === "completed") return inject("completed", result.info.output ?? "")
            if (result.info?.status === "error") return inject("error", result.info.error ?? "")
            return Effect.void
          }),
          Effect.forkIn(scope, { startImmediately: true }),
        )
      })

      if (yield* background.extend({ id: nextSession.id, run: runTask() })) {
        return {
          title: params.description,
          metadata: {
            ...metadata,
            background: true,
            jobId: nextSession.id,
          },
          output: renderOutput({
            sessionID: nextSession.id,
            state: "running",
            summary: "Background task updated",
            text: BACKGROUND_UPDATED,
          }),
        }
      }

      const info = yield* background.start({
        id: nextSession.id,
        type: id,
        title: params.description,
        metadata,
        onPromote: Effect.all([
          ctx.metadata({
            title: params.description,
            metadata: { ...metadata, background: true, jobId: nextSession.id },
          }),
          notify(nextSession.id),
        ]),
        run: runTask().pipe(Effect.onInterrupt(() => ops.cancel(nextSession.id))),
      })

      function backgroundResult() {
        return {
          title: params.description,
          metadata: {
            ...metadata,
            background: true,
            jobId: info.id,
          },
          output: renderOutput({
            sessionID: nextSession.id,
            state: "running",
            summary: "Background task started",
            text: BACKGROUND_STARTED,
          }),
        }
      }

      // Deterministic stateful hook: never block parent, always return running state immediately.
      // Parent remains aware via [SUBAGENT STATE] injection and can use task status/cancel/update
      // in real-time before child completes. This fixes the architectural flaw where
      // foreground tasks blocked the parent and made it stateless.
      const runCancel = yield* EffectBridge.make()
      const cancel = ops.cancel(nextSession.id)
      function onAbort() {
        runCancel.fork(cancel)
      }
      // Setup abort handling (ESC -> cancel child) without blocking
      yield* Effect.sync(() => {
        ctx.abort.addEventListener("abort", onAbort)
      }).pipe(
        Effect.ensuring(Effect.sync(() => ctx.abort.removeEventListener("abort", onAbort))),
        Effect.forkIn(scope, { startImmediately: true }),
      )
      yield* notify(info.id)
      return backgroundResult()
    })

    return {
      description: flags.experimentalBackgroundSubagents
        ? [DESCRIPTION, BACKGROUND_DESCRIPTION].join("\n\n")
        : DESCRIPTION,
      parameters: Parameters as unknown as any,
      jsonSchema: flags.experimentalBackgroundSubagents ? undefined : ToolJsonSchema.fromSchema(BaseParameters as any),
      execute: (params: any, ctx: Tool.Context) =>
        run(params as any, ctx as any).pipe(Effect.orDie),
    } as unknown as any
  }),
)
