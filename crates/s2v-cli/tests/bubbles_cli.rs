//! `bubbles` コマンドの結合テスト。ビルドした実行ファイルを子プロセスとして起動し、生の TCP で話しかける。
//! ffmpeg が無い環境ではテストは失敗する。飛ばすのは `S2V_SKIP_FFMPEG_TESTS=1` のときだけ。
//! 子プロセスは必ず後始末する(失敗してもテストが止まらないように、待ちには期限を付ける)。

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use s2v_cli::{run_build, BuildArgs, Outcome};

const W: usize = 320;
const H: usize = 180;
const WAIT: Duration = Duration::from_secs(60);

fn skip() -> bool {
    std::env::var("S2V_SKIP_FFMPEG_TESTS").as_deref() == Ok("1")
}

const HEADER: &str = "@scene 居間\n@pause\nsentence 500\ncast 300\nparagraph 1000\n@cast\nA:田中,pan=-30\n\n@script\n";
const SIMPLE: &str =
    "#paragraph[S01] 朝\n#visual (image) 朝の居間。\n#note[N01] 翌朝、港。\n[S01-L001] A:おはよう。\n";

struct Project {
    _tmp: tempfile::TempDir,
    dir: PathBuf,
    script: PathBuf,
    config: PathBuf,
}

impl Project {
    fn log(&self) -> String {
        fs::read_to_string(self.dir.join("Process.log")).unwrap_or_default()
    }
    fn build(&self) -> Outcome {
        run_build(&BuildArgs {
            script: self.script.clone(),
            voice_dir: None,
            config: Some(self.config.clone()),
            no_cache: false,
        })
    }
}

fn font_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("s2v-render/tests/fonts/test-vrt2.ttf")
}

fn config_text(render_section: &str) -> String {
    let base = include_str!("../../../config.toml");
    let a = base.find("\n[render]").expect("[render] 節");
    let b = base.find("\n[prompts]").expect("[prompts] 節");
    format!("{}\n[render]\n{render_section}\n{}", &base[..a], &base[b..])
}

fn write_voice(path: &Path) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for i in 0..72_000usize {
        let v = if (9_600..57_600).contains(&i) {
            0.1 * (2.0 * std::f64::consts::PI * 440.0 * (i - 9_600) as f64 / 48_000.0).sin()
        } else {
            0.0
        };
        w.write_sample((v * 32_767.0).round() as i16).unwrap();
    }
    w.finalize().unwrap();
}

fn solid_png() -> Vec<u8> {
    let mut pm = tiny_skia::Pixmap::new(W as u32, H as u32).unwrap();
    pm.fill(tiny_skia::Color::from_rgba8(40, 80, 160, 255));
    pm.encode_png().unwrap()
}

/// 台本を書き、収録を置き、(あれば)素材を置き、build を通す。
fn project(with_asset: bool) -> Project {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("台本 テスト");
    let voices = dir.join("voices");
    let tracks = dir.join("tracks");
    fs::create_dir_all(&voices).unwrap();
    fs::create_dir_all(&tracks).unwrap();
    let script = dir.join("第1話.txt");
    fs::write(&script, format!("{HEADER}{SIMPLE}")).unwrap();
    write_voice(&voices.join("S01_L001.wav"));
    if with_asset {
        fs::write(tracks.join("S01-C01.png"), solid_png()).unwrap();
    }
    let config = dir.join("config.toml");
    let render = format!(
        "width = {W}\nheight = {H}\nfps = 30\nfont_path = '{}'\nfont_size = 16.0\nbubble_padding = 6\nnote_padding = 4\nmargin = 8\nbubble_gap = 6\ntail_length = 12\npreset = \"ultrafast\"\ncrf = 10\n",
        font_path().display()
    );
    fs::write(&config, config_text(&render)).unwrap();
    let p = Project {
        _tmp: tmp,
        dir,
        script,
        config,
    };
    assert!(p.build() != Outcome::Failed, "build が失敗しました");
    p
}

/// 起動した子プロセス。落とすときに必ず終わらせる。
struct Bubbles {
    child: Child,
    lines: Receiver<String>,
    stderr: Arc<Mutex<String>>,
    url: Option<String>,
}

