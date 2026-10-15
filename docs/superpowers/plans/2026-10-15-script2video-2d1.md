# Script2Video 計画 2d-1(bubbles のサーバー) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 吹き出し・ト書きの配置を調整する GUI のためのローカルサーバー(`s2v-bubbles`)と、コマンド `script2video bubbles` を作る。`render` と同じ関数で配置・描画・時刻を計算し、GUI と出力の一致をコードの構造で保証する。画面は仮の最小のもの(本体は計画 2d-2)。

**Architecture:** 先に `render.rs` の項目の組み立て・配置・描画を `s2v-render::scene` へ、起動時の検査を `s2v-cli::prepare` へ切り出す(振る舞いは変えない)。新しいライブラリ `s2v-bubbles` は、HTTP に依存しない純粋な部品(`security`・`range`・`validate`)、メディアの固定と配信(`media`)、ffmpeg によるコマの取り出し(`frame`)、API(`api`)、`tiny_http` の受け付けのループ(`server`)からなる。API と画面は主スレッドで順に処理し、メディアは要求ごとのスレッド(8 本まで)、コマの取り出しは専用ワーカー1本。`s2v-cli::bubbles` は `prepare` → `serve` を呼ぶ。

**Tech Stack:** Rust 1.96、`tiny_http` 0.12(新しい依存はこれだけ)、`getrandom`(Cargo.lock に既にある版)、既存の `s2v-render`・`s2v-video`・`s2v-timeline`・`s2v-core`、`serde_json`。ffmpeg・ffprobe(6.0 以上)。

**Spec:** `docs/superpowers/specs/2026-10-14-script2video-video-design.md`(以下「設計書」)の §5.1・§5.2・§5.4(要求と応答の型の表が唯一の定義)・§5.3 の時刻・背景・重ねる順と原点、§8 の bubbles のサーバー、§2.3(ロック・ログ)。決定の経緯は `docs/superpowers/specs/2026-10-15-script2video-bubbles-design.md`(以下「設計メモ」)。前提として計画 2a・2b・2c が main にある。

## Global Constraints

- 応答・コメント・エラーメッセージ・コミットメッセージは日本語(既存のコードに合わせる)。
- **共通部分は共有する**(Human 指示)。`render` と `bubbles` は、項目の組み立て・配置・描画・コマの計算・起動時の検査に同じ関数を使う。同じ処理を2か所に書かない。
- 127.0.0.1 だけで待ち受ける。トークン: 32 バイトの乱数を 16 進 64 文字。クッキー `s2v_<port>=<T>; HttpOnly; SameSite=Strict; Path=/`。比較は定数時間。トークンは Process.log に書かない。
- 全ての要求: クッキーが無い・違う → 403(`GET /?token=` を除く)。Host が `127.0.0.1:<port>` か `localhost:<port>` でなければ 403。`POST`: `Origin` が `http://127.0.0.1:<port>` か `http://localhost:<port>` でなければ 403、`Content-Type: application/json` 以外は 415、本文 1 MiB 超は 413。CORS のヘッダーは返さない。
- 全ての応答のヘッダー: `Content-Security-Policy: default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' blob:; media-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'`、`X-Content-Type-Options: nosniff`、`Referrer-Policy: no-referrer`、`Cache-Control: no-store`。
- エラーの応答: 403・404・405・400・413・415・416・422・500・503(設計書 §5.4)。本文に内部のパスやスタックを出さない。
- メディア: 要求のパスをファイルシステムのパスとして解釈しない(ID から引く)。素材と `full_dialogue.wav` は Windows の共有モード `FILE_SHARE_READ`(書き込み・削除・改名を他に許さない)で開き、終了まで保持する。配信は位置指定の読み出し(`seek_read`。短い読み出しは繰り返す)。他のプロセスが書き込みのために開いていて開けない素材は「素材なし」として警告。
- メディアのスレッドは同時 8 本まで(超えたら 503)。`/media/frame` は専用ワーカー1本、使用中は主スレッドが即座に 503、ffmpeg に `-threads 2`、10 秒で打ち切り、出力 32 MiB まで(超えたら 500)。接続数・待っている要求の数・読み書きの期限の上限は設けない(`tiny_http` の既定のまま)。
- 素材を読む ffmpeg・ffprobe の全ての呼び出し(`render` を含む)に `-protocol_whitelist file`。
- 時刻はコマ番号で扱う。カット・項目のコマの範囲、重ねる順、素材の中のコマ k はサーバーが `render` と同じ関数で計算する。素材の中のコマ: `d = n − n0`、`d < asset.frames` なら `k = d`、それ以外は `looped` なら `k = d mod asset.frames`、そうでなければ `k = asset.frames − 1`。
- Range: 設計書 §5.4 の規則(単一の範囲だけ、`b ≥ L` の切り詰め、`bytes=-n`、416 の `Content-Range: bytes */L` と `Content-Length: 0`、常に `Accept-Ranges: bytes`)。`HEAD` は本文なしで `GET` と同じヘッダー。
- 保存(設計書 §5.2): `bubbles.json`・`.bak` がリンクか通常のファイルでなければ 500(何も変えない)。既存の内容を `.bak` へ、新しい内容を `bubbles.json` へ、どちらも同じフォルダに `create_new`(乱数の名前)で作った一時ファイル経由で置き換える。既存の未知の ID は引き継ぐ。要求の新しい未知の ID は 422。
- 終了コード: 0 正常終了、1 警告ありで起動、2 起動前の失敗(検査・ポート・ロック)。
- 起動と終了の順: 待ち受けの用意 → URL → ブラウザ(`--no-open` なら表示だけ)→ 受け付けのループ。`/api/quit` の終了: 受け付けを止める → メディアのスレッドと ffmpeg を止める(最大 5 秒待ち、ffmpeg は強制終了)→ Process.log に終了 → ロックを外す。Ctrl+C ではプロセスがそのまま終わる(ロックとハンドルは OS が外す)。メディアのスレッドは Process.log に直接書かない(主スレッドへ知らせる)。
- ffmpeg を使う結合テストは ffmpeg が無ければ失敗する。`S2V_SKIP_FFMPEG_TESTS=1` のときだけ飛ばす。
- 検証コマンド(各タスクの最後に全部通す): `cargo fmt --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace`。
- コミットメッセージの末尾に、空行のあと実装者のモデルの `Co-Authored-By:` 行。

## Review Focus

1. **台詞に `<script>` や `"` や改行を含む台本**: `/api/state` の JSON が壊れず、本文はそのまま文字列として返る(HTML として解釈される経路が無い)。→ Task 9 のテスト `state_json_escapes_script_text`。
2. **別の台本の `bubbles` を同時に起動**: クッキー名がポートごとに違い、互いに 403 にならない。→ Task 4 のテスト `cookie_name_includes_port`、Task 9 のテスト `two_servers_do_not_share_cookie`。
3. **ブラウザが動画を長く読み出している最中に保存する**: 保存の API がすぐに応答する(メディアが主スレッドを止めない)。→ Task 9 のテスト `api_responds_while_media_is_being_read`。
4. **`bubbles` を開いたまま素材を上書き・削除しようとする**: 失敗し(共有モード)、配信される内容は起動時のまま。→ Task 6 のテスト `locked_asset_cannot_be_overwritten_or_deleted`。
5. **保存の途中でディスクが書けない・`bubbles.json` がフォルダ**: 元の `bubbles.json` は変わらない(`.bak` の段階の失敗なら何も変えない)。→ Task 8 のテスト `save_failures_keep_original`。

---

## ファイル構成

