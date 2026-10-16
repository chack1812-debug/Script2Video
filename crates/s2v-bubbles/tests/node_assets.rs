//! 画面の JS のテスト(Node の node:test。禁止の検査も含む)を cargo test から呼ぶ(設計書 §5.3・§8)。
//! Node が無ければ失敗する。S2V_SKIP_NODE_TESTS=1 のときだけ飛ばす。

#[test]
fn node_unit_tests_pass() {
    if std::env::var("S2V_SKIP_NODE_TESTS").as_deref() == Ok("1") {
        return;
    }
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let out = std::process::Command::new("node")
        .args(["--test", "assets-test/*.test.mjs"])
        .current_dir(dir)
        .output()
        .expect("node を起動できません(PATH に Node が必要です。飛ばすなら S2V_SKIP_NODE_TESTS=1)");
    assert!(
        out.status.success(),
        "node --test が失敗しました\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