impl Drop for Bubbles {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn spawn(p: &Project, extra: &[&str]) -> Bubbles {
    spawn_with(p, extra, true, &[])
}

fn spawn_with(p: &Project, extra: &[&str], no_open: bool, env: &[(&str, &str)]) -> Bubbles {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_script2video"));
    cmd.envs(env.iter().copied());
    if !no_open {
        cmd.arg("bubbles")
            .arg(&p.script)
            .arg("--config")
            .arg(&p.config)
            .args(extra);
    } else {
        cmd.arg("bubbles")
            .arg(&p.script)
            .arg("--config")
            .arg(&p.config)
            .arg("--no-open")
            .args(extra);
    }
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("script2video を起動できません");
    let (tx, lines) = channel();
    let out = child.stdout.take().unwrap();
    std::thread::spawn(move || {
        for l in BufReader::new(out).lines().map_while(Result::ok) {
            if tx.send(l).is_err() {
                break;
            }
        }
    });
    let stderr = Arc::new(Mutex::new(String::new()));
    let err = child.stderr.take().unwrap();
    let sink = Arc::clone(&stderr);
    std::thread::spawn(move || {
        for l in BufReader::new(err).lines().map_while(Result::ok) {
            let mut s = sink.lock().unwrap();
            s.push_str(&l);
            s.push('\n');
        }
    });
    Bubbles {
        child,
        lines,
        stderr,
        url: None,
    }
}

impl Bubbles {
    /// 標準出力から URL の行を待つ。
    fn wait_url(&mut self) -> String {
        let deadline = Instant::now() + WAIT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left.max(Duration::from_millis(1))) {
                Ok(l) if l.starts_with("http://127.0.0.1:") => {
                    self.url = Some(l.clone());
                    return l;
                }
                Ok(_) => {}
                Err(_) => panic!(
                    "URL が出ませんでした\nstderr:\n{}",
                    self.stderr.lock().unwrap()
                ),
            }
        }
    }

    /// 終了コードを待つ(期限つき)。
    fn wait_code(&mut self) -> i32 {
        let deadline = Instant::now() + WAIT;
        loop {
            if let Some(st) = self.child.try_wait().unwrap() {
                return st.code().unwrap_or(-1);
            }
            assert!(
                Instant::now() < deadline,
                "終了しませんでした\nstderr:\n{}",
                self.stderr.lock().unwrap()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn port(&self) -> u16 {
        let url = self.url.as_ref().expect("wait_url を先に呼ぶ");
        url.trim_start_matches("http://127.0.0.1:")
            .split('/')
            .next()
            .unwrap()
            .parse()
            .unwrap()
    }

    fn token(&self) -> String {
        self.url
            .as_ref()
            .unwrap()
            .split("token=")
            .nth(1)
            .unwrap()
            .to_string()
    }
}

/// 生の HTTP 要求。戻り値は (状態コード, ヘッダーの文字列, 本文)。
fn http(
    port: u16,
    method: &str,
    path: &str,
    headers: &[(&str, String)],
    body: &[u8],
) -> (u16, String, Vec<u8>) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("接続できません");
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    s.set_write_timeout(Some(Duration::from_secs(20))).unwrap();
    let mut req =
        format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n");
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    if method == "POST" {
        req.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    req.push_str("\r\n");
    s.write_all(req.as_bytes()).unwrap();
    s.write_all(body).unwrap();
    let mut raw = Vec::new();
    let _ = s.read_to_end(&mut raw);
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("応答が不完全です");
    let head = String::from_utf8_lossy(&raw[..split]).to_string();
    let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
    (status, head, raw[split + 4..].to_vec())
}

/// トークンでログインして、クッキーの値(`名前=値`)を返す。
fn login(b: &Bubbles) -> String {
    let (status, head, _) = http(b.port(), "GET", &format!("/?token={}", b.token()), &[], b"");
    assert_eq!(status, 303, "{head}");
    let line = head
        .lines()
        .find(|l| l.to_ascii_lowercase().starts_with("set-cookie:"))
        .expect("Set-Cookie がありません");
    line.split_once(':')
        .unwrap()
        .1
        .trim()
        .split(';')
        .next()
        .unwrap()
        .to_string()
}

fn quit(b: &Bubbles, cookie: &str) {
    let port = b.port();
    let (status, head, _) = http(
        port,
        "POST",
        "/api/quit",
        &[
            ("Cookie", cookie.to_string()),
            ("Origin", format!("http://127.0.0.1:{port}")),
            ("Content-Type", "application/json".to_string()),
        ],
        b"{}",
    );
    assert_eq!(status, 200, "{head}");
}

fn state(b: &Bubbles, cookie: &str) -> serde_json::Value {
    let (status, head, body) = http(
        b.port(),
        "GET",
        "/api/state",
        &[("Cookie", cookie.to_string())],
        b"",
    );
    assert_eq!(status, 200, "{head}");
    serde_json::from_slice(&body).unwrap()
}

#[test]
fn starts_prints_url_and_quits() {
    if skip() {
        return;
    }
    let p = project(true);
    let mut b = spawn(&p, &[]);
    b.wait_url();
    let cookie = login(&b);
    let st = state(&b, &cookie);
    assert_eq!(st["canvas"]["w"], W);
    quit(&b, &cookie);
    let code = b.wait_code();
    assert!(
        code == 0 || code == 1,
        "終了コード {code}\n{}",
        b.stderr.lock().unwrap()
    );
    let log = p.log();
    assert!(log.contains("== bubbles "), "{log}");
    assert!(log.contains("bubbles を終了しました"), "{log}");
    assert!(
        !log.contains(&b.token()),
        "トークンが Process.log に書かれています"
    );
}

#[test]
fn lock_conflict_is_failed() {
    if skip() {
        return;
    }
    let p = project(true);
    let mut b = spawn(&p, &[]);
    b.wait_url();
    let cookie = login(&b);
    assert_eq!(p.build(), Outcome::Failed, "起動中の build は失敗するはず");
    quit(&b, &cookie);
    b.wait_code();
    assert!(p.build() != Outcome::Failed, "quit の後は build できるはず");
}

#[test]
fn missing_asset_starts_with_warning() {
    if skip() {
        return;
    }
    let p = project(false);
    let mut b = spawn(&p, &[]);
    b.wait_url();
    let cookie = login(&b);
    let st = state(&b, &cookie);
    let warnings = st["warnings"].to_string();
    assert!(warnings.contains("素材なし"), "{warnings}");
    quit(&b, &cookie);
    assert_eq!(b.wait_code(), 1);
}

#[test]
fn busy_port_is_failed() {
    if skip() {
        return;
    }
    let p = project(true);
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port().to_string();
    let mut b = spawn(&p, &["--port", &port]);
    assert_eq!(b.wait_code(), 2, "{}", b.stderr.lock().unwrap());
    drop(l);
}

#[test]
fn stale_build_is_failed() {
    if skip() {
        return;
    }
    let p = project(true);
    // build の後に音声を書き換える
    let wav = p.dir.join("full_dialogue.wav");
    let mut bytes = fs::read(&wav).unwrap();
    let n = bytes.len();
    bytes[n - 2] ^= 0x55;
    bytes[n - 1] ^= 0x55;
    fs::write(&wav, bytes).unwrap();
    let mut b = spawn(&p, &[]);
    assert_eq!(b.wait_code(), 2, "{}", b.stderr.lock().unwrap());
}

#[test]
fn browser_open_failure_makes_exit_code_one() {
    if skip() {
        return;
    }
    let p = project(true);
    let mut b = spawn_with(&p, &[], false, &[("S2V_BUBBLES_TEST_OPEN_FAIL", "1")]);
    b.wait_url();
    let cookie = login(&b);
    quit(&b, &cookie);
    assert_eq!(b.wait_code(), 1, "{}", b.stderr.lock().unwrap());
    assert!(
        p.log().contains("ブラウザを開けませんでした"),
        "{}",
        p.log()
    );
}