| ファイル | 役割 | タスク |
|---|---|---|
| `crates/s2v-render/src/scene.rs`(新規) | 項目の組み立て・欠字の警告・組版の文脈・配置(手動・自動・切り詰め)・1項目の描画・重ね合わせ。`render.rs` から移す | 1 |
| `crates/s2v-render/src/bubbles.rs` | `validate_file`(検証を `parse_bubbles` から切り出す) | 1 |
| `crates/s2v-video/src/frames.rs` | `stacking_order`(`plan_cuts` から切り出す)、`asset_frames`、`asset_frame` | 1 |
| `crates/s2v-cli/src/prepare.rs`(新規) | 起動時の検査と準備(`render.rs` から移す) | 2 |
| `crates/s2v-cli/src/render.rs` | `scene`・`prepare` を使う形に置き換える | 1・2 |
| `crates/s2v-video/src/ffmpeg.rs`、`src/probe.rs`、`src/audio_check.rs` | `-protocol_whitelist file` | 3 |
| `crates/s2v-bubbles/`(新規)`Cargo.toml`、`src/lib.rs` | クレートの入口 | 4 |
| `crates/s2v-bubbles/src/security.rs` | トークン・クッキー・Host・Origin・Content-Type・ヘッダー・定数時間の比較 | 4 |
| `crates/s2v-bubbles/tests/tiny_http_behavior.rs` | `tiny_http` の挙動の確認と記録 | 4 |
| `crates/s2v-bubbles/src/range.rs` | Range の解釈 | 5 |
| `crates/s2v-bubbles/src/media.rs` | 共有モードで開いたハンドル、位置指定の読み出し、メディアの応答の組み立て、スレッドの上限 | 6 |
| `crates/s2v-bubbles/src/frame.rs` | コマの取り出しの ffmpeg の引数とワーカー | 7 |
| `crates/s2v-bubbles/src/api.rs` | `state`・`layout`・`render`・`save` の中身(HTTP に依存しない) | 8 |
| `crates/s2v-bubbles/src/server.rs`、`src/assets/index.html`・`app.js`・`app.css` | 受け付けのループ、振り分け、終了の手順、仮の画面 | 9 |
| `crates/s2v-bubbles/tests/http.rs` | `TcpStream` による HTTP の結合テスト | 9 |
| `crates/s2v-cli/src/bubbles.rs`、`src/main.rs`、`src/lib.rs`、`tests/bubbles_cli.rs`、`README.md` | コマンド | 10 |

---

### Task 1: 共通の手順を `s2v-render::scene` と `s2v-video::frames` に切り出す

振る舞いは変えない。`render` の既存のテスト(単体・`render_ffmpeg`)がすべてそのまま通ることが条件。

**Files:**
- Create: `crates/s2v-render/src/scene.rs`
- Modify: `crates/s2v-render/src/lib.rs`(`pub mod scene;`)、`crates/s2v-render/src/bubbles.rs`、`crates/s2v-video/src/frames.rs`、`crates/s2v-cli/src/render.rs`

**Interfaces:**
- Produces(`s2v_render::scene`。`render.rs` の非公開の型・関数をそのまま公開して移す):
  - `pub struct Item { pub id: String, pub kind: ItemKind, pub text: String, pub start_ms: i64, pub end_ms: i64, pub order: usize }`(`#[derive(Debug, Clone, PartialEq)]`)
  - `pub fn build_items(tl: &Timeline, manual: &BTreeMap<String, Placed>, warnings: &mut Vec<String>) -> Result<Vec<Item>, String>`
  - `pub fn missing_glyph_warnings(face: &FontFace, items: &[Item]) -> Vec<String>`
  - `pub fn layout_ctx<'a>(face: &'a FontFace, cfg: &RenderConfig, kind: ItemKind) -> LayoutContext<'a>`(旧 `render_ctx`)
  - `pub struct Placement { pub placed: BTreeMap<String, Placed>, pub clipped: BTreeSet<String> }`
  - `pub fn place_items(items: &[Item], manual: &BTreeMap<String, Placed>, cfg: &RenderConfig, face: &FontFace, warnings: &mut Vec<String>) -> Placement`
  - `pub fn draw_item(item: &Item, placed: &Placed, clipped: bool, cfg: &RenderConfig, face: &FontFace) -> Result<Overlay, String>`(旧 `write_overlays` の中の Style・ctx・ItemSpec の組み立てと `draw_one`)
  - `pub fn compose(cfg: &RenderConfig, layers: &[(&Placed, &Overlay)]) -> tiny_skia::Pixmap`(配置の左上 + `Overlay` の `dx`・`dy` に置いて `composite`。`layers` は重ねる順)
- Produces(`s2v_render::bubbles`): `pub fn validate_file(file: &BubblesFile) -> Vec<String>`(`parse_bubbles` の中の、型への変換の後の検証をそのまま切り出す。`parse_bubbles` はこれを呼ぶ)
- Produces(`s2v_video::frames`):
  - `pub fn stacking_order(items: &[ShownItem]) -> Vec<String>`(`plan_cuts` の並べ替え(ト書き → 吹き出し、表示開始の順、同時なら order)を切り出す。`plan_cuts` はこれを使う)
  - `pub fn asset_frames(duration_ts: i64, time_base: &str, fps: u32) -> Option<i64>`(`time_base` は `"1/90000"` の形。`floor(duration_ts × num × fps / den)`、i128 で計算。書式違い・0 以下は None)
  - `pub fn asset_frame(n: i64, n0: i64, asset_frames: i64, looped: bool) -> i64`(Global Constraints の式。`asset_frames ≤ 0` なら 0)

- [ ] **Step 1: 失敗するテストを書く**

`frames.rs` の tests に:

```rust
#[test]
fn asset_frame_freezes_or_loops() {
    // 素材 60 コマ、カットの先頭 n0 = 100
    assert_eq!(asset_frame(100, 100, 60, false), 0);
    assert_eq!(asset_frame(159, 100, 60, false), 59);
    assert_eq!(asset_frame(160, 100, 60, false), 59); // 最後のコマで止める
    assert_eq!(asset_frame(300, 100, 60, false), 59);
    assert_eq!(asset_frame(160, 100, 60, true), 0); // ループ
    assert_eq!(asset_frame(225, 100, 60, true), 5);
    assert_eq!(asset_frame(105, 100, 0, false), 0);
}

#[test]
fn asset_frames_from_probe_values() {
    assert_eq!(asset_frames(180_000, "1/90000", 30), Some(60)); // 2 秒
    assert_eq!(asset_frames(1001, "1/30000", 30), Some(1)); // 0.0333… 秒
    assert_eq!(asset_frames(0, "1/90000", 30), None);
    assert_eq!(asset_frames(100, "x", 30), None);
    assert_eq!(asset_frames(100, "1/0", 30), None);
}

#[test]
fn stacking_order_matches_plan_cuts() {
    let items = vec![
        ShownItem { id: "L2".into(), is_note: false, start_ms: 500, end_ms: 900, order: 1 },
        ShownItem { id: "N1".into(), is_note: true, start_ms: 0, end_ms: 900, order: 5 },
        ShownItem { id: "L1".into(), is_note: false, start_ms: 500, end_ms: 900, order: 0 },
        ShownItem { id: "L0".into(), is_note: false, start_ms: 100, end_ms: 900, order: 9 },
    ];
    assert_eq!(stacking_order(&items), ["N1", "L0", "L1", "L2"]);
    let plans = plan_cuts(&[("C".into(), 0, 1000)], &items, 1000, 30);
    let last = &plans[0].intervals.last().unwrap().items;
    assert_eq!(last, &["N1", "L0", "L1", "L2"]);
}
```

`bubbles.rs` の tests に:

```rust
#[test]
fn validate_file_reports_the_same_errors_as_parse() {
    let ok = r#"{"version":1,"canvas":{"w":1920,"h":1080},"items":{}}"#;
    let f = parse_bubbles(ok).unwrap();
    assert!(validate_file(&f).is_empty());
    let mut bad = f.clone();
    bad.items.insert(
        "S01-L001".into(),
        Placed { kind: ItemKind::Note, x: 0, y: 0, w: 10, h: 10, tail: None, start_offset_ms: 0, end_offset_ms: 0 },
    );
    bad.items.insert(
        "S01-L002".into(),
        Placed { kind: ItemKind::Bubble, x: 1915, y: 0, w: 10, h: 10, tail: None, start_offset_ms: 0, end_offset_ms: 0 },
    );
    let errs = validate_file(&bad);
    assert!(errs.iter().any(|e| e.contains("S01-L001")), "{errs:?}"); // 種類の不一致
    assert!(errs.iter().any(|e| e.contains("S01-L002")), "{errs:?}"); // 画面の外
}
```

