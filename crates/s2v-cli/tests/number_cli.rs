use std::fs;

use s2v_cli::{run_number, Outcome};

const SCRIPT: &str =
    "@scene 居間\n@cast\nA:田中\n\nB:佐藤\n\n@script\n#paragraph 朝\nA:おはよう\nB:おはよう\n";

#[test]
fn number_rewrites_script_and_writes_bak_ids_and_html() {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("台本 テスト");
    fs::create_dir(&work).unwrap();
    let script = work.join("第1話.txt");
    fs::write(&script, SCRIPT).unwrap();

    assert_eq!(run_number(&script), Outcome::Ok);

    let text = fs::read_to_string(&script).unwrap();
    assert!(
        text.contains("#paragraph[S01] 朝\n[S01-L001] A:おはよう\n[S01-L002] B:おはよう\n"),
        "{text}"
    );
    assert_eq!(fs::read_to_string(work.join("第1話.bak")).unwrap(), SCRIPT);
    assert!(fs::read_to_string(work.join("第1話.ids"))
        .unwrap()
        .contains("max_line=2"));
    assert!(fs::read_to_string(work.join("第1話.html"))
        .unwrap()
        .contains("S01-L002"));

    // 2回目は何も変わらない
    assert_eq!(run_number(&script), Outcome::Ok);
    assert_eq!(fs::read_to_string(&script).unwrap(), text);
}

#[test]
fn number_refuses_duplicates_and_leaves_file_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("dup.txt");
    let src = "@scene 居間\n@cast\nA:田中\n\n@script\n#paragraph[S01]\n[S01-L001] A:一\n[S01-L001] A:二\n";
    fs::write(&script, src).unwrap();
    assert_eq!(run_number(&script), Outcome::Failed);
    assert_eq!(fs::read_to_string(&script).unwrap(), src);
    assert!(!dir.path().join("dup.bak").exists());
}

#[test]
fn number_failure_writes_no_ids_or_html() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("dup.txt");
    let src = "@scene 居間\n@cast\nA:田中\n\n@script\n#paragraph[S01]\n[S01-L001] A:一\n[S01-L001] A:二\n";
    fs::write(&script, src).unwrap();
    assert_eq!(run_number(&script), Outcome::Failed);
    assert!(!dir.path().join("dup.ids").exists());
    assert!(!dir.path().join("dup.html").exists());
}

#[test]
fn number_rejects_missing_and_non_utf8_script() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(run_number(&dir.path().join("none.txt")), Outcome::Failed);

    let bad = dir.path().join("sjis.txt");
    let bytes = [0x82u8, 0xA0, b'\n'];
    fs::write(&bad, bytes).unwrap();
    assert_eq!(run_number(&bad), Outcome::Failed);
    assert_eq!(fs::read(&bad).unwrap(), bytes);
    assert!(!dir.path().join("sjis.bak").exists());
}

#[test]
fn bak_is_byte_exact_with_bom_and_crlf() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("crlf.txt");
    let src = format!("\u{feff}{}", SCRIPT.replace('\n', "\r\n"));
    fs::write(&script, &src).unwrap();
    assert_eq!(run_number(&script), Outcome::Ok);
    assert_eq!(
        fs::read(dir.path().join("crlf.bak")).unwrap(),
        src.as_bytes()
    );
    let out = fs::read(&script).unwrap();
    assert!(out.starts_with("\u{feff}".as_bytes()));
    assert!(String::from_utf8(out)
        .unwrap()
        .contains("[S01-L001] A:おはよう\r\n"));
}

#[test]
fn script_named_like_a_derived_file_is_refused_untouched() {
    for name in ["foo.ids", "foo.html", "foo.bak", "FOO.IDS"] {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join(name);
        fs::write(&script, SCRIPT).unwrap();
        assert_eq!(run_number(&script), Outcome::Failed, "{name}");
        assert_eq!(fs::read(&script).unwrap(), SCRIPT.as_bytes(), "{name}");
        let entries = fs::read_dir(dir.path()).unwrap().count();
        assert_eq!(entries, 1, "{name}: 他のファイルが作られた");
    }
}

