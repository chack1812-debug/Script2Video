//! tiny_http 0.12 の挙動の確認(設計書 §5.4)。結果は計画 2d-1 の報告に記録する。
//! - Request は Send で、別スレッドへ移して応答できる
//! - 1つの応答の書き込みが遅くても(別スレッドに移していれば)、次の要求を recv できる
//! - Server::unblock で recv が戻る

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use tiny_http::{Response, Server};

fn start() -> (Arc<Server>, SocketAddr) {
    let server = Server::http("127.0.0.1:0").expect("待ち受けできません");
    let addr = server
        .server_addr()
        .to_ip()
        .expect("IP アドレスではありません");
    (Arc::new(server), addr)
}

/// 要求を送り、応答の全体を読んで文字列で返す。
fn get(addr: SocketAddr) -> String {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    write!(
        s,
        "GET / HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    out
}

#[test]
fn request_can_be_answered_on_another_thread_while_recv_continues() {
    let (server, addr) = start();
    let c1 = thread::spawn(move || get(addr));
    // 1つ目の要求を受け取り、別スレッドへ移して 2 秒待ってから応答する。
    let first = server.recv().unwrap();
    let slow = thread::spawn(move || {
        thread::sleep(Duration::from_secs(2));
        first.respond(Response::from_string("slow")).unwrap();
    });
    // その間に2つ目の接続を送り、主スレッドの recv で受けてすぐ応答する。
    let started = Instant::now();
    let c2 = thread::spawn(move || get(addr));
    let second = server.recv().unwrap();
    second.respond(Response::from_string("fast")).unwrap();
    let body2 = c2.join().unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );
    assert!(body2.ends_with("fast"), "{body2}");
    slow.join().unwrap();
    assert!(c1.join().unwrap().ends_with("slow"));
}

#[test]
fn unblock_makes_recv_return() {
    let (server, _addr) = start();
    let s2 = Arc::clone(&server);
    let t = thread::spawn(move || {
        thread::sleep(Duration::from_millis(200));
        s2.unblock();
    });
    let started = Instant::now();
    let r = server.recv();
    // 0.12 の recv は Result。unblock 後は Err で戻る(Ok(Request) では通さない)。
    assert!(r.is_err(), "unblock 後の recv は Err のはずが Ok だった");
    assert!(started.elapsed() < Duration::from_secs(5));
    t.join().unwrap();
}