`scene.rs` の tests(`render.rs` の `build_items`・`place_items`・`draw_one` を確かめていた単体テストは、`render.rs` から `scene.rs` へ移す。テストの中身は変えない)。加えて:

```rust
#[test]
fn compose_places_overlay_at_placed_plus_origin() {
    // 尻尾つきの吹き出しは Overlay の dx・dy が 0 でないことがある。重ね合わせは placed.x + dx に置く
    // (render.rs の write_overlays と同じ式)。テスト用フォントで 1 項目を描き、compose の結果の
    // 不透明な画素の外接矩形の左上が (placed.x + ov.dx, placed.y + ov.dy) 以上であることを確かめる。
}
```

(このテストの本文は、`draw.rs` の既存のテストの補助(テスト用フォント `tests/fonts/test-vrt2.ttf` と `RenderConfig::default()`)を使って具体的に書く。)

- [ ] **Step 2: 失敗を確かめる** — Run: `cargo test -p s2v-video frames`、`cargo test -p s2v-render` / Expected: 関数が無くコンパイルエラー。

- [ ] **Step 3: 実装する**

1. `frames.rs`: `plan_cuts` の `sorted` の並べ替えを `stacking_order` に移し、`plan_cuts` は `stacking_order` の結果の順で `spans` を作る。`asset_frames`・`asset_frame` を加える:

```rust
/// 動画の素材の長さを出力の fps のコマ数にする(切り捨て)。time_base は "num/den"。
pub fn asset_frames(duration_ts: i64, time_base: &str, fps: u32) -> Option<i64> {
    let (num, den) = time_base.split_once('/')?;
    let (num, den): (i128, i128) = (num.trim().parse().ok()?, den.trim().parse().ok()?);
    if duration_ts <= 0 || num <= 0 || den <= 0 {
        return None;
    }
    let v = (duration_ts as i128 * num * fps as i128).div_euclid(den);
    (v > 0).then_some(v as i64)
}

/// カットのコマ n に表示する素材の中のコマ(最後のコマで止める、loop ならループする)。
pub fn asset_frame(n: i64, n0: i64, asset_frames: i64, looped: bool) -> i64 {
    if asset_frames <= 0 {
        return 0;
    }
    let d = (n - n0).max(0);
    if d < asset_frames {
        d
    } else if looped {
        d % asset_frames
    } else {
        asset_frames - 1
    }
}
```

2. `bubbles.rs`: `parse_bubbles` の検証部分を `validate_file` に移す(戻り値の誤りの文は変えない)。
3. `scene.rs`: `render.rs` の `Item`・`build_items`・`missing_glyph_warnings`・`render_ctx`(→ `layout_ctx`)・`place_items`(戻り値を `Placement` に)・`draw_one` を移し、`draw_item`・`compose` を加える。`write_overlays` は `draw_item` と `compose` を使う形に直す。`render.rs` は `use s2v_render::scene::{…}` で使う。

- [ ] **Step 4: テストが通ることを確かめる** — Run: `cargo test -p s2v-video`、`cargo test -p s2v-render`、`cargo test -p s2v-cli`(`render_ffmpeg` を含む。ffmpeg あり)/ Expected: すべて PASS(既存のテストの本文は変えていない)。

- [ ] **Step 5: 全体の検証とコミット** — `refactor: render の項目・配置・描画とコマの順を s2v-render::scene と s2v-video::frames に切り出す`

---

### Task 2: 起動時の検査を `s2v-cli::prepare` に切り出す

振る舞いは変えない。`render` の既存のテストがすべて通ることが条件。

**Files:**
- Create: `crates/s2v-cli/src/prepare.rs`
- Modify: `crates/s2v-cli/src/lib.rs`(`pub mod prepare;`)、`crates/s2v-cli/src/render.rs`

**Interfaces:**
- Consumes: Task 1 の `scene`。
- Produces:
  - `pub struct Preflight { pub config: Config, pub dir: PathBuf, pub lock: ScriptLock }`
  - `pub fn preflight(script: &Path, config: Option<&Path>) -> Result<Preflight, String>`(`render.rs` の `render()` の、Process.log を開く前の検査: 予約名・リンク・台本の有無・設定の読み込みと `validate`・ロック。Err は標準エラーにだけ出す文)
  - `pub enum MissingAssets { Error, Allow }`(render は Error、bubbles は Allow)
  - `pub struct Prepared { pub timeline: Timeline, pub face: FontFace, pub ffmpeg: String, pub ffprobe: String, pub manual: BTreeMap<String, Placed>, pub items: Vec<Item>, pub placement: Placement, pub assets: BTreeMap<String, PreparedAsset>, pub missing: Vec<String>, pub warnings: Vec<String> }`
  - `pub struct PreparedAsset { pub cut_id: String, pub path: PathBuf, pub kind: AssetKind, pub looped: bool, pub background: Background, pub video: Option<Stream> }`(`video` は動画の素材の ffprobe の映像ストリーム。`asset_frames` の計算に使う)
  - `pub fn prepare(script: &Path, cfg: &RenderConfig, dir: &Path, missing: MissingAssets, log: &mut ProcLog) -> Result<Prepared, String>`(`render_main` の手順 2〜7: 鮮度(音声も確かめる)・フォント・ffmpeg/ffprobe の版・`bubbles.json` の読み込みと `scale_to`・項目・配置・素材の照合と `default_image`・ffprobe による中身の検査。`missing = Allow` のときは素材の不足を `missing` に入れて警告し、Err にしない。`default_image` があれば render と同じく代わりに使う)
  - ffmpeg の起動の補助(`run_tool`・`run_checked`・`tool_path`・`tool_version`・`gate_version`・`check_tools`・`probe_file`)も `prepare.rs` へ移して `pub(crate)` にする(`render.rs` と `bubbles.rs` が使う)。

- [ ] **Step 1: 失敗するテストを書く** — `prepare.rs` の tests:

```rust
#[test]
fn missing_assets_policy() {
    // render の単体テストの補助と同じ要領で、台本・build の出力・素材なしの台本フォルダを作る(ffmpeg は使う)
    // MissingAssets::Error → Err に「素材が足りません」
    // MissingAssets::Allow → Ok、prepared.missing にカット ID、warnings に「素材なし」を含む文
}
```

(本文は `crates/s2v-cli/tests/render_ffmpeg.rs` の `project` の要領で台本を build して書く。ffmpeg の規則は Global Constraints。この1本は `tests/prepare_ffmpeg.rs` に置いてよい。)

- [ ] **Step 2: 失敗を確かめる** — Run: `cargo test -p s2v-cli prepare` / Expected: コンパイルエラー。
- [ ] **Step 3: 実装する** — `render.rs` の該当部分を `prepare.rs` へ移し、`render()` は `preflight` → `ProcLog::append` → `prepare(…, MissingAssets::Error, …)` → 残り(作業フォルダ以降)の形にする。`resolve_backgrounds` は `PreparedAsset` を作る形に直す(ffprobe の映像ストリームを保持する)。
- [ ] **Step 4: テストが通ることを確かめる** — Run: `cargo test -p s2v-cli`(ffmpeg あり)/ Expected: すべて PASS。
- [ ] **Step 5: 全体の検証とコミット** — `refactor: render の起動時の検査を s2v-cli::prepare に切り出す`

---

### Task 3: 素材を読む ffmpeg・ffprobe に `-protocol_whitelist file` を付ける

**Files:**
- Modify: `crates/s2v-video/src/ffmpeg.rs`(`cut_args` の全ての入力、`concat_args`、`mux_args` の音声の入力)、`crates/s2v-video/src/probe.rs`(`probe_args`、`asset_probe_args`)、`crates/s2v-video/src/audio_check.rs`(`decode_args`)

