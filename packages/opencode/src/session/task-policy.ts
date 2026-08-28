const PACKAGE_STRING_MAX = 1024

export type Clause = {
  index: number
  start: number
  content: string
}

export type PolicyRule = {
  mode: string
  rule: string
  pattern: string
  priority: number
}

export type Candidate = {
  mode: string
  rule: string
  text: string
  clause_index: number
  clause: string
  start: number
  end: number
  priority: number
}

export type ContractResolution = {
  mode: string
  winner: (Candidate & { priority: number }) | null
  evidence: {
    resolution: string
    candidate_count: number
    candidates: Array<Record<string, unknown>>
    winner: Record<string, unknown> | null
  }
}

export function contractText(text: unknown): string {
  let value = String(text)
  value = value.replace(/```[\s\S]*?```|~~~[\s\S]*?~~~/g, " ")
  value = value.replace(/`[^`\n]*`|"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|\u201c[^\u201d]*\u201d|\u2018[^\u2019]*\u2019/g, " ")
  value = value
    .split("\n")
    .map((line) => (line.replace(/^\s+/, "").startsWith(">") ? "" : line))
    .join("\n")
  const folded = value.toLowerCase()
  const normalized = folded.normalize("NFKD")
  const stripped = Array.from(normalized)
    .filter((character) => !/\p{M}/u.test(character))
    .join("")
  return stripped.replace(/\u0131/g, "i")
}

export function contractClauses(objective: unknown): { normalized: string; clauses: Clause[] } {
  const normalized = contractText(objective)
  const clauses: Clause[] = []
  for (const match of normalized.matchAll(/[^.!?;\n]+/g)) {
    const raw = match[0]
    const leading = raw.length - raw.replace(/^\s+/, "").length
    const content = raw.trim()
    if (content) clauses.push({ index: clauses.length, start: (match.index ?? 0) + leading, content: content })
  }
  return { normalized: normalized, clauses: clauses }
}

function tupleGreater(a: [number, number], b: [number, number]): boolean {
  if (a[0] !== b[0]) return a[0] > b[0]
  return a[1] > b[1]
}

export function resolveContract(normalized: string, clauses: Clause[], rules: PolicyRule[], defaultMode: string, inherited: [string, string] | null): ContractResolution {
  const candidates: Candidate[] = []
  for (const clause of clauses) {
    for (const rule of rules) {
      const pattern = new RegExp(rule.pattern, "g")
      for (const match of clause.content.matchAll(pattern)) {
        const start = clause.start + (match.index ?? 0)
        candidates.push({
          mode: rule.mode,
          rule: rule.rule,
          text: match[0],
          clause_index: clause.index,
          clause: clause.content.slice(0, PACKAGE_STRING_MAX),
          start: start,
          end: start + match[0].length,
          priority: rule.priority,
        })
      }
    }
  }
  const effective: Candidate[] = []
  for (const candidate of candidates) {
    let suppressed = false
    for (const other of candidates) {
      if (other === candidate || other.clause_index !== candidate.clause_index) continue
      const contains = other.start <= candidate.start && other.end >= candidate.end
      const moreSpecific = tupleGreater([other.end - other.start, other.priority], [candidate.end - candidate.start, candidate.priority])
      if (contains && moreSpecific) {
        suppressed = true
        break
      }
    }
    if (!suppressed) effective.push(candidate)
  }
  effective.sort((a, b) => {
    if (a.clause_index !== b.clause_index) return a.clause_index - b.clause_index
    if (a.start !== b.start) return a.start - b.start
    if (a.end !== b.end) return a.end - b.end
    if (a.priority !== b.priority) return a.priority - b.priority
    return a.rule < b.rule ? -1 : a.rule > b.rule ? 1 : 0
  })
  let winner: Candidate | null
  let evidenceCandidates: Candidate[]
  let resolution: string
  if (inherited) {
    winner = {
      mode: inherited[0],
      rule: inherited[1],
      text: "inherited",
      clause_index: clauses.length,
      clause: "",
      start: normalized.length,
      end: normalized.length,
      priority: 100,
    }
    evidenceCandidates = effective.slice(-15).concat([winner])
    resolution = inherited[1] + "_override"
  } else if (effective.length) {
    winner = effective[effective.length - 1]
    evidenceCandidates = effective.slice(-16)
    resolution = "last_explicit_directive"
  } else {
    winner = null
    evidenceCandidates = []
    resolution = "default_" + defaultMode
  }
  const mode = winner ? winner.mode : defaultMode
  const evidence = evidenceCandidates.map((item) => ({
    mode: item.mode,
    rule: item.rule,
    text: item.text,
    clause_index: item.clause_index,
    clause: item.clause,
    start: item.start,
    end: item.end,
  }))
  return {
    mode: mode,
    winner: winner,
    evidence: {
      resolution: resolution,
      candidate_count: effective.length + (inherited ? 1 : 0),
      candidates: evidence,
      winner: evidence.length ? evidence[evidence.length - 1] : null,
    },
  }
}

export const READ_RULES: PolicyRule[] = [
  { mode: "write_allowed", rule: "en_disable_read_only", pattern: String.raw`\b(?:do\s+not|don['\u2019]?t)\s+use\s+(?:the\s+)?read[\s-]+only(?:\s+mode)?\b`, priority: 30 },
  { mode: "write_allowed", rule: "en_read_only_not_required", pattern: String.raw`\bread[\s-]+only(?:\s+(?:mode|task))?\s+(?:is\s+)?not\s+(?:required|requested|enabled|wanted)\b`, priority: 30 },
  { mode: "write_allowed", rule: "en_not_read_only_task", pattern: String.raw`\b(?:this|the)\s+(?:task\s+)?is\s+not\s+(?:a\s+)?read[\s-]+only(?:\s+(?:task|mode))?\b`, priority: 30 },
  { mode: "write_allowed", rule: "en_not_read_only", pattern: String.raw`\bnot\s+(?:a\s+)?read[\s-]+only(?:\s+(?:task|mode))?\b`, priority: 25 },
  { mode: "write_allowed", rule: "tr_not_read_only", pattern: String.raw`\bsalt\s+okunur\s+(?:olmayan|degil|olmasin)(?:\s+(?:bir\s+)?(?:gorev|mod))?\b`, priority: 30 },
  { mode: "write_allowed", rule: "en_make_changes", pattern: String.raw`\b(?:make|apply)\s+(?:the\s+)?(?:file\s+)?changes?\b`, priority: 20 },
  { mode: "write_allowed", rule: "en_write_files", pattern: String.raw`\b(?:write|modify|change)\s+(?:(?:to|any|the)\s+){0,2}files?\b`, priority: 20 },
  { mode: "write_allowed", rule: "tr_write_files", pattern: String.raw`\b(?:dosyayi|dosyalari)\s+(?:degistir|yaz)\b`, priority: 20 },
  { mode: "write_allowed", rule: "tr_make_changes", pattern: String.raw`\bdegisiklik(?:leri)?\s+yap\b`, priority: 20 },
  { mode: "write_allowed", rule: "tr_write_allowed", pattern: String.raw`\byazilabilir\s+(?:bir\s+)?(?:gorev|mod)\b`, priority: 20 },
  { mode: "read_only", rule: "tr_hicbir_dosya_degistirme", pattern: String.raw`\bhicbir\s+dosyay?i?\s+degistirme(?:yin)?\b`, priority: 20 },
  { mode: "read_only", rule: "tr_dosya_degistirme", pattern: String.raw`\b(?:dosyayi|dosyalari)\s+(?:degistirme(?:yin)?|yazma(?:yin)?)\b`, priority: 20 },
  { mode: "read_only", rule: "en_do_not_modify_files", pattern: String.raw`\b(?:do\s+not|don['\u2019]?t)\s+(?:modify|change|write)\s+(?:(?:any|the)\s+)?files?\b`, priority: 20 },
  { mode: "read_only", rule: "en_no_file_changes", pattern: String.raw`\b(?:make\s+)?no\s+file\s+changes?\b`, priority: 20 },
  { mode: "read_only", rule: "tr_salt_okunur", pattern: String.raw`\bsalt\s+okunur(?:\s+(?:bir\s+)?(?:gorev|mod|inceleme))?\b`, priority: 10 },
  { mode: "read_only", rule: "en_read_only", pattern: String.raw`\bread[\s-]+only(?:\s+(?:task|mode|review))?\b`, priority: 10 },
]

export const NETWORK_RULES: PolicyRule[] = [
  { mode: "web_forbidden", rule: "tr_no_web_search", pattern: String.raw`\bweb\s+aramasi\s+yapma(?:yin)?\b`, priority: 30 },
  { mode: "web_forbidden", rule: "tr_no_web", pattern: String.raw`\bwebi?\s+kullanma(?:yin)?\b`, priority: 30 },
  { mode: "web_forbidden", rule: "tr_no_web_tools", pattern: String.raw`\bweb_search\s*(?:(?:/|veya|ve)\s*)fetch_url\s+kullanma(?:yin)?\b`, priority: 35 },
  { mode: "web_forbidden", rule: "en_no_web", pattern: String.raw`\b(?:do\s+not|don['\u2019]?t)\s+use\s+(?:the\s+)?(?:web(?:\s+search)?|web_search(?:\s*(?:/|or|and)\s*fetch_url)?)\b`, priority: 30 },
  { mode: "web_forbidden", rule: "en_no_web_search", pattern: String.raw`\bno\s+web\s+search\b`, priority: 30 },
  { mode: "web_allowed", rule: "tr_use_web_search", pattern: String.raw`\bweb\s+aramasi\s+yap\b`, priority: 20 },
  { mode: "web_allowed", rule: "tr_use_web", pattern: String.raw`\bwebi?\s+kullan\b`, priority: 20 },
  { mode: "web_allowed", rule: "tr_search_internet", pattern: String.raw`\binternetten\s+ara\b`, priority: 20 },
  { mode: "web_allowed", rule: "en_web_search_allowed", pattern: String.raw`\bweb\s+search\s+(?:is\s+)?allowed\b`, priority: 25 },
  { mode: "web_allowed", rule: "en_use_web", pattern: String.raw`\buse\s+(?:the\s+)?web(?:\s+search)?\b`, priority: 20 },
  { mode: "web_allowed", rule: "en_search_web", pattern: String.raw`\bsearch\s+(?:the\s+)?web\b`, priority: 20 },
]

export type TaskPolicy = Record<string, unknown>

export function deriveTaskPolicy(objective: unknown, inheritedReadOnly = false, inheritedWebForbidden = false): TaskPolicy {
  const { normalized, clauses } = contractClauses(objective)
  const read = resolveContract(normalized, clauses, READ_RULES, "write_allowed", inheritedReadOnly ? ["read_only", "inherited_read_only"] : null)
  const network = resolveContract(normalized, clauses, NETWORK_RULES, "web_allowed", inheritedWebForbidden ? ["web_forbidden", "inherited_web_forbidden"] : null)
  return {
    mode: read.mode,
    read_only: read.mode === "read_only",
    matched_rule: read.winner ? read.winner.rule : null,
    matched_text: read.winner ? read.winner.text : "",
    inherited_read_only: Boolean(inheritedReadOnly),
    network_mode: network.mode,
    web_forbidden: network.mode === "web_forbidden",
    network_matched_rule: network.winner ? network.winner.rule : null,
    network_matched_text: network.winner ? network.winner.text : "",
    inherited_web_forbidden: Boolean(inheritedWebForbidden),
    parser_version: 3,
    policy_evidence: read.evidence,
    network_policy_evidence: network.evidence,
  }
}