#[test]
fn unreadable_ids_fails_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("s.txt");
    fs::write(&script, SCRIPT).unwrap();
    // ディレクトリは .ids として読めない(NotFound 以外のエラー)
    fs::create_dir(dir.path().join("s.ids")).unwrap();
    assert_eq!(run_number(&script), Outcome::Failed);
    assert_eq!(fs::read(&script).unwrap(), SCRIPT.as_bytes());
    assert!(!dir.path().join("s.bak").exists());
    assert!(!dir.path().join("s.html").exists());
}

#[test]
fn non_utf8_ids_fails_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("s.txt");
    fs::write(&script, SCRIPT).unwrap();
    fs::write(dir.path().join("s.ids"), [0xffu8, 0xfe, 0x82]).unwrap();
    assert_eq!(run_number(&script), Outcome::Failed);
    assert_eq!(fs::read(&script).unwrap(), SCRIPT.as_bytes());
    assert_eq!(
        fs::read(dir.path().join("s.ids")).unwrap(),
        [0xffu8, 0xfe, 0x82]
    );
    assert!(!dir.path().join("s.bak").exists());
}

#[test]
fn unwritable_ids_target_leaves_script_untouched() {
    // 読めるが書けない .ids を作るのは移植性が低いので、書込失敗は
    // 「.ids がディレクトリ」で代用する(読込段階で失敗し、何も書かない)。
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("w.txt");
    fs::write(&script, SCRIPT).unwrap();
    fs::create_dir(dir.path().join("w.ids")).unwrap();
    assert_eq!(run_number(&script), Outcome::Failed);
    assert_eq!(fs::read(&script).unwrap(), SCRIPT.as_bytes());
    assert!(!dir.path().join("w.bak").exists());
}

#[test]
fn second_run_keeps_script_and_bak_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("t.txt");
    fs::write(&script, SCRIPT).unwrap();
    assert_eq!(run_number(&script), Outcome::Ok);
    let bak = dir.path().join("t.bak");
    let mtime = |p: &std::path::Path| fs::metadata(p).unwrap().modified().unwrap();
    let (script_m, bak_m) = (mtime(&script), mtime(&bak));
    let script_bytes = fs::read(&script).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert_eq!(run_number(&script), Outcome::Ok);
    assert_eq!(fs::read(&script).unwrap(), script_bytes);
    assert_eq!(fs::read(&bak).unwrap(), SCRIPT.as_bytes());
    assert_eq!(mtime(&script), script_m);
    assert_eq!(mtime(&bak), bak_m);
}

#[test]
fn ids_high_water_is_continued() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("h.txt");
    fs::write(&script, SCRIPT).unwrap();
    fs::write(dir.path().join("h.ids"), "max_line=10\nmax_scene=4\n").unwrap();
    assert_eq!(run_number(&script), Outcome::Ok);
    let text = fs::read_to_string(&script).unwrap();
    assert!(
        text.contains("#paragraph[S05] 朝\n[S05-L011] A:おはよう\n[S05-L012] B:おはよう\n"),
        "{text}"
    );

    // 最後の行を削除して新しい行を足しても、番号は高水位から続く
    let edited = text.replace("[S05-L012] B:おはよう\n", "A:新しい\n");
    fs::write(&script, edited).unwrap();
    assert_eq!(run_number(&script), Outcome::Ok);
    let text = fs::read_to_string(&script).unwrap();
    assert!(text.contains("[S05-L013] A:新しい"), "{text}");
    assert!(!text.contains("L012"), "{text}");
}

#[test]
fn html_escapes_dialogue_through_whole_command() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("e.txt");
    let src = "@scene 居間\n@cast\nA:田中\n\n@script\n#paragraph 朝\nA:\"やあ\" <script>alert(1)</script>\n";
    fs::write(&script, src).unwrap();
    assert_eq!(run_number(&script), Outcome::Ok);
    let html = fs::read_to_string(dir.path().join("e.html")).unwrap();
    assert!(!html.contains("<script>"), "{html}");
    assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
    assert!(html.contains("&quot;やあ&quot;"));
}

#[test]
fn silent_scene_is_rendered_without_crashing() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("q.txt");
    fs::write(
        &script,
        "@scene 無音\n@cast\nA:田中\n\n@script\n#pause 1.0\n",
    )
    .unwrap();
    assert_ne!(run_number(&script), Outcome::Failed);
    assert!(dir.path().join("q.html").exists());
}