**Interfaces:**
- Produces: 各関数の引数の配列で、`-i` の直前の入力オプションとして `-protocol_whitelist file` を入れる(ffmpeg)。ffprobe では入力のパスの前に `-protocol_whitelist file`。concat の一覧の入力にも付ける(concat の各ファイルは file プロトコルで開かれる)。

- [ ] **Step 1: 失敗するテストを書く** — 既存の完全一致のゴールデン(`ffmpeg.rs` の `cut_args` などのテスト、`probe.rs`・`audio_check.rs` の引数のテスト)を、`-protocol_whitelist file` を含む期待値に書き換える。加えて `ffmpeg.rs` に:

```rust
#[test]
fn every_input_has_protocol_whitelist() {
    // cut_args(画像・動画・loop)・concat_args・mux_args の全ての "-i" の直前の2つが
    // ["-protocol_whitelist", "file"] か、その前に入力オプションが並ぶ場合も "-i" より前に必ずあること
    for args in [/* 各関数の代表的な呼び出し */] {
        let ii: Vec<usize> = args.iter().enumerate().filter(|(_, a)| *a == "-i").map(|(i, _)| i).collect();
        for &i in &ii {
            let before = &args[..i];
            let last_i = before.iter().rposition(|a| a == "-i").map_or(0, |p| p + 2);
            assert!(before[last_i..].windows(2).any(|w| w == ["-protocol_whitelist", "file"]), "{args:?}");
        }
    }
}
```

(配列の中身は既存のテストの `s()` などの補助で具体的に書く。)

- [ ] **Step 2: 失敗を確かめる** — Run: `cargo test -p s2v-video` / Expected: ゴールデンと新しいテストが FAIL。
- [ ] **Step 3: 実装する** — 各関数に入れる。
- [ ] **Step 4: テストが通ることを確かめる** — Run: `cargo test -p s2v-video`、`cargo test -p s2v-cli --test render_ffmpeg`(実際の ffmpeg で通ること)/ Expected: PASS。
- [ ] **Step 5: 全体の検証とコミット** — `fix: 素材を読む ffmpeg・ffprobe に -protocol_whitelist file を付ける`

---

### Task 4: `s2v-bubbles` クレート、`tiny_http` の挙動の確認、`security`

**Files:**
- Create: `crates/s2v-bubbles/Cargo.toml`、`src/lib.rs`、`src/security.rs`、`tests/tiny_http_behavior.rs`
- Modify: ワークスペースの `Cargo.toml`(members に `crates/s2v-bubbles`)

`Cargo.toml` の依存: `tiny_http = "0.12"`、`getrandom`(Cargo.lock にある版に合わせる。`cargo tree -i getrandom` で確かめ、同じ主版を指定)、`s2v-core`・`s2v-render`・`s2v-timeline`・`s2v-video`(path)、`serde`・`serde_json`、`anyhow`。dev: `tempfile`。

**Interfaces:**
- Produces(`security`。HTTP に依存しない純粋な関数):
  - `pub struct Token(String)`、`pub fn new_token() -> Token`(32 バイト → 16進 64 文字の小文字)、`impl Token { pub fn as_str(&self) -> &str }`
  - `pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool`
  - `pub fn cookie_name(port: u16) -> String`(`s2v_<port>`)
  - `pub fn set_cookie_header(port: u16, token: &Token) -> String`(`s2v_<port>=<T>; HttpOnly; SameSite=Strict; Path=/`)
  - `pub fn cookie_ok(cookie_header: Option<&str>, port: u16, token: &Token) -> bool`(`Cookie:` の値を `; ` で分け、名前が一致するものの値を定数時間で比較。同じ名前が複数あれば偽)
  - `pub fn host_ok(host: Option<&str>, port: u16) -> bool`(`127.0.0.1:<port>` か `localhost:<port>`。大文字小文字を区別しない。ポートの省略は偽)
  - `pub fn origin_ok(origin: Option<&str>, port: u16) -> bool`(`http://127.0.0.1:<port>` か `http://localhost:<port>`。末尾の `/` や path があれば偽)
  - `pub fn json_content_type_ok(ct: Option<&str>) -> bool`(`application/json`、`; charset=utf-8` は可。大文字小文字を区別しない)
  - `pub const MAX_BODY: usize = 1 << 20;`
  - `pub fn common_headers() -> Vec<(&'static str, &'static str)>`(CSP・nosniff・no-referrer・no-store。値は Global Constraints の文字列をそのまま)

- [ ] **Step 1: `tiny_http` の挙動の確認テストを書く**(`tests/tiny_http_behavior.rs`。設計書 §5.4 の「最初のタスクで確かめて記録する」)

```rust
//! tiny_http 0.12 の挙動の確認(設計書 §5.4)。結果は計画 2d-1 の報告に記録する。
//! - Request は Send で、別スレッドへ移して応答できる
//! - 1つの応答の書き込みが遅くても(別スレッドに移していれば)、次の要求を recv できる
//! - Server::unblock で recv が戻る

#[test]
fn request_can_be_answered_on_another_thread_while_recv_continues() {
    // 127.0.0.1:0 で Server::http。1つ目の接続の要求を別スレッドへ移し、そこで 2 秒待ってから応答する。
    // その間に2つ目の接続の要求が主スレッドの recv で受け取れ、すぐに応答できること(経過 < 1 秒)。
}

#[test]
fn unblock_makes_recv_return() {
    // 別スレッドから server.unblock() を呼ぶと、主スレッドの recv が Err か None で戻ること。
}
```

(本文は `std::net::TcpStream` で `GET / HTTP/1.1\r\nHost: …\r\nConnection: close\r\n\r\n` を送り、応答を読む補助を書いて具体化する。)

- [ ] **Step 2: `security` の失敗するテストを書く**

```rust
#[test]
fn token_is_64_lowercase_hex_and_random() {
    let a = new_token();
    let b = new_token();
    assert_eq!(a.as_str().len(), 64);
    assert!(a.as_str().bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)));
    assert_ne!(a.as_str(), b.as_str());
}

#[test]
fn cookie_name_includes_port() {
    // Review Focus 2
    assert_eq!(cookie_name(51234), "s2v_51234");
    let t = new_token();
    assert_eq!(
        set_cookie_header(51234, &t),
        format!("s2v_51234={}; HttpOnly; SameSite=Strict; Path=/", t.as_str())
    );
}

#[test]
fn cookie_check() {
    let t = new_token();
    let good = format!("s2v_80={}", t.as_str());
    assert!(cookie_ok(Some(&good), 80, &t));
    assert!(cookie_ok(Some(&format!("a=1; {good}; b=2")), 80, &t));
    assert!(!cookie_ok(Some(&good), 81, &t)); // 別のポートの名前
    assert!(!cookie_ok(None, 80, &t));
    assert!(!cookie_ok(Some("s2v_80=00"), 80, &t));
    assert!(!cookie_ok(Some(&format!("{good}; {good}")), 80, &t)); // 重複は拒否
}

#[test]
fn host_and_origin() {
    assert!(host_ok(Some("127.0.0.1:8080"), 8080));
    assert!(host_ok(Some("LOCALHOST:8080"), 8080));
    assert!(!host_ok(Some("127.0.0.1"), 8080));
    assert!(!host_ok(Some("evil.example:8080"), 8080));
    assert!(!host_ok(Some("127.0.0.1:8081"), 8080));
    assert!(!host_ok(None, 8080));
    assert!(origin_ok(Some("http://127.0.0.1:8080"), 8080));
    assert!(origin_ok(Some("http://localhost:8080"), 8080));
    for bad in ["https://127.0.0.1:8080", "http://127.0.0.1:8080/", "http://127.0.0.1", "null", "http://evil:8080"] {
        assert!(!origin_ok(Some(bad), 8080), "{bad}");
    }
    assert!(!origin_ok(None, 8080));
}

#[test]
fn content_type_and_constant_time() {
    assert!(json_content_type_ok(Some("application/json")));
    assert!(json_content_type_ok(Some("Application/JSON; charset=utf-8")));
    assert!(!json_content_type_ok(Some("text/plain")));
    assert!(!json_content_type_ok(None));
    assert!(constant_time_eq(b"abc", b"abc"));
    assert!(!constant_time_eq(b"abc", b"abd"));
    assert!(!constant_time_eq(b"abc", b"ab"));
}

#[test]
fn common_headers_are_exact() {
    let h = common_headers();
    assert!(h.contains(&("Content-Security-Policy", "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' blob:; media-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'")));
    assert!(h.contains(&("X-Content-Type-Options", "nosniff")));
    assert!(h.contains(&("Referrer-Policy", "no-referrer")));
    assert!(h.contains(&("Cache-Control", "no-store")));
    assert!(!h.iter().any(|(k, _)| k.starts_with("Access-Control")));
}
```

