//! ローカル GUI サーバーの結合テスト(生の TCP で HTTP を送る。ポートは OS が決める)。

mod common;

use common::*;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' blob:; media-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'";

fn assert_common_headers(r: &Resp) {
    assert_eq!(
        r.header("Content-Security-Policy"),
        Some(CSP),
        "{:?}",
        r.headers
    );
    assert_eq!(r.header("X-Content-Type-Options"), Some("nosniff"));
    assert_eq!(r.header("Referrer-Policy"), Some("no-referrer"));
    assert_eq!(r.header("Cache-Control"), Some("no-store"));
    assert!(
        !r.headers
            .iter()
            .any(|(k, _)| k.to_ascii_lowercase().starts_with("access-control")),
        "{:?}",
        r.headers
    );
}

#[test]
fn token_gives_cookie_and_redirect() {
    let dir = tempfile::tempdir().unwrap();
    let srv = start_default(dir.path());
    let p = srv.port;
    let r = Req::new("GET", &format!("/?token={}", srv.token), p).send(p);
    assert_eq!(r.status, 303);
    assert_eq!(r.header("Location"), Some("/"));
    assert_eq!(
        r.header("Set-Cookie"),
        Some(format!("s2v_{p}={}; HttpOnly; SameSite=Strict; Path=/", srv.token).as_str())
    );
    // 違うトークン(同じ長さ・短い・空)は 403 でクッキーを設定しない
    for bad in ["0".repeat(64), "abc".to_string(), String::new()] {
        let r = Req::new("GET", &format!("/?token={bad}"), p).send(p);
        assert_eq!(r.status, 403, "{bad}");
        assert!(r.header("Set-Cookie").is_none());
        assert_common_headers(&r);
    }
    // トークンは本文にもヘッダーにも出ない
    assert!(!r.text().contains(&srv.token));
}

#[test]
fn requests_without_cookie_are_403() {
    let dir = tempfile::tempdir().unwrap();
    let srv = start_default(dir.path());
    let p = srv.port;
    for path in [
        "/",
        "/app.js",
        "/app.css",
        "/geom.js",
        "/api/state",
        "/media/audio",
        "/media/asset/S01-C01",
        "/media/frame/S01-C01?n=0",
        "/nothing",
    ] {
        let r = Req::new("GET", path, p).send(p);
        assert_eq!(r.status, 403, "{path}");
        assert!(r.header("Set-Cookie").is_none());
    }
    let r = Req::new("POST", "/api/quit", p)
        .origin(&origin_of(p))
        .json("{}")
        .send(p);
    assert_eq!(r.status, 403);
    // 別の名前・別の値のクッキーも 403
    let r = Req::new("GET", "/api/state", p)
        .cookie(&format!("s2v_{}={}", p + 1, srv.token))
        .send(p);
    assert_eq!(r.status, 403);
    let r = Req::new("GET", "/api/state", p)
        .cookie(&format!("s2v_{p}={}", "0".repeat(64)))
        .send(p);
    assert_eq!(r.status, 403);
    // サーバーは生きている
    let c = login(p, &srv.token);
    assert_eq!(
        Req::new("GET", "/api/state", p).cookie(&c).send(p).status,
        200
    );
}

#[test]
fn bad_host_is_403() {
    let dir = tempfile::tempdir().unwrap();
    let srv = start_default(dir.path());
    let p = srv.port;
    let c = login(p, &srv.token);
    for host in [
        format!("evil.example:{p}"),
        "127.0.0.1".to_string(),
        format!("127.0.0.1:{}", p + 1),
        String::new(),
    ] {
        let r = Req::new("GET", "/api/state", p)
            .cookie(&c)
            .host(&host)
            .send(p);
        assert_eq!(r.status, 403, "{host}");
        // トークンの入口でも Host が先に検査される
        let r = Req::new("GET", &format!("/?token={}", srv.token), p)
            .host(&host)
            .send(p);
        assert_eq!(r.status, 403, "{host}");
        assert!(r.header("Set-Cookie").is_none());
    }
    // Host ヘッダーなし
    let r = send(
        p,
        &format!("GET /api/state HTTP/1.0\r\nCookie: {c}\r\nConnection: close\r\n\r\n"),
    );
    assert_eq!(r.status, 403);
    // localhost も許す
    let r = Req::new("GET", "/api/state", p)
        .cookie(&c)
        .host(&format!("localhost:{p}"))
        .send(p);
    assert_eq!(r.status, 200);
}

