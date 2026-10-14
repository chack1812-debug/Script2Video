# Script2Video

台本と収録音声から、動画用の音声・字幕・タイムラインを作るコマンドラインツール。

```
script2video number <台本.txt>
script2video build <台本.txt> [--voice-dir <収録フォルダ>] [--config <設定.toml>] [--no-cache]
```

## 終了コード

- `0`: すべて正常
- `1`: 警告または不足した台詞あり(出力は完成している)
- `2`: 出力できない(設定値の範囲外、台本の構文エラー、配置エラー、`work/` に書けない、入出力の失敗など)

出力を始める前に判明する失敗(設定値の範囲外・台本の構文エラー)では、前回の出力には触れません。
出力を始めた後に失敗した場合は、古い成果物が残らないよう、前回の `full_dialogue.wav`、`tracks/*.wav`、`timeline/` の `timeline.json`・`subtitles.srt`・`timeline.fcpxml` だけを消します(`output.mp4`・`voices/`・`work/`・台本とその `.bak` `.ids` `.html` には触れません)。失敗の理由は `Process.log` に記録します。

## 使い方

```bash
# 1. 台本に台詞IDとシーン番号を書き込む(台本名.bak・台本名.ids・台本名.html も出力)
cargo run --release -p s2v-cli -- number 第1話.txt

# 2. 台詞ごとに収録する(ファイル名は台詞IDの - を _ にしたもの: S01_L003.wav。モノラル)
#    台詞番号の桁数は問いません(S01_L1.wav も S01_L001.wav も同じ台詞)

# 3. 音声・字幕・タイムラインを作る(出力は台本と同じフォルダ。収録は 台本フォルダ/voices/ に置く)
cargo run --release -p s2v-cli -- build 第1話.txt
# 収録を別の場所に置いたときだけ指定する
cargo run --release -p s2v-cli -- build 第1話.txt --voice-dir 収録/
```

`build` のオプション: `--voice-dir 収録フォルダ`(省略時は台本と同じフォルダの `voices/`。無い・フォルダでないときはエラー。相対パスは台本フォルダではなく現在のフォルダからの相対、既定の `voices` だけが台本フォルダからの相対)、`--config 設定.toml`(省略時は同梱の `config.toml`)、`--no-cache`(中間ファイルを使わない)。

台本の書式は [台本仕様.md](台本仕様.md)、設計は [docs/superpowers/specs/2026-10-12-script2video-design.md](docs/superpowers/specs/2026-10-12-script2video-design.md) を参照。

## 出力(台本フォルダ)

```
台本フォルダ
|-台本.txt
|-full_dialogue.wav
|-output.mp4            完成動画。④最終合成が出力する(①は作らない・消さない)
|-Process.log           ログ(台本フォルダ直下)
|-voices/               収録音声(--voice-dir 省略時の既定)
|   |-S01_L0001.wav
|-timeline/
|   |-subtitles.srt
|   |-timeline.fcpxml
|   |-timeline.json
|-tracks/               音声トラック dialogue.wav / bgm.wav / se.wav(将来の映像素材他もここ)
|-work/                 中間ファイル(resampled/ lines/ lines_prefade/ cache.json)
```

`build` は台本のあるフォルダに直接出力する(台本名のサブフォルダは作らない)。`output.mp4` は ④最終合成が出力するもので、①は作らず、消さず、触れない。`voices/` は収録の置き場で、①は読むだけ。

| ファイル | 内容 |
|---|---|
| `full_dialogue.wav` | 台詞・BGM・SE の合成(24bit、マスターのソフトリミッタ後) |
| `tracks/dialogue.wav` `tracks/bgm.wav` `tracks/se.wav` | 動画編集ソフト用の別トラック(32bit 浮動小数、マスター前) |
| `timeline/timeline.json` | 後続工程が読む正本(schema_version 2) |
| `timeline/subtitles.srt` | 字幕 |
| `timeline/timeline.fcpxml` | Filmora 向け(3つのトラックを3レーンに配置) |
| `Process.log` | 処理の記録・警告・エラー |
| `work/` | 中間ファイル(削除してよい) |

次の場合は、何も書かず・消さずに終了コード 2 で止まります: 収録フォルダ(`--voice-dir`、省略時は `voices/`)が `work/`・`tracks/`・`timeline/` のどれかと同じ場所(相対パス・存在しないパス・大文字小文字違いも同じ場所とみなします)、台本のファイル名が `full_dialogue.wav`・`Process.log`・`output.mp4` のどれか(大文字小文字は区別しません。`voices.txt` などは問題ありません)、`work/`・`tracks/`・`timeline/`(と既定の `voices/`)がリンク(シンボリックリンク・ジャンクション)または台本フォルダの外を指している場合。

## ①a の完了条件の確認

実収録の素材(複数の収録セッション・音量の異なる台詞を含む)で、次を行う。

1. 設定ファイルの `[normalize] dump_prefade = true` にして `build` する。
2. `cargo run -p s2v-cli --example verify_loudness -- <台本フォルダ>` を実行し、すべて `OK` になることを確かめる。

確認の内容: 測定範囲が 400ms 以上の台詞は、フェード前の音量が目標の ±1 LU に入ること。400ms 未満の台詞は、適用ゲインが「目標 − 測定値」と一致すること。ゲインが制限に掛かった台詞は対象外。出力は何も書き換えない。

終了コード: 0=すべて OK、1=不合格あり、2=確認できない(`timeline/timeline.json`・`work/cache.json`・`work/lines_prefade/` がない、確認できる台詞が 0 件など)。