- [ ] **Step 3: 失敗を確かめる** — Run: `cargo test -p s2v-bubbles` / Expected: `security` の関数が無くコンパイルエラー(`tiny_http_behavior` は通ってよい)。
- [ ] **Step 4: 実装する**

```rust
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut d = 0u8;
    for (x, y) in a.iter().zip(b) {
        d |= x ^ y;
    }
    d == 0
}

pub fn new_token() -> Token {
    let mut buf = [0u8; 32];
    getrandom::fill(&mut buf).expect("乱数を得られません"); // 版が 0.2 なら getrandom::getrandom(&mut buf)
    Token(buf.iter().map(|b| format!("{b:02x}")).collect())
}
```

残りの関数は Interfaces の規則どおりに書く。

- [ ] **Step 5: テストが通ることを確かめる** — Run: `cargo test -p s2v-bubbles` / Expected: PASS。`tiny_http_behavior` の結果(Send で別スレッドに移せたか、遅い応答の間に次の要求を受けられたか、unblock の挙動)を報告に書く。想定と違えば(例: 別スレッドに移しても次の recv が止まる)止めて報告する(以降のタスクの前提が変わるため)。
- [ ] **Step 6: 全体の検証とコミット** — `feat: s2v-bubbles クレートと、トークン・クッキー・Host・Origin などの安全の部品を加える`

---

### Task 5: Range の解釈(`range.rs`)

**Files:**
- Create: `crates/s2v-bubbles/src/range.rs`
- Modify: `crates/s2v-bubbles/src/lib.rs`

**Interfaces:**
- Produces:
  - `pub enum RangeResult { Full, Partial { start: u64, end: u64 }, Unsatisfiable }`(`end` は含む)
  - `pub fn parse_range(header: Option<&str>, len: u64) -> RangeResult`

規則(設計書 §5.4): ヘッダーが無ければ `Full`。`bytes=a-b`・`bytes=a-`・`bytes=-n` の1つだけ。空白は前後のみ許す。`b ≥ len` なら `b = len − 1`。`bytes=-n`: `n > len` なら全体(`0..len−1`)。複数(`,` を含む)・単位が `bytes` でない・数字以外・あふれ(u64 に入らない)・`a > b`・`a ≥ len`・`bytes=-0`・`len == 0` → `Unsatisfiable`。

- [ ] **Step 1: 失敗するテストを書く**

```rust
use RangeResult::*;

#[test]
fn ranges() {
    assert_eq!(parse_range(None, 100), Full);
    assert_eq!(parse_range(Some("bytes=0-9"), 100), Partial { start: 0, end: 9 });
    assert_eq!(parse_range(Some("bytes=90-"), 100), Partial { start: 90, end: 99 });
    assert_eq!(parse_range(Some("bytes=90-500"), 100), Partial { start: 90, end: 99 }); // 切り詰め
    assert_eq!(parse_range(Some("bytes=-10"), 100), Partial { start: 90, end: 99 });
    assert_eq!(parse_range(Some("bytes=-500"), 100), Partial { start: 0, end: 99 });
    assert_eq!(parse_range(Some(" bytes=0-0 "), 100), Partial { start: 0, end: 0 });
    for bad in [
        "bytes=-0", "bytes=100-", "bytes=100-200", "bytes=5-4", "bytes=0-1,5-6", "items=0-1",
        "bytes=a-b", "bytes=", "bytes=-", "bytes=99999999999999999999-", "bytes=0-99999999999999999999",
        "bytes = 0-1", "bytes=+1-2",
    ] {
        assert_eq!(parse_range(Some(bad), 100), Unsatisfiable, "{bad}");
    }
    assert_eq!(parse_range(Some("bytes=0-0"), 0), Unsatisfiable); // 空のファイル
    assert_eq!(parse_range(None, 0), Full);
}
```

注: `bytes=0-99999999999999999999` は b があふれるので Unsatisfiable(切り詰めの前に数値として読めない)。

- [ ] **Step 2: 失敗を確かめる** / **Step 3: 実装する** / **Step 4: 通ることを確かめる** — Run: `cargo test -p s2v-bubbles range`
- [ ] **Step 5: 全体の検証とコミット** — `feat: bubbles のメディアの Range の解釈を加える`

---

### Task 6: メディアの固定と配信(`media.rs`)

**Files:**
- Create: `crates/s2v-bubbles/src/media.rs`
- Modify: `crates/s2v-bubbles/src/lib.rs`

**Interfaces:**
- Consumes: Task 5 の `parse_range`。
- Produces:
  - `pub struct LockedFile { file: std::fs::File, len: u64, content_type: &'static str }`
  - `pub fn open_locked(path: &Path) -> std::io::Result<LockedFile>`(Windows: `OpenOptions::new().read(true).share_mode(1 /* FILE_SHARE_READ */)`。それ以外の OS: 通常の読み取り(固定は Windows のみ。コメントに書く)。`len` は開いたハンドルの `metadata().len()`。`content_type` は拡張子から: png→`image/png`、jpg/jpeg→`image/jpeg`、mp4→`video/mp4`、mov→`video/quicktime`、webm→`video/webm`、wav→`audio/wav`。それ以外は Err(InvalidInput))
  - `impl LockedFile { pub fn len(&self) -> u64; pub fn content_type(&self) -> &'static str; pub fn read_at(&self, buf: &mut [u8], offset: u64) -> std::io::Result<usize> }`(Windows は `seek_read`、Unix は `read_at`)
  - `pub struct MediaResponse { pub status: u16, pub headers: Vec<(String, String)>, pub body: Option<(u64, u64)> }`(`body` は読み出す範囲 `start..=end`。HEAD と 416 は None)
  - `pub fn media_response(file: &LockedFile, method_is_head: bool, range_header: Option<&str>) -> MediaResponse`(200/206/416 と `Content-Type`・`Content-Length`・`Content-Range`・`Accept-Ranges: bytes`)
  - `pub struct RangeReader<'a> { … }`(`impl Read`。`read_at` で `start..=end` を順に読む。短い読み出しを繰り返す)。`pub fn reader(file: &LockedFile, start: u64, end: u64) -> RangeReader<'_>`
  - `pub struct Slots { … }`、`pub fn Slots::new(max: usize) -> Arc<Slots>`、`pub fn try_acquire(self: &Arc<Self>) -> Option<SlotGuard>`(同時 8 本の上限。`SlotGuard` の drop で返す。`AtomicUsize` の compare_exchange)

- [ ] **Step 1: 失敗するテストを書く**

