//! 実際の ffmpeg で、動画の指定したコマを PNG で取り出せることを確かめる。
//! ffmpeg が無い環境ではテストは失敗する。飛ばすのは `S2V_SKIP_FFMPEG_TESTS=1` のときだけ。

use s2v_bubbles::frame::{frame_args, FrameWorker};
use std::process::Command;

fn skip() -> bool {
    std::env::var("S2V_SKIP_FFMPEG_TESTS").as_deref() == Ok("1")
}

#[test]
fn extracts_the_requested_frame_of_a_video() {
    if skip() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let video = dir.path().join("gray.mp4");
    // コマ N の左半分が N*4 の灰色(RGB)、右半分は黒の 60 コマ
    let out = Command::new("ffmpeg")
        .args(["-nostdin", "-hide_banner", "-v", "error", "-y"])
        .args([
            "-f",
            "lavfi",
            "-i",
            "color=c=black:s=64x32:r=30:d=2,format=gbrp",
        ])
        .args([
            "-vf",
            "geq=r='if(lt(X,32),4*N,0)':g='if(lt(X,32),4*N,0)':b='if(lt(X,32),4*N,0)',scale=out_color_matrix=bt709:out_range=tv,format=yuv420p",
        ])
        .args(["-frames:v", "60", "-c:v", "libx264", "-crf", "0"])
        .args(["-colorspace", "bt709", "-color_range", "tv"])
        .arg(&video)
        .output()
        .expect("ffmpeg を起動できません(PATH に ffmpeg が必要です。飛ばすなら S2V_SKIP_FFMPEG_TESTS=1)");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let worker = FrameWorker::start("ffmpeg".to_string());
    let args = frame_args(video.to_str().unwrap(), 10, 30, "bt709", "tv");
    let png = worker
        .try_submit(args)
        .unwrap()
        .recv()
        .unwrap()
        .expect("コマを取り出せる");
    worker.shutdown();

    let pm = tiny_skia::Pixmap::decode_png(&png).expect("PNG として読める");
    assert_eq!((pm.width(), pm.height()), (64, 32));
    let left = pm.pixel(8, 16).unwrap();
    let right = pm.pixel(56, 16).unwrap();
    for c in [left.red(), left.green(), left.blue()] {
        assert!((i32::from(c) - 40).abs() <= 2, "左半分 {c}");
    }
    assert!(right.red() <= 4, "右半分 {}", right.red());
}
