import { Effect } from "effect"
import { Database } from "@opencode-ai/core/database/database"
import { BackgroundJob } from "@/background/job"
import { Session } from "./session"
import { MessageV2 } from "./message-v2"
import { SessionV1 } from "@opencode-ai/core/v1/session"
import type { SessionID } from "./schema"

export type HybridStatus = "running" | "completed" | "error" | "cancelled"

export type HybridInfo = {
  id: string
  title?: string
  status: HybridStatus
  started_at: number
  elapsed: number
  tail?: string
  metadata?: Record<string, unknown>
  source: "job" | "durable" | "merged"
  sessionID: SessionID
}

function durableStatusFromMessages(msgs: SessionV1.WithParts[]): HybridStatus {
  if (msgs.length === 0) return "running"
  const hasPendingTool = msgs.some((m) =>
    m.parts.some((p: SessionV1.Part) => p.type === "tool" && (p.state.status === "pending" || p.state.status === "running")),
  )
  if (hasPendingTool) return "running"
  const latest = MessageV2.latest(msgs)
  const last = (latest.assistant ?? latest.finished) as SessionV1.Assistant | undefined
  if (last?.error) return "error"
  if (last?.finish && !["tool-calls", "unknown"].includes(last.finish)) return "completed"
  if (last && !last.finish) return "running"
  if (latest.user && !last) return "running"
  return msgs.length > 0 ? "running" : "running"
}

function tailFromMessages(msgs: SessionV1.WithParts[]): string | undefined {
  for (let i = msgs.length - 1; i >= 0; i--) {
    const msg = msgs[i]
    if (!msg) continue
    for (let j = msg.parts.length - 1; j >= 0; j--) {
      const part = msg.parts[j] as SessionV1.Part
      if (!part) continue
      if (part.type === "text" && part.text) return part.text.trim().slice(-500)
      if (part.type === "tool" && part.state.status === "completed" && typeof (part.state as any).output === "string" && (part.state as any).output)
        return (part.state as any).output.trim().slice(-500)
      if (part.type === "tool" && part.state.status === "error" && (part.state as any).error)
        return String((part.state as any).error).slice(-500)
    }
  }
  return undefined
}

export const durableStatus = Effect.fn("SubagentState.durableStatus")(function* (sessionID: SessionID) {
  const database = yield* Database.Service
  const msgs = yield* MessageV2.filterCompactedEffect(sessionID).pipe(
    Effect.provideService(Database.Service, database),
    Effect.catch(() => Effect.succeed([] as SessionV1.WithParts[])),
  )
  return durableStatusFromMessages(msgs)
})

export const hybridList = Effect.fn("SubagentState.hybridList")(function* (parentID: SessionID) {
  const background = yield* BackgroundJob.Service
  const sessions = yield* Session.Service
  const database = yield* Database.Service

  const now = Date.now()
  const jobs = yield* background.list()
  const jobMap = new Map<string, HybridInfo>()
  for (const job of jobs) {
    const meta = job.metadata as Record<string, unknown> | undefined
    const parent = meta?.parentSessionId
    if (typeof parent !== "string" || parent !== parentID) continue
    const sid = (meta?.sessionId as string | undefined) ?? job.id
    const info: HybridInfo = {
      id: job.id,
      title: job.title,
      status: job.status as HybridStatus,
      started_at: job.started_at,
      elapsed: Math.floor((now - job.started_at) / 1000),
      tail: job.output ? job.output.trim().slice(-500) : job.error?.slice(-500),
      metadata: job.metadata,
      source: "job",
      sessionID: sid as unknown as SessionID,
    }
    jobMap.set(job.id, info)
    if (sid !== job.id) jobMap.set(sid, info)
  }

  const children = yield* sessions.children(parentID).pipe(Effect.catch(() => Effect.succeed([] as Session.Info[])))
  const result = new Map<string, HybridInfo>()

  for (const [key, info] of jobMap.entries()) {
    result.set(key, info)
  }

  for (const child of children) {
    const cid = child.id as string
    const existing = result.get(cid)
    if (existing) {
      if (!existing.title && child.title) existing.title = child.title
      existing.source = "merged"
      const msgs = yield* MessageV2.filterCompactedEffect(child.id).pipe(
        Effect.provideService(Database.Service, database),
        Effect.catch(() => Effect.succeed([] as SessionV1.WithParts[])),
      )
      const dur = durableStatusFromMessages(msgs)
      const durableTail = tailFromMessages(msgs)
      if (existing.status !== "running" && dur === "running") {
        existing.status = "running"
        existing.tail = durableTail ?? existing.tail
        existing.source = "durable"
      } else if (!existing.tail && durableTail) {
        existing.tail = durableTail
      }
      const childElapsed = Math.floor((now - child.time.created) / 1000)
      if (childElapsed > existing.elapsed) existing.elapsed = childElapsed
      continue
    }
    const msgs = yield* MessageV2.filterCompactedEffect(child.id).pipe(
      Effect.provideService(Database.Service, database),
      Effect.catch(() => Effect.succeed([] as SessionV1.WithParts[])),
    )
    const dur = durableStatusFromMessages(msgs)
    result.set(cid, {
      id: cid,
      title: child.title,
      status: dur,
      started_at: child.time.created,
      elapsed: Math.floor((now - child.time.created) / 1000),
      tail: tailFromMessages(msgs),
      metadata: { parentSessionId: parentID, sessionId: cid } as Record<string, unknown>,
      source: "durable",
      sessionID: child.id,
    })
  }

  const deduped = new Map<string, HybridInfo>()
  for (const info of result.values()) {
    const key = info.sessionID as string
    if (!deduped.has(key)) deduped.set(key, info)
  }
  return Array.from(deduped.values()).toSorted((a, b) => a.started_at - b.started_at)
})

