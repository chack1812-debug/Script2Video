//! ローカル GUI サーバーの受け付けと振り分け(設計書 §5.4)。ここが安全の境界。
//!
//! 主スレッドは要求を1つずつ処理する。メディア(素材・音声)の応答は要求ごとのスレッド(同時 8 本まで)、
//! `/media/frame` は専用ワーカーへ渡して結果を待つ別スレッドで応答するので、主スレッドを止めない。

use crate::api::{self, ApiError, Session};
use crate::frame::{frame_args, FrameError, FrameWorker};
use crate::media::{media_response, reader, LockedFile, Slots};
use crate::security::{
    common_headers, constant_time_eq, cookie_ok, host_ok, json_content_type_ok, new_token,
    origin_ok, set_cookie_header, Token, MAX_BODY,
};
use s2v_render::assets::AssetKind;
use s2v_video::frames::asset_frame;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::{self, Cursor, Read};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Weak};
use std::thread;
use std::time::{Duration, Instant};
use tiny_http::{Header, Method, Request, Response, StatusCode};

const INDEX_HTML: &str = include_str!("assets/index.html");
const APP_JS: &str = include_str!("assets/app.js");
const APP_CSS: &str = include_str!("assets/app.css");

/// メディアのスレッドの同時数の上限。
const MAX_MEDIA_THREADS: usize = 8;
/// 終了のとき、応答のスレッドの終わりを待つ最大の時間。
const SHUTDOWN_WAIT: Duration = Duration::from_secs(5);
/// 受け付けを待つ間隔(終了の印と主スレッドへの知らせを見る)。
const POLL: Duration = Duration::from_millis(200);
const JSON_TYPE: &str = "application/json; charset=utf-8";

pub struct ServeOptions {
    /// 指定がなければ OS が決める。
    pub port: Option<u16>,
    pub ffmpeg: String,
    /// テスト用の差し込み口: 指定すればこの frame のワーカーを使う(`ffmpeg` は使わない)。
    #[doc(hidden)]
    pub frame_worker: Option<FrameWorker>,
}

/// 主スレッドへの知らせ(トークンを含めない)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogEvent {
    Info(String),
    Warn(String),
    Error(String),
}

/// 受け付けを止めさせる口(`/api/quit` の経路とテストから呼ぶ)。
#[derive(Clone)]
pub struct QuitHandle {
    http: Weak<tiny_http::Server>,
    flag: Arc<AtomicBool>,
}

impl QuitHandle {
    pub fn quit(&self) {
        self.flag.store(true, Ordering::SeqCst);
        if let Some(http) = self.http.upgrade() {
            http.unblock();
        }
    }
}

/// 動いている応答のスレッドの数(終了のとき待つ)。
struct ActiveGuard(Arc<AtomicUsize>);

impl ActiveGuard {
    fn new(counter: &Arc<AtomicUsize>) -> ActiveGuard {
        counter.fetch_add(1, Ordering::SeqCst);
        ActiveGuard(Arc::clone(counter))
    }
}

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

pub struct Server {
    http: Arc<tiny_http::Server>,
    port: u16,
    token: Token,
    session: Session,
    media: BTreeMap<String, Arc<LockedFile>>,
    audio: Arc<LockedFile>,
    slots: Arc<Slots>,
    frame: FrameWorker,
    quit: Arc<AtomicBool>,
    active: Arc<AtomicUsize>,
    log_tx: Sender<LogEvent>,
    log_rx: Receiver<LogEvent>,
}

enum Flow {
    Continue,
    Quit,
}

