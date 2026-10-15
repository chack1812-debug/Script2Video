//! 安全の部品(トークン・クッキー・Host・Origin・Content-Type・共通ヘッダー)。HTTP に依存しない純粋な関数。

/// 本文の上限(1 MiB)。超えたら 413。
pub const MAX_BODY: usize = 1 << 20;

/// 起動ごとの合言葉(32 バイトの乱数を 16 進 64 文字にしたもの)。
pub struct Token(String);

impl Token {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 乱数から新しいトークンを作る。
pub fn new_token() -> Token {
    let mut buf = [0u8; 32];
    getrandom::fill(&mut buf).expect("乱数を得られません");
    Token(buf.iter().map(|b| format!("{b:02x}")).collect())
}

/// 長さが同じなら全バイトを見て比較する(途中で打ち切らない)。長さが違えば偽。
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut d = 0u8;
    for (x, y) in a.iter().zip(b) {
        d |= x ^ y;
    }
    d == 0
}

/// クッキー名。ポートごとに違うので、別の台本の `bubbles` と干渉しない。
pub fn cookie_name(port: u16) -> String {
    format!("s2v_{port}")
}

/// `Set-Cookie` の値。
pub fn set_cookie_header(port: u16, token: &Token) -> String {
    format!(
        "{}={}; HttpOnly; SameSite=Strict; Path=/",
        cookie_name(port),
        token.as_str()
    )
}

/// `Cookie:` の値を `; ` で分け、名前が一致するものの値を定数時間で比較する。
/// 同じ名前が複数あれば偽。
pub fn cookie_ok(cookie_header: Option<&str>, port: u16, token: &Token) -> bool {
    let Some(header) = cookie_header else {
        return false;
    };
    let name = cookie_name(port);
    let mut values = header.split("; ").filter_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == name).then_some(v)
    });
    let Some(first) = values.next() else {
        return false;
    };
    if values.next().is_some() {
        return false;
    }
    constant_time_eq(first.as_bytes(), token.as_str().as_bytes())
}

/// `Host` が `127.0.0.1:<port>` か `localhost:<port>`(大文字小文字を区別しない)。ポートの省略は偽。
pub fn host_ok(host: Option<&str>, port: u16) -> bool {
    let Some(host) = host else {
        return false;
    };
    host.eq_ignore_ascii_case(&format!("127.0.0.1:{port}"))
        || host.eq_ignore_ascii_case(&format!("localhost:{port}"))
}

/// `Origin` が `http://127.0.0.1:<port>` か `http://localhost:<port>`。末尾の `/` や path があれば偽。
pub fn origin_ok(origin: Option<&str>, port: u16) -> bool {
    let Some(origin) = origin else {
        return false;
    };
    origin == format!("http://127.0.0.1:{port}") || origin == format!("http://localhost:{port}")
}

/// `Content-Type` が `application/json`(`; charset=utf-8` は可。大文字小文字を区別しない)。
pub fn json_content_type_ok(ct: Option<&str>) -> bool {
    let Some(ct) = ct else {
        return false;
    };
    let mut parts = ct.split(';');
    let essence = parts.next().unwrap_or("").trim();
    if !essence.eq_ignore_ascii_case("application/json") {
        return false;
    }
    match parts.next() {
        None => true,
        Some(param) => parts.next().is_none() && param.trim().eq_ignore_ascii_case("charset=utf-8"),
    }
}

/// 全ての応答に付けるヘッダー。CORS のヘッダーは含めない。
pub fn common_headers() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "Content-Security-Policy",
            "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' blob:; media-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'",
        ),
        ("X-Content-Type-Options", "nosniff"),
        ("Referrer-Policy", "no-referrer"),
        ("Cache-Control", "no-store"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_64_lowercase_hex_and_random() {
        let a = new_token();
        let b = new_token();
        assert_eq!(a.as_str().len(), 64);
        assert!(a
            .as_str()
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)));
        assert_ne!(a.as_str(), b.as_str());
    }

    #[test]
    fn cookie_name_includes_port() {
        // Review Focus 2
        assert_eq!(cookie_name(51234), "s2v_51234");
        let t = new_token();
        assert_eq!(
            set_cookie_header(51234, &t),
            format!(
                "s2v_51234={}; HttpOnly; SameSite=Strict; Path=/",
                t.as_str()
            )
        );
    }

    #[test]
    fn cookie_check() {
        let t = new_token();
        let good = format!("s2v_80={}", t.as_str());
        assert!(cookie_ok(Some(&good), 80, &t));
        assert!(cookie_ok(Some(&format!("a=1; {good}; b=2")), 80, &t));
        assert!(!cookie_ok(Some(&good), 81, &t)); // 別のポートの名前
        assert!(!cookie_ok(None, 80, &t));
        assert!(!cookie_ok(Some("s2v_80=00"), 80, &t));
        assert!(!cookie_ok(Some(&format!("{good}; {good}")), 80, &t)); // 重複は拒否
    }

    #[test]
    fn host_and_origin() {
        assert!(host_ok(Some("127.0.0.1:8080"), 8080));
        assert!(host_ok(Some("LOCALHOST:8080"), 8080));
        assert!(!host_ok(Some("127.0.0.1"), 8080));
        assert!(!host_ok(Some("evil.example:8080"), 8080));
        assert!(!host_ok(Some("127.0.0.1:8081"), 8080));
        assert!(!host_ok(None, 8080));
        assert!(origin_ok(Some("http://127.0.0.1:8080"), 8080));
        assert!(origin_ok(Some("http://localhost:8080"), 8080));
        for bad in [
            "https://127.0.0.1:8080",
            "http://127.0.0.1:8080/",
            "http://127.0.0.1",
            "null",
            "http://evil:8080",
        ] {
            assert!(!origin_ok(Some(bad), 8080), "{bad}");
        }
        assert!(!origin_ok(None, 8080));
    }

    #[test]
    fn content_type_and_constant_time() {
        assert!(json_content_type_ok(Some("application/json")));
        assert!(json_content_type_ok(Some(
            "Application/JSON; charset=utf-8"
        )));
        assert!(!json_content_type_ok(Some("text/plain")));
        assert!(!json_content_type_ok(None));
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }

    #[test]
    fn common_headers_are_exact() {
        let h = common_headers();
        assert!(h.contains(&("Content-Security-Policy", "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' blob:; media-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'")));
        assert!(h.contains(&("X-Content-Type-Options", "nosniff")));
        assert!(h.contains(&("Referrer-Policy", "no-referrer")));
        assert!(h.contains(&("Cache-Control", "no-store")));
        assert!(!h.iter().any(|(k, _)| k.starts_with("Access-Control")));
    }
}
