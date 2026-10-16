# Script2Video

台本と収録音声から、動画用の音声・字幕・タイムラインを作るコマンドラインツール。

```
script2video number <台本.txt>
script2video build <台本.txt> [--voice-dir <収録フォルダ>] [--config <設定.toml>] [--no-cache]
script2video prompts <台本.txt> [--config <設定.toml>]
script2video bubbles <台本.txt> [--config <設定.toml>] [--port <N>] [--no-open]
script2video render <台本.txt> [--config <設定.toml>]
```

## 終了コード

- `0`: すべて正常
- `1`: 警告または不足した台詞あり(出力は完成している)
- `2`: 出力できない(設定値の範囲外、台本の構文エラー、配置エラー、`work/` に書けない、入出力の失敗など)

出力を始める前に判明する失敗(設定値の範囲外・台本の構文エラー)では、前回の出力には触れません。
出力を始めた後に失敗した場合は、古い成果物が残らないよう、前回の `full_dialogue.wav`、`tracks/*.wav`、`timeline/` の `timeline.json`・`subtitles.srt`・`timeline.fcpxml` だけを消します(`voices/`・`work/`・台本とその `.bak` `.ids` `.html` には触れません)。台本の最後に書いた `#pause` は、その長さだけ `full_dialogue.wav` の末尾の無音になります。`build` は `output.mp4` を作りません。前回の `output.mp4` があれば、出力を始める時点で `output.stale.mp4` に改名します(前の `output.stale.mp4` は上書き)。改名は `Process.log` を作り直す前に行い、改名できなければ何も出力せず(前回の `Process.log` も変えず)、標準エラーにだけ理由を出して終了コード 2 で止まります。改名できたときは、作り直した `Process.log` の先頭に記録します。台本がシンボリックリンクのときは、`number`・`build` ともに何も書かずに終了コード 2 で止まります(実体のファイルを指定してください)。

同じ台本フォルダで `number`・`build` を同時に実行すると、後から始めた方が終了コード 2 で止まります(`.script2video.lock` で排他)。失敗の理由は `Process.log` に記録します。

## インストール

```bash
# リポジトリ直下で実行。~/.cargo/bin/script2video(.exe) に入る(PATH に ~/.cargo/bin が必要)
cargo install --path crates/s2v-cli --locked
script2video --help
```

コードを更新したら、同じコマンドをもう一度実行して入れ直す(`--force` を付けると同じバージョンでも入れ直す)。

## 使い方

```bash
# 1. 台本に台詞IDとシーン番号を書き込む(台本名.bak・台本名.ids・台本名.html も出力)
script2video number 第1話.txt

# 2. 台詞ごとに収録する(ファイル名は台詞IDの - を _ にしたもの: S01_L003.wav。モノラル)
#    台詞番号の桁数は問いません(S01_L1.wav も S01_L001.wav も同じ台詞)

# 3. 音声・字幕・タイムラインを作る(出力は台本と同じフォルダ。収録は 台本フォルダ/voices/ に置く)
script2video build 第1話.txt
# 収録を別の場所に置いたときだけ指定する
script2video build 第1話.txt --voice-dir 収録/
```

`build` のオプション: `--voice-dir 収録フォルダ`(省略時は台本と同じフォルダの `voices/`。無い・フォルダでないときはエラー。相対パスは台本フォルダではなく現在のフォルダからの相対、既定の `voices` だけが台本フォルダからの相対)、`--config 設定.toml`(省略時は同梱の `config.toml`)、`--no-cache`(中間ファイルを使わない)。

映像の区切りは台本に書きます。`#cut` でカットを切り替え、`#note` でト書きを出し、`#visual (video loop)` でループ動画を指定できます。`number` はこれらにもシーンごとの番号を付けます(`#cut[C02]`・`#note[N01]`)。