impl Server {
    /// 127.0.0.1 で待ち受ける。`opts.port` が使えなければ Err。
    pub fn bind(
        session: Session,
        media: BTreeMap<String, LockedFile>,
        audio: LockedFile,
        opts: ServeOptions,
    ) -> Result<Server, String> {
        let http =
            tiny_http::Server::http(("127.0.0.1", opts.port.unwrap_or(0))).map_err(
                |e| match opts.port {
                    Some(p) => format!("ポート {p} で待ち受けできません: {e}"),
                    None => format!("待ち受けできません: {e}"),
                },
            )?;
        let port = http
            .server_addr()
            .to_ip()
            .ok_or_else(|| "待ち受けのアドレスを得られません".to_string())?
            .port();
        let (log_tx, log_rx) = channel();
        Ok(Server {
            http: Arc::new(http),
            port,
            token: new_token(),
            session,
            media: media.into_iter().map(|(k, v)| (k, Arc::new(v))).collect(),
            audio: Arc::new(audio),
            slots: Slots::new(MAX_MEDIA_THREADS),
            frame: opts
                .frame_worker
                .unwrap_or_else(|| FrameWorker::start(opts.ffmpeg)),
            quit: Arc::new(AtomicBool::new(false)),
            active: Arc::new(AtomicUsize::new(0)),
            log_tx,
            log_rx,
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// ブラウザで開く URL(トークンを含む。表示するのはここだけ)。
    pub fn url(&self) -> String {
        format!(
            "http://127.0.0.1:{}/?token={}",
            self.port,
            self.token.as_str()
        )
    }

    pub fn quit_handle(&self) -> QuitHandle {
        QuitHandle {
            http: Arc::downgrade(&self.http),
            flag: Arc::clone(&self.quit),
        }
    }

    /// 受け付けのループ。`/api/quit` か `QuitHandle::quit` で終了の手順を行って戻る。
    /// ロックを外すのは呼び出し側(この関数が戻った後)。
    /// 戻った後に残ったメディアのスレッドは、呼び出し側がプロセスを終えることで片づく。
    pub fn run(self, log: &mut dyn FnMut(LogEvent)) -> Result<(), String> {
        let mut fatal: Option<String> = None;
        let mut errors = 0u32;
        loop {
            for ev in self.log_rx.try_iter() {
                log(ev);
            }
            if self.quit.load(Ordering::SeqCst) {
                break;
            }
            match self.http.recv_timeout(POLL) {
                Ok(Some(req)) => {
                    errors = 0;
                    if let Flow::Quit = self.handle(req) {
                        break;
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    if self.quit.load(Ordering::SeqCst) {
                        break;
                    }
                    errors += 1;
                    if errors >= 100 {
                        fatal = Some(format!("受け付けを続けられません: {e}"));
                        break;
                    }
                    log(LogEvent::Warn(format!("受け付けに失敗しました: {e}")));
                    thread::sleep(Duration::from_millis(50));
                }
            }
        }

        // 終了の手順: 受け付けを止める → ffmpeg を止める → 応答のスレッドの終わりを待つ
        let Server {
            http,
            frame,
            active,
            log_rx,
            ..
        } = self;
        drop(http);
        frame.shutdown();
        let deadline = Instant::now() + SHUTDOWN_WAIT;
        while active.load(Ordering::SeqCst) > 0 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        for ev in log_rx.try_iter() {
            log(ev);
        }
        log(LogEvent::Info("bubbles を終了しました".to_string()));
        match fatal {
            Some(m) => Err(m),
            None => Ok(()),
        }
    }

    fn note(&self, ev: LogEvent) {
        let _ = self.log_tx.send(ev);
    }

    /// 拒否の応答(403)。理由は記録に残す(トークンは含めない)。
    fn deny(&self, req: Request, path: &str, reason: &str) -> Flow {
        let shown: String = path.chars().take(80).collect();
        self.note(LogEvent::Warn(format!(
            "403 {} {:?}: {reason}",
            req.method(),
            shown
        )));
        reply_error(req, 403, "許可されていない要求です");
        Flow::Continue
    }

    fn handle(&self, mut req: Request) -> Flow {
        let method = req.method().clone();
        let url = req.url().to_string();
        let (path, query) = match url.split_once('?') {
            Some((p, q)) => (p, q),
            None => (url.as_str(), ""),
        };

        // 1. Host
        if !host_ok(one_header(&req, "Host"), self.port) {
            return self.deny(req, path, "Host が違います");
        }
        // 2. トークン → クッキー
        if method == Method::Get && path == "/" {
            if let Some(t) = query_param(query, "token") {
                if constant_time_eq(t.as_bytes(), self.token.as_str().as_bytes()) {
                    let cookie = set_cookie_header(self.port, &self.token);
                    let mut resp =
                        Response::new(StatusCode(303), vec![], io::empty(), Some(0), None);
                    add(&mut resp, "Location", "/");
                    add(&mut resp, "Set-Cookie", &cookie);
                    finish(req, resp);
                } else {
                    return self.deny(req, path, "トークンが違います");
                }
                return Flow::Continue;
            }
        }
        // 3. クッキー
        let cookie = cookie_header(&req);
        if !cookie_ok(cookie.as_deref(), self.port, &self.token) {
            return self.deny(req, path, "クッキーがありません");
        }

        // 7. POST は全てのパスで、振り分けの前に Origin → Content-Type → 本文の大きさを検査する
        let mut body = None;
        if method == Method::Post {
            if !origin_ok(one_header(&req, "Origin"), self.port) {
                return self.deny(req, path, "Origin が違います");
            }
            if !json_content_type_ok(one_header(&req, "Content-Type")) {
                reply_error(req, 415, "Content-Type は application/json にしてください");
                return Flow::Continue;
            }
            match read_body(&mut req) {
                Ok(b) => body = Some(b),
                Err(status) => {
                    let msg = if status == 413 {
                        "本文が大きすぎます"
                    } else {
                        "本文を読めません"
                    };
                    reply_error(req, status, msg);
                    return Flow::Continue;
                }
            }
        }

        match path {
            // 4. 画面
            "/" => self.page(req, &method, INDEX_HTML, "text/html; charset=utf-8"),
            "/app.js" => self.page(req, &method, APP_JS, "text/javascript; charset=utf-8"),
            "/app.css" => self.page(req, &method, APP_CSS, "text/css; charset=utf-8"),
            // 8. 状態
            "/api/state" => {
                if method != Method::Get {
                    return method_not_allowed(req, "GET");
                }
                let v = api::state_json(&self.session, &self.session.loaded);
                reply_json(req, 200, &v);
                Flow::Continue
            }
            // 5. メディア
            "/media/audio" => self.media(req, &method, Some(Arc::clone(&self.audio))),
            "/api/render" | "/api/layout" | "/api/save" | "/api/quit" => {
                if method != Method::Post {
                    return method_not_allowed(req, "POST");
                }
                self.post(req, path, &body.unwrap_or_default())
            }
            _ => {
                if let Some(id) = path.strip_prefix("/media/asset/") {
                    let file = self.media.get(id).map(Arc::clone);
                    return self.media(req, &method, file);
                }
                if let Some(id) = path.strip_prefix("/media/frame/") {
                    if method != Method::Get {
                        return method_not_allowed(req, "GET");
                    }
                    return self.frame(req, id, query);
                }
                // 9. 未知のパス
                reply_error(req, 404, "見つかりません");
                Flow::Continue
            }
        }
    }

    fn page(&self, req: Request, method: &Method, body: &'static str, ctype: &str) -> Flow {
        if *method != Method::Get {
            return method_not_allowed(req, "GET");
        }
        reply_bytes(req, 200, ctype, &[], body.as_bytes().to_vec());
        Flow::Continue
    }

    /// `GET`・`HEAD` のメディア。素材が無ければ 404、枠が無ければ 503、あればスレッドへ移す。
    fn media(&self, req: Request, method: &Method, file: Option<Arc<LockedFile>>) -> Flow {
        let is_head = match method {
            Method::Get => false,
            Method::Head => true,
            _ => return method_not_allowed(req, "GET, HEAD"),
        };
        let Some(file) = file else {
            reply_error(req, 404, "見つかりません");
            return Flow::Continue;
        };
        let Some(slot) = self.slots.try_acquire() else {
            reply_error(req, 503, "メディアの要求が多すぎます。少し待ってください");
            return Flow::Continue;
        };
        let range = one_header(&req, "Range").map(str::to_string);
        let active = ActiveGuard::new(&self.active);
        let spawned = thread::Builder::new()
            .name("bubbles-media".into())
            .spawn(move || {
                let _slot = slot;
                let _active = active;
                respond_media(req, &file, is_head, range.as_deref());
            });
        if let Err(e) = spawned {
            self.note(LogEvent::Error(format!(
                "メディアのスレッドを始められません: {e}"
            )));
        }
        Flow::Continue
    }

    /// `GET /media/frame/<ID>?n=<n>`。結果の受け取りは別スレッドで待つ。
    fn frame(&self, req: Request, id: &str, query: &str) -> Flow {
        let s = &self.session;
        let asset = s
            .assets
            .get(id)
            .filter(|a| a.kind == AssetKind::Video && self.media.contains_key(id));
        let (Some(asset), Some([n0, n1])) = (asset, api::cut_frames(s, id)) else {
            reply_error(req, 404, "見つかりません");
            return Flow::Continue;
        };
        let n = query_param(query, "n")
            .filter(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|v| v.parse::<i64>().ok());
        let Some(n) = n else {
            reply_error(req, 400, "n は整数のコマ番号にしてください");
            return Flow::Continue;
        };
        if n < n0 || n >= n1 {
            reply_error(req, 400, "n がカットのコマの範囲の外です");
            return Flow::Continue;
        }
        let k = asset_frame(n, n0, asset.frames.unwrap_or(i64::MAX), asset.looped);
        let args = frame_args(
            &asset.path.to_string_lossy(),
            k,
            s.cfg.fps,
            &asset.matrix,
            &asset.range,
        );
        let rx = match self.frame.try_submit(args) {
            Ok(rx) => rx,
            Err(FrameError::Busy) => {
                reply_error(req, 503, "コマを取り出している最中です。少し待ってください");
                return Flow::Continue;
            }
            Err(e) => {
                self.note(LogEvent::Error(format!("/media/frame/{id}: {e:?}")));
                reply_error(req, 500, "コマを取り出せません");
                return Flow::Continue;
            }
        };
        let active = ActiveGuard::new(&self.active);
        let tx = self.log_tx.clone();
        let id = id.to_string();
        let spawned = thread::Builder::new()
            .name("bubbles-frame".into())
            .spawn(move || {
                let _active = active;
                match rx.recv() {
                    Ok(Ok(png)) => reply_bytes(req, 200, "image/png", &[], png),
                    Ok(Err(e)) => {
                        let _ = tx.send(LogEvent::Error(format!("/media/frame/{id}: {e:?}")));
                        reply_error(req, 500, "コマを取り出せません");
                    }
                    Err(_) => {
                        let _ = tx.send(LogEvent::Error(format!(
                            "/media/frame/{id}: ワーカーが応答せず止まりました"
                        )));
                        reply_error(req, 500, "コマを取り出せません");
                    }
                }
            });
        if let Err(e) = spawned {
            self.note(LogEvent::Error(format!(
                "コマのスレッドを始められません: {e}"
            )));
        }
        Flow::Continue
    }

    /// 検査を通った POST の処理。
    fn post(&self, req: Request, path: &str, body: &[u8]) -> Flow {
        let s = &self.session;
        match path {
            "/api/quit" => {
                reply_json(req, 200, &json!({"quitting": true}));
                self.quit.store(true, Ordering::SeqCst);
                return Flow::Quit;
            }
            "/api/layout" => match api::parse_manual(body, s).and_then(|m| api::layout_json(s, &m))
            {
                Ok(v) => reply_json(req, 200, &v),
                Err(e) => self.api_error(req, e),
            },
            "/api/save" => match api::parse_manual(body, s).and_then(|m| api::save(s, &m)) {
                Ok(warnings) => {
                    self.note(LogEvent::Info("bubbles.json を保存しました".to_string()));
                    reply_json(req, 200, &json!({"saved": true, "warnings": warnings}));
                }
                Err(e) => self.api_error(req, e),
            },
            "/api/render" => {
                let result = api::parse_manual(body, s).and_then(|manual| {
                    let v: Value = serde_json::from_slice(body)
                        .map_err(|_| ApiError::BadRequest("JSON を読めません".into()))?;
                    let id = v
                        .get("id")
                        .and_then(Value::as_str)
                        .ok_or_else(|| ApiError::BadRequest("id がありません".into()))?;
                    api::render_png(s, id, &manual)
                });
                match result {
                    Ok((png, dx, dy, clipped)) => {
                        let extra = [
                            ("X-S2V-Origin", format!("{dx},{dy}")),
                            ("X-S2V-Clipped", if clipped { "1" } else { "0" }.to_string()),
                        ];
                        reply_bytes(req, 200, "image/png", &extra, png);
                    }
                    Err(e) => self.api_error(req, e),
                }
            }
            _ => reply_error(req, 404, "見つかりません"),
        }
        Flow::Continue
    }

    fn api_error(&self, req: Request, e: ApiError) {
        match e {
            ApiError::BadRequest(m) => reply_error(req, 400, &m),
            ApiError::NotFound => reply_error(req, 404, "見つかりません"),
            ApiError::Unprocessable(errs) => reply_json(req, 422, &json!({"errors": errs})),
            ApiError::Internal(m) => {
                self.note(LogEvent::Error(format!("500 {m}")));
                reply_error(req, 500, &m);
            }
        }
    }
}

/// 同じ名前のヘッダーが1つだけならその値。複数あれば検査に通らない空文字を返す。
fn one_header<'a>(req: &'a Request, name: &'static str) -> Option<&'a str> {
    let mut it = req.headers().iter().filter(|h| h.field.equiv(name));
    let first = it.next()?;
    if it.next().is_some() {
        return Some("");
    }
    Some(first.value.as_str())
}

/// `Cookie` ヘッダー(複数あれば `; ` でつなぐ)。
fn cookie_header(req: &Request) -> Option<String> {
    let v: Vec<&str> = req
        .headers()
        .iter()
        .filter(|h| h.field.equiv("Cookie"))
        .map(|h| h.value.as_str())
        .collect();
    (!v.is_empty()).then(|| v.join("; "))
}

/// クエリの `key=value`(最初のもの)。値は解釈しない(パーセント復号をしない)。
fn query_param<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        (k == key).then_some(v)
    })
}

/// 本文を読む。`Content-Length` が無い・1 MiB 超は 413、読み出しも 1 MiB + 1 で打ち切る。
fn read_body(req: &mut Request) -> Result<Vec<u8>, u16> {
    match req.body_length() {
        None => return Err(413),
        Some(n) if n > MAX_BODY => return Err(413),
        Some(_) => {}
    }
    let mut buf = Vec::new();
    req.as_reader()
        .take(MAX_BODY as u64 + 1)
        .read_to_end(&mut buf)
        .map_err(|_| 400u16)?;
    if buf.len() > MAX_BODY {
        return Err(413);
    }
    Ok(buf)
}

fn add<R: Read>(resp: &mut Response<R>, name: &str, value: &str) {
    if let Ok(h) = Header::from_bytes(name.as_bytes(), value.as_bytes()) {
        resp.add_header(h);
    }
}

/// 共通のヘッダーを付けて送る(全ての応答はここを通る)。
fn finish<R: Read>(req: Request, mut resp: Response<R>) {
    for (k, v) in common_headers() {
        add(&mut resp, k, v);
    }
    let _ = req.respond(resp);
}

fn reply_bytes(req: Request, status: u16, ctype: &str, extra: &[(&str, String)], body: Vec<u8>) {
    let len = body.len();
    let mut resp = Response::new(
        StatusCode(status),
        vec![],
        Cursor::new(body),
        Some(len),
        None,
    );
    add(&mut resp, "Content-Type", ctype);
    for (k, v) in extra {
        add(&mut resp, k, v);
    }
    finish(req, resp);
}

fn reply_json(req: Request, status: u16, v: &Value) {
    let body = serde_json::to_vec(v).unwrap_or_else(|_| b"{}".to_vec());
    reply_bytes(req, status, JSON_TYPE, &[], body);
}

fn reply_error(req: Request, status: u16, msg: &str) {
    reply_json(req, status, &json!({"error": msg}));
}

fn method_not_allowed(req: Request, allow: &str) -> Flow {
    let body = serde_json::to_vec(&json!({"error": "許されないメソッドです"})).unwrap_or_default();
    let len = body.len();
    let mut resp = Response::new(StatusCode(405), vec![], Cursor::new(body), Some(len), None);
    add(&mut resp, "Content-Type", JSON_TYPE);
    add(&mut resp, "Allow", allow);
    finish(req, resp);
    Flow::Continue
}

/// メディアのスレッドの本体: Range に応じて 200 / 206 / 416 を返す。
fn respond_media(req: Request, file: &LockedFile, is_head: bool, range: Option<&str>) {
    let m = media_response(file, is_head, range);
    let body: Box<dyn Read + '_> = match m.body {
        Some((start, end)) => Box::new(reader(file, start, end)),
        None => Box::new(io::empty()),
    };
    // Content-Length を必ず付ける(分割転送にしない)
    let mut resp = Response::new(StatusCode(m.status), vec![], body, None, None)
        .with_chunked_threshold(usize::MAX);
    for (k, v) in &m.headers {
        add(&mut resp, k, v);
    }
    finish(req, resp);
}