#[test]
fn post_needs_origin_and_json_and_size() {
    let dir = tempfile::tempdir().unwrap();
    let srv = start_default(dir.path());
    let p = srv.port;
    let c = login(p, &srv.token);
    let body = r#"{"manual":{}}"#;
    // Origin なし・違う → 403
    let r = Req::new("POST", "/api/layout", p)
        .cookie(&c)
        .json(body)
        .send(p);
    assert_eq!(r.status, 403);
    for bad in [
        "http://evil.example",
        &format!("http://evil.example:{p}"),
        &format!("https://127.0.0.1:{p}"),
        &format!("http://127.0.0.1:{p}/"),
        "null",
    ] {
        let r = Req::new("POST", "/api/layout", p)
            .cookie(&c)
            .origin(bad)
            .json(body)
            .send(p);
        assert_eq!(r.status, 403, "{bad}");
    }
    // Content-Type
    for ct in [
        "text/plain",
        "application/x-www-form-urlencoded",
        "application/jsonx",
    ] {
        let mut req = Req::new("POST", "/api/layout", p)
            .cookie(&c)
            .origin(&origin_of(p))
            .content_type(ct);
        req.body = Some(body);
        assert_eq!(req.send(p).status, 415, "{ct}");
    }
    // Content-Type なし
    let r = Req::new("POST", "/api/layout", p)
        .cookie(&c)
        .origin(&origin_of(p))
        .send(p);
    assert_eq!(r.status, 415);
    // 1 MiB + 1(本文は送らなくても、宣言だけで 413)
    let r = Req::new("POST", "/api/layout", p)
        .cookie(&c)
        .origin(&format!("http://localhost:{p}"))
        .content_type("application/json")
        .content_length((1 << 20) + 1)
        .send(p);
    assert_eq!(r.status, 413);
    // 1 MiB ちょうどの宣言は大きさの検査を通る(中身が JSON でないので 400)
    let big = " ".repeat(1 << 20);
    let r = Req::new("POST", "/api/layout", p)
        .cookie(&c)
        .origin(&origin_of(p))
        .json(&big)
        .send(p);
    assert_eq!(r.status, 400);
    // 正しい要求は 200 で、localhost の Origin も許す
    let r = Req::new("POST", "/api/layout", p)
        .cookie(&c)
        .origin(&format!("http://localhost:{p}"))
        .content_type("application/json; charset=utf-8")
        .json(body)
        .send(p);
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(
        r.header("Content-Type"),
        Some("application/json; charset=utf-8")
    );
    assert!(r.json().is_array());
    assert_common_headers(&r);
}

#[test]
fn common_headers_on_every_response() {
    let dir = tempfile::tempdir().unwrap();
    let srv = start_default(dir.path());
    let p = srv.port;
    let c = login(p, &srv.token);
    let ok = Req::new("GET", "/app.js", p).cookie(&c).send(p);
    assert_eq!(ok.status, 200);
    assert_common_headers(&ok);
    let forbidden = Req::new("GET", "/app.js", p).send(p);
    assert_eq!(forbidden.status, 403);
    assert_common_headers(&forbidden);
    let nf = Req::new("GET", "/nothing", p).cookie(&c).send(p);
    assert_eq!(nf.status, 404);
    assert_common_headers(&nf);
    // 405 と 415 と 413
    let m = Req::new("POST", "/app.js", p)
        .cookie(&c)
        .origin(&origin_of(p))
        .json("{}")
        .send(p);
    assert_eq!(m.status, 405);
    assert_common_headers(&m);
    let m = Req::new("GET", "/api/save", p).cookie(&c).send(p);
    assert_eq!(m.status, 405);
    // メディア・HEAD・416 にも付く
    let a = Req::new("GET", "/media/audio", p).cookie(&c).send(p);
    assert_eq!(a.status, 200);
    assert_common_headers(&a);
    let a = Req::new("GET", "/media/audio", p)
        .cookie(&c)
        .range("bytes=999999999-")
        .send(p);
    assert_eq!(a.status, 416);
    assert_common_headers(&a);
    // エラーの本文は {"error": 日本語}
    assert!(!nf.json()["error"].as_str().unwrap().is_empty());
    // 画面の種類
    for (path, ct) in [
        ("/", "text/html; charset=utf-8"),
        ("/app.js", "text/javascript; charset=utf-8"),
        ("/app.css", "text/css; charset=utf-8"),
        ("/geom.js", "text/javascript; charset=utf-8"),
    ] {
        let r = Req::new("GET", path, p).cookie(&c).send(p);
        assert_eq!(r.status, 200, "{path}");
        assert_eq!(r.header("Content-Type"), Some(ct), "{path}");
        assert!(!r.body.is_empty());
    }
}

