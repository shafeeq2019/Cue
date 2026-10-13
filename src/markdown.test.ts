import { describe, expect, it } from "vitest";
import { parseBlocks, parseInline } from "./markdown";

describe("markdown", () => {
  it("parses code fences, including unterminated ones while streaming", () => {
    const b = parseBlocks("Intro\n\n```ts\nconst a = 1;\n```\nafter");
    expect(b.map((x) => x.t)).toEqual(["p", "code", "p"]);
    expect(b[1]).toEqual({ t: "code", lang: "ts", v: "const a = 1;" });
    const s = parseBlocks("```py\nprint(1)");
    expect(s).toEqual([{ t: "code", lang: "py", v: "print(1)" }]);
  });

  it("parses lists, headings and quotes", () => {
    const b = parseBlocks("# Title\n- a\n- b\n\n3. x\n4. y\n> quote");
    expect(b.map((x) => x.t)).toEqual(["h", "ul", "ol", "quote"]);
    expect(b[2]).toMatchObject({ t: "ol", start: 3 });
  });

  it("parses inline styles without treating snake_case as emphasis", () => {
    expect(parseInline("a **b** `c` *d*")).toEqual([
      { t: "text", v: "a " },
      { t: "strong", c: [{ t: "text", v: "b" }] },
      { t: "text", v: " " },
      { t: "code", v: "c" },
      { t: "text", v: " " },
      { t: "em", c: [{ t: "text", v: "d" }] },
    ]);
    expect(parseInline("snake_case_name")).toEqual([{ t: "text", v: "snake_case_name" }]);
    expect(parseInline("[docs](https://x.y)")).toEqual([{ t: "link", text: "docs", href: "https://x.y" }]);
  });

  it("keeps HTML as plain text", () => {
    expect(parseInline("<img src=x onerror=alert(1)>")).toEqual([{ t: "text", v: "<img src=x onerror=alert(1)>" }]);
  });
});