```
#visual (image) 港
#note[N01] 翌朝、港。
[S01-L001] A:おはよう。
#cut[C02]
#visual (video loop) 波
#note[N02] 二人は黙る。
```

詳しくは [台本仕様.md](台本仕様.md) の §2.8(`#cut`・`#visual`)・§2.9(`#note`)を参照。

台本の書式は [台本仕様.md](台本仕様.md)、設計は [docs/superpowers/specs/2026-10-12-script2video-design.md](docs/superpowers/specs/2026-10-12-script2video-design.md) と、映像工程の設計 [docs/superpowers/specs/2026-10-14-script2video-video-design.md](docs/superpowers/specs/2026-10-14-script2video-video-design.md) を参照。

## prompts(素材のプロンプトを作る)

`build` の後に実行する。`timeline.json` から、画像・動画の生成に使うプロンプトをカットごとに作り、台本フォルダの `prompts/` に書く。

```bash
script2video prompts 第1話.txt
script2video prompts 第1話.txt --config config.toml
```

出力は、シーンごとの `prompts/S01.md` … と、全カットの一覧 `prompts/index.md`。`S01.md` にはカットごとの節があり、各節に次を書く(長さ 0 のカットは節を作らない。全カットが長さ 0 のシーンはファイルを作らない)。

- 作る素材のファイル名(`tracks/S01.png` など。`#visual` に種類が無ければ「`tracks/S01.png` か `tracks/S01.mp4`」)
- 種類(画像・動画。動画はループの有無)
- 長さ(動画と種類が未指定のカットだけ。秒、小数1桁)
- プロンプト本文(コピーしやすいようコードブロックに入れる)

プロンプト本文は次の順で、空の要素は行ごと省く: `[prompts]` の `style`、シーン名、カットの `#visual`、登場人物とその外見(`@cast` の外見。無い役は名前だけ)、`[prompts]` の `suffix`。登場人物は、そのカットの間に話す人を台本の並びの初出順に並べる。`#visual` の無いカットは、節に「`#visual: なし`」と書いて警告する。

`index.md` の表には、カット ID・素材のファイル名・種類・長さ・「素材」の列(`tracks/` に見つかれば「あり(`S01.jpg`)」、無ければ「なし」、長さ 0 は「不要(長さ 0)」)を出す。`tracks/` の素材に誤り(同じカットに複数ある・種類違い・リンク)があれば、`index.md` の末尾の「tracks/ の確認事項」と `Process.log` に書く。ただし終了コードは変えない(誤りは `render` が終了コード 2 にする)。

`config.toml` の `[prompts]`:

| キー | 既定 | 意味 |
|---|---|---|
| `style` | 空 | プロンプトの先頭に付ける画風の指定 |
| `suffix` | 縦横比 16:9。画像の中に文字・吹き出しを入れない。 | プロンプトの末尾に付ける注意書き |

`prompts/` の中で上書き・削除するのは `index.md` と `S<数字>.md` だけ。新しいファイルをすべて書いた後で、今回は作らないシーンの前回のファイルを消す(書き込みに失敗したときは消さない)。自分のメモは別の名前(`memo.md` など)で置ける。

終了コード:

- `0`: 成功
- `1`: `#visual` の無いカットがある、または台本が `build` の後に変わった(プロンプトは書く)
- `2`: `build` がまだ(または `timeline.json` が使えない)、`prompts` がリンクか台本フォルダの外を指している、台本フォルダに `prompts` というファイルがある、書き込みの失敗、他の `number`・`build`・`render`・`prompts` が同じ台本フォルダで実行中(何も消さない)

## bubbles(吹き出し・ト書きの配置を画面で調整する)

```
script2video bubbles 第1話.txt
script2video bubbles 第1話.txt --config config.toml --port 8765 --no-open
```

画面(ブラウザ)の構成:

- 左の一覧: シーン → カット → 台詞・ト書きの順に並ぶ。手動で置いた項目には「手動」、枠に収まらず切り詰めた項目には「切詰」の印が付く。項目をクリックするとその項目の開始コマへ移って選択する(カットをクリックするとそのカットの先頭コマへ移る)。
- 中央のプレビュー: 動画と同じ縦横比の画面に、カットの背景素材を設定の `fit` で置き、吹き出しとト書きを `render` と同じ見た目で描く。下のコマのスライダーで位置を動かし、再生・停止ボタンで `full_dialogue.wav` を鳴らしながら再生する(動画素材は消音して音声に合わせる)。素材のコマ数がカットより短いとき、ループする素材は繰り返し、ループしない素材は最後のコマで止まる。
- 右の数値欄: 選んだ項目の x・y・幅・高さ・尻尾の dx/dy・開始のずれ(ms)・終了のずれ(ms)。

操作:

- 吹き出し・ト書きの本体をドラッグして動かし、四隅をドラッグして大きさを変え、尻尾の先をドラッグして尻尾の向きを変える。
- 項目を選んで、文字入力の欄にいないときは矢印キーで 1px、Shift を押すと 10px 動かす。
- 数値欄は Enter か欄から離れたときに確定する。確定すると手動の配置になる。「自動に戻す」でその項目を自動配置に戻す。
- 保存はボタンか Ctrl+S。収まらない文字になる値は受け付けず、直前に確定した値に戻して理由を表示する。元に戻す・やり直しはこの版にはない。
- 保存されるのは手動で置いた項目だけ。「終了」は未保存の変更があると確認を出し、タブを閉じるときも警告する。ターミナルの Ctrl+C はすぐ終わり、未保存の変更は失われる。
- `bubbles` が動いている間は素材ファイルを上書き・削除・改名できない。`bubbles` を起動し直すなどでセッションが切れると、画面にその旨が出る。`bubbles` を起動し直して新しい URL を開くこと。
- ブラウザが再生できない素材は、サーバー(ffmpeg)がコマ単位の画像にして見せる。

前提: ffmpeg・ffprobe が使えること(`render` と同じ検査)。先に `build` を実行しておく。台本・収録が `build` の後に変わっていると、`render` と同じく起動しません(終了コード 2)。

- 起動すると標準出力に `http://127.0.0.1:<ポート>/?token=<64 桁の16進>` の 1 行を出し、既定のブラウザで開く(Windows のみ。開けなかったときは警告を出すので、表示した URL を自分で開く)。`--no-open` ならブラウザを開かず URL を表示するだけ。`--port` を省くと空いているポートを自動で選ぶ。指定したポートが使えなければ終了コード 2。
- URL の `token` は起動のたびに変わる使い捨ての合言葉で、`Process.log` には書かない。ブラウザは最初の要求でクッキー(`s2v_<ポート>`)を受け取る。別の台本の `bubbles` を同時に起動しても、ポートごとにクッキー名が違うので混ざらない。
- 起動中は `tracks/` の素材と `full_dialogue.wav` を、他のプログラムから上書き・削除・改名できないように開いたままにする。素材の差し替えは `bubbles` を終えてから行う。他のプログラムが書き込みのために開いている素材は「素材なし」として警告し、その素材なしで続ける。素材が無いカットも警告して続ける。
- 保存先は台本フォルダの `bubbles.json`。保存のたびに直前の内容を `bubbles.json.bak` に残す(既存の `.bak` は置き換える)。`bubbles.json` や `.bak` がリンクやフォルダのときは何も変えずに保存を断る。
- 終了は画面の「終了」(`POST /api/quit`)。受け付けを止め、ffmpeg とメディアの応答を止めて(最大 5 秒待つ)、`Process.log` に終了を書いてから終わる。Ctrl+C ではプロセスがそのまま終わるので、未保存の変更は失われ、`Process.log` に終了の行は書かれない(ロックと素材のハンドルは OS が外す)。
- 終了コード: `0` 正常に終了、`1` 警告ありで起動して終了、`2` 起動前の失敗(検査・ポート・ロック・`full_dialogue.wav` を開けない)。同じ台本フォルダで `number`・`build`・`prompts`・`render` や別の `bubbles` が動いていると、後から始めた方が終了コード 2 で止まる。
- 守る相手: 127.0.0.1 だけで待ち受け、トークン・クッキー・Host/Origin の検査・`Content-Security-Policy` で、ブラウザで開いた他のウェブページや同じ PC の他のローカルのサービスからの操作を防ぐ。同じ PC で動く他のプログラム(トークンやクッキーを読める相手)からは守れない。

