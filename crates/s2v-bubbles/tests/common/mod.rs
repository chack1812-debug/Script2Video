//! HTTP の結合テストの共通の補助(Session の組み立て、起動、生の HTTP の送受信)。
#![allow(dead_code)]

use s2v_bubbles::api::{Session, SessionAsset};
use s2v_bubbles::media::{open_locked, LockedFile};
use s2v_bubbles::server::{LogEvent, QuitHandle, ServeOptions, Server};
use s2v_core::config::RenderConfig;
use s2v_render::font::FontFace;
use s2v_render::scene::build_items;
use s2v_timeline::{CutEntry, LineEntry, Normalize, NoteEntry, SceneEntry, Timeline};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;
use std::time::Duration;

/// 読み書きの期限(テストが固まらないための上限)。
pub const IO_TIMEOUT: Duration = Duration::from_secs(20);

fn line(id: &str, text: &str, s: i64, e: i64) -> LineEntry {
    LineEntry {
        id: id.into(),
        scene: "S01".into(),
        cast: "A".into(),
        text: text.into(),
        start_ms: s,
        end_ms: e,
        take: id.into(),
        track: None,
        measured_lufs: None,
        requested_gain_db: None,
        norm_gain_db: None,
        norm_clamped: None,
        src_start_ms: None,
        src_end_ms: None,
        missing: false,
        reason: None,
    }
}

fn cut(id: &str, s: i64, e: i64) -> CutEntry {
    CutEntry {
        id: id.into(),
        start_ms: s,
        end_ms: e,
        visual: None,
    }
}

/// C01 0–3000、C02 3000–6000、C03 長さ 0(6000)。台詞 L001・L002、ト書き N01。
pub fn timeline(l1_text: &str) -> Timeline {
    Timeline {
        schema_version: 3,
        script: "s.txt".into(),
        script_sha256: "0".repeat(64),
        audio_sha256: "0".repeat(64),
        sample_rate: 48_000,
        duration_ms: 6_000,
        normalize: Normalize { target_lufs: -16.0 },
        cast: vec![],
        scenes: vec![SceneEntry {
            id: "S01".into(),
            name: Some("朝".into()),
            index: 1,
            silent: false,
            start_ms: 0,
            end_ms: 6_000,
            cuts: vec![
                cut("S01-C01", 0, 3_000),
                cut("S01-C02", 3_000, 6_000),
                cut("S01-C03", 6_000, 6_000),
            ],
            notes: vec![NoteEntry {
                id: "S01-N01".into(),
                text: "翌朝。".into(),
                start_ms: 0,
                end_ms: 2_000,
            }],
        }],
        takes: vec![],
        lines: vec![
            line("S01-L001", l1_text, 500, 2_000),
            line("S01-L002", "さようなら。", 2_500, 3_500),
        ],
        events: vec![],
    }
}

pub fn font() -> FontFace {
    let p =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../s2v-render/tests/fonts/test-vrt2.ttf");
    FontFace::load(&p, 0).unwrap()
}

pub fn session_in(dir: &Path, l1_text: &str, assets: BTreeMap<String, SessionAsset>) -> Session {
    let cfg = RenderConfig {
        width: 320,
        height: 180,
        ..RenderConfig::default()
    };
    let timeline = timeline(l1_text);
    let items = build_items(&timeline, &BTreeMap::new(), &mut Vec::new()).unwrap();
    Session {
        timeline,
        cfg,
        face: font(),
        dir: dir.to_path_buf(),
        items,
        assets,
        loaded: BTreeMap::new(),
        warnings: vec![],
    }
}

/// 長さ `len` のファイル(中身は 0..255 の繰り返し)を作って固定して開く。
pub fn locked_file(dir: &Path, name: &str, len: usize) -> (PathBuf, LockedFile) {
    let path = dir.join(name);
    let mut f = std::fs::File::create(&path).unwrap();
    let chunk: Vec<u8> = (0..=255u8).collect();
    let mut left = len;
    while left > 0 {
        let n = left.min(chunk.len());
        f.write_all(&chunk[..n]).unwrap();
        left -= n;
    }
    drop(f);
    (path.clone(), open_locked(&path).unwrap())
}

