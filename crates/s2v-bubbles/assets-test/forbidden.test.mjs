import { test } from "node:test";
import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import { checkJs, checkHtml } from "./forbidden.mjs";

test("forbidden_checker_detects_violations", () => {
  for (const bad of ["a.innerHTML = x", "e.outerHTML=1", "e.insertAdjacentHTML('a', s)", "document.write(s)",
    "eval(s)", "new Function('x')", "setTimeout('f()', 1)", "setInterval(\"f()\", 1)",
    "e.setAttribute('style', 'x')", "e.setAttribute(\"onclick\", f)", "e.style.cssText = s"]) {
    assert.ok(checkJs(bad).length > 0, bad);
  }
  for (const bad of ["<div onclick=\"x\">", "<DIV\nONLOAD=x>", "<p style = \"a\">", "<STYLE>p{}</STYLE>",
    "<script>alert(1)</script>", "<script src=\"https://cdn.example/x.js\"></script>", "<script src=\"//cdn.example/x.js\"></script>",
    "<link rel=\"stylesheet\" href=\"https://cdn.example/x.css\">", "<link href=\"//cdn/x.css\">", "<img src=\"https://x/y.png\">"]) {
    assert.ok(checkHtml(bad).length > 0, bad);
  }
});

test("forbidden_checker_allows_good_code", () => {
  for (const ok of ["e.style.left = '1px'", "e.textContent = s", "setTimeout(() => f(), 300)",
    "// innerHTML は使わない(コメント)", "/* eval( */ const a = 1;", "e.addEventListener('click', f)"]) {
    assert.deepEqual(checkJs(ok), [], ok);
  }
  for (const ok of ["<script type=\"module\" src=\"/app.js\"></script>", "<link rel=\"stylesheet\" href=\"/app.css\">",
    "<p>style や onclick という言葉は本文なら可</p>", "<button id=\"save\" type=\"button\">保存</button>"]) {
    assert.deepEqual(checkHtml(ok), [], ok);
  }
});

test("assets_have_no_dangerous_apis", () => {
  const dir = new URL("../src/assets/", import.meta.url);
  for (const f of readdirSync(dir)) {
    const s = readFileSync(new URL(f, dir), "utf8");
    if (f.endsWith(".js")) assert.deepEqual(checkJs(s), [], f);
    if (f.endsWith(".html")) assert.deepEqual(checkHtml(s), [], f);
  }
});
