import { Duration, Effect, Schema } from "effect"
import { ChildProcess } from "effect/unstable/process"
import { AppProcess } from "@opencode-ai/core/process"
import { FSUtil } from "@opencode-ai/core/fs-util"
import os from "os"
import path from "path"
import * as Tool from "./tool"
import { Git } from "../git"
import { InstanceState } from "@/effect/instance-state"
import DESCRIPTION from "./verify.txt"

export const Parameters = Schema.Struct({
  eylem: Schema.String.annotate({
    description:
      "Ne istendigi dogal dilde (bayrak yok). Ornek: 'zig build derlensin ve testler gecsin', 'src/main.zig degismeli'",
  }),
  kosul: Schema.optional(
    Schema.String.annotate({
      description:
        "Uretilen kodda bulunmasi zorunlu sembol/kosul. Ornek: 'TaskGroup'. Kod calissa bile kosul yoksa sonuc OLMADI olur.",
    }),
  ),
  komut: Schema.optional(
    Schema.String.annotate({
      description: "Calisma katmani icin kosulacak komut. Ornek: 'python main.py'",
    }),
  ),
})

type Katman = {
  katman: number
  ad: string
  komut: string
  exit: number | undefined
  sonuc: "geciyor" | "basarisiz" | "atlandi"
  satir: string
}

type Metadata = {
  dil: string | undefined
  katmanlar: Katman[]
  degisenDosyalar: string[]
}

const stripAnsi = (text: string) => text.replace(/\u001b\[[0-9;]*[A-Za-z]/g, "")

const satirlar = (text: string) =>
  stripAnsi(text)
    .split(/\r?\n/)
    .map((satir) => satir.trim())
    .filter(Boolean)

const ilkSatir = (text: string) => {
  const lines = satirlar(text)
  const hata = lines.find((satir) => /(error|failed|panic|hata|timed out)/i.test(satir))
  return (hata ?? lines[0] ?? "(cikti yok)").replace(/^(error|hata|failed|failure)\s*[:—-]\s*/i, "").slice(0, 240)
}

const pytestNeden = (text: string) => {
  const lines = satirlar(text)
  const fail = lines.find((satir) => satir.startsWith("FAILED"))
  const hata = lines.find((satir) => satir.startsWith("E ") || satir.startsWith("> "))
  const ozet = lines.find((satir) => /^\d+ failed/i.test(satir))
  const adet = ozet?.match(/^\d+/)?.[0] ?? "?"
  const hedef = fail ? fail.slice("FAILED".length).trim() : "test"
  return `testte ${adet} hata — ilk hata: ${hedef}${hata ? ` — ${hata.slice(2).trim()}` : ""}`
}

const neden = (numara: number, ad: string, exit: number, text: string, dil: string | undefined) => {
  if (numara === 4) return `komut basarisiz (exit ${exit}): ${ilkSatir(text)}`
  if (numara === 3 && dil === "python") return pytestNeden(text)
  if (numara === 0 && dil === "python") {
    const konum = text.match(/File "([^"]+)", line (\d+)/)
    if (konum) return `${ad} hatasi: ${konum[1]}:${konum[2]} — ${ilkSatir(text)}`
  }
  return `${ad} hatasi: ${ilkSatir(text)}`
}

