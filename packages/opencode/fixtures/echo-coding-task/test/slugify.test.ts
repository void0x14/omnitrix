import { expect, test } from "bun:test"
import { slugify } from "../src/slugify"

test("slugify normalizes whitespace and punctuation", () => {
  expect(slugify("  Echo / Agent  ")).toBe("echo-agent")
  expect(slugify("Türkçe teknik görev")).toBe("türkçe-teknik-görev")
})