```rust
#[test]
fn media_response_statuses() {
    // 一時ファイル a.mp4(100 バイト)
    // Range なし → 200、Content-Length 100、Accept-Ranges bytes、Content-Type video/mp4、body Some((0, 99))
    // bytes=10-19 → 206、Content-Range "bytes 10-19/100"、Content-Length 10、body Some((10, 19))
    // bytes=200- → 416、Content-Range "bytes */100"、Content-Length 0、body None
    // HEAD + bytes=10-19 → 206 と同じヘッダー、body None
    // 空のファイル + Range なし → 200、Content-Length 0
}

#[test]
fn reader_returns_exact_bytes() {
    // 0..=255 のバイト列を書いた一時ファイルで reader(…, 10, 19) を最後まで読むと 10..=19
}

#[test]
fn unknown_extension_is_rejected() {
    // a.txt → open_locked が Err
}

#[cfg(windows)]
#[test]
fn locked_asset_cannot_be_overwritten_or_deleted() {
    // Review Focus 4: open_locked で開いたまま、
    //   std::fs::write(path, b"new") が Err、std::fs::remove_file(path) が Err、std::fs::rename(path, other) が Err。
    //   別のハンドルで読み取り(OpenOptions::new().read(true).open)は Ok(ffmpeg が読めること)。
    //   read_at で読める内容は元のまま。
    // drop した後は remove_file が Ok。
}

#[cfg(windows)]
#[test]
fn file_open_for_writing_elsewhere_cannot_be_locked() {
    // OpenOptions::new().write(true).open(path) を保持したまま open_locked → Err(共有違反)
}

#[test]
fn slots_limit() {
    let s = Slots::new(2);
    let a = s.try_acquire().unwrap();
    let _b = s.try_acquire().unwrap();
    assert!(s.try_acquire().is_none());
    drop(a);
    assert!(s.try_acquire().is_some()); // 返せば次が取れる(503 からの回復)
}
```

- [ ] **Step 2: 失敗を確かめる** / **Step 3: 実装する** / **Step 4: 通ることを確かめる** — Run: `cargo test -p s2v-bubbles media`
- [ ] **Step 5: 全体の検証とコミット** — `feat: bubbles のメディアを共有モードで固定し、Range つきで配信する部品を加える`

---

### Task 7: コマの取り出し(`frame.rs`)

**Files:**
- Create: `crates/s2v-bubbles/src/frame.rs`
- Modify: `crates/s2v-bubbles/src/lib.rs`

**Interfaces:**
- Consumes: `s2v_video::frames::asset_frame`、`s2v_video::ffmpeg` の色の扱い(`Background::Video` の `matrix`・`range` を `-vf scale=…` に使うかは、`cut_args` の動画の入力と同じ指定にする)。
- Produces:
  - `pub fn frame_args(input: &str, k: i64, fps: u32, matrix: &str, range: &str) -> Vec<String>`: `-nostdin -hide_banner -v error -threads 2 -protocol_whitelist file -ss <k/fps を小数6桁の秒> -i <input> -vf "scale=in_color_matrix=<matrix>:in_range=<range>:out_color_matrix=bt709:out_range=tv,format=rgb24" -frames:v 1 -f image2pipe -vcodec png -`(入力の前の `-ss` でシークする(先頭から復号しないので長い素材でも速い。ffmpeg の既定の正確なシーク)。色の指定は `cut_args` の動画の入力の scale と同じ値。標準出力に PNG。`-n` は出力がパイプなので付けない(Global Constraints の `-n` はファイルに書く呼び出しのため。理由をコメントに書く))
  - `pub enum FrameError { Busy, Timeout, TooLarge, Failed(String) }`
  - `pub struct FrameWorker { … }`、`pub fn FrameWorker::start(ffmpeg: String) -> FrameWorker`(専用スレッド1本)、`pub fn try_submit(&self, args: Vec<String>) -> Result<Receiver<Result<Vec<u8>, FrameError>>, FrameError>`(使用中なら即座に `Err(Busy)`)、`pub fn shutdown(self)`(走っている ffmpeg を止めてスレッドを終える)
  - ワーカーの中: `Command::new(ffmpeg).args(args).stdin(null).stdout(piped).stderr(piped)` で起動し、標準出力を別スレッドで最大 32 MiB + 1 バイトまで読む。10 秒経つか 32 MiB を超えたら `child.kill()` して `Timeout`/`TooLarge`。終了コードが 0 でなければ `Failed(標準エラーの末尾 20 行)`。
  - 定数: `pub const FRAME_TIMEOUT: Duration = Duration::from_secs(10);`、`pub const FRAME_MAX_BYTES: usize = 32 << 20;`

- [ ] **Step 1: 失敗するテストを書く**

```rust
#[test]
fn frame_args_are_exact() {
    assert_eq!(frame_args("C:/a b/S01.mov", 5, 30, "bt709", "tv"), vec![/* 上の Produces の配列をそのまま */]);
}

#[test]
fn busy_worker_rejects_immediately() {
    // ffmpeg の代わりに長く眠るコマンドを使う: Windows は "powershell" と ["-NoProfile","-Command","Start-Sleep 3"]
    // (FrameWorker::start に実行ファイル名を渡す設計なので、テストではそれを差し替える)
    // 1つ目を submit → 受け付け。2つ目を submit → 即座に Err(Busy)(経過 < 200ms)
}

#[test]
fn timeout_kills_the_process() {
    // FRAME_TIMEOUT をテストでは短くできるよう、FrameWorker::start_with(ffmpeg, timeout, max_bytes) を用意し、
    // start は既定値で start_with を呼ぶ。眠るコマンドで timeout 300ms → Err(Timeout)、その後の submit は受け付けられる
}

#[test]
fn too_large_output_is_rejected() {
    // 大量に出力するコマンド(powershell で 1 MiB を書く)と max_bytes = 1024 → Err(TooLarge)
}
```

`tests/frame_ffmpeg.rs`(実際の ffmpeg。Global Constraints の規則):

```rust
#[test]
fn extracts_the_requested_frame_of_a_video() {
    // render_ffmpeg.rs と同じ要領で、コマ k の左半分が k*4 の灰色の 60 コマの動画を作る
    // frame_args(…, 10, 30, "bt709", "tv") を FrameWorker で実行し、PNG を tiny_skia で読み、左半分の灰色 ≒ 40(±5)
}
```

- [ ] **Step 2: 失敗を確かめる** / **Step 3: 実装する** / **Step 4: 通ることを確かめる** — Run: `cargo test -p s2v-bubbles frame`、`cargo test -p s2v-bubbles --test frame_ffmpeg`
- [ ] **Step 5: 全体の検証とコミット** — `feat: ブラウザで再生できない素材のコマを ffmpeg で取り出すワーカーを加える`

---

### Task 8: API の中身(`api.rs`。HTTP に依存しない)

**Files:**
- Create: `crates/s2v-bubbles/src/api.rs`
- Modify: `crates/s2v-bubbles/src/lib.rs`

**Interfaces:**
- Consumes: Task 1 の `scene`・`validate_file`・`stacking_order`・`asset_frames`、Task 2 の `Prepared` の中身(ただし `s2v-bubbles` は `s2v-cli` に依存しないので、必要なものを下の `Session` で受け取る)。
- Produces:
  - `pub struct SessionAsset { pub cut_id: String, pub kind: AssetKind, pub looped: bool, pub frames: Option<i64>, pub matrix: String, pub range: String, pub path: PathBuf }`
  - `pub struct Session { pub timeline: Timeline, pub cfg: RenderConfig, pub face: FontFace, pub dir: PathBuf, pub items: Vec<Item>, pub assets: BTreeMap<String, SessionAsset>, pub loaded: BTreeMap<String, Placed>, pub warnings: Vec<String> }`(`loaded` は読み込んだ `bubbles.json` の全項目(拡大縮小済み、未知の ID を含む))
  - `pub fn state_json(s: &Session, manual: &BTreeMap<String, Placed>) -> serde_json::Value`(設計書 §5.4 の `/api/state` の型どおり。`cuts[].frames` は `plan_cuts` から(コマ数 0 のカットは `[n0, n0]`)、`items[].frames` は `[frame(start_ms), frame(end_ms)]`、`order` は `stacking_order` の順位、`placed`・`clipped` は `place_items`、`image` は `draw_item` の `dx`・`dy` と pixmap の大きさ、`min_size` は `min_size`、`asset.url` は `/media/asset/<ID>`、`frame_url` は動画なら `/media/frame/<ID>`、`fit` と `background` は設定から)
  - `pub fn layout_json(s: &Session, manual: &BTreeMap<String, Placed>) -> serde_json::Value`(`state_json` の `items` と同じ形)
  - `pub enum ApiError { BadRequest(String), NotFound, Unprocessable(Vec<String>), Internal(String) }`
  - `pub fn parse_manual(body: &[u8], s: &Session) -> Result<BTreeMap<String, Placed>, ApiError>`(JSON `{"manual":{…}}` を読み、`validate_file`(canvas は設定の解像度)で検証。timeline に無い ID → `Unprocessable`。種類の不一致・上限・あふれ → `Unprocessable`。JSON の誤り → `BadRequest`)
  - `pub fn render_png(s: &Session, id: &str, manual: &BTreeMap<String, Placed>) -> Result<(Vec<u8>, i32, i32, bool), ApiError>`(全体の `place_items` → 項目 `id` を `draw_item`。`id` が無ければ `NotFound`。`id` が `manual` にあり `fits` が偽なら `Unprocessable`(`place_items` の拡大より先に判定する)。戻り値は (PNG, dx, dy, clipped))
  - `pub fn save(s: &Session, manual: &BTreeMap<String, Placed>) -> Result<Vec<String>, ApiError>`(各項目の `fits`(偽なら `Unprocessable`)と、effective_times の開始 < 終了(逆転なら `Unprocessable`)。中身 = `manual` + `loaded` のうち timeline に無い ID。Global Constraints の保存の手順で書く。戻り値は警告)
  - `pub fn atomic_replace(path: &Path, bytes: &[u8]) -> std::io::Result<()>`(同じフォルダに `create_new` の一時ファイル `.<name>.<乱数16進>.tmp` → `sync_all` → `rename`。失敗時は一時ファイルを消す)