#[test]
fn media_range_and_head() {
    let dir = tempfile::tempdir().unwrap();
    let (path, audio) = locked_file(dir.path(), "full_dialogue.wav", 4096);
    let expected = std::fs::read(&path).unwrap_or_else(|_| {
        // 共有モードで読めない環境では期待値を自分で作る
        (0..4096usize).map(|i| (i % 256) as u8).collect()
    });
    let srv = start(
        session_in(dir.path(), "あいう", BTreeMap::new()),
        BTreeMap::new(),
        audio,
    );
    let p = srv.port;
    let c = login(p, &srv.token);
    let get = |range: Option<&str>| {
        let mut r = Req::new("GET", "/media/audio", p).cookie(&c);
        r.range = range;
        r.send(p)
    };
    let full = get(None);
    assert_eq!(full.status, 200);
    assert_eq!(full.body, expected);
    assert_eq!(full.header("Accept-Ranges"), Some("bytes"));
    assert_eq!(full.header("Content-Type"), Some("audio/wav"));
    assert_eq!(full.header("Content-Length"), Some("4096"));

    let part = get(Some("bytes=0-9"));
    assert_eq!(part.status, 206);
    assert_eq!(part.header("Content-Range"), Some("bytes 0-9/4096"));
    assert_eq!(part.header("Content-Length"), Some("10"));
    assert_eq!(part.body, expected[..10]);
    assert_eq!(part.header("Accept-Ranges"), Some("bytes"));

    let suffix = get(Some("bytes=-4"));
    assert_eq!(suffix.status, 206);
    assert_eq!(suffix.header("Content-Range"), Some("bytes 4092-4095/4096"));
    assert_eq!(suffix.body, expected[4092..]);

    let clipped = get(Some("bytes=4090-99999"));
    assert_eq!(clipped.status, 206);
    assert_eq!(
        clipped.header("Content-Range"),
        Some("bytes 4090-4095/4096")
    );

    let bad = get(Some("bytes=999999999-"));
    assert_eq!(bad.status, 416);
    assert_eq!(bad.header("Content-Range"), Some("bytes */4096"));
    assert_eq!(bad.header("Content-Length"), Some("0"));
    assert_eq!(bad.header("Accept-Ranges"), Some("bytes"));
    assert!(bad.body.is_empty());
    assert_eq!(get(Some("bytes=0-1,5-6")).status, 416);

    // HEAD は本文なしで GET と同じヘッダー
    for range in [None, Some("bytes=0-9"), Some("bytes=999999999-")] {
        let mut h = Req::new("HEAD", "/media/audio", p).cookie(&c);
        h.range = range;
        let h = h.send(p);
        let g = get(range);
        assert_eq!(h.status, g.status, "{range:?}");
        assert!(h.body.is_empty());
        for name in [
            "Content-Type",
            "Content-Length",
            "Content-Range",
            "Accept-Ranges",
        ] {
            assert_eq!(h.header(name), g.header(name), "{name} {range:?}");
        }
    }
    // POST は 405
    let r = Req::new("POST", "/media/audio", p)
        .cookie(&c)
        .origin(&origin_of(p))
        .json("{}")
        .send(p);
    assert_eq!(r.status, 405);
}

#[test]
fn unknown_cut_is_404() {
    let dir = tempfile::tempdir().unwrap();
    let srv = start_default(dir.path());
    let p = srv.port;
    let c = login(p, &srv.token);
    for path in [
        "/media/asset/S99-C01",
        "/media/asset/..%2F..%2Fx",
        "/media/asset/../../x",
        "/media/asset/",
        "/media/asset/S01-C01", // 素材の無いカット
        "/media/frame/S99-C01?n=0",
        "/media/frame/S01-C01?n=0", // 動画の素材でない(素材なし)
        "/media/audio/x",
    ] {
        let r = Req::new("GET", path, p).cookie(&c).send(p);
        assert_eq!(r.status, 404, "{path}");
        assert!(!r.text().contains("tempdir"));
    }
}

/// 読まずに保持する、大きな音声の要求。
fn hold_audio(port: u16, cookie: &str) -> TcpStream {
    let mut s = connect(port);
    let req = Req::new("GET", "/media/audio", port)
        .cookie(cookie)
        .range("bytes=0-")
        .raw()
        .replace("Connection: close", "Connection: keep-alive");
    s.write_all(req.as_bytes()).unwrap();
    // 応答の最初の 1 バイトが来る = スレッドが走っている
    let mut b = [0u8; 1];
    s.read_exact(&mut b).unwrap();
    s
}

fn big_audio_server(dir: &std::path::Path) -> Running {
    let path = dir.join("full_dialogue.wav");
    let f = std::fs::File::create(&path).unwrap();
    f.set_len(64 << 20).unwrap();
    drop(f);
    let audio = s2v_bubbles::media::open_locked(&path).unwrap();
    start(
        session_in(dir, "あいう", BTreeMap::new()),
        BTreeMap::new(),
        audio,
    )
}

