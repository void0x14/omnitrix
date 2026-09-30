export * as PermissionV1 from "./permission"

import { Schema } from "effect"
export * from "@opencode-ai/schema/permission-v1"
import { ID } from "@opencode-ai/schema/permission-v1"

export class RejectedError extends Schema.TaggedErrorClass<RejectedError>()("PermissionRejectedError", {}) {
  override get message() {
    return "The user rejected permission to use this specific tool call."
  }
}

export class CorrectedError extends Schema.TaggedErrorClass<CorrectedError>()("PermissionCorrectedError", {
  feedback: Schema.String,
}) {
  override get message() {
    return `The user rejected permission to use this specific tool call with the following feedback: ${this.feedback}`
  }
}

export class DeniedError extends Schema.TaggedErrorClass<DeniedError>()("PermissionDeniedError", {
  ruleset: Schema.Any,
}) {
  override get message() {
    const rules = Array.isArray(this.ruleset) ? this.ruleset : []
    const actions = [
      ...new Set(
        rules
          .filter((rule): rule is Record<string, unknown> => typeof rule === "object" && rule !== null)
          .map((rule) => `${String(rule.permission ?? "permission")}:${String(rule.action ?? "deny")}`),
      ),
    ]
    const summary = actions.length > 0 ? ` Matching policy actions: ${actions.slice(0, 4).join(", ")}.` : ""
    return `Tool call denied by permission policy.${summary} Do not retry the same call without changing its target or permission context.`
  }
}

export class NotFoundError extends Schema.TaggedErrorClass<NotFoundError>()("Permission.NotFoundError", {
  requestID: ID,
}) {}

export type Error = DeniedError | RejectedError | CorrectedError
