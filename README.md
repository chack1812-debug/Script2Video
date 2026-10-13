# Script2Video

台本と収録音声から、動画用の音声・字幕・タイムラインを作るコマンドラインツール。

```
script2video number <台本.txt>
script2video build <台本.txt> --voice-dir <収録フォルダ> [--config <設定.toml>] [--no-cache]
```

## 終了コード

- `0`: すべて正常
- `1`: 警告または不足した台詞あり(出力は完成している)
- `2`: 出力できない(設定値の範囲外、台本の構文エラー、配置エラー、`work/` に書けない、入出力の失敗など)

出力を始める前に判明する失敗(設定値の範囲外・台本の構文エラー)では、前回の出力には触れません。
出力を始めた後に失敗した場合は、古い成果物が残らないよう、前回の `full_dialogue.wav`、`tracks/*.wav`、`timeline.json`、`subtitles.srt`、`timeline.fcpxml` を消します(`work/` は消しません)。失敗の理由は `Process.log` に記録します。

## 使い方

```bash
# 1. 台本に台詞IDとシーン番号を書き込む(台本名.bak・台本名.ids・台本名.html も出力)
cargo run --release -p s2v-cli -- number 第1話.txt

# 2. 台詞ごとに収録する(ファイル名は台詞IDの - を _ にしたもの: S01_L003.wav。モノラル)

# 3. 音声・字幕・タイムラインを作る(出力は台本と同じフォルダの 第1話/)
cargo run --release -p s2v-cli -- build 第1話.txt --voice-dir 収録/
```

`build` のオプション: `--config 設定.toml`(省略時は同梱の `config.toml`)、`--no-cache`(中間ファイルを使わない)。

台本の書式は [台本仕様.md](台本仕様.md)、設計は [docs/superpowers/specs/2026-10-12-script2video-design.md](docs/superpowers/specs/2026-10-12-script2video-design.md) を参照。

## 出力(台本名/)

| ファイル | 内容 |
|---|---|
| `full_dialogue.wav` | 台詞・BGM・SE の合成(24bit、マスターのソフトリミッタ後) |
| `tracks/dialogue.wav` `tracks/bgm.wav` `tracks/se.wav` | 動画編集ソフト用の別トラック(32bit 浮動小数、マスター前) |
| `timeline.json` | 後続工程が読む正本(schema_version 2) |
| `subtitles.srt` | 字幕 |
| `timeline.fcpxml` | Filmora 向け(3つのトラックを3レーンに配置) |
| `Process.log` | 処理の記録・警告・エラー |
| `work/` | 中間ファイル(削除してよい) |

## ①a の完了条件の確認

実収録の素材(複数の収録セッション・音量の異なる台詞を含む)で、次を行う。

1. 設定ファイルの `[normalize] dump_prefade = true` にして `build` する。
2. `cargo run -p s2v-cli --example verify_loudness -- 第1話` を実行し、すべて `OK` になることを確かめる。

確認の内容: 測定範囲が 400ms 以上の台詞は、フェード前の音量が目標の ±1 LU に入ること。400ms 未満の台詞は、適用ゲインが「目標 − 測定値」と一致すること。ゲインが制限に掛かった台詞は対象外。出力は何も書き換えない。

終了コード: 0=すべて OK、1=不合格あり、2=確認できない(`timeline.json`・`work/cache.json`・`work/lines_prefade/` がない、確認できる台詞が 0 件など)。