- [ ] **Step 1: 失敗するテストを書く**(一時フォルダに台本の timeline を手で作る補助。`crates/s2v-cli/src/prompts_md.rs` のテストの `tl()` の要領で `Timeline` を組み立て、`RenderConfig` は既定値に `font_path` をテスト用フォント)

```rust
#[test]
fn state_matches_render_frames_and_order() {
    // state_json の cuts[].frames が plan_cuts の (start_frame, start_frame + frames) と一致
    // items[].frames が frame(start_ms), frame(end_ms) と一致、order が stacking_order の順
}

#[test]
fn state_json_escapes_script_text() {
    // Review Focus 1: 台詞の本文 "<script>alert(1)</script>\"\n" が、serde_json::to_string の結果を
    // 読み戻した items[].text とそのまま一致する(本文は文字列としてだけ入る)
}

#[test]
fn render_rejects_manual_that_does_not_fit_and_draws_clipped_auto() {
    // manual に 10x10 の手動配置 → Unprocessable
    // manual 空で、画面より大きい min_size の長文の自動項目 → Ok、clipped = true
    // 尻尾つきの吹き出しで dx・dy が draw_item の Overlay と一致
}

#[test]
fn parse_manual_validates() {
    // 未知の ID → Unprocessable、種類の不一致 → Unprocessable、画面の外 → Unprocessable、JSON の誤り → BadRequest、
    // 10001 件 → Unprocessable、|offset| > 60000 → Unprocessable
}

#[test]
fn save_writes_bak_and_keeps_unknown_ids() {
    // loaded に timeline に無い "S09-L001" がある。save(manual = {S01-L001}) の後の bubbles.json は
    // S01-L001 と S09-L001 を含み、canvas は設定の解像度。.bak は元の内容と同じバイト列
}

#[test]
fn save_failures_keep_original() {
    // Review Focus 5:
    // (a) bubbles.json.bak という名前のフォルダがある → Internal、bubbles.json は元のまま、一時ファイルが残らない
    // (b) bubbles.json がフォルダ → Internal、何も変えない
    // (c) bubbles.json がリンク(make_dir_link ではなくファイルのシンボリックリンクが作れない環境では、
    //     ディレクトリへのジャンクションを bubbles.json という名前で作る)→ Internal
    // (d) offset の逆転 → Unprocessable、何も変えない
}
```

- [ ] **Step 2: 失敗を確かめる** / **Step 3: 実装する** / **Step 4: 通ることを確かめる** — Run: `cargo test -p s2v-bubbles api`
- [ ] **Step 5: 全体の検証とコミット** — `feat: bubbles の API(状態・配置・描画・保存)の中身を加える`

---

### Task 9: サーバー(`server.rs`)、仮の画面、HTTP の結合テスト

**Files:**
- Create: `crates/s2v-bubbles/src/server.rs`、`src/assets/index.html`・`app.js`・`app.css`、`tests/http.rs`
- Modify: `crates/s2v-bubbles/src/lib.rs`

**Interfaces:**
- Consumes: Task 4〜8。
- Produces:
  - `pub struct ServeOptions { pub port: Option<u16>, pub ffmpeg: String }`
  - `pub enum LogEvent { Info(String), Warn(String), Error(String) }`(主スレッドへ知らせる。トークンを含めない)
  - `pub struct Server { … }`、`pub fn Server::bind(session: Session, media: BTreeMap<String, LockedFile>, audio: LockedFile, opts: ServeOptions) -> Result<Server, String>`(127.0.0.1 で待ち受ける。`--port` が使えなければ Err)、`pub fn port(&self) -> u16`、`pub fn url(&self) -> String`(`http://127.0.0.1:<port>/?token=<T>`)、`pub fn quit_handle(&self) -> QuitHandle`(`/api/quit` の経路とテストから呼ぶ)、`pub fn run(self, log: &mut dyn FnMut(LogEvent)) -> Result<(), String>`(受け付けのループ。`/api/quit` か `QuitHandle::quit` で終了の手順を行って戻る)

振り分け(主スレッド):
1. Host を検査(違えば 403)。
2. `GET /?token=<T>`: 正しければ `Set-Cookie` と `Location: /` の 303、違えば 403。
3. それ以外: クッキーを検査(違えば 403)。
4. `GET /`・`/app.js`・`/app.css`: 埋め込み(`include_str!`)。`Content-Type` は `text/html; charset=utf-8`・`text/javascript; charset=utf-8`・`text/css; charset=utf-8`。
5. `GET`・`HEAD` の `/media/asset/<ID>`・`/media/audio`: `Slots::try_acquire`(無ければ 503)→ `Request` をスレッドへ移し、`media_response` と `reader` で応答。素材の無い ID は 404。
6. `GET /media/frame/<ID>?n=<n>`: 動画の素材でなければ 404。n が整数でない・カットの範囲外 → 400。`asset_frame` で k を求め、`frame_args` を `FrameWorker::try_submit`(Busy なら 503)。受け取りは、主スレッドを止めないよう、`Request` と `Receiver` を別スレッドに移して待ち、そのスレッドが応答する。
7. `POST /api/render`・`/api/layout`・`/api/save`・`/api/quit`: Origin・Content-Type・本文の大きさ(`Content-Length` が無い・1 MiB 超 → 413。読み出しも 1 MiB + 1 で打ち切る)を検査してから `api` を呼ぶ。`/api/render` の応答に `X-S2V-Origin: <dx>,<dy>`・`X-S2V-Clipped: 0|1`。
8. `GET /api/state`。
9. 未知のパス 404、許されないメソッド 405。
10. 全ての応答に `common_headers()`。エラーの本文は `{"error":"<日本語の短い理由>"}`(パスを含めない)。

終了の手順: `server.unblock()` 相当で受け付けを止め、メディアのスレッドの終わりを最大 5 秒待ち、`FrameWorker::shutdown`、`log(Info("bubbles を終了しました"))`。ロックは呼び出し側(`s2v-cli`)が `run` の戻りの後に外す。

仮の画面(`index.html`・`app.js`・`app.css`): `/api/state` を取得し、シーン・カット・項目の ID と件数と警告を `textContent` で一覧にし、「保存」(現在の `manual` をそのまま `/api/save`)と「終了」(`/api/quit`)のボタンだけを持つ。`innerHTML` を使わない。インラインのスクリプト・スタイルを使わない(CSP)。

- [ ] **Step 1: 失敗する結合テストを書く**(`tests/http.rs`。`Session` は Task 8 のテストの補助と同じ要領で作る。生の HTTP を `TcpStream` で送る補助 `send(port, raw: &str) -> (status, headers, body)` と、トークン → クッキーの補助 `login(port, token) -> cookie`)

