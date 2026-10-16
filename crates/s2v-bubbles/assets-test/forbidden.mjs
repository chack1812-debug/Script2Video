// 画面の JS・HTML が CSP の下で禁止されている書き方をしていないかの検査(Node 専用)。
// 規則は設計書 §5.3 と実装計画 2d-2 の Global Constraints の「禁止」。

// JS のコメント(行コメントとブロックコメント)を取り除く。文字列・テンプレートの中身は残す簡易な走査。
function stripJsComments(src) {
  let out = "";
  let i = 0;
  while (i < src.length) {
    const c = src[i];
    const n = src[i + 1];
    if (c === "/" && n === "/") {
      while (i < src.length && src[i] !== "\n") i++;
    } else if (c === "/" && n === "*") {
      const end = src.indexOf("*/", i + 2);
      i = end < 0 ? src.length : end + 2;
      out += " ";
    } else if (c === '"' || c === "'" || c === "`") {
      // 文字列は残す。' と " は改行で打ち切る(正規表現リテラルの引用符での巻き込み対策)
      let j = i + 1;
      while (j < src.length && src[j] !== c) {
        if (src[j] === "\\") j++;
        else if (c !== "`" && src[j] === "\n") break;
        j++;
      }
      out += src.slice(i, j + 1);
      i = j + 1;
    } else {
      out += c;
      i++;
    }
  }
  return out;
}

const JS_RULES = [
  [/\binnerHTML\b/, "innerHTML"],
  [/\bouterHTML\b/, "outerHTML"],
  [/\binsertAdjacentHTML\b/, "insertAdjacentHTML"],
  [/\bdocument\s*\.\s*write(ln)?\b/, "document.write"],
  [/\beval\s*\(/, "eval"],
  [/\bnew\s+Function\b/, "new Function"],
  [/\b(setTimeout|setInterval)\s*\(\s*['"`]/, "文字列を渡す setTimeout/setInterval"],
  [/\bsetAttribute\s*\(\s*['"`]\s*style\s*['"`]/i, 'setAttribute("style")'],
  [/\bsetAttribute\s*\(\s*['"`]\s*on[a-z]+\s*['"`]/i, 'setAttribute("on…")'],
  [/\.\s*style\s*\.\s*cssText\b/, ".style.cssText"],
];

/** JS のソースの違反を文字列の配列で返す(無ければ空)。 */
export function checkJs(source) {
  const s = stripJsComments(source);
  return JS_RULES.filter(([re]) => re.test(s)).map(([, name]) => `禁止: ${name}`);
}

const TAG_RE = /<\/?[a-zA-Z](?:"[^"]*"|'[^']*'|[^>"'])*>/g;
const ATTR_RE = /([^\s"'=<>\/]+)(?:\s*=\s*("[^"]*"|'[^']*'|[^\s"'>]+))?/g;

/** HTML のソースの違反を文字列の配列で返す(無ければ空)。タグの中だけを見る。 */
export function checkHtml(source) {
  const out = [];
  const s = source.replace(/<!--[\s\S]*?-->/g, " ");
  for (const m of s.matchAll(TAG_RE)) {
    const tag = m[0];
    if (tag.startsWith("</")) continue;
    const nameMatch = /^<([a-zA-Z][^\s>\/]*)/.exec(tag);
    const name = nameMatch[1].toLowerCase();
    if (name === "style") out.push("禁止: <style>");
    const attrs = new Map();
    for (const a of tag.slice(nameMatch[0].length).matchAll(ATTR_RE)) {
      const key = a[1].toLowerCase();
      const val = a[2] === undefined ? "" : a[2].replace(/^["']|["']$/g, "");
      if (!attrs.has(key)) attrs.set(key, val);
      if ((key === "src" || key === "href") && (!val.startsWith("/") || val.startsWith("//"))) {
        out.push(`禁止: 同じオリジンでない ${key}="${val}" (<${name}>)`);
      }
      if (key === "style") out.push(`禁止: style 属性 (<${name}>)`);
      if (/^on[a-z]+$/.test(key)) out.push(`禁止: イベントハンドラ属性 ${key} (<${name}>)`);
    }
    if (name === "script" && !attrs.has("src")) out.push("禁止: src の無い <script>");
  }
  return out;
}