## render(動画を作る)

`build` の出力と `tracks/` の素材から、吹き出し・ト書き・音声を合成して `output.mp4` を作る。

```bash
script2video render 第1話.txt
script2video render 第1話.txt --config config.toml
```

前提: ffmpeg・ffprobe(どちらも 6.0 以上)が PATH にある(または `[render] ffmpeg` / `ffprobe` で場所を指定)。ffmpeg と ffprobe の両方の版を検査し、git の日付版(2023 年以降)も受け付ける。古ければ失敗し、版を読み取れないときは警告を出して続ける。`build` を先に実行し、収録や台本を変えたら `build` をやり直す。台本だけが `build` の後に変わったときは、警告(終了コード 1)を出して続ける。`full_dialogue.wav` が `timeline.json` と別の `build` のもの、`timeline.json` の版が違う、台本の長さが 0 または 24 時間超のときは失敗する。

素材は台本フォルダの `tracks/` に置く。カットごとに 1 つで、名前はカット ID から決まる。シーンの最初のカット(`S01-C01`)は `S01`、それ以降は `S01_C02` のように `_C番号` を付ける。

```
tracks/S01.png         S01-C01 の画像
tracks/S01_C02.mp4     S01-C02 の動画
```

拡張子は画像が `png`・`jpg`・`jpeg`、動画が `mp4`・`mov`・`webm`(大文字小文字は区別しない。WebP は不可)。動画は最後のコマで止まり、台本で `#visual (video loop)` と書いたカットだけループする。素材が無いカットは、`[render] default_image` があれば警告つきでそれに置き換え、無ければ失敗する。

吹き出しとト書きは縦書きで、画面に自動配置する。台本フォルダに `bubbles.json` があれば、そこに書かれた項目の位置・大きさを使う。自動配置の吹き出しが長すぎて収まらないときは、画面に収まる大きさに切り詰めて警告を出す(終了コード 1)。`bubbles.json` で手動に置いた項目の文字が保存された枠に収まらないときも、枠を必要な大きさまで広げて警告を出す(終了コード 1)。広げても画面に入らないときは、画面の大きさに切り詰めて枠の中で切って描き、警告を出す。フォントに無い文字は、項目ごとに警告を出して代わりの字形で描く。

出力は台本フォルダの `output.mp4`(H.264 + AAC、`-movflags +faststart`)。中間ファイルは `work/render/` に作る(削除してよい)。失敗したときは `output.mp4` を変えない。

終了コードは `build` と同じ: `0` 成功、`1` 警告あり(出力はできている)、`2` 失敗。

主な失敗の理由:

- 素材が足りない・同じカットに複数ある(拡張子違いを含む)・画像のカットに動画が置かれているなど種類が台本と違う
- 動画の素材が HDR や BT.2020 など、BT.709 以外の色で明示されている
- `font_path` のフォントが読めない・縦書きに必要な機能が無い
- `full_dialogue.wav` が `timeline.json` と別の `build` のもの、`timeline.json` の版が違う、台本の長さが 0 または 24 時間超(`build` をやり直す)
- 設定値の範囲外、`bubbles.json` の誤り、ffmpeg が使えない・出力の検査に通らない

`config.toml` の `[render]` の主な設定(全項目は同梱の `config.toml` を参照):