#[test]
fn api_responds_while_media_is_being_read() {
    // Review Focus 3
    let dir = tempfile::tempdir().unwrap();
    let srv = big_audio_server(dir.path());
    let p = srv.port;
    let c = login(p, &srv.token);
    let _held = hold_audio(p, &c);
    let t = Instant::now();
    let r = post_json(p, &c, "/api/layout", r#"{"manual":{}}"#);
    assert_eq!(r.status, 200);
    assert!(t.elapsed() < Duration::from_secs(1), "{:?}", t.elapsed());
    let t = Instant::now();
    let r = post_json(p, &c, "/api/save", r#"{"manual":{}}"#);
    assert_eq!(r.status, 200, "{}", r.text());
    assert!(t.elapsed() < Duration::from_secs(1), "{:?}", t.elapsed());
}

#[test]
fn media_slots_full_is_503_then_recovers() {
    let dir = tempfile::tempdir().unwrap();
    let srv = big_audio_server(dir.path());
    let p = srv.port;
    let c = login(p, &srv.token);
    let mut held: Vec<TcpStream> = (0..8).map(|_| hold_audio(p, &c)).collect();
    let r = Req::new("HEAD", "/media/audio", p).cookie(&c).send(p);
    assert_eq!(r.status, 503);
    assert_common_headers(&r);
    // API は影響を受けない
    assert_eq!(
        post_json(p, &c, "/api/layout", r#"{"manual":{}}"#).status,
        200
    );
    // 1 本を閉じると、枠が戻る
    drop(held.pop());
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let r = Req::new("HEAD", "/media/audio", p).cookie(&c).send(p);
        if r.status == 200 {
            break;
        }
        assert_eq!(r.status, 503);
        assert!(Instant::now() < deadline, "枠が戻りません");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn bubble_json(x: i32, y: i32, w: u32, h: u32, so: i64, eo: i64) -> String {
    format!(
        r#"{{"kind":"bubble","x":{x},"y":{y},"w":{w},"h":{h},"tail":null,"start_offset_ms":{so},"end_offset_ms":{eo}}}"#
    )
}

#[test]
fn save_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let srv = start_default(dir.path());
    let p = srv.port;
    let c = login(p, &srv.token);
    let body = format!(
        r#"{{"manual":{{"S01-L001":{}}}}}"#,
        bubble_json(100, 10, 200, 160, 0, 0)
    );
    let r = post_json(p, &c, "/api/save", &body);
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()["saved"], true);
    assert!(r.json()["warnings"].is_array());
    let saved = std::fs::read_to_string(dir.path().join("bubbles.json")).unwrap();
    assert!(saved.contains("S01-L001"));
    assert!(!dir.path().join("bubbles.json.bak").exists());
    // 2 回目で .bak ができる
    let body2 = format!(
        r#"{{"manual":{{"S01-L001":{}}}}}"#,
        bubble_json(10, 10, 200, 160, 0, 0)
    );
    assert_eq!(post_json(p, &c, "/api/save", &body2).status, 200);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("bubbles.json.bak")).unwrap(),
        saved
    );
    // 壊れた JSON → 400、逆転 → 422(errors の一覧)、未知の ID → 422
    let r = post_json(p, &c, "/api/save", "{not json");
    assert_eq!(r.status, 400);
    assert!(r.json()["error"].is_string());
    let r = post_json(
        p,
        &c,
        "/api/save",
        r#"{"manual":{"S01-L001":{"kind":"x"}}}"#,
    );
    assert_eq!(r.status, 400);
    let inv = format!(
        r#"{{"manual":{{"S01-L001":{}}}}}"#,
        bubble_json(100, 10, 200, 160, 9000, 0)
    );
    let r = post_json(p, &c, "/api/save", &inv);
    assert_eq!(r.status, 422, "{}", r.text());
    assert!(!r.json()["errors"].as_array().unwrap().is_empty());
    let unknown = format!(
        r#"{{"manual":{{"S09-L777":{}}}}}"#,
        bubble_json(1, 1, 50, 50, 0, 0)
    );
    assert_eq!(post_json(p, &c, "/api/save", &unknown).status, 422);
    // 失敗しても保存済みの内容は変わらない
    assert!(std::fs::read_to_string(dir.path().join("bubbles.json"))
        .unwrap()
        .contains("\"x\": 10"));
    // 保存の記録は Info で主スレッドへ渡る(トークンを含まない)
    let mut srv = srv;
    let (res, logs) = srv.stop().unwrap();
    assert!(res.is_ok());
    assert!(logs.iter().any(|l| l.contains("保存")), "{logs:?}");
    assert!(logs.iter().all(|l| !l.contains(&srv.token)), "{logs:?}");
}