type RunResult = (Result<(), String>, Vec<String>);

/// 起動したサーバー。
pub struct Running {
    pub port: u16,
    pub token: String,
    pub quit: QuitHandle,
    pub join: Option<JoinHandle<RunResult>>,
}

impl Running {
    /// 終了の手順を行って、`run` の戻り(結果とログ)を待つ。
    pub fn stop(&mut self) -> Option<RunResult> {
        self.quit.quit();
        self.join.take().map(|j| j.join().unwrap())
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

pub fn start(session: Session, media: BTreeMap<String, LockedFile>, audio: LockedFile) -> Running {
    start_with_worker(session, media, audio, None)
}

pub fn start_with_worker(
    session: Session,
    media: BTreeMap<String, LockedFile>,
    audio: LockedFile,
    frame_worker: Option<s2v_bubbles::frame::FrameWorker>,
) -> Running {
    start_full(session, media, audio, frame_worker, None)
}

pub fn start_full(
    session: Session,
    media: BTreeMap<String, LockedFile>,
    audio: LockedFile,
    frame_worker: Option<s2v_bubbles::frame::FrameWorker>,
    fail_spawn: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> Running {
    let server = Server::bind(
        session,
        media,
        audio,
        ServeOptions {
            port: None,
            ffmpeg: "ffmpeg".to_string(),
            frame_worker,
            fail_spawn,
        },
    )
    .expect("待ち受けできません");
    let port = server.port();
    let url = server.url();
    let token = url.split("token=").nth(1).unwrap().to_string();
    assert_eq!(url, format!("http://127.0.0.1:{port}/?token={token}"));
    let quit = server.quit_handle();
    let join = std::thread::spawn(move || {
        let mut logs = Vec::new();
        let r = server.run(&mut |ev| {
            logs.push(match ev {
                LogEvent::Info(m) => format!("INFO {m}"),
                LogEvent::Warn(m) => format!("WARN {m}"),
                LogEvent::Error(m) => format!("ERROR {m}"),
            })
        });
        (r, logs)
    });
    Running {
        port,
        token,
        quit,
        join: Some(join),
    }
}

/// 既定の起動(素材なし、小さな音声)。`dir` は tempdir。
pub fn start_default(dir: &Path) -> Running {
    let (_, audio) = locked_file(dir, "full_dialogue.wav", 4096);
    start(
        session_in(dir, "あいう", BTreeMap::new()),
        BTreeMap::new(),
        audio,
    )
}

pub struct Resp {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Resp {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("JSON でない: {e}: {}", self.text()))
    }
}

pub fn connect(port: u16) -> TcpStream {
    let s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(IO_TIMEOUT)).unwrap();
    s.set_write_timeout(Some(IO_TIMEOUT)).unwrap();
    s
}

/// 応答の先頭(ヘッダーまで)を読んで解釈する。本文は読まない。
pub fn read_head(s: &mut TcpStream) -> (u16, Vec<(String, String)>) {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    while !buf.ends_with(b"\r\n\r\n") {
        let n = s.read(&mut byte).expect("応答のヘッダーを読めません");
        assert!(
            n > 0,
            "ヘッダーの途中で閉じた: {:?}",
            String::from_utf8_lossy(&buf)
        );
        buf.push(byte[0]);
    }
    let text = String::from_utf8_lossy(&buf).into_owned();
    let mut lines = text.split("\r\n");
    let status: u16 = lines
        .next()
        .unwrap()
        .split(' ')
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let headers = lines
        .filter(|l| !l.is_empty())
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect();
    (status, headers)
}

/// 生の HTTP を送り、応答の全体を読む(`Connection: close` を付ける前提。本文は Content-Length 分)。
pub fn send(port: u16, raw: &str) -> Resp {
    let mut s = connect(port);
    s.write_all(raw.as_bytes()).unwrap();
    let (status, headers) = read_head(&mut s);
    let len: Option<usize> = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse().ok());
    let is_head = raw.starts_with("HEAD ");
    let mut body = Vec::new();
    match (len, is_head) {
        (_, true) => {}
        (Some(n), _) => {
            body.resize(n, 0);
            s.read_exact(&mut body).expect("本文を読めません");
        }
        (None, _) => {
            s.read_to_end(&mut body).unwrap();
        }
    }
    Resp {
        status,
        headers,
        body,
    }
}