| キー | 既定 | 意味 |
|---|---|---|
| `width` / `height` | 1920 / 1080 | 解像度(偶数) |
| `fps` | 30 | フレームレート |
| `fit` | `contain` | 素材の収め方(`contain` は余白あり、`cover` は切り取り) |
| `default_image` | なし | 素材が無いときの代わりの画像(絶対パス、または台本フォルダからの相対) |
| `font_path` | `C:\Windows\Fonts\YuGothM.ttc` | 縦書きに使うフォント(他の OS では指定が必要) |
| `jobs` | 1 | カットを同時に作る ffmpeg の数 |

性能試験(手動、30 分以上かかることがある): `cargo test -p s2v-cli --release --test render_perf -- --ignored --nocapture`

## 出力(台本フォルダ)

```
台本フォルダ
|-台本.txt
|-full_dialogue.wav
|-output.mp4            完成動画。④最終合成が出力する(①は作らない。あれば build が output.stale.mp4 に改名する)
|-output.stale.mp4      改名された古い完成動画(次の build で上書きされる)
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

`build` は台本のあるフォルダに直接出力する(台本名のサブフォルダは作らない)。`output.mp4` は ④最終合成が出力するもので、①は作らない。前回の `output.mp4` があれば、出力を始める時点で `output.stale.mp4` に改名する(前の `output.stale.mp4` は上書き)。`voices/` は収録の置き場で、①は読むだけ。

| ファイル | 内容 |
|---|---|
| `full_dialogue.wav` | 台詞・BGM・SE の合成(24bit、マスターのソフトリミッタ後) |
| `tracks/dialogue.wav` `tracks/bgm.wav` `tracks/se.wav` | 動画編集ソフト用の別トラック(32bit 浮動小数、マスター前) |
| `timeline/timeline.json` | 後続工程が読む正本(schema_version 3。`scenes[].cuts[]`・`scenes[].notes[]` を持ち、`scenes[].visual` は廃止。台本と音声の SHA-256 を記録) |
| `timeline/subtitles.srt` | 字幕 |
| `timeline/timeline.fcpxml` | Filmora 向け(3つのトラックを3レーンに配置) |
| `Process.log` | 処理の記録・警告・エラー |
| `work/` | 中間ファイル(削除してよい) |

次の場合は、何も書かず・消さずに終了コード 2 で止まります: 収録フォルダ(`--voice-dir`、省略時は `voices/`)が `work/`・`tracks/`・`timeline/` のどれかと同じ場所(相対パス・存在しないパス・大文字小文字違いも同じ場所とみなします)、台本のファイル名が予約名(`full_dialogue.wav`・`Process.log`・`output.mp4`・`output.stale.mp4`・`bubbles.json`・`bubbles.json.bak`・`.script2video.lock`・`prompts`)のどれか(大文字小文字は区別しません。`voices.txt` などは問題ありません)、`work/`・`tracks/`・`timeline/`(と既定の `voices/`)がリンク(シンボリックリンク・ジャンクション)または台本フォルダの外を指している場合。

## ①a の完了条件の確認

実収録の素材(複数の収録セッション・音量の異なる台詞を含む)で、次を行う。

1. 設定ファイルの `[normalize] dump_prefade = true` にして `build` する。
2. `cargo run -p s2v-cli --example verify_loudness -- <台本フォルダ>` を実行し、すべて `OK` になることを確かめる。

確認の内容: 測定範囲が 400ms 以上の台詞は、フェード前の音量が目標の ±1 LU に入ること。400ms 未満の台詞は、適用ゲインが「目標 − 測定値」と一致すること。ゲインが制限に掛かった台詞は対象外。出力は何も書き換えない。

終了コード: 0=すべて OK、1=不合格あり、2=確認できない(`timeline/timeline.json`・`work/cache.json`・`work/lines_prefade/` がない、確認できる台詞が 0 件など)。