export const hybridRunning = Effect.fn("SubagentState.hybridRunning")(function* (parentID: SessionID) {
  const all = yield* hybridList(parentID)
  return all.filter((item) => item.status === "running")
})

export type HybridDeps = {
  background: BackgroundJob.Interface
  sessions: Session.Interface
  database: Database.Interface
}

export const hybridListWith = Effect.fn("SubagentState.hybridListWith")(function* (
  parentID: SessionID,
  deps: HybridDeps,
) {
  const { background, sessions, database } = deps
  const now = Date.now()
  const jobs = yield* background.list()
  const jobMap = new Map<string, HybridInfo>()
  for (const job of jobs) {
    const meta = job.metadata as Record<string, unknown> | undefined
    const parent = meta?.parentSessionId
    if (typeof parent !== "string" || parent !== parentID) continue
    const sid = (meta?.sessionId as string | undefined) ?? job.id
    const info: HybridInfo = {
      id: job.id,
      title: job.title,
      status: job.status as HybridStatus,
      started_at: job.started_at,
      elapsed: Math.floor((now - job.started_at) / 1000),
      tail: job.output ? job.output.trim().slice(-500) : job.error?.slice(-500),
      metadata: job.metadata,
      source: "job",
      sessionID: sid as unknown as SessionID,
    }
    jobMap.set(job.id, info)
    if (sid !== job.id) jobMap.set(sid, info)
  }
  const children = yield* sessions.children(parentID)
  const result = new Map<string, HybridInfo>()
  for (const [key, info] of jobMap.entries()) {
    result.set(key, info)
  }
  for (const child of children) {
    const cid = child.id as string
    const existing = result.get(cid)
    if (existing) {
      if (!existing.title && child.title) existing.title = child.title
      existing.source = "merged"
      const msgs = yield* MessageV2.filterCompactedEffect(child.id).pipe(
        Effect.provideService(Database.Service, database),
      )
      const dur = durableStatusFromMessages(msgs)
      const durableTail = tailFromMessages(msgs)
      if (existing.status !== "running" && dur === "running") {
        existing.status = "running"
        existing.tail = durableTail ?? existing.tail
        existing.source = "durable"
      } else if (!existing.tail && durableTail) {
        existing.tail = durableTail
      }
      const childElapsed = Math.floor((now - child.time.created) / 1000)
      if (childElapsed > existing.elapsed) existing.elapsed = childElapsed
      continue
    }
    const msgs = yield* MessageV2.filterCompactedEffect(child.id).pipe(
      Effect.provideService(Database.Service, database),
    )
    const dur = durableStatusFromMessages(msgs)
    result.set(cid, {
      id: cid,
      title: child.title,
      status: dur,
      started_at: child.time.created,
      elapsed: Math.floor((now - child.time.created) / 1000),
      tail: tailFromMessages(msgs),
      metadata: { parentSessionId: parentID, sessionId: cid } as Record<string, unknown>,
      source: "durable",
      sessionID: child.id,
    })
  }
  const deduped = new Map<string, HybridInfo>()
  for (const info of result.values()) {
    const key = info.sessionID as string
    if (!deduped.has(key)) deduped.set(key, info)
  }
  return Array.from(deduped.values()).toSorted((a, b) => a.started_at - b.started_at)
})

export const hybridRunningWith = Effect.fn("SubagentState.hybridRunningWith")(function* (
  parentID: SessionID,
  deps: HybridDeps,
) {
  const all = yield* hybridListWith(parentID, deps)
  return all.filter((item) => item.status === "running")
})

export * as SubagentState from "./subagent-state"