/// 要求の組み立て。`cookie`・`origin` は `Some` のときだけ付ける。
pub struct Req<'a> {
    pub method: &'a str,
    pub path: &'a str,
    pub host: Option<String>,
    pub cookie: Option<String>,
    pub origin: Option<String>,
    pub content_type: Option<&'a str>,
    pub range: Option<&'a str>,
    pub body: Option<&'a str>,
    pub content_length: Option<usize>,
}

impl<'a> Req<'a> {
    pub fn new(method: &'a str, path: &'a str, port: u16) -> Req<'a> {
        Req {
            method,
            path,
            host: Some(format!("127.0.0.1:{port}")),
            cookie: None,
            origin: None,
            content_type: None,
            range: None,
            body: None,
            content_length: None,
        }
    }
    pub fn cookie(mut self, c: &str) -> Self {
        self.cookie = Some(c.to_string());
        self
    }
    pub fn origin(mut self, o: &str) -> Self {
        self.origin = Some(o.to_string());
        self
    }
    pub fn host(mut self, h: &str) -> Self {
        self.host = Some(h.to_string());
        self
    }
    pub fn range(mut self, r: &'a str) -> Self {
        self.range = Some(r);
        self
    }
    /// JSON の本文(Content-Type も付ける)。
    pub fn json(mut self, body: &'a str) -> Self {
        self.content_type = Some("application/json");
        self.body = Some(body);
        self
    }
    pub fn content_type(mut self, ct: &'a str) -> Self {
        self.content_type = Some(ct);
        self
    }
    pub fn content_length(mut self, n: usize) -> Self {
        self.content_length = Some(n);
        self
    }
    pub fn raw(&self) -> String {
        let mut s = format!("{} {} HTTP/1.1\r\n", self.method, self.path);
        if let Some(h) = &self.host {
            s += &format!("Host: {h}\r\n");
        }
        if let Some(c) = &self.cookie {
            s += &format!("Cookie: {c}\r\n");
        }
        if let Some(o) = &self.origin {
            s += &format!("Origin: {o}\r\n");
        }
        if let Some(r) = self.range {
            s += &format!("Range: {r}\r\n");
        }
        if let Some(ct) = self.content_type {
            s += &format!("Content-Type: {ct}\r\n");
        }
        if self.method == "POST" {
            let n = self
                .content_length
                .unwrap_or_else(|| self.body.map_or(0, str::len));
            s += &format!("Content-Length: {n}\r\n");
        }
        s += "Connection: close\r\n\r\n";
        if let Some(b) = self.body {
            s += b;
        }
        s
    }
    pub fn send(&self, port: u16) -> Resp {
        send(port, &self.raw())
    }
}

/// `GET /?token=T` でログインし、クッキー(`名前=値`)を返す。
pub fn login(port: u16, token: &str) -> String {
    let r = Req::new("GET", &format!("/?token={token}"), port).send(port);
    assert_eq!(r.status, 303, "{}", r.text());
    let sc = r.header("Set-Cookie").expect("Set-Cookie");
    sc.split(';').next().unwrap().to_string()
}

pub fn origin_of(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

/// ログイン済みの JSON の POST。
pub fn post_json(port: u16, cookie: &str, path: &str, body: &str) -> Resp {
    Req::new("POST", path, port)
        .cookie(cookie)
        .origin(&origin_of(port))
        .json(body)
        .send(port)
}