```rust
#[test] fn token_gives_cookie_and_redirect() {}          // 正しいトークン → 303、Set-Cookie s2v_<port>=…、Location /。違うトークン → 403、Set-Cookie なし
#[test] fn requests_without_cookie_are_403() {}          // /, /app.js, /api/state, /media/audio, POST /api/quit(Origin つき)がすべて 403
#[test] fn bad_host_is_403() {}                          // Host: evil.example:<port> → 403(クッキーがあっても)
#[test] fn post_needs_origin_and_json_and_size() {}      // Origin なし/違う → 403、Content-Type text/plain → 415、Content-Length 1 MiB + 1 → 413
#[test] fn common_headers_on_every_response() {}         // 200・403・404 の応答に CSP・nosniff・no-referrer・no-store、Access-Control-* なし
#[test] fn media_range_and_head() {}                     // /media/audio: Range なし 200、bytes=0-9 206 と Content-Range、bytes=999999999- 416 と bytes */L と Content-Length 0、HEAD は本文なしで同じヘッダー、Accept-Ranges bytes
#[test] fn unknown_cut_is_404() {}                       // /media/asset/S99-C01、/media/asset/..%2F..%2Fx → 404
#[test] fn api_responds_while_media_is_being_read() {}   // Review Focus 3: 大きな音声(数十 MiB)を bytes=0- で要求し、応答を読まずに保持したまま、POST /api/layout が 1 秒以内に 200
#[test] fn media_slots_full_is_503_then_recovers() {}    // 8 本のメディアの要求を読まずに保持 → 9 本目 503。1 本を閉じて少し待つと次が 200
#[test] fn save_round_trip() {}                          // POST /api/save → 200、bubbles.json と .bak。壊れた JSON → 400、逆転 → 422
#[test] fn render_returns_png_with_origin_headers() {}   // POST /api/render → image/png、X-S2V-Origin、X-S2V-Clipped
#[test] fn quit_stops_the_server() {}                    // POST /api/quit → 200 {"quitting":true}、run が戻る、ポートに接続できなくなる
#[test] fn two_servers_do_not_share_cookie() {}          // Review Focus 2: 2 つのサーバーにそれぞれログインし、片方のクッキーではもう片方が 403、両方とも自分のクッキーで 200
#[test] fn embedded_assets_have_no_inner_html() {}       // include_str! の app.js に "innerHTML" が無い
```

`tests/http_ffmpeg.rs`(実際の ffmpeg):

```rust
#[test] fn frame_endpoint_returns_png_and_rejects_out_of_range() {}  // 動画の素材のカットで ?n=<n0+3> → 200 image/png、?n=<n1> → 400、?n=x → 400
```

- [ ] **Step 2: 失敗を確かめる** — Run: `cargo test -p s2v-bubbles --test http` / Expected: `Server` が無くコンパイルエラー。
- [ ] **Step 3: 実装する** — 上の振り分けと終了の手順。
- [ ] **Step 4: 通ることを確かめる** — Run: `cargo test -p s2v-bubbles`(ffmpeg あり)/ Expected: PASS。
- [ ] **Step 5: 全体の検証とコミット** — `feat: bubbles のサーバー(認証・振り分け・メディアのスレッド・終了)と仮の画面を加える`

---

### Task 10: `script2video bubbles` コマンド、CLI のテスト、README

**Files:**
- Create: `crates/s2v-cli/src/bubbles.rs`、`crates/s2v-cli/tests/bubbles_cli.rs`
- Modify: `crates/s2v-cli/src/lib.rs`(`pub mod bubbles; pub use bubbles::{run_bubbles, BubblesArgs};`)、`src/main.rs`、`crates/s2v-cli/Cargo.toml`(`s2v-bubbles`)、`README.md`

**Interfaces:**
- Consumes: Task 2 の `preflight`・`prepare(…, MissingAssets::Allow, …)`、Task 6 の `open_locked`、Task 9 の `Server`。
- Produces: `pub struct BubblesArgs { pub script: PathBuf, pub config: Option<PathBuf>, pub port: Option<u16>, pub no_open: bool }`、`pub fn run_bubbles(args: &BubblesArgs) -> Outcome`。CLI: `script2video bubbles <台本> [--config <path>] [--port N] [--no-open]`。

処理の順:
1. `preflight`(失敗は標準エラーと エラー 2)。`ProcLog::append(Process.log, "bubbles")`。
2. `prepare(…, MissingAssets::Allow, …)`(Err はログに書いて 2)。警告はログへ。
3. 素材ごとに `open_locked`(失敗は「素材なし」として警告し、その素材を除く)。`full_dialogue.wav` を `open_locked`(失敗は 2)。動画の素材は `asset_frames(stream.duration_ts, stream.time_base, fps)` で `frames` を求める(None なら警告して `frames` は None。画面は `<video>` に任せる)。
4. `Session` を作り、`Server::bind`(ポートが使えなければ 2)。URL を標準出力に表示(Process.log には書かない)。`--no-open` でなければ `Command::new("cmd").args(["/C", "start", "", &url])` で開く(失敗は警告)。Windows 以外では開かずに表示だけ。
5. Ctrl+C: 新しい依存を加えずに標準ライブラリだけでは受け取れないので、**Ctrl+C ではプロセスがそのまま終わる**(設計書 §5.1・§5.4。ロックとハンドルは OS が外し、同じコンソールの ffmpeg も Ctrl+C を受けて終わる。Process.log の終了の行は書かれない)。`QuitHandle` は `/api/quit` の経路だけで使う。
6. `run` が戻ったらログに終了、`log.check()`。ロック(`Preflight.lock`)を drop。終了コード: 起動時の警告があれば 1、無ければ 0。

- [ ] **Step 1: 失敗するテストを書く**(`tests/bubbles_cli.rs`。build を通した台本で、実行ファイル `env!("CARGO_BIN_EXE_script2video")` を `bubbles --no-open --port 0` ではなく `--no-open` で子プロセスとして起動する)

```rust
#[test] fn starts_prints_url_and_quits() {}      // 標準出力から URL を読み、トークンでログイン、POST /api/quit → 子プロセスが終了コード 0 か 1 で終わる。Process.log に "== bubbles " と終了の行、トークンが含まれない
#[test] fn lock_conflict_is_failed() {}          // bubbles の起動中に run_build(同じ台本)→ Outcome::Failed、quit 後は build できる
#[test] fn missing_asset_starts_with_warning() {} // 素材なしでも起動し、/api/state の warnings に素材なしの文、quit 後の終了コード 1
#[test] fn busy_port_is_failed() {}              // TcpListener で埋めたポートを --port に → 終了コード 2
#[test] fn stale_build_is_failed() {}            // build 後に full_dialogue.wav を書き換え → 終了コード 2(待ち受けない)
```

- [ ] **Step 2: 失敗を確かめる** / **Step 3: 実装する**(`main.rs` に `Bubbles { script, #[arg(long)] config, #[arg(long)] port: Option<u16>, #[arg(long)] no_open: bool }`)/ **Step 4: 通ることを確かめる** — Run: `cargo test -p s2v-cli --test bubbles_cli`
- [ ] **Step 5: README を更新する** — `bubbles` の節(`render` の前): 使い方、前提(ffmpeg・ffprobe)、ブラウザが開く・`--no-open`・`--port`、起動中は素材を上書き・削除・改名できない(差し替えは終えてから)、`bubbles.json` と `.bak`、終了(画面の「終了」、Ctrl+C では未保存の変更が失われる)、終了コード、守る相手(他のウェブページ・他のローカルのサービス。同じ PC の他のプログラムからは守れない)。2d-1 の画面は仮のもの(配置の操作は 2d-2)であることを書く。
- [ ] **Step 6: 全体の検証とコミット** — `feat: bubbles コマンドで配置の調整のサーバーを起動する`
- [ ] **Step 7: 手動の確認(Human)** — sample の台本で `build` → `bubbles` を起動し、ブラウザで仮の画面が開き、項目の一覧と警告が出て、「終了」で終わることを確かめてもらう。
