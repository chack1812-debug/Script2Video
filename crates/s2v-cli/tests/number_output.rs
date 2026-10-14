use std::fs;
use std::process::Command;

#[test]
fn number_command_prints_counts_of_cuts_and_notes() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("out.txt");
    let src = "@scene 居間\n@cast\nA:田中\n\n@script\n#paragraph\n#note 朝\nA:一\n#cut\n";
    fs::write(&script, src).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_script2video"))
        .arg("number")
        .arg(&script)
        .output()
        .unwrap();

    assert!(output.status.success(), "{:?}", output.status);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("台詞 1 行、シーン 1 個、カット 1 個、ト書き 1 個に番号を付けました"),
        "{stdout}"
    );
}
