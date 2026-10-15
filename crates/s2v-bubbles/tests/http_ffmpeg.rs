//! 実際の ffmpeg で、`/media/frame` が動画のコマを返すことを確かめる。
//! ffmpeg が無い環境ではテストは失敗する。飛ばすのは `S2V_SKIP_FFMPEG_TESTS=1` のときだけ。

mod common;

use common::*;
use s2v_bubbles::api::SessionAsset;
use s2v_bubbles::media::open_locked;
use s2v_render::assets::AssetKind;
use std::collections::BTreeMap;
use std::process::Command;

fn skip() -> bool {
    std::env::var("S2V_SKIP_FFMPEG_TESTS").as_deref() == Ok("1")
}

#[test]
fn frame_endpoint_returns_png_and_rejects_out_of_range() {
    if skip() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let video = dir.path().join("gray.mp4");
    // コマ N の左半分が N*2 の灰色、右半分は黒の 90 コマ(30 fps で 3 秒 = C01 の長さ)
    let out = Command::new("ffmpeg")
        .args(["-nostdin", "-hide_banner", "-v", "error", "-y"])
        .args([
            "-f",
            "lavfi",
            "-i",
            "color=c=black:s=64x32:r=30:d=3,format=gbrp",
        ])
        .args([
            "-vf",
            "geq=r='if(lt(X,32),2*N,0)':g='if(lt(X,32),2*N,0)':b='if(lt(X,32),2*N,0)',scale=out_color_matrix=bt709:out_range=tv,format=yuv420p",
        ])
        .args(["-frames:v", "90", "-c:v", "libx264", "-crf", "0"])
        .args(["-colorspace", "bt709", "-color_range", "tv"])
        .arg(&video)
        .output()
        .expect("ffmpeg を起動できません(PATH に ffmpeg が必要です。飛ばすなら S2V_SKIP_FFMPEG_TESTS=1)");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

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
            path: video.clone(),
        },
    );
    let mut media = BTreeMap::new();
    media.insert("S01-C01".to_string(), open_locked(&video).unwrap());
    let (_, audio) = locked_file(dir.path(), "full_dialogue.wav", 100);
    let srv = start(session_in(dir.path(), "あいう", assets), media, audio);
    let p = srv.port;
    let c = login(p, &srv.token);

    // state の frames と asset の記述
    let st = Req::new("GET", "/api/state", p).cookie(&c).send(p).json();
    let cut = &st["scenes"][0]["cuts"][0];
    let n0 = cut["frames"][0].as_i64().unwrap();
    let n1 = cut["frames"][1].as_i64().unwrap();
    assert_eq!(cut["asset"]["frame_url"], "/media/frame/S01-C01");
    assert_eq!((n0, n1), (0, 90));

    let r = Req::new("GET", &format!("/media/frame/S01-C01?n={}", n0 + 3), p)
        .cookie(&c)
        .send(p);
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.header("Content-Type"), Some("image/png"));
    let pm = tiny_skia::Pixmap::decode_png(&r.body).expect("PNG として読める");
    assert_eq!((pm.width(), pm.height()), (64, 32));
    let left = pm.pixel(8, 16).unwrap();
    assert!(
        (i32::from(left.red()) - 6).abs() <= 3,
        "左半分 {}",
        left.red()
    );

    // 範囲の外・整数でない・欠けている → 400
    for q in [
        format!("n={n1}"),
        "n=-1".to_string(),
        "n=x".to_string(),
        "n=".to_string(),
        "".to_string(),
        "n=99999999999999999999".to_string(),
    ] {
        let path = if q.is_empty() {
            "/media/frame/S01-C01".to_string()
        } else {
            format!("/media/frame/S01-C01?{q}")
        };
        let r = Req::new("GET", &path, p).cookie(&c).send(p);
        assert_eq!(r.status, 400, "{path}");
    }
    // 素材の無いカット・コマ数 0 のカット → 404
    assert_eq!(
        Req::new("GET", "/media/frame/S01-C02?n=100", p)
            .cookie(&c)
            .send(p)
            .status,
        404
    );
    // 素材のファイルもそのまま配信できる(Range 付き)
    let r = Req::new("GET", "/media/asset/S01-C01", p)
        .cookie(&c)
        .range("bytes=0-3")
        .send(p);
    assert_eq!(r.status, 206);
    assert_eq!(r.header("Content-Type"), Some("video/mp4"));
}