const tirnakliKomut = (eylem: string) => {
  const tirnak = eylem.match(/["'`]([^"'`]+)["'`]/)
  if (tirnak?.[1]) return tirnak[1]
  const emir = eylem.match(/(?:calistir|kos|run|execute)\s*[:=]\s*(.+)$/i)
  return emir?.[1]?.trim()
}

const dosyaBahsi = (eylem: string) =>
  Array.from(
    eylem.matchAll(/[A-Za-z0-9_.\-/]+\.(?:zig|rs|py|go|ts|tsx|js|jsx|mjs|cjs|sh|toml|json|yaml|yml|md|c|h|cpp|hpp)\b/g),
  ).map((m) => m[0].replace(/[),;]+$/, ""))

const uzantili = (dosyalar: string[], uzanti: string) => dosyalar.filter((dosya) => dosya.endsWith(uzanti))

export const VerifyTool = Tool.define<
  typeof Parameters,
  Metadata,
  Git.Service | FSUtil.Service | AppProcess.Service
>("verify", Effect.gen(function* () {
  const git = yield* Git.Service
  const fs = yield* FSUtil.Service
  const appProcess = yield* AppProcess.Service

  const py = process.platform === "win32" ? "python" : "python3"
  const pycache = path.join(os.tmpdir(), "opencode-verify-pyc")
  const env = {
    ...process.env,
    LANG: "C",
    LC_ALL: "C",
    CI: "1",
    CARGO_TERM_COLOR: "never",
    PYTHONDONTWRITEBYTECODE: "1",
    PYTHONPYCACHEPREFIX: pycache,
  }

  const dosyaVar = Effect.fn("Verify.dosyaVar")(function* (cwd: string, ad: string) {
    return yield* fs.existsSafe(path.join(cwd, ad))
  })

  const globVar = Effect.fn("Verify.globVar")(function* (cwd: string, kalip: string) {
    return yield* fs.glob(kalip, { cwd }).pipe(Effect.catch(() => Effect.succeed([] as string[])))
  })

  const dilTespit = Effect.fn("Verify.dilTespit")(function* (cwd: string) {
    if ((yield* dosyaVar(cwd, "build.zig")) || (yield* dosyaVar(cwd, "build.zig.zon"))) return "zig"
    if (yield* dosyaVar(cwd, "Cargo.toml")) return "rust"
    if (yield* dosyaVar(cwd, "go.mod")) return "go"
    if (yield* dosyaVar(cwd, "package.json")) return (yield* dosyaVar(cwd, "tsconfig.json")) ? "ts" : "js"
    if (
      (yield* dosyaVar(cwd, "pyproject.toml")) ||
      (yield* dosyaVar(cwd, "requirements.txt")) ||
      (yield* dosyaVar(cwd, "setup.py"))
    )
      return "python"
    if ((yield* globVar(cwd, "*.py")).length) return "python"
    if ((yield* globVar(cwd, "*.zig")).length) return "zig"
    return undefined
  })

  const katman = Effect.fn("Verify.katman")(function* (cwd: string, komut: string[], timeout: Duration.Input) {
    const result = yield* appProcess
      .run(ChildProcess.make(komut[0], komut.slice(1), { cwd, env, stdin: "ignore" }), {
        combineOutput: true,
        maxOutputBytes: 1024 * 1024,
        timeout,
      })
      .pipe(
        Effect.catch((error) => {
          const mesaj = error.cause instanceof Error ? error.cause.message : error.message
          return Effect.succeed({
            exitCode: /timed out/i.test(mesaj) ? 124 : 127,
            output: Buffer.from(mesaj, "utf8"),
          })
        }),
      )
    return { exit: result.exitCode, text: result.output?.toString("utf8") ?? "" }
  })

  const kosulAra = Effect.fn("Verify.kosulAra")(function* (cwd: string, kosul: string, dosyalar: string[]) {
    for (const dosya of dosyalar) {
      const icerik = yield* fs.readFileStringSafe(path.join(cwd, dosya)).pipe(
        Effect.catch(() => Effect.succeed(undefined)),
      )
      if (icerik?.includes(kosul)) return true
    }
    return false
  })

  return {
    description: DESCRIPTION,
    parameters: Parameters,
    execute: (params: Schema.Schema.Type<typeof Parameters>) =>
      Effect.gen(function* () {
        const instance = yield* InstanceState.context
        const cwd = instance.directory
        yield* fs.ensureDir(pycache).pipe(Effect.catch(() => Effect.void))

        const gitVarmi = (yield* git.run(["rev-parse", "--git-dir"], { cwd })).exitCode === 0
        const degisen = gitVarmi ? (yield* git.status(cwd)).map((item) => item.file) : []
        const dil = yield* dilTespit(cwd)

        const testIsteniyor = /test/i.test(params.eylem)
        const lintIsteniyor = /(lint|clippy|ruff|mypy|fmt|format)/i.test(params.eylem)
        const komut = params.komut ?? tirnakliKomut(params.eylem)

        const zigs = uzantili(degisen, ".zig")
        const pys = uzantili(degisen, ".py")
        const gos = uzantili(degisen, ".go")

        const tablo: Record<number, string[] | undefined> = {}
        if (dil === "zig") {
          tablo[1] = ["zig", "build"]
          tablo[2] = lintIsteniyor && zigs.length ? ["zig", "fmt", "--check", ...zigs] : undefined
          tablo[3] = testIsteniyor ? ["zig", "build", "test"] : undefined
        }
        if (dil === "rust") {
          tablo[1] = ["cargo", "check"]
          tablo[2] = lintIsteniyor ? ["cargo", "clippy", "--", "-D", "warnings"] : undefined
          tablo[3] = testIsteniyor ? ["cargo", "test"] : undefined
        }
        if (dil === "go") {
          tablo[0] = gos.length ? ["gofmt", "-e", "-l", ...gos] : undefined
          tablo[1] = ["go", "build", "./..."]
          tablo[3] = testIsteniyor ? ["go", "test", "./..."] : undefined
        }
        if (dil === "python") {
          tablo[0] = pys.length ? [py, "-m", "py_compile", ...pys] : undefined
          tablo[2] = lintIsteniyor && pys.length ? ["ruff", "check", ...pys] : undefined
          tablo[3] = testIsteniyor ? ["pytest", "-x", "-q", "-p", "no:cacheprovider"] : undefined
        }
        if (dil === "ts" || dil === "js") {
          tablo[1] = dil === "ts" ? ["tsc", "--noEmit"] : undefined
          tablo[3] = testIsteniyor ? ["bun", "test"] : undefined
        }

        const katmanlar: Katman[] = []

        const kos = (numara: number, ad: string, calistir: string[] | undefined, timeout: Duration.Input) =>
          Effect.gen(function* () {
            if (!calistir) {
              katmanlar.push({ katman: numara, ad, komut: "", exit: undefined, sonuc: "atlandi", satir: "" })
              return undefined
            }
            const satir = calistir.join(" ")
            const r = yield* katman(cwd, calistir, timeout)
            if (r.exit === 127) {
              const kayit = {
                katman: numara,
                ad,
                komut: satir,
                exit: 127,
                sonuc: "basarisiz" as const,
                satir: `${ad} calistirilamadi — arac yok: ${calistir[0]}`,
              }
              katmanlar.push(kayit)
              return kayit
            }
            if (r.exit === 124) {
              const kayit = {
                katman: numara,
                ad,
                komut: satir,
                exit: 124,
                sonuc: "basarisiz" as const,
                satir: `${ad} zaman asimi`,
              }
              katmanlar.push(kayit)
              return kayit
            }
            if (r.exit !== 0) {
              const kayit = {
                katman: numara,
                ad,
                komut: satir,
                exit: r.exit,
                sonuc: "basarisiz" as const,
                satir: neden(numara, ad, r.exit, r.text, dil),
              }
              katmanlar.push(kayit)
              return kayit
            }
            katmanlar.push({ katman: numara, ad, komut: satir, exit: 0, sonuc: "geciyor", satir: "" })
            return undefined
          })

        const sonuc = (output: string): Tool.ExecuteResult<Metadata> => ({
          title: output === "OLDU" ? "OLDU" : "OLMADI",
          output,
          metadata: { dil, katmanlar, degisenDosyalar: degisen },
        })

        if (testIsteniyor) {
          if (dil === "python") {
            const varMi =
              (yield* globVar(cwd, "test_*.py")).length +
                (yield* globVar(cwd, "*_test.py")).length >
                0 || (yield* dosyaVar(cwd, "tests"))
            if (!varMi) return sonuc("OLMADI: test dosyasi bulunamadi")
          }
          if (dil === "ts" || dil === "js") {
            const varMi =
              (yield* globVar(cwd, "**/*.test.ts")).length +
                (yield* globVar(cwd, "**/*.test.js")).length +
                (yield* globVar(cwd, "**/*.test.tsx")).length +
                (yield* globVar(cwd, "**/*.test.jsx")).length >
              0
            if (!varMi) return sonuc("OLMADI: test dosyasi bulunamadi")
          }
          if (dil === "go") {
            const varMi = (yield* globVar(cwd, "**/*_test.go")).length > 0
            if (!varMi) return sonuc("OLMADI: test dosyasi bulunamadi")
          }
          if (!dil) return sonuc("OLMADI: dil tespit edilemedi, test komutu bilinmiyor")
        }

        if (dil === "zig") {
          for (const dosya of zigs) {
            const hata = yield* kos(0, "sozdizimi", ["zig", "ast-check", dosya], "15 seconds")
            if (hata) return sonuc(`OLMADI: ${hata.satir}`)
          }
        }

        const hata0 = yield* kos(0, "sozdizimi", dil === "zig" ? undefined : tablo[0], "15 seconds")
        if (hata0) return sonuc(`OLMADI: ${hata0.satir}`)
        const hata1 = yield* kos(1, "derleme", tablo[1], "120 seconds")
        if (hata1) return sonuc(`OLMADI: ${hata1.satir}`)
        const hata2 = yield* kos(2, "lint", tablo[2], "120 seconds")
        if (hata2) return sonuc(`OLMADI: ${hata2.satir}`)
        const hata3 = yield* kos(3, "test", tablo[3], "180 seconds")
        if (hata3) return sonuc(`OLMADI: ${hata3.satir}`)
        const hata4 = yield* kos(4, "calisma", komut ? ["sh", "-c", komut] : undefined, "60 seconds")
        if (hata4) return sonuc(`OLMADI: ${hata4.satir}`)

        for (const bahis of dosyaBahsi(params.eylem)) {
          if (!gitVarmi) return sonuc("OLMADI: git deposu yok, dosya degisimi dogrulanamadi")
          const degisti = degisen.some((dosya) => dosya === bahis || path.basename(dosya) === path.basename(bahis))
          if (!degisti) return sonuc(`OLMADI: beklenen dosya degismemis: ${bahis}`)
        }
        if (params.kosul) {
          if (!gitVarmi) return sonuc(`OLMADI: git deposu yok, kosul dogrulanamadi: ${params.kosul}`)
          const varMi = yield* kosulAra(cwd, params.kosul, degisen)
          if (!varMi) return sonuc(`OLMADI: istenen ${params.kosul} bulunamadi (degisen dosyalarda yok)`)
        }

        return sonuc("OLDU")
      }),
  } satisfies Tool.DefWithoutID<typeof Parameters, Metadata>
  }),
)
