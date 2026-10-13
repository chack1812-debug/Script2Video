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
