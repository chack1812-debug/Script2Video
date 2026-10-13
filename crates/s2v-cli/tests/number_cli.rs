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
