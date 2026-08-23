
import { Effect } from "effect"
import { AppProcess } from "@opencode-ai/core/process"
import { ChildProcess } from "effect/unstable/process"
import { InstanceState } from "@/effect/instance-state"
import { Tool } from "@/tool/tool"
import os from "os"

const cekirdekEnv = () => ({
  ...process.env,
  LANG: "C",
  LC_ALL: "C",
  CI: "1",
})

const otoLine = (text: string) =>
  text
    .split("\n")
    .map((l) => l.trim())
    .filter(Boolean)[0] ?? "(cikti yok)"

const dosyaKomut = (dosya: string) => {
  if (dosya.endsWith(".py")) return ["python3", "-m", "py_compile", dosya] as string[]
  if (dosya.endsWith(".zig")) return ["zig", "ast-check", dosya] as string[]
  if (dosya.endsWith(".go")) return ["gofmt", "-e", "-l", dosya] as string[]
  if (dosya.endsWith(".ts") || dosya.endsWith(".tsx")) return ["tsc", "--noEmit", dosya] as string[]
  if (dosya.endsWith(".js") || dosya.endsWith(".mjs")) return ["node", "--check", dosya] as string[]
  return undefined
}

const dosyaDogrula = Effect.fn("Cekirdek.dosyaDogrula")(function* (dosya: string, cwd: string) {
  const appProcess = yield* AppProcess.Service
  const komut = dosyaKomut(dosya)
  if (!komut) return "OLDU"
  const r = yield* appProcess
    .run(ChildProcess.make(komut[0], komut.slice(1), { cwd, env: cekirdekEnv(), stdin: "ignore" }), {
      combineOutput: true,
      maxOutputBytes: 8192,
      timeout: "20 seconds",
    })
    .pipe(Effect.catch(() => Effect.succeed({ exitCode: 127, output: Buffer.from("") })))
  if (r.exitCode !== 0) {
    const text = r.output?.toString("utf8") ?? ""
    const konum = text.match(/([A-Za-z0-9_.\-/]+):(\d+)/)
    const hata = text.split("\n").find((l) => /error|SyntaxError|unexpected|expected|found/i.test(l)) ?? text.trim()
    const mesaj = hata.trim().slice(0, 200)
    return konum ? `OLMADI: ${konum[1]}:${konum[2]} — ${mesaj}` : `OLMADI: ${mesaj}`
  }
  return "OLDU"
})

const YAZANLAR = new Set(["write", "edit", "apply_patch", "patch", "hashline_write"])

const hedefDosyalar = (toolId: string, args: Record<string, unknown>) => {
  const yazma = YAZANLAR.has(toolId) || typeof args.content === "string"
  if (!yazma) return []
  for (const anahtar of ["filePath", "file", "path"] as string[]) {
    const deger = args[anahtar]
    if (typeof deger === "string") return [deger]
  }
  const text = args.patchText
  if (typeof text === "string") {
    return Array.from(text.matchAll(/(?:Update File|Add File|Move to|Rename from|Delete File):\s*(\S+)/g)).map(
      (m) => m[1],
    )
  }
  return [] as string[]
}

export const cekirdekSonuc = Effect.fn("Cekirdek.sonuc")(function* (input: {
  toolId: string
  args: Record<string, unknown>
  result: Tool.ExecuteResult
}) {
  const ham = input.result.output
  const instance = yield* InstanceState.context
  const cwd = instance.directory
  if (input.toolId === "bash") {
    const exit = input.result.metadata?.exit
    if (typeof exit !== "number") return undefined
    return exit === 0 ? "OLDU" : `OLMADI: (exit ${exit}): ${otoLine(ham)}`
  }
  const dosyalar = hedefDosyalar(input.toolId, input.args)
  if (dosyalar.length > 0) {
    for (const dosya of dosyalar) {
      const v = yield* dosyaDogrula(dosya, cwd)
      if (v !== "OLDU") return v
    }
    return "OLDU"
  }
  return `OLDU — ${otoLine(ham).slice(0, 300)}`
})

export const invoke = Effect.fn("Cekirdek.invoke")(function* (input: {
  item: Tool.Def
  args: Record<string, unknown>
  ctx: Tool.Context
}) {
  const result = yield* input.item.execute(input.args, input.ctx)
  const v = yield* cekirdekSonuc({
    toolId: input.item.id,
    args: input.args,
    result,
  })
  if (v === undefined) return result
  return {
    ...result,
    title: v.split("\n")[0].slice(0, 80),
    output: v,
    metadata: { ...result.metadata, hamCikti: result.output },
  }
})

export * as Cekirdek from "./invoke"
