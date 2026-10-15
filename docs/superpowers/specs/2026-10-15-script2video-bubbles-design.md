# Script2Video ③ bubbles(吹き出し・ト書きの配置の GUI)設計メモ

正本は `2026-10-14-script2video-video-design.md`(以下「設計書」)の §5。このメモは 2026-10-15 のブレインストーミングで決めたことと、その理由を残す。決定は設計書の §2.1・§5.1・§5.4・§8・§10 に反映済み。

## 目的と成功の条件

- 自動配置の吹き出し・ト書きを、人が画面で動かし、大きさや表示時刻を調整して `bubbles.json` に保存する。`render` はその配置を使う。
- 成功の条件(設計書 §9): sample の台本で `bubbles` で調整 → `render` し、GUI で見た配置と `output.mp4` の同じ時刻のコマが一致する。

## 決めたこと

| 論点 | 決定 | 理由・却下した案 |
|---|---|---|
| 同期のサーバーとメディアのストリーミング | API・画面は1つずつ順に処理し、メディア(`/media/…`)だけ要求ごとに別スレッド(同時 8 本まで、超えたら 503) | ブラウザの動画・音声の長い Range の読み出しが API を止めないため。却下: 206 を 2 MiB に切る(単純だが Human が B を選んだ)、ffmpeg でコマだけ返す(同期が粗い) |
| ffmpeg の検査 | `render` の手順 1 と同じ検査を同じ関数で行う(無い・古い → エラー 2) | `bubbles` でも再生できない素材のコマの取り出しに ffmpeg を使うため。設計書 §5.1 の文言どおり |
| ブラウザで再生できない素材(HEVC・ProRes の `.mov` など) | 画面は `<video>` の読み込みエラーのときだけ `GET /media/frame/<カットID>?t=` に切り替え、サーバーが ffmpeg でそのコマを PNG にする | 見た目を確実にするため。却下: 「再生できません」の表示だけ、起動時の ffprobe によるコーデックの推定 |
| 計画の分け方 | 2d-1(サーバー)と 2d-2(画面)の2つ | 安全面を先に固めて Codex のレビューを通し、画面はその API の上に載せる。レビューの範囲を絞る |
| 共通部分 | できるだけ共有する(Human 指示)。`s2v-render::scene`(項目の組み立て・大きさと配置の決定(手動・自動・切り詰め)・1項目の描画)と `s2v-cli::prepare`(予約名・リンク・設定・ロック・Process.log・鮮度・フォント・ffmpeg/ffprobe の版・`bubbles.json` の読み込みと拡大縮小・素材の照合と ffprobe の検査)。違いは引数で表す(素材の不足: render はエラー、bubbles は許容) | GUI と出力の配置の一致をコードの構造で保証するため。`scene` を `s2v-cli` に置かないのは、`s2v-bubbles` → `s2v-cli` の依存が循環するため |
| 新しい依存 | `tiny_http` のみ。トークンは既存の `getrandom`(Cargo.lock にある)。ブラウザは `cmd /C start "" <URL>` を引数の配列で起動 | 依存を増やさない |

## 構成(2d-1)

- `s2v-render/src/scene.rs`(新規、`render.rs` から移す): `render` と `s2v-bubbles` が使う。切り出しは振る舞いを変えず、`render` の既存のテストがすべて通ることを条件にする。
- `s2v-cli/src/prepare.rs`(新規、`render.rs` から移す): `render` と `bubbles` の起動時の検査。
- `s2v-bubbles`(新規のライブラリ):
  - `security.rs`: トークン(32 バイト → 16進 64 文字)、クッキー、Host・Origin の検査、Content-Type と本文の大きさ、共通の応答ヘッダー、定数時間の比較。HTTP に依存しない純粋な関数。
  - `range.rs`: Range の解釈(設計書 §5.4 の規則)。純粋な関数。
  - `media.rs`: 起動時に開いたハンドルの保持とメディアの応答(Range・`HEAD`・`Content-Type`)。Windows では `seek_read` で要求ごとに独立した位置から読む。
  - `frame.rs`: ffmpeg でコマを PNG にする(ffmpeg のパスは呼び出し側から受ける)。
  - `api.rs`: `state`・`render`・`layout`・`save`・`quit`。
  - `server.rs`: `tiny_http` の受け付けループ(メディアはスレッドへ、それ以外は順に処理)。
  - `assets/`: 埋め込みの HTML/JS/CSS。2d-1 では状態を表示するだけの仮の画面。
- `s2v-cli/src/bubbles.rs`(新規): コマンド。`prepare` → `s2v_bubbles::serve` → ブラウザを開く。ロックは終了まで保持する。

## API の具体化

- `GET /api/state`: canvas、fps、duration、シーン → カット → 項目の一覧(ID・種類・本文・表示時刻)、カットごとの素材(URL・種類・なし)、手動配置の現在値、全項目の配置の結果、項目ごとの `min_size`、警告の一覧。本文は JSON の文字列で返し、画面は `textContent` だけで表示する。
- `POST /api/render`: `{id, w, h, tail}` を受け、1項目の PNG を返す。本文と種類はサーバーが timeline から取る(クライアントからは受けない)。`fits` が偽なら 422。
- `POST /api/layout`: 手動配置の一式を受け、全項目の配置の結果を返す(保存しない)。
- `POST /api/save`: 手動配置の一式を検証(設計書 §5.2 の上限、offset の逆転、`fits`、ID の存在と種類の一致)。不正なら 422 で理由。正しければ `.bak` → 一時ファイル経由で保存。timeline に無い ID の手動配置も残す。保存の失敗は 500 で、元の `bubbles.json` は変えない。
- `GET /media/frame/<カットID>?t=<ミリ秒>`: t は台本全体の時刻で、カットの区間 `[start_ms, end_ms)` の外なら 400。素材の中の時刻は `t − start_ms` を `render` と同じ規則(最後のコマで止める、`loop` ならループ)で求める。
- `POST /api/quit`: 応答してから受け付けを止め、ロックを外して終わる。

## エラーの応答

403(クッキー・Host・Origin)、404(未知のパス・カット ID)、405(メソッド)、400(壊れたクエリ・JSON、範囲外の t)、413(1 MiB 超)、415(Content-Type)、409(コマの取り出しで素材が起動時と違う)、416(Range)、422(検証)、500(ffmpeg・保存の失敗)、503(メディアのスレッドが上限)。本文に内部のパスやスタックを出さない。

## テスト(2d-1)

- 単体: `security`、`range`(設計書 §8 の列挙)、`scene`(切り出し後も `render` の既存のテストが通る)。
- HTTP の結合(`s2v-bubbles/tests`): 一時フォルダでサーバーを立て、標準ライブラリの `TcpStream` で生の HTTP を送る。設計書 §8 の bubbles のサーバーの列挙すべてと、メディアの読み出しの途中でも API が応答すること。
- ffmpeg: コマの取り出しと、素材の差し替えの 409(`S2V_SKIP_FFMPEG_TESTS` の規則は 2b と同じ)。
- CLI: `script2video bubbles --no-open` を起動して URL を読み、`/api/quit` で終わる。ロックの競合で別のコマンドが エラー 2。
- 2d-2 の担当: 埋め込みの JS に `innerHTML` が無いこと、Chrome の自動操作。