#[test]
fn render_returns_png_with_origin_headers() {
    let dir = tempfile::tempdir().unwrap();
    let srv = start_default(dir.path());
    let p = srv.port;
    let c = login(p, &srv.token);
    let r = post_json(p, &c, "/api/render", r#"{"id":"S01-L001","manual":{}}"#);
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.header("Content-Type"), Some("image/png"));
    assert!(r.body.starts_with(b"\x89PNG\r\n\x1a\n"));
    let o = r.header("X-S2V-Origin").expect("X-S2V-Origin");
    let (dx, dy) = o.split_once(',').unwrap();
    dx.parse::<i32>().unwrap();
    dy.parse::<i32>().unwrap();
    assert!(matches!(r.header("X-S2V-Clipped"), Some("0") | Some("1")));
    assert_common_headers(&r);
    // 未知の id は 404、id なしは 400、手動の大きさが小さすぎれば 422
    assert_eq!(
        post_json(p, &c, "/api/render", r#"{"id":"S09-L001","manual":{}}"#).status,
        404
    );
    assert_eq!(
        post_json(p, &c, "/api/render", r#"{"manual":{}}"#).status,
        400
    );
    let small = format!(
        r#"{{"id":"S01-L001","manual":{{"S01-L001":{}}}}}"#,
        bubble_json(10, 10, 10, 10, 0, 0)
    );
    assert_eq!(post_json(p, &c, "/api/render", &small).status, 422);
}

#[test]
fn state_json_escapes_script_text() {
    // Review Focus 1
    let dir = tempfile::tempdir().unwrap();
    let text = "<script>alert(1)</script>\"\n";
    let (_, audio) = locked_file(dir.path(), "full_dialogue.wav", 100);
    let srv = start(
        session_in(dir.path(), text, BTreeMap::new()),
        BTreeMap::new(),
        audio,
    );
    let p = srv.port;
    let c = login(p, &srv.token);
    let r = Req::new("GET", "/api/state", p).cookie(&c).send(p);
    assert_eq!(r.status, 200);
    assert_eq!(
        r.header("Content-Type"),
        Some("application/json; charset=utf-8")
    );
    assert_common_headers(&r);
    let raw = r.text();
    assert!(!raw.contains('\n'), "{raw}"); // 改行は \n にエスケープされる
    let v = r.json();
    let it = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == "S01-L001")
        .unwrap();
    assert_eq!(it["text"], text);
    // 画面本体は HTML としてスクリプトを含まない
    let page = Req::new("GET", "/", p).cookie(&c).send(p).text();
    assert!(!page.contains("alert(1)"));
}

#[test]
fn quit_stops_the_server() {
    let dir = tempfile::tempdir().unwrap();
    let mut srv = start_default(dir.path());
    let p = srv.port;
    let c = login(p, &srv.token);
    let r = post_json(p, &c, "/api/quit", "{}");
    assert_eq!(r.status, 200);
    assert_eq!(r.json(), serde_json::json!({"quitting": true}));
    // run が戻る(5 秒の待ちを含めても 10 秒以内)
    let j = srv.join.take().unwrap();
    let t = Instant::now();
    while !j.is_finished() {
        assert!(t.elapsed() < Duration::from_secs(10), "run が戻りません");
        std::thread::sleep(Duration::from_millis(20));
    }
    let (res, logs) = j.join().unwrap();
    assert!(res.is_ok());
    assert_eq!(
        logs.last().map(String::as_str),
        Some("INFO bubbles を終了しました")
    );
    assert!(logs.iter().all(|l| !l.contains(&srv.token)), "{logs:?}");
    // ポートに接続できなくなる
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if TcpStream::connect(("127.0.0.1", p)).is_err() {
            break;
        }
        assert!(Instant::now() < deadline, "ポートが閉じません");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn quit_handle_stops_the_server() {
    let dir = tempfile::tempdir().unwrap();
    let mut srv = start_default(dir.path());
    let (res, _) = srv.stop().unwrap();
    assert!(res.is_ok());
}

#[test]
fn two_servers_do_not_share_cookie() {
    // Review Focus 2
    let d1 = tempfile::tempdir().unwrap();
    let d2 = tempfile::tempdir().unwrap();
    let a = start_default(d1.path());
    let b = start_default(d2.path());
    assert_ne!(a.port, b.port);
    assert_ne!(a.token, b.token);
    let ca = login(a.port, &a.token);
    let cb = login(b.port, &b.token);
    // ブラウザは同じホストの両方のクッキーを送る
    let both = format!("{ca}; {cb}");
    assert_eq!(
        Req::new("GET", "/api/state", a.port)
            .cookie(&both)
            .send(a.port)
            .status,
        200
    );
    assert_eq!(
        Req::new("GET", "/api/state", b.port)
            .cookie(&both)
            .send(b.port)
            .status,
        200
    );
    // 片方のクッキーだけではもう片方が 403
    assert_eq!(
        Req::new("GET", "/api/state", b.port)
            .cookie(&ca)
            .send(b.port)
            .status,
        403
    );
    assert_eq!(
        Req::new("GET", "/api/state", a.port)
            .cookie(&cb)
            .send(a.port)
            .status,
        403
    );
    assert_eq!(
        Req::new("GET", "/api/state", a.port)
            .cookie(&ca)
            .send(a.port)
            .status,
        200
    );
    assert_eq!(
        Req::new("GET", "/api/state", b.port)
            .cookie(&cb)
            .send(b.port)
            .status,
        200
    );
}

#[test]
fn embedded_modules_are_served() {
    let dir = tempfile::tempdir().unwrap();
    let srv = start_default(dir.path());
    let p = srv.port;
    let c = login(p, &srv.token);
    for path in [
        "/geom.js",
        "/timeline.js",
        "/model.js",
        "/api.js",
        "/view.js",
        "/app.js",
    ] {
        let r = Req::new("GET", path, p).cookie(&c).send(p);
        assert_eq!(r.status, 200, "{path}");
        assert_eq!(
            r.header("Content-Type"),
            Some("text/javascript; charset=utf-8"),
            "{path}"
        );
        assert!(!r.body.is_empty(), "{path}");
        let r = Req::new("GET", path, p).send(p);
        assert_eq!(r.status, 403, "{path}");
    }
}

#[test]
fn index_loads_app_as_module() {
    let dir = tempfile::tempdir().unwrap();
    let srv = start_default(dir.path());
    let p = srv.port;
    let c = login(p, &srv.token);
    let r = Req::new("GET", "/", p).cookie(&c).send(p);
    assert_eq!(r.status, 200);
    let t = r.text();
    assert!(t.contains(r#"type="module""#), "{t}");
    assert!(t.contains(r#"src="/app.js""#), "{t}");
}

#[test]
fn embedded_assets_have_no_inner_html() {
    let html = include_str!("../src/assets/index.html");
    let css = include_str!("../src/assets/app.css");
    let found = forbidden::check_html(html);
    assert!(found.is_empty(), "index.html: {found:?}");
    // src/assets の全ての .js を対象にする(Node が無くても最低限の検査が残る)
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/assets");
    let mut checked = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("js") {
            continue;
        }
        let js = std::fs::read_to_string(&path).unwrap();
        let found = forbidden::check_js(&js);
        assert!(found.is_empty(), "{}: {found:?}", path.display());
        checked += 1;
    }
    assert!(checked >= 2, "app.js と geom.js を検査できていません");
    // CSP: インラインのスクリプト・スタイルを使わない
    assert!(html.contains(r#"<script type="module" src="/app.js">"#));
    assert!(html.contains(r#"href="/app.css""#));
    assert!(!html.contains("<style"));
    assert!(!html.contains(" style="));
    assert!(!html.contains(" onclick="));
    assert!(!html.contains("javascript:"));
    assert!(!css.contains("@import"));
    assert!(!css.contains("url(http"));
}

#[test]
fn unknown_path_404_and_method_405() {
    let dir = tempfile::tempdir().unwrap();
    let srv = start_default(dir.path());
    let p = srv.port;
    let c = login(p, &srv.token);
    assert_eq!(Req::new("GET", "/x", p).cookie(&c).send(p).status, 404);
    assert_eq!(
        Req::new("GET", "/api/nothing", p).cookie(&c).send(p).status,
        404
    );
    for path in ["/api/render", "/api/layout", "/api/save", "/api/quit"] {
        assert_eq!(
            Req::new("GET", path, p).cookie(&c).send(p).status,
            405,
            "{path}"
        );
    }
    for (m, path) in [
        ("POST", "/api/state"),
        ("POST", "/"),
        ("PUT", "/api/state"),
        ("DELETE", "/media/audio"),
    ] {
        let r = Req::new(m, path, p)
            .cookie(&c)
            .origin(&origin_of(p))
            .json("{}")
            .send(p);
        assert_eq!(r.status, 405, "{m} {path}");
    }
}

#[test]
fn post_checks_apply_to_every_path() {
    let dir = tempfile::tempdir().unwrap();
    let srv = start_default(dir.path());
    let p = srv.port;
    let c = login(p, &srv.token);
    for path in ["/media/audio", "/app.js", "/unknown", "/api/state"] {
        let r = Req::new("POST", path, p).cookie(&c).json("{}").send(p);
        assert_eq!(r.status, 403, "{path}");
    }
    let r = Req::new("POST", "/media/audio", p)
        .cookie(&c)
        .origin(&origin_of(p))
        .content_type("text/plain")
        .send(p);
    assert_eq!(r.status, 415);
    let r = Req::new("POST", "/media/audio", p)
        .cookie(&c)
        .origin(&origin_of(p))
        .content_type("application/json")
        .content_length((1 << 20) + 1)
        .send(p);
    assert_eq!(r.status, 413);
    // 検査を通れば 404 / 405
    let ok = |path| {
        Req::new("POST", path, p)
            .cookie(&c)
            .origin(&origin_of(p))
            .json("{}")
            .send(p)
            .status
    };
    assert_eq!(ok("/unknown"), 404);
    assert_eq!(ok("/media/audio"), 405);
}

#[test]
fn quit_returns_even_if_media_is_held() {
    let dir = tempfile::tempdir().unwrap();
    let mut srv = big_audio_server(dir.path());
    let p = srv.port;
    let c = login(p, &srv.token);
    let _held = hold_audio(p, &c);
    assert_eq!(post_json(p, &c, "/api/quit", "{}").status, 200);
    let j = srv.join.take().unwrap();
    let t = Instant::now();
    while !j.is_finished() {
        assert!(t.elapsed() < Duration::from_secs(7), "run が戻りません");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(j.join().unwrap().0.is_ok());
}

/// 引数を無視して数秒眠る実行ファイル(ffmpeg の代わり)。
fn sleeper(dir: &std::path::Path) -> String {
    #[cfg(windows)]
    {
        let p = dir.join("sleeper.cmd");
        std::fs::write(&p, "@echo off\r\nping -n 4 127.0.0.1 >nul 2>&1\r\n").unwrap();
        p.to_string_lossy().into_owned()
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join("sleeper.sh");
        std::fs::write(&p, "#!/bin/sh\nsleep 3\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p.to_string_lossy().into_owned()
    }
}

#[test]
fn frame_worker_busy_is_503_then_recovers() {
    use s2v_bubbles::api::SessionAsset;
    use s2v_render::assets::AssetKind;
    let dir = tempfile::tempdir().unwrap();
    let (vpath, vfile) = locked_file(dir.path(), "v.mp4", 100);
    let mut assets = BTreeMap::new();
    assets.insert(
        "S01-C01".to_string(),
        SessionAsset {
            cut_id: "S01-C01".into(),
            kind: AssetKind::Video,
            looped: false,
            frames: Some(90),
            matrix: "bt709".into(),
            range: "tv".into(),
            path: vpath,
        },
    );
    let media = BTreeMap::from([("S01-C01".to_string(), vfile)]);
    let (_, audio) = locked_file(dir.path(), "full_dialogue.wav", 100);
    let worker = s2v_bubbles::frame::FrameWorker::start_with(
        sleeper(dir.path()),
        Duration::from_secs(10),
        32 << 20,
    );
    let srv = start_with_worker(
        session_in(dir.path(), "あいう", assets),
        media,
        audio,
        Some(worker),
    );
    let p = srv.port;
    let c = login(p, &srv.token);
    let first = {
        let c = c.clone();
        std::thread::spawn(move || {
            Req::new("GET", "/media/frame/S01-C01?n=3", p)
                .cookie(&c)
                .send(p)
                .status
        })
    };
    std::thread::sleep(Duration::from_millis(500));
    // 1 本目の応答を待っている間の 2 本目は、即座に 503(主スレッドも止まらない)
    let t = Instant::now();
    let r = Req::new("GET", "/media/frame/S01-C01?n=4", p)
        .cookie(&c)
        .send(p);
    assert_eq!(r.status, 503);
    assert!(t.elapsed() < Duration::from_secs(1));
    assert_common_headers(&r);
    assert_eq!(
        post_json(p, &c, "/api/layout", r#"{"manual":{}}"#).status,
        200
    );
    // 眠るだけで画像を出さないので 1 本目は 500(空の出力を成功にしない)
    assert_eq!(first.join().unwrap(), 500);
    // 終わった後の 3 本目は 503 にならない(500)
    let r = Req::new("GET", "/media/frame/S01-C01?n=5", p)
        .cookie(&c)
        .send(p);
    assert_eq!(r.status, 500);
}

/// PNG の署名で始まる出力をする実行ファイル(ffmpeg の代わり)。
#[cfg(windows)]
fn png_emitter(dir: &std::path::Path) -> String {
    let p = dir.join("png.cmd");
    std::fs::write(
        &p,
        "@echo off
powershell -NoProfile -Command \"$o=[Console]::OpenStandardOutput(); $b=[byte[]](0x89,0x50,0x4E,0x47,0x0D,0x0A,0x1A,0x0A,1,2,3); $o.Write($b,0,$b.Length); $o.Flush()\"
",
    )
    .unwrap();
    p.to_string_lossy().into_owned()
}

#[cfg(windows)]
#[test]
fn frame_endpoint_returns_png_signature_from_worker() {
    use s2v_bubbles::api::SessionAsset;
    use s2v_render::assets::AssetKind;
    let dir = tempfile::tempdir().unwrap();
    let (vpath, vfile) = locked_file(dir.path(), "v.mp4", 100);
    let assets = BTreeMap::from([(
        "S01-C01".to_string(),
        SessionAsset {
            cut_id: "S01-C01".into(),
            kind: AssetKind::Video,
            looped: false,
            frames: Some(90),
            matrix: "bt709".into(),
            range: "tv".into(),
            path: vpath,
        },
    )]);
    let media = BTreeMap::from([("S01-C01".to_string(), vfile)]);
    let (_, audio) = locked_file(dir.path(), "full_dialogue.wav", 100);
    let worker = s2v_bubbles::frame::FrameWorker::start_with(
        png_emitter(dir.path()),
        Duration::from_secs(20),
        32 << 20,
    );
    let srv = start_with_worker(
        session_in(dir.path(), "あいう", assets),
        media,
        audio,
        Some(worker),
    );
    let p = srv.port;
    let c = login(p, &srv.token);
    let r = Req::new("GET", "/media/frame/S01-C01?n=3", p)
        .cookie(&c)
        .send(p);
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.header("Content-Type"), Some("image/png"));
    assert!(r
        .body
        .starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]));
}

#[test]
fn media_spawn_failure_is_500_and_slot_returns() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    let dir = tempfile::tempdir().unwrap();
    let (_, audio) = locked_file(dir.path(), "full_dialogue.wav", 4096);
    let fail = Arc::new(AtomicBool::new(true));
    let srv = start_full(
        session_in(dir.path(), "あいう", BTreeMap::new()),
        BTreeMap::new(),
        audio,
        None,
        Some(Arc::clone(&fail)),
    );
    let p = srv.port;
    let c = login(p, &srv.token);
    // 枠の数(8)より多く失敗させても、枠は返っている
    for _ in 0..10 {
        let r = Req::new("GET", "/media/audio", p).cookie(&c).send(p);
        assert_eq!(r.status, 500);
        assert_common_headers(&r);
        assert!(r.json()["error"].is_string());
    }
    fail.store(false, Ordering::SeqCst);
    let r = Req::new("GET", "/media/audio", p).cookie(&c).send(p);
    assert_eq!(r.status, 200);
    assert_eq!(r.body.len(), 4096);
}

#[cfg(windows)]
#[test]
fn frame_spawn_failure_is_500_and_next_request_succeeds() {
    use s2v_bubbles::api::SessionAsset;
    use s2v_render::assets::AssetKind;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    let dir = tempfile::tempdir().unwrap();
    let (vpath, vfile) = locked_file(dir.path(), "v.mp4", 100);
    let assets = BTreeMap::from([(
        "S01-C01".to_string(),
        SessionAsset {
            cut_id: "S01-C01".into(),
            kind: AssetKind::Video,
            looped: false,
            frames: Some(90),
            matrix: "bt709".into(),
            range: "tv".into(),
            path: vpath,
        },
    )]);
    let media = BTreeMap::from([("S01-C01".to_string(), vfile)]);
    let (_, audio) = locked_file(dir.path(), "full_dialogue.wav", 100);
    let worker = s2v_bubbles::frame::FrameWorker::start_with(
        png_emitter(dir.path()),
        Duration::from_secs(20),
        32 << 20,
    );
    let fail = Arc::new(AtomicBool::new(true));
    let srv = start_full(
        session_in(dir.path(), "あいう", assets),
        media,
        audio,
        Some(worker),
        Some(Arc::clone(&fail)),
    );
    let p = srv.port;
    let c = login(p, &srv.token);
    let r = Req::new("GET", "/media/frame/S01-C01?n=3", p)
        .cookie(&c)
        .send(p);
    assert_eq!(r.status, 500);
    assert_common_headers(&r);
    fail.store(false, Ordering::SeqCst);
    // ワーカーが先の仕事を終えるまでは 503 になりうる
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let r = Req::new("GET", "/media/frame/S01-C01?n=4", p)
            .cookie(&c)
            .send(p);
        if r.status == 200 {
            break;
        }
        assert_eq!(r.status, 503);
        assert!(Instant::now() < deadline, "回復しません");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn forbidden_checker_detects_violations() {
    for bad in [
        "a.innerHTML = x",
        "e.outerHTML=1",
        "e.insertAdjacentHTML('a', s)",
        "document.write(s)",
        "eval(s)",
        "new Function('x')",
        "setTimeout('f()', 1)",
        "setInterval(\"f()\", 1)",
        "e.setAttribute('style', 'x')",
        "e.setAttribute(\"onclick\", f)",
        "e.style.cssText = s",
    ] {
        assert!(!forbidden::check_js(bad).is_empty(), "{bad}");
    }
    for bad in [
        "<div onclick=\"x\">",
        "<DIV\nONLOAD=x>",
        "<p style = \"a\">",
        "<STYLE>p{}</STYLE>",
        "<script>alert(1)</script>",
        "<script src=\"https://cdn.example/x.js\"></script>",
        "<script src=\"//cdn.example/x.js\"></script>",
        "<link rel=\"stylesheet\" href=\"https://cdn.example/x.css\">",
        "<link href=\"//cdn/x.css\">",
        "<img src=\"https://x/y.png\">",
    ] {
        assert!(!forbidden::check_html(bad).is_empty(), "{bad}");
    }
}

#[test]
fn forbidden_checker_allows_good_code() {
    for ok in [
        "e.style.left = '1px'",
        "e.textContent = s",
        "setTimeout(() => f(), 300)",
        "// innerHTML は使わない(コメント)",
        "/* eval( */ const a = 1;",
        "e.addEventListener('click', f)",
        "retrieval(x); // eval(",
    ] {
        assert_eq!(forbidden::check_js(ok), Vec::<String>::new(), "{ok}");
    }
    for ok in [
        "<script type=\"module\" src=\"/app.js\"></script>",
        "<link rel=\"stylesheet\" href=\"/app.css\">",
        "<p>style や onclick という言葉は本文なら可</p>",
        "<button id=\"save\" type=\"button\">保存</button>",
    ] {
        assert_eq!(forbidden::check_html(ok), Vec::<String>::new(), "{ok}");
    }
}
