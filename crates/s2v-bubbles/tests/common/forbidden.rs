//! 画面の JS・HTML の禁止の検査(Node の assets-test/forbidden.mjs と同じ規則)。
//! Node が無い・S2V_SKIP_NODE_TESTS=1 のときも禁止の検査が残るように、Rust 側にも置く。
//! 簡易な走査(JS の正規表現リテラルの中の引用符は ' と " なら改行で打ち切る)。

/// JS のコメント(行コメントとブロックコメント)を取り除く。文字列・テンプレートの中身は残す。
fn strip_js_comments(src: &str) -> String {
    let c: Vec<char> = src.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < c.len() {
        let n = c.get(i + 1).copied();
        if c[i] == '/' && n == Some('/') {
            while i < c.len() && c[i] != '\n' {
                i += 1;
            }
        } else if c[i] == '/' && n == Some('*') {
            i += 2;
            while i < c.len() && !(c[i] == '*' && c.get(i + 1) == Some(&'/')) {
                i += 1;
            }
            i = (i + 2).min(c.len());
            out.push(' ');
        } else if c[i] == '"' || c[i] == '\'' || c[i] == '`' {
            let q = c[i];
            let mut j = i + 1;
            while j < c.len() && c[j] != q {
                if c[j] == '\\' {
                    j += 1;
                } else if q != '`' && c[j] == '\n' {
                    break;
                }
                j += 1;
            }
            let end = (j + 1).min(c.len());
            out.extend(&c[i..end]);
            i = end;
        } else {
            out.push(c[i]);
            i += 1;
        }
    }
    out
}

/// `needle` が語頭の境界(直前が識別子の文字か `.` でない)で現れるか。
fn has_word_start(hay: &str, needle: &str) -> bool {
    hay.match_indices(needle).any(|(i, _)| {
        !hay[..i]
            .chars()
            .next_back()
            .is_some_and(|p| p.is_alphanumeric() || p == '_' || p == '$' || p == '.')
    })
}

/// JS のソースの違反(無ければ空)。
pub fn check_js(source: &str) -> Vec<String> {
    // 空白を除き小文字にして比べる(`document . write` や `setAttribute( "STYLE"` も捕まえる)
    let s: String = strip_js_comments(source)
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect();
    let mut out = Vec::new();
    let mut flag = |bad: bool, name: &str| {
        if bad {
            out.push(format!("禁止: {name}"));
        }
    };
    flag(s.contains("innerhtml"), "innerHTML");
    flag(s.contains("outerhtml"), "outerHTML");
    flag(s.contains("insertadjacenthtml"), "insertAdjacentHTML");
    flag(s.contains("document.write"), "document.write");
    flag(has_word_start(&s, "eval("), "eval");
    flag(has_word_start(&s, "newfunction"), "new Function");
    let timer_str = ["settimeout(", "setinterval("].iter().any(|t| {
        ['\'', '"', '`']
            .iter()
            .any(|q| s.contains(&format!("{t}{q}")))
    });
    flag(timer_str, "文字列を渡す setTimeout/setInterval");
    flag(s.contains(".style.csstext"), ".style.cssText");
    let mut attr_style = false;
    let mut attr_on = false;
    for q in ['\'', '"', '`'] {
        let head = format!("setattribute({q}");
        for (i, _) in s.match_indices(&head) {
            let rest = &s[i + head.len()..];
            attr_style |= rest.starts_with(&format!("style{q}"));
            if let Some(r) = rest.strip_prefix("on") {
                let letters = r.chars().take_while(char::is_ascii_lowercase).count();
                attr_on |= letters > 0 && r[letters..].starts_with(q);
            }
        }
    }
    flag(attr_style, "setAttribute(\"style\")");
    flag(attr_on, "setAttribute(\"on…\")");
    out
}

/// HTML のソースの違反(無ければ空)。タグの中だけを見て、大文字小文字を区別しない。
pub fn check_html(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut s = source.to_string();
    while let Some(a) = s.find("<!--") {
        let end = s[a..].find("-->").map_or(s.len(), |e| a + e + 3);
        s.replace_range(a..end, " ");
    }
    let c: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < c.len() {
        if c[i] != '<'
            || !c
                .get(i + 1)
                .is_some_and(|x| x.is_ascii_alphabetic() || *x == '/')
        {
            i += 1;
            continue;
        }
        // タグの終わり(引用符の中の > は飛ばす)
        let mut j = i + 1;
        let mut q: Option<char> = None;
        while j < c.len() {
            match q {
                Some(qc) if c[j] == qc => q = None,
                Some(_) => {}
                None if c[j] == '"' || c[j] == '\'' => q = Some(c[j]),
                None if c[j] == '>' => break,
                None => {}
            }
            j += 1;
        }
        let tag: Vec<char> = c[i + 1..j.min(c.len())].to_vec();
        i = j + 1;
        if tag.first() == Some(&'/') {
            continue;
        }
        let name_len = tag
            .iter()
            .take_while(|x| !x.is_whitespace() && **x != '/')
            .count();
        let name: String = tag[..name_len].iter().collect::<String>().to_lowercase();
        if name == "style" {
            out.push("禁止: <style>".to_string());
        }
        let mut has_src = false;
        let mut k = name_len;
        while k < tag.len() {
            if tag[k].is_whitespace() || tag[k] == '/' {
                k += 1;
                continue;
            }
            let s0 = k;
            while k < tag.len() && !tag[k].is_whitespace() && !"=/\"'".contains(tag[k]) {
                k += 1;
            }
            if k == s0 {
                k += 1; // 引用符などの読み飛ばし
                continue;
            }
            let key: String = tag[s0..k].iter().collect::<String>().to_lowercase();
            let mut m = k;
            while m < tag.len() && tag[m].is_whitespace() {
                m += 1;
            }
            let mut val = String::new();
            if tag.get(m) == Some(&'=') {
                m += 1;
                while m < tag.len() && tag[m].is_whitespace() {
                    m += 1;
                }
                if matches!(tag.get(m), Some('"') | Some('\'')) {
                    let qc = tag[m];
                    let vs = m + 1;
                    m = vs;
                    while m < tag.len() && tag[m] != qc {
                        m += 1;
                    }
                    val = tag[vs..m].iter().collect();
                    m += 1;
                } else {
                    let vs = m;
                    while m < tag.len() && !tag[m].is_whitespace() {
                        m += 1;
                    }
                    val = tag[vs..m].iter().collect();
                }
                k = m;
            }
            if key == "style" {
                out.push(format!("禁止: style 属性 (<{name}>)"));
            }
            if key.len() > 2 && key.starts_with("on") && key.chars().all(|x| x.is_ascii_lowercase())
            {
                out.push(format!("禁止: イベントハンドラ属性 {key} (<{name}>)"));
            }
            if (key == "src" || key == "href") && (!val.starts_with('/') || val.starts_with("//")) {
                out.push(format!(
                    "禁止: 同じオリジンでない {key}=\"{val}\" (<{name}>)"
                ));
            }
            if key == "src" && !has_src {
                has_src = true;
            }
        }
        if name == "script" {
            if !has_src {
                out.push("禁止: src の無い <script>".to_string());
            }
        }
    }
    out
}
