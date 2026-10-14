# Script2Video 計画 2b(縦書きの描画部品 s2v-render と最終合成 render) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 吹き出し(台詞)とト書きを縦書きの PNG に描く部品 `s2v-render` と、素材・音声・吹き出しを ffmpeg で合成して `output.mp4` を作るコマンド `script2video render` を作る。

**Architecture:** `s2v-render`(新規)は、フォントの読み込みと検査、禁則つきの縦書きの組版、tiny-skia による描画と合成、`bubbles.json` の型と検証、自動配置、素材の照合を持つ(③の GUI と共用する)。`s2v-video`(新規)は、コマの計算と区間の分割、重ね画像の一覧(ffconcat)、ffmpeg・ffprobe の引数の組み立て、ffprobe の結果の検査、音声の検証を持つ(外部プロセスの実行は呼び出し側)。`s2v-cli` の `render.rs` が全体を順に実行する。

**Tech Stack:** Rust 1.96、rustybuzz 0.20(`rustybuzz::ttf_parser` を使う)、tiny-skia 0.12(png 機能)、unicode-segmentation 1、realfft 3、hound 3、serde / serde_json、ffmpeg・ffprobe(6.0 以上。外部)。

**Spec:** `docs/superpowers/specs/2026-10-14-script2video-video-design.md`(以下「設計書」)の §4(render)、§4.7(設定)、§5.2(`bubbles.json`)、§6(描画部品)。前提として計画 2a(`s2v-timeline`、`freshness::check_fresh`、`ScriptLock`、`ProcLog::append`、`fsutil`、`layout`)が main にある。

## Global Constraints

- 応答・コメント・エラーメッセージ・コミットメッセージは日本語(既存のコードに合わせる)。
- 出力: H.264(libx264、`-profile:v high`、`yuv420p`、BT.709・tv のタグ)+ AAC-LC 48000 Hz 2ch、`-movflags +faststart`。既定 1920×1080・30fps(設計書 §4.7)。
- `frame(t) = floor((t × fps + 500) / 1000)`(t はミリ秒。i128 で計算)。全体のコマ数は `ceil(duration_ms × fps / 1000)`。`duration_ms` が 24 時間(86_400_000)を超えたら エラー 2(既存の `freshness::check_fresh(script, true)` が検査する)。
- ffmpeg の全ての呼び出しに `-nostdin -hide_banner -n`。シェルを経由せず引数の配列で起動する。作業フォルダは `work/render/seg/`。ASCII の相対名(`cut_0001.mp4`、`ov_0001.png`、`ov_empty.png`、`ovl_0001.txt`、`concat.txt`)を使う。
- 重ね画像の一覧の全ての `file` に `option framerate <2×fps>`。区間 k の始まり `u_0 = 0`、`u_k = (r_k − 0.5)/fps`、`u_m = N/fps`、`duration` はマイクロ秒に丸めた値の差(小数6桁)、最後の画像を `duration` なしで再掲する。
- 重ね画像は画面の大きさの PNG(straight alpha。tiny-skia の `encode_png` は乗算済みを戻して書く)。重ねる順は ト書き → 吹き出し(表示開始の順、同時なら台本の並び)。
- 色: 出力は原色・伝達関数・行列・レンジとも BT.709・tv。動画の素材の `color_primaries`・`color_transfer` が明示されていて `bt709` 以外なら エラー 2。行列は `bt709`(値なし・unknown も)、`bt470bg`・`smpte170m`(→ scale の `bt601`)を受け、それ以外は エラー 2。レンジは `tv`・`pc`(値なし・unknown は `tv`)。JPEG は `bt601`・`pc` を入力に明示。PNG は RGB。sRGB の伝達関数は変換しない(承認済みの近似)。
- 素材の拡張子: 画像 `png`・`jpg`・`jpeg`、動画 `mp4`・`mov`・`webm`(大文字小文字を区別しない)。WebP は受け付けない。
- 縦書きの機能: フェイスの GSUB に `vrt2` があれば `-vert,+vrt2`、無ければ `+vert`。どちらも無ければ エラー 2。`vhea`・`vmtx` が無ければ エラー 2。
- 禁則: 行頭禁則 `、。,.・:;?!ー」』)】〉》〕ぁぃぅぇぉっゃゅょゎァィゥェォッャュョヮヵヶ々〜` と、その全角形(`，．：；？！）〕` など)。行末禁則 `「『(【〈《〔` とその全角形(`（`)。
- `bubbles.json` の `version` は 1。未知のキーは エラー。上限: 枠と尻尾の先端は画面の内側、`|offset| ≤ 60000`、`items` は 10000 件まで、canvas は §4.7 の解像度の範囲。
- 終了コード: 0 成功、1 警告あり、2 失敗(設計書 §4.8)。失敗時は `output.mp4` を変えない。
- ffmpeg を使う結合テストは、ffmpeg が無ければ**失敗**する。環境変数 `S2V_SKIP_FFMPEG_TESTS=1` のときだけ飛ばす。
- 検証コマンド(各タスクの最後に全部通す): `cargo fmt --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace`。
- コミットメッセージの末尾に、空行のあと実装者のモデルの `Co-Authored-By:` 行。

## Review Focus

1. **台詞が長く、吹き出しが画面に収まらない**(自動配置の最小の大きさが画面より大きい): 黙って画面外へはみ出さず、画面の大きさに収めて警告(終了コード 1)にする。→ Task 7 のテスト `oversized_item_is_clamped_with_warning`。
2. **素材の名前の大文字小文字の違い**(`s01.PNG`): 素材として見つかる。→ Task 8 のテスト `stem_and_extension_match_case_insensitively`。
3. **同じカットに `S01.png` と `S01.jpg` の両方**: エラー 2 で、どちらのファイル名も一覧に出る。→ Task 8 のテスト `two_assets_for_one_cut_is_an_error_naming_both`。
4. **`bubbles.json` を手で編集して壊した**(JSON の誤り・未知のキー): エラー 2 で、どこが悪いかを表示する。前回の `output.mp4` は変わらない。→ Task 6 のテスト `unknown_key_and_broken_json_are_errors`、Task 12 のテスト `broken_bubbles_json_keeps_previous_output`。
5. **ffmpeg が PATH に無い / 版が古い**: エラー 2 で「ffmpeg が見つかりません」などを表示し、何も書かない(`work/render/` も作らない)。→ Task 10 のテスト `version_parsing`、Task 12 のテスト `missing_ffmpeg_is_failed_without_writing`。

---

## ファイル構成

| ファイル | 役割 | タスク |
|---|---|---|
| `config.toml`、`crates/s2v-core/src/config.rs` | `[render]`・`[prompts]` の設定と検証 | 1 |
| `crates/s2v-render/`(新規クレート)`Cargo.toml`、`src/lib.rs` | クレートの入口 | 2 |
| `crates/s2v-render/tests/fonts/`(`test-vrt2.ttf`・`test-vert.ttf`・`test-novert.ttf`・`OFL.txt`)、`scripts/make_test_fonts.py` | テスト用のフォント(Noto Sans JP の部分集合) | 2 |
| `crates/s2v-render/src/font.rs` | フォントの読み込みと検査(§6.4) | 2 |
| `crates/s2v-render/src/kinsoku.rs` | 列の分割と禁則(§6.1 の手順。フォントに依存しない) | 3 |
| `crates/s2v-render/src/layout.rs` | 単位の整形、縦中横・回転、組版、`min_size`・`fits`(§6.1〜6.2) | 4 |
| `crates/s2v-render/src/draw.rs` | 吹き出し・ト書きの描画、合成(§6.2〜6.3) | 5 |
| `crates/s2v-render/src/bubbles.rs` | `bubbles.json` の型・検証・拡大縮小・時刻・最小への拡大(§5.2) | 6 |
| `crates/s2v-render/src/autolayout.rs` | 自動配置(§4.4) | 7 |
| `crates/s2v-render/src/assets.rs` | 素材の照合(§4.3、ファイル名とリンクの検査) | 8 |
| `crates/s2v-video/`(新規クレート)`src/lib.rs`、`src/frames.rs` | コマ・区間・重ね画像の一覧(§4.2、§4.5) | 9 |
| `crates/s2v-video/src/ffmpeg.rs` | ffmpeg の版の判定と引数の組み立て(§4.5) | 10 |
| `crates/s2v-video/src/probe.rs`、`src/audio_check.rs` | ffprobe の結果の解析と検査、音声の検証(§4.3 の色、§4.5 の連結の検査、§4.6) | 11 |
| `crates/s2v-cli/src/render.rs`、`src/main.rs`、`src/lib.rs`、`tests/render_ffmpeg.rs` | `render` コマンドと ffmpeg の結合テスト(§4.1) | 12 |
| `README.md`、`crates/s2v-cli/tests/render_perf.rs` | 説明と性能試験 | 13 |

---

### Task 1: 設定 `[render]`・`[prompts]`

**Files:**
- Modify: `config.toml`(末尾に `[render]` と `[prompts]` を追加)
- Modify: `crates/s2v-core/src/config.rs`

**Interfaces:**
- Produces: `Config.render: RenderConfig`、`Config.prompts: PromptsConfig`(どちらも `#[serde(default)]`)。`RenderConfig` の各フィールドは設計書 §4.7 の表のとおり。`pub enum Fit { Contain, Cover }`(`#[serde(rename_all = "lowercase")]`)。`RenderConfig::rgb(s: &str) -> Option<[u8; 3]>`(`#RRGGBB` を読む)。`Config::validate()` に `[render]` の検査を加える。

`RenderConfig`:

```rust
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct RenderConfig {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub crf: u32,
    pub preset: String,
    pub audio_bitrate: String,
    pub fit: Fit,
    pub background: String,
    pub default_image: Option<String>,
    pub ffmpeg: Option<String>,
    pub ffprobe: Option<String>,
    pub jobs: u32,
    pub font_path: String,
    pub font_index: u32,
    pub font_size: f32,
    pub line_spacing: f32,
    pub bubble_fill: String,
    pub bubble_stroke: String,
    pub bubble_stroke_width: f32,
    pub bubble_padding: u32,
    pub note_fill: String,
    pub note_stroke: String,
    pub note_stroke_width: f32,
    pub note_padding: u32,
    pub text_color: String,
    pub margin: u32,
    pub bubble_gap: u32,
    pub tail_length: u32,
    pub bubble_max_height_ratio: f32,
    pub note_max_height_ratio: f32,
}
```

既定値(`impl Default`)は設計書 §4.7 の表と同じ: 1920 / 1080 / 30 / 20 / "medium" / "192k" / Contain / "#000000" / None / None / None / 1 / `C:\Windows\Fonts\YuGothM.ttc` / 0 / 44.0 / 0.4 / "#FFFFFF" / "#000000" / 4.0 / 28 / "#FFFBE6" / "#000000" / 3.0 / 20 / "#000000" / 40 / 24 / 80 / 0.55 / 0.6。

`PromptsConfig { style: String(既定 ""), suffix: String(既定 "縦横比 16:9。画像の中に文字・吹き出しを入れない。") }`(計画 2c で使う。ここでは型と既定値だけ)。

検査(`validate` に追加。メッセージは「render.width は 16〜7680 の偶数にしてください: 1921」の形):
- width: 16〜7680 の偶数。height: 16〜4320 の偶数。fps: 1〜120。crf: 0〜51。preset: `ultrafast superfast veryfast faster fast medium slow slower veryslow` のどれか。audio_bitrate: `^[1-9][0-9]{1,3}k$`。jobs: 1〜64。
- 色(background・bubble_fill・bubble_stroke・note_fill・note_stroke・text_color): `#RRGGBB`(16進、大文字小文字可)。
- font_size: 8〜400(有限)。line_spacing: 0〜3。stroke_width: 0〜50。padding: 0〜500。margin・bubble_gap・tail_length: 0〜2000。max_height_ratio: 0 より大きく 1 以下。
- font_path は空でない。

- [ ] **Step 1: 失敗するテストを書く(`config.rs` の tests)**

```rust
#[test]
fn render_defaults_match_the_spec() {
    let c = Config::builtin();
    let r = &c.render;
    assert_eq!((r.width, r.height, r.fps, r.crf), (1920, 1080, 30, 20));
    assert_eq!(r.preset, "medium");
    assert_eq!(r.fit, Fit::Contain);
    assert_eq!(r.jobs, 1);
    assert_eq!(r.font_path, "C:\\Windows\\Fonts\\YuGothM.ttc");
    assert_eq!(r.font_size, 44.0);
    assert_eq!((r.margin, r.bubble_gap, r.tail_length), (40, 24, 80));
    assert_eq!(c.prompts.suffix, "縦横比 16:9。画像の中に文字・吹き出しを入れない。");
    assert!(c.validate().is_empty(), "{:?}", c.validate());
}

#[test]
fn render_section_may_be_omitted() {
    let mut text = include_str!("../../../config.toml").to_string();
    let cut = text.find("[render]").unwrap();
    text.truncate(cut);
    let c = Config::from_toml(&text).unwrap();
    assert_eq!(c.render.width, 1920);
}

#[test]
fn render_values_are_validated() {
    let mut c = Config::builtin();
    c.render.width = 1921;
    c.render.fps = 0;
    c.render.preset = "turbo".into();
    c.render.background = "black".into();
    c.render.jobs = 0;
    c.render.bubble_max_height_ratio = 0.0;
    c.render.audio_bitrate = "192".into();
    let errs = c.validate();
    for key in ["render.width", "render.fps", "render.preset", "render.background", "render.jobs", "render.bubble_max_height_ratio", "render.audio_bitrate"] {
        assert!(errs.iter().any(|e| e.contains(key)), "{key}: {errs:?}");
    }
}

#[test]
fn rgb_parses_hex_colors() {
    assert_eq!(RenderConfig::rgb("#FFfbE6"), Some([0xFF, 0xFB, 0xE6]));
    assert_eq!(RenderConfig::rgb("FFFBE6"), None);
    assert_eq!(RenderConfig::rgb("#FFFBE"), None);
    assert_eq!(RenderConfig::rgb("#GGGGGG"), None);
}
```

- [ ] **Step 2: 失敗を確かめる** — Run: `cargo test -p s2v-core config` / Expected: `render` が未定義でコンパイルエラー。

- [ ] **Step 3: 実装する** — 上の型・既定値・検査を書く。`Config` に `#[serde(default)] pub render: RenderConfig` と `#[serde(default)] pub prompts: PromptsConfig` を加える。`config.toml` の末尾に、既定値と同じ値の `[render]`(各キーに日本語の短いコメント)と `[prompts]` を書く。`audio_bitrate` の検査は regex を使わず、末尾が `k` で、残りが 2〜4 桁の数字で先頭が 0 でないことを確かめる。

- [ ] **Step 4: テストが通ることを確かめる** — Run: `cargo test -p s2v-core config`

- [ ] **Step 5: 全体の検証とコミット** — `feat: 設定に [render] と [prompts] を加える`

---

### Task 2: `s2v-render` クレート、テスト用フォント、フォントの読み込みと検査

**Files:**
- Modify: `Cargo.toml`(members に `"crates/s2v-render"`、`"crates/s2v-video"` は Task 9)
- Create: `crates/s2v-render/Cargo.toml`、`src/lib.rs`、`src/font.rs`
- Create: `scripts/make_test_fonts.py`、`crates/s2v-render/tests/fonts/{test-vrt2.ttf,test-vert.ttf,test-novert.ttf,OFL.txt,README.md}`

**Interfaces:**
- Produces:

```rust
pub struct FontFace {
    data: std::sync::Arc<Vec<u8>>,
    index: u32,
    pub units_per_em: u16,
    pub ascender: i16,
    pub descender: i16,
    pub vertical: VerticalFeature,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerticalFeature { Vrt2, Vert }
impl FontFace {
    pub fn load(path: &std::path::Path, index: u32) -> Result<FontFace, FontError>;
    pub fn from_bytes(data: Vec<u8>, index: u32) -> Result<FontFace, FontError>;
    pub fn data(&self) -> &[u8];
    pub fn index(&self) -> u32;
    /// rustybuzz に渡す縦書きの機能(Vrt2 なら [-vert, +vrt2]、Vert なら [+vert])
    pub fn vertical_features(&self) -> Vec<rustybuzz::Feature>;
    /// text の文字のうち、フォントに字形が無いもの(重複なし、出現順)
    pub fn missing_chars(&self, text: &str) -> Vec<char>;
}
#[derive(Debug, Clone, PartialEq)]
pub enum FontError {
    Read(String),
    BadIndex { index: u32, count: u32, names: Vec<String> },
    Parse(String),
    NoVerticalMetrics,
    NoVerticalFeature,
}
impl std::fmt::Display for FontError  // 日本語のメッセージ。BadIndex は収録数と各フェイスの名前を並べる
```

- [ ] **Step 1: テスト用フォントを作る**

`scripts/make_test_fonts.py`(fonttools を使う。生成物はリポジトリにコミットする。再生成の手順は `tests/fonts/README.md` に書く):

```python
"""テスト用フォント(Noto Sans JP の部分集合、OFL)を作る。
使い方: python -I scripts/make_test_fonts.py <NotoSansJP-VF.ttf のパス>
"""
import sys
from pathlib import Path
from fontTools.ttLib import TTFont
from fontTools.varLib.instancer import instantiateVariableFont
from fontTools import subset

TEXT = (
    "あいうえおかきくけこさしすせそたちつてとなにぬねのはひふへほまみむめもやゆよらりるれろわをん"
    "ぁぃぅぇぉっゃゅょゎァィゥェォッャュョヮヵヶ々〜ー"
    "アイウエオカキクケコサシスセソタチツテトナニヌネノハヒフヘホマミムメモヤユヨラリルレロワヲン"
    "一二三四五六七八九十港朝昼夜海波岸壁翌二人黙早今日晴雨台詞吹出書枠縦横字"
    "、。，．・：；？！「」『』（）【】〈〉《》〔〕　"
    "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz !?.,:;()-+%"
)
OUT = Path(__file__).resolve().parent.parent / "crates" / "s2v-render" / "tests" / "fonts"

def build(src, features, out_name):
    font = TTFont(src)
    font = instantiateVariableFont(font, {"wght": 400})
    opts = subset.Options()
    opts.layout_features = features
    opts.name_IDs = ["*"]
    opts.name_languages = ["*"]
    opts.notdef_outline = True
    opts.glyph_names = False
    sub = subset.Subsetter(opts)
    sub.populate(text=TEXT)
    sub.subset(font)
    font.save(OUT / out_name)

def main():
    src = sys.argv[1]
    OUT.mkdir(parents=True, exist_ok=True)
    build(src, ["vert", "vrt2", "kern", "palt"], "test-vrt2.ttf")
    build(src, ["vert"], "test-vert.ttf")
    build(src, [], "test-novert.ttf")

if __name__ == "__main__":
    main()
```

実行: `python -I scripts/make_test_fonts.py "C:\Windows\Fonts\NotoSansJP-VF.ttf"`。`OFL.txt` には SIL Open Font License 1.1 の全文と、`Copyright 2014-2021 Adobe (http://www.adobe.com/), with Reserved Font Name 'Source'`(Noto Sans JP の著作権表示。フォントの name テーブルの ID 0 の文字列をそのまま写す)を置く。`README.md` には、元のフォント、部分集合の文字、再生成のコマンドを書く。生成したフォントで、`test-vrt2.ttf` は GSUB に `vrt2` と `vert` があり、`test-vert.ttf` は `vert` だけ、`test-novert.ttf` はどちらも無いことを、fonttools で確かめて README に書く。

- [ ] **Step 2: 失敗するテストを書く(`font.rs` の tests)**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    pub(crate) fn font_path(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("fonts").join(name)
    }

    #[test]
    fn vrt2_is_preferred_and_vert_is_disabled() {
        let f = FontFace::load(&font_path("test-vrt2.ttf"), 0).unwrap();
        assert_eq!(f.vertical, VerticalFeature::Vrt2);
        let feats: Vec<String> = f.vertical_features().iter().map(|x| format!("{x:?}")).collect();
        // vert は値 0(無効)、vrt2 は値 1(有効)
        assert_eq!(f.vertical_features().len(), 2, "{feats:?}");
        assert!(f.vertical_features().iter().any(|x| x.tag == rustybuzz::ttf_parser::Tag::from_bytes(b"vert") && x.value == 0));
        assert!(f.vertical_features().iter().any(|x| x.tag == rustybuzz::ttf_parser::Tag::from_bytes(b"vrt2") && x.value == 1));
    }

    #[test]
    fn vert_only_font_uses_vert() {
        let f = FontFace::load(&font_path("test-vert.ttf"), 0).unwrap();
        assert_eq!(f.vertical, VerticalFeature::Vert);
        let v = f.vertical_features();
        assert_eq!(v.len(), 1);
        assert_eq!((v[0].tag, v[0].value), (rustybuzz::ttf_parser::Tag::from_bytes(b"vert"), 1));
    }

    #[test]
    fn font_without_vertical_feature_is_rejected() {
        assert_eq!(FontFace::load(&font_path("test-novert.ttf"), 0).err(), Some(FontError::NoVerticalFeature));
    }

    #[test]
    fn bad_index_and_missing_file() {
        let e = FontFace::load(&font_path("test-vrt2.ttf"), 3).err().unwrap();
        assert!(matches!(e, FontError::BadIndex { index: 3, count: 1, .. }), "{e:?}");
        assert!(e.to_string().contains("1"), "{e}");
        assert!(matches!(FontFace::load(&font_path("無い.ttf"), 0), Err(FontError::Read(_))));
        assert!(matches!(FontFace::from_bytes(b"not a font".to_vec(), 0), Err(FontError::Parse(_))));
    }

    #[test]
    fn missing_chars_lists_each_once_in_order() {
        let f = FontFace::load(&font_path("test-vrt2.ttf"), 0).unwrap();
        assert_eq!(f.missing_chars("港ゑ朝😀港😀龘"), vec!['ゑ', '😀', '龘']); // ゑ・龘・絵文字は部分集合に無い
        assert!(f.missing_chars("港、朝。").is_empty());
    }

    #[test]
    fn system_yu_gothic_is_accepted_when_present() {
        let p = std::path::Path::new("C:\\Windows\\Fonts\\YuGothM.ttc");
        if !p.exists() {
            return; // Windows 以外・游ゴシックが無い環境では確かめない
        }
        let f = FontFace::load(p, 0).unwrap();
        assert_eq!(f.vertical, VerticalFeature::Vrt2);
    }
}
```

(注: `missing_chars` の期待値の `ゑ`・`龘` は、Step 1 の TEXT に含めていないので部分集合に無い。TEXT を変えたらこのテストも合わせる。)

- [ ] **Step 3: 失敗を確かめる** — Run: `cargo test -p s2v-render font` / Expected: コンパイルエラー。

- [ ] **Step 4: 実装する**

`crates/s2v-render/Cargo.toml`:

```toml
[package]
name = "s2v-render"
version = "0.1.0"
edition = "2021"

[dependencies]
s2v-core = { path = "../s2v-core" }
s2v-timeline = { path = "../s2v-timeline" }
rustybuzz = "0.20"
tiny-skia = "0.12"
unicode-segmentation = "1"
serde.workspace = true
serde_json = "1"
anyhow.workspace = true

[dev-dependencies]
tempfile = "3"
```

`font.rs` の要点:
- `load` は `std::fs::read` → 失敗は `Read`。`from_bytes` は `rustybuzz::ttf_parser::fonts_in_collection(&data)`(TTC なら Some(個数)、単体なら None → 1)で個数を求め、`index >= 個数` なら `BadIndex`(`names` は各フェイスの name ID 4(完全名)。読めなければ空文字)。
- `rustybuzz::ttf_parser::Face::parse(&data, index)` の失敗は `Parse`。
- `vhea` が無い(`face.tables().vhea.is_none()`)か `vmtx` が無い(`face.tables().vmtx.is_none()`)なら `NoVerticalMetrics`。
- GSUB の機能の一覧(`face.tables().gsub` の `features` を走査して `tag` を集める)に `vrt2` があれば `Vrt2`、無くて `vert` があれば `Vert`、どちらも無ければ `NoVerticalFeature`。
- `vertical_features`: `Vrt2` → `vec![Feature::new(Tag::from_bytes(b"vert"), 0, ..), Feature::new(Tag::from_bytes(b"vrt2"), 1, ..)]`、`Vert` → `vec![Feature::new(Tag::from_bytes(b"vert"), 1, ..)]`。
- `missing_chars`: `face.glyph_index(c).is_none()` の文字。ASCII の制御文字と空白(U+0020)と全角空白(U+3000)は対象外。
- `units_per_em`・`ascender`・`descender` は face から写す。`data` は `Arc<Vec<u8>>` で持ち、呼び出し側が `rustybuzz::Face::from_slice(face.data(), face.index())` で都度作れるようにする(自己参照の構造体を作らない)。

`lib.rs`: `pub mod font;`(以降のタスクで `kinsoku`・`layout`・`draw`・`bubbles`・`autolayout`・`assets` を加える)。

- [ ] **Step 5: テストが通ることを確かめる** — Run: `cargo test -p s2v-render font`

- [ ] **Step 6: 全体の検証とコミット** — `feat: s2v-render クレートとテスト用フォントを加え、縦書きのフォントを読み込んで検査する`(フォントの .ttf と OFL.txt も add する)

---

### Task 3: 列の分割と禁則(フォントに依存しない)

**Files:**
- Create: `crates/s2v-render/src/kinsoku.rs`
- Modify: `crates/s2v-render/src/lib.rs`(`pub mod kinsoku;`)

**Interfaces:**
- Produces:

```rust
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UnitClass {
    pub height: f32,
    /// 列の頭に置かない(行頭禁則。単位の最初の文字で判定)
    pub no_start: bool,
    /// 列の末尾に置かない(行末禁則。単位の最後の文字で判定)
    pub no_end: bool,
}
pub fn is_no_start(c: char) -> bool;
pub fn is_no_end(c: char) -> bool;
/// 設計書 §6.1 の手順で列に分ける。戻り値は各列の単位の範囲(連続)。units が空なら空。
pub fn break_columns(units: &[UnitClass], inner_h: f32) -> Vec<std::ops::Range<usize>>;
```

規則(設計書 §6.1):
1. **詰め込み**: 先頭から、列の高さが `inner_h` を超えない限り現在の列に入れる。入らなければ新しい列(単位が1つで高さを超えるときは、その単位だけの列)。比較には誤差 `1e-3` を許す。
2. **修復**: 境目 k(列 k と k+1)を k=0 から順に処理する。この境目の処理の直前の (列 k, 列 k+1) を保存する。
   - 列 k の末尾が `no_end` で、列 k に 2 つ以上あれば、末尾を列 k+1 の頭へ移す(移動 1 回)。
   - 列 k+1 の頭が `no_start` である間、列 k に 2 つ以上あり、この境目での移動が 2 回未満なら、列 k の末尾を列 k+1 の頭へ移す。
   - 移動が終わっても「列 k の末尾が `no_end`」または「列 k+1 の頭が `no_start`」なら、保存した状態に戻す。
   - 列 k+1 の高さが `inner_h` を超え、かつ 2 つ以上ある間、列 k+1 の末尾を列 k+2 の頭へ移す(列 k+2 が無ければ作る)。
   - k を 1 進める(列が増えていればそこまで処理する)。

- [ ] **Step 1: 失敗するテストを書く**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn u(h: f32) -> UnitClass { UnitClass { height: h, no_start: false, no_end: false } }
    fn s(h: f32) -> UnitClass { UnitClass { height: h, no_start: true, no_end: false } }
    fn e(h: f32) -> UnitClass { UnitClass { height: h, no_start: false, no_end: true } }

    #[test]
    fn classifies_characters() {
        for c in "、。，．・：；？！ー」』）】〉》〕ぁっゃァッヵ々〜".chars() {
            assert!(is_no_start(c), "{c}");
        }
        for c in "「『（【〈《〔(".chars() {
            assert!(is_no_end(c), "{c}");
        }
        for c in "あア港A1".chars() {
            assert!(!is_no_start(c) && !is_no_end(c), "{c}");
        }
    }

    #[test]
    fn greedy_fill() {
        assert_eq!(break_columns(&[u(1.0); 5], 2.0), vec![0..2, 2..4, 4..5]);
        assert_eq!(break_columns(&[], 2.0), Vec::<std::ops::Range<usize>>::new());
        // 1つで高さを超える単位は単独の列
        assert_eq!(break_columns(&[u(1.0), u(3.0), u(1.0)], 2.0), vec![0..1, 1..2, 2..3]);
    }

    #[test]
    fn no_start_at_column_head_pushes_previous_unit() {
        // あい|、う → あ|い、う
        assert_eq!(break_columns(&[u(1.0), u(1.0), s(1.0), u(1.0)], 2.0), vec![0..1, 1..3, 3..4]);
    }

    #[test]
    fn no_end_at_column_tail_moves_forward() {
        // あ「|いう → あ|「い|う
        assert_eq!(break_columns(&[u(1.0), e(1.0), u(1.0), u(1.0)], 2.0), vec![0..1, 1..3, 3..4]);
    }

    #[test]
    fn push_out_overflow_and_revert() {
        // あいう|、。え(高さ3): う を送って解消 → 列(う、。え)が高さ4であふれ、え を次の列へ
        let units = [u(1.0), u(1.0), u(1.0), s(1.0), s(1.0), u(1.0)];
        assert_eq!(break_columns(&units, 3.0), vec![0..2, 2..5, 5..6]);
        // あい|、。|」(高さ2): 境目0は い を送って解消し、あふれた 。 が次の列の頭へ。
        // 境目1は、列(い、)の 、 を送っても次の列の頭が 、 のままで、列が1つになって追い出せない → 取り消す
        let units = [u(1.0), u(1.0), s(1.0), s(1.0), s(1.0)];
        assert_eq!(break_columns(&units, 2.0), vec![0..1, 1..3, 3..5]);
    }

    #[test]
    fn single_unit_column_is_not_pushed() {
        // 列 k が1つだけなら追い出せない → そのまま
        assert_eq!(break_columns(&[u(2.0), s(1.0)], 2.0), vec![0..1, 1..2]);
    }

    #[test]
    fn overflow_cascades_to_new_columns() {
        let units = [u(1.0), u(1.0), s(1.0), u(1.0), u(1.0)];
        let cols = break_columns(&units, 2.0);
        // どの列も高さ2以下(1つだけの列を除く)、全ての単位を1回ずつ含む
        let covered: usize = cols.iter().map(|r| r.len()).sum();
        assert_eq!(covered, 5);
        for w in cols.windows(2) {
            assert_eq!(w[0].end, w[1].start);
        }
        for r in &cols {
            let h: f32 = units[r.clone()].iter().map(|x| x.height).sum();
            assert!(h <= 2.0 + 1e-3 || r.len() == 1, "{cols:?}");
        }
        assert_eq!(cols, vec![0..1, 1..3, 3..5]);
    }

    #[test]
    fn deterministic_for_same_input() {
        let units: Vec<UnitClass> = (0..40).map(|i| UnitClass { height: 1.0 + (i % 3) as f32 * 0.25, no_start: i % 7 == 3, no_end: i % 11 == 5 }).collect();
        assert_eq!(break_columns(&units, 5.0), break_columns(&units, 5.0));
    }
}
```

- [ ] **Step 2: 失敗を確かめる** — Run: `cargo test -p s2v-render kinsoku`

- [ ] **Step 3: 実装する**

```rust
//! 縦書きの列の分割と禁則(映像工程設計書 §6.1)。フォントに依存しない純粋な手順。

use std::ops::Range;

const EPS: f32 = 1e-3;

const NO_START: &str = "、。,.・:;?!ー」』)】〉》〕ぁぃぅぇぉっゃゅょゎァィゥェォッャュョヮヵヶ々〜，．：；？！）";
const NO_END: &str = "「『(【〈《〔（";

pub fn is_no_start(c: char) -> bool {
    NO_START.contains(c)
}

pub fn is_no_end(c: char) -> bool {
    NO_END.contains(c)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UnitClass {
    pub height: f32,
    pub no_start: bool,
    pub no_end: bool,
}

pub fn break_columns(units: &[UnitClass], inner_h: f32) -> Vec<Range<usize>> {
    if units.is_empty() {
        return Vec::new();
    }
    let height = |col: &[usize]| col.iter().map(|&i| units[i].height).sum::<f32>();
    // 1. 詰め込み
    let mut cols: Vec<Vec<usize>> = Vec::new();
    let mut cur: Vec<usize> = Vec::new();
    let mut h = 0.0f32;
    for (i, unit) in units.iter().enumerate() {
        if !cur.is_empty() && h + unit.height > inner_h + EPS {
            cols.push(std::mem::take(&mut cur));
            h = 0.0;
        }
        cur.push(i);
        h += unit.height;
    }
    cols.push(cur);
    // 2. 修復
    let mut k = 0;
    while k + 1 < cols.len() {
        let saved = (cols[k].clone(), cols[k + 1].clone());
        let mut moves = 0;
        let last = |c: &Vec<usize>| units[*c.last().expect("列は空にならない")];
        if cols[k].len() >= 2 && last(&cols[k]).no_end {
            let x = cols[k].pop().expect("2つ以上");
            cols[k + 1].insert(0, x);
            moves += 1;
        }
        while moves < 2 && cols[k].len() >= 2 && units[cols[k + 1][0]].no_start {
            let x = cols[k].pop().expect("2つ以上");
            cols[k + 1].insert(0, x);
            moves += 1;
        }
        if last(&cols[k]).no_end || units[cols[k + 1][0]].no_start {
            cols[k] = saved.0;
            cols[k + 1] = saved.1;
        }
        while height(&cols[k + 1]) > inner_h + EPS && cols[k + 1].len() >= 2 {
            let x = cols[k + 1].pop().expect("2つ以上");
            if k + 2 == cols.len() {
                cols.push(Vec::new());
            }
            cols[k + 2].insert(0, x);
        }
        k += 1;
    }
    cols.iter()
        .map(|c| c[0]..c[c.len() - 1] + 1)
        .collect()
}
```

(`push_out_overflow_and_revert` などの期待値は、上の手順を手で追った結果。実装の結果と違えば、手順どおりかを先に確かめ、手順どおりならテストの期待値の誤りとして報告する。勝手に手順を変えない。)

- [ ] **Step 4: テストが通ることを確かめる** — Run: `cargo test -p s2v-render kinsoku`

- [ ] **Step 5: 全体の検証とコミット** — `feat: 縦書きの列の分割と禁則の手順を加える`

---

### Task 4: 縦書きの組版(単位の整形、縦中横、回転、`min_size`、`fits`)

**Files:**
- Create: `crates/s2v-render/src/layout.rs`
- Modify: `crates/s2v-render/src/lib.rs`

**Interfaces:**
- Consumes: Task 2 の `FontFace`、Task 3 の `UnitClass`・`break_columns`・`is_no_start`・`is_no_end`
- Produces:

```rust
pub struct LayoutContext<'a> {
    pub face: &'a FontFace,
    pub font_size: f32,
    pub line_spacing: f32,
    pub padding: u32,
    /// 1列の高さの上限(px。自動配置と min_size に使う)
    pub max_inner_h: u32,
}
/// 1字形の配置(px。単位の枠の左上を原点に、x 右・y 下)
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedGlyph {
    pub glyph_id: u16,
    /// 字形の原点(フォントの横書きの原点。ベースライン上)の位置
    pub x: f32,
    pub y: f32,
    pub transform: GlyphTransform,
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GlyphTransform {
    /// そのまま(縦書きの字形)
    Upright,
    /// 縦中横: 横に scale_x 倍して置く
    Tcy { scale_x: f32 },
    /// 時計回りに 90° 回転
    Rotated,
}
#[derive(Debug, Clone, PartialEq)]
pub struct Unit {
    pub text: String,
    pub height: f32,
    pub glyphs: Vec<PlacedGlyph>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct Column {
    pub units: std::ops::Range<usize>,
    pub height: f32,
}
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub units: Vec<Unit>,
    pub columns: Vec<Column>,
    /// 列の幅の合計(列の幅 = font_size、列の間 = font_size × line_spacing)
    pub width: f32,
    /// 最も高い列の高さ
    pub height: f32,
}
pub fn shape_units(ctx: &LayoutContext, text: &str) -> Vec<Unit>;
pub fn layout(ctx: &LayoutContext, text: &str, inner_h: f32) -> Layout;
/// max_inner_h のもとでの、余白を含む最小の大きさ(切り上げの整数 px)
pub fn min_size(ctx: &LayoutContext, text: &str) -> (u32, u32);
/// 大きさ w×h の枠の内側で組版して収まるか(設計書 §6.2)
pub fn fits(ctx: &LayoutContext, text: &str, w: u32, h: u32) -> bool;
/// 列 i の中心の x(内側の幅 inner_w の中で、列の束を中央に置き、右から左へ)
pub fn column_center_x(layout: &Layout, ctx: &LayoutContext, i: usize, inner_w: f32) -> f32;
```

規則(設計書 §6.1):
- 文字列を **連なり** に分ける: (a) ASCII の数字だけが 1〜2 文字続く連なり → 縦中横、(b) それ以外の ASCII の図形文字・空白(U+0020)の連なり(3 文字以上の数字を含む)→ 回転、(c) それ以外 → 縦書き。
- 縦書きの連なり: `rustybuzz::UnicodeBuffer` に入れ、`set_direction(Direction::TopToBottom)`、`face.vertical_features()` を渡して `rustybuzz::shape`。字形の `cluster`(バイトの位置)を書記素(`unicode_segmentation::UnicodeSegmentation::grapheme_indices(true)`)に割り当て、字形を持たない書記素は直前の単位にまとめる。単位の高さは、属する字形の `y_advance` の絶対値の合計 × `s`(`s = font_size / units_per_em`)。字形の原点は、単位の上端からの縦のペン位置 `pen` に対して `x = font_size/2 + x_offset × s`、`y = -(pen_y + y_offset) × s`(`pen_y` は 0 から `y_advance` を足していく。rustybuzz の縦書きの位置は、縦の原点の補正を引いた値なので、横書きの字形の輪郭をそのまま原点に置けば正しく並ぶ)。
- 縦中横: 横書き(LeftToRight)で整形し、幅 `w_px` を求める。`scale_x = min(1, font_size / w_px)`。1文字分の枠(高さ font_size)の中央に置く: `x = (font_size − w_px × scale_x) / 2`、`y = (font_size + (ascender + descender) × s) / 2`。単位の高さは font_size。
- 回転: 横書きで整形し、書記素ごとに単位にする。単位の高さは、その単位の字形の `x_advance` の合計 × s。単位の中で、横書きのペン位置 `px` の字形は、時計回りに 90° 回して `x = font_size/2 − (ascender + descender) × s / 2`、`y = (px + x_offset × s)` に置く(描画側で回転を掛ける)。
- 単位の分類: `no_start = is_no_start(単位の最初の文字)`、`no_end = is_no_end(単位の最後の文字)`。
- `layout`: `break_columns` で列に分ける。`width = n × font_size + (n − 1) × font_size × line_spacing`(n は列数。0 なら 0)。`height` は最も高い列。
- `min_size`: `layout(ctx, text, max_inner_h)` の `(ceil(width) + 2×padding, ceil(height) + 2×padding)`。空文字なら `(2×padding + font_size の切り上げ, 同)`。
- `fits`: 内側 `(w − 2p, h − 2p)` が正で、`layout(ctx, text, inner_h)` の `width ≤ inner_w + 1e-3`、かつ各列の高さ ≤ `inner_h + 1e-3`(単位1つで超える列があれば偽)。
- 同じ入力(フォントのバイト列・設定・文字列)なら同じ結果になる(浮動小数の計算の順を固定する。HashMap を使わない)。

- [ ] **Step 1: 失敗するテストを書く**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::FontFace;

    fn face() -> FontFace {
        FontFace::load(&std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fonts/test-vrt2.ttf"), 0).unwrap()
    }
    fn ctx(face: &FontFace) -> LayoutContext<'_> {
        LayoutContext { face, font_size: 40.0, line_spacing: 0.5, padding: 10, max_inner_h: 200 }
    }

    #[test]
    fn kanji_units_are_one_em_high() {
        let f = face();
        let units = shape_units(&ctx(&f), "港朝");
        assert_eq!(units.len(), 2);
        for u in &units {
            assert!((u.height - 40.0).abs() < 1.0, "{}", u.height);
            assert_eq!(u.glyphs.len(), 1);
            assert_eq!(u.glyphs[0].transform, GlyphTransform::Upright);
        }
    }

    #[test]
    fn vertical_forms_are_substituted() {
        // 縦書きの「、」「ー」「（」は横書きと別の字形(vrt2/vert)になる
        let f = face();
        let c = ctx(&f);
        let rb = rustybuzz::Face::from_slice(f.data(), f.index()).unwrap();
        for ch in ['、', 'ー', '（', '「'] {
            let horizontal = rb.glyph_index(ch).unwrap().0;
            let u = &shape_units(&c, &ch.to_string())[0];
            assert_ne!(u.glyphs[0].glyph_id, horizontal, "{ch}");
        }
    }

    #[test]
    fn two_digits_are_tate_chu_yoko_three_are_rotated() {
        let f = face();
        let units = shape_units(&ctx(&f), "12日345");
        assert_eq!(units[0].text, "12");
        assert!(matches!(units[0].glyphs[0].transform, GlyphTransform::Tcy { .. }));
        assert_eq!(units[0].glyphs.len(), 2);
        assert!((units[0].height - 40.0).abs() < 1e-3);
        assert_eq!(units[1].text, "日");
        let rotated: Vec<&Unit> = units[2..].iter().collect();
        assert_eq!(rotated.len(), 3);
        assert!(rotated.iter().all(|u| u.glyphs[0].transform == GlyphTransform::Rotated));
    }

    #[test]
    fn latin_word_is_rotated_per_grapheme() {
        let f = face();
        let units = shape_units(&ctx(&f), "AB港");
        assert_eq!(units.iter().map(|u| u.text.as_str()).collect::<Vec<_>>(), vec!["A", "B", "港"]);
        assert!(units[0].height < 40.0);
    }

    #[test]
    fn combining_marks_stay_with_their_base() {
        let f = face();
        // が = か + 結合濁点(U+3099)は1単位
        let units = shape_units(&ctx(&f), "か\u{3099}き");
        assert_eq!(units.len(), 2);
        assert_eq!(units[0].text, "か\u{3099}");
    }

    #[test]
    fn layout_breaks_into_columns_with_kinsoku() {
        let f = face();
        let c = ctx(&f);
        // 内側の高さ 120 = 3文字。「あいう、え」→ あい|う、え(、を列頭に置かない)
        let l = layout(&c, "あいう、え", 120.0);
        assert_eq!(l.columns.iter().map(|c| c.units.clone()).collect::<Vec<_>>(), vec![0..2, 2..5]);
        assert!((l.width - (2.0 * 40.0 + 1.0 * 20.0)).abs() < 1e-3);
    }

    #[test]
    fn min_size_and_fits_agree() {
        let f = face();
        let c = ctx(&f);
        let text = "一二三四五六七八九十港朝";
        let (w, h) = min_size(&c, text);
        assert!(fits(&c, text, w, h));
        assert!(fits(&c, text, w + 50, h + 50));
        assert!(!fits(&c, text, w, h - 45)); // 背を低くすると列が増えて幅が足りない
        // 幅を広げて背を低くする調整はできる
        let wide = w + 60 + 20;
        assert!(fits(&c, text, wide, h - 40));
        assert!(!fits(&c, text, 15, 15)); // 内側が正でない
    }

    #[test]
    fn same_input_gives_identical_layout() {
        let f = face();
        let c = ctx(&f);
        assert_eq!(layout(&c, "港の朝、12日。AB", 160.0), layout(&c, "港の朝、12日。AB", 160.0));
    }

    #[test]
    fn empty_text_has_minimal_size() {
        let f = face();
        assert_eq!(min_size(&ctx(&f), ""), (60, 60));
    }
}
```

(`min_size_and_fits_agree` の `h - 45`・`h - 40` などの数値は、テスト用フォントの実際の送り幅で変わりうる。12文字・max_inner_h 200(=5文字)で 3 列、背を 40 減らすと 4 文字/列 → 3 列のまま(12/4)、45 減らすと 3 文字/列 → 4 列になる、という意図。実際の値で意図が崩れるなら、意図を保つように数値を直し、報告する。)

- [ ] **Step 2: 失敗を確かめる** — Run: `cargo test -p s2v-render layout`

- [ ] **Step 3: 実装する** — 上の規則どおりに書く。`rustybuzz::shape(&face, &features, buffer)` の結果は `glyph_infos()`(`glyph_id`・`cluster`)と `glyph_positions()`(`x_advance`・`y_advance`・`x_offset`・`y_offset`)。回転の連なりと縦中横の連なりの整形は `Direction::LeftToRight`・機能なし。連なりの分割は関数 `split_runs(text) -> Vec<(RunKind, Range<usize>)>` に分け、単体テストを1つ加える(`"12日345AB 港"` → `[(Tcy, 0..2), (Upright, 2..5), (Rotated, 5..13), (Upright, 13..16)]` のバイト範囲)。

- [ ] **Step 4: テストが通ることを確かめる** — Run: `cargo test -p s2v-render layout`

- [ ] **Step 5: 全体の検証とコミット** — `feat: 縦書きの組版(縦中横・回転・禁則・最小の大きさ・収まりの判定)を加える`

---

### Task 5: 吹き出し・ト書きの描画と合成

**Files:**
- Create: `crates/s2v-render/src/draw.rs`
- Modify: `crates/s2v-render/src/lib.rs`

**Interfaces:**
- Consumes: Task 4 の `LayoutContext`・`layout`・`column_center_x`・`GlyphTransform`、Task 1 の `RenderConfig::rgb`
- Produces:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ItemKind { Bubble, Note }

#[derive(Debug, Clone)]
pub struct Style {
    pub fill: [u8; 3],
    pub stroke: [u8; 3],
    pub stroke_width: f32,
    pub text: [u8; 3],
}
impl Style {
    pub fn bubble(cfg: &s2v_core::config::RenderConfig) -> Style;
    pub fn note(cfg: &s2v_core::config::RenderConfig) -> Style;
}
pub struct ItemSpec<'a> {
    pub kind: ItemKind,
    pub text: &'a str,
    pub w: u32,
    pub h: u32,
    /// 枠の中心から尻尾の先端への相対位置(吹き出しだけ)
    pub tail: Option<(i32, i32)>,
}
pub struct Overlay {
    /// 画像の左上の、枠の左上からの相対位置(尻尾があれば負になりうる)
    pub dx: i32,
    pub dy: i32,
    pub pixmap: tiny_skia::Pixmap,
}
pub fn render(ctx: &LayoutContext, style: &Style, spec: &ItemSpec) -> Result<Overlay, String>;
/// 画面の大きさの透明な画像に、(絶対位置 x, y, 画像) を順に重ねる
pub fn composite(canvas_w: u32, canvas_h: u32, layers: &[(i32, i32, &tiny_skia::Pixmap)]) -> tiny_skia::Pixmap;
```

規則(設計書 §6.2〜6.3):
- 画像は、枠 `(0,0)-(w,h)` と尻尾の先端と線の太さを含む外接矩形(整数に切り上げ)。外接矩形の左上が `(dx, dy)`。
- 吹き出しの本体: 角丸の矩形、角の半径 `r = min(w, h) × 0.4`。尻尾: 枠の中心 `c` から先端 `t` への向きに垂直な方向に、中心から ±`min(w,h) × 0.12` の2点と先端の三角形。描き方: (1) 本体と尻尾の経路を、線の色・太さ `2 × stroke_width` で縁取る、(2) 本体と尻尾を塗りの色で塗る。これで外側にだけ `stroke_width` の縁が残り、本体と尻尾の継ぎ目の線は消える。
- ト書き: 矩形。線は内側に収める(矩形を `stroke_width/2` だけ内側にずらして縁取る)。塗りを先に、縁取りを後に。
- 文字: 内側の枠 `(padding, padding)-(w − padding, h − padding)` で `layout(ctx, text, inner_h)` を組み、列の束を内側の枠の中央(横)・上下中央(最も高い列の高さで)に置く。字形の輪郭は `ttf_parser::Face::outline_glyph` を `tiny_skia::PathBuilder` に写す(y は上向きなので反転する)。`Upright`・`Tcy` はそのまま(Tcy は横に scale_x)、`Rotated` は字形の原点を中心に時計回り 90° 回す。塗りは text の色、アンチエイリアスあり。
- `fits(ctx, text, w, h)` が偽なら `Err("文字が枠に収まりません …")`。
- `composite`: `tiny_skia::Pixmap::new(canvas_w, canvas_h)`(全面透明)に `draw_pixmap` で順に重ねる(SourceOver)。PNG の書き出しは呼び出し側が `pixmap.encode_png()`(straight alpha になる)。

- [ ] **Step 1: 失敗するテストを書く**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::FontFace;
    use crate::layout::LayoutContext;

    fn face() -> FontFace {
        FontFace::load(&std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fonts/test-vrt2.ttf"), 0).unwrap()
    }
    fn style() -> Style {
        Style { fill: [255, 255, 255], stroke: [0, 0, 0], stroke_width: 4.0, text: [0, 0, 0] }
    }
    fn px(p: &tiny_skia::Pixmap, x: u32, y: u32) -> [u8; 4] {
        let c = p.pixel(x, y).unwrap().demultiply();
        [c.red(), c.green(), c.blue(), c.alpha()]
    }

    #[test]
    fn note_box_has_fill_inside_border_and_transparent_outside() {
        let f = face();
        let c = LayoutContext { face: &f, font_size: 40.0, line_spacing: 0.4, padding: 20, max_inner_h: 400 };
        let o = render(&c, &style(), &ItemSpec { kind: ItemKind::Note, text: "翌朝", w: 100, h: 140, tail: None }).unwrap();
        assert_eq!((o.dx, o.dy), (0, 0));
        assert_eq!((o.pixmap.width(), o.pixmap.height()), (100, 140));
        assert_eq!(px(&o.pixmap, 1, 70), [0, 0, 0, 255]); // 縁
        assert_eq!(px(&o.pixmap, 10, 5 + 4), [255, 255, 255, 255]); // 塗り(文字の無い場所)
    }

    #[test]
    fn bubble_with_tail_extends_the_image_downwards() {
        let f = face();
        let c = LayoutContext { face: &f, font_size: 40.0, line_spacing: 0.4, padding: 28, max_inner_h: 400 };
        let o = render(&c, &style(), &ItemSpec { kind: ItemKind::Bubble, text: "港", w: 120, h: 160, tail: Some((0, 160)) }).unwrap();
        assert!(o.dy <= 0 && o.dx <= 0);
        // 先端(中心 (60,80) + (0,160) = (60,240))の近くに不透明な画素がある
        let tip_y = (240 - o.dy) as u32 - 3;
        assert!(px(&o.pixmap, (60 - o.dx) as u32, tip_y)[3] > 0);
        // 角(0,0)の付近は角丸なので透明
        assert_eq!(px(&o.pixmap, (0 - o.dx) as u32 + 1, (0 - o.dy) as u32 + 1)[3], 0);
        // 本体と尻尾の継ぎ目(本体の下端の中央の少し内側)は塗りの色
        assert_eq!(px(&o.pixmap, (60 - o.dx) as u32, (158 - o.dy) as u32), [255, 255, 255, 255]);
    }

    #[test]
    fn text_is_drawn_in_text_color() {
        let f = face();
        let c = LayoutContext { face: &f, font_size: 40.0, line_spacing: 0.4, padding: 20, max_inner_h: 400 };
        let o = render(&c, &style(), &ItemSpec { kind: ItemKind::Note, text: "一", w: 80, h: 80, tail: None }).unwrap();
        // 「一」の横線は枠の中央の高さを通る
        let row: Vec<[u8; 4]> = (20..60).map(|x| px(&o.pixmap, x, 40)).collect();
        assert!(row.iter().any(|p| p[0] < 50 && p[3] == 255), "{row:?}");
    }

    #[test]
    fn text_that_does_not_fit_is_an_error() {
        let f = face();
        let c = LayoutContext { face: &f, font_size: 40.0, line_spacing: 0.4, padding: 20, max_inner_h: 400 };
        assert!(render(&c, &style(), &ItemSpec { kind: ItemKind::Note, text: "一二三四五", w: 60, h: 60, tail: None }).is_err());
    }

    #[test]
    fn rendering_is_deterministic() {
        let f = face();
        let c = LayoutContext { face: &f, font_size: 40.0, line_spacing: 0.4, padding: 28, max_inner_h: 400 };
        let spec = ItemSpec { kind: ItemKind::Bubble, text: "港の朝、12日。", w: 200, h: 300, tail: Some((-30, 200)) };
        let a = render(&c, &style(), &spec).unwrap();
        let b = render(&c, &style(), &spec).unwrap();
        assert_eq!(a.pixmap.data(), b.pixmap.data());
    }

    #[test]
    fn composite_layers_in_order() {
        let mut red = tiny_skia::Pixmap::new(10, 10).unwrap();
        red.fill(tiny_skia::Color::from_rgba8(255, 0, 0, 255));
        let mut blue = tiny_skia::Pixmap::new(10, 10).unwrap();
        blue.fill(tiny_skia::Color::from_rgba8(0, 0, 255, 255));
        let out = composite(30, 20, &[(0, 0, &red), (5, 5, &blue)]);
        assert_eq!(px(&out, 2, 2), [255, 0, 0, 255]);
        assert_eq!(px(&out, 7, 7), [0, 0, 255, 255]); // 後の層が上
        assert_eq!(px(&out, 25, 15)[3], 0); // 何もない所は透明
        // PNG は straight alpha で書ける(半透明の色が戻る)
        let mut half = tiny_skia::Pixmap::new(1, 1).unwrap();
        half.fill(tiny_skia::Color::from_rgba8(200, 100, 50, 128));
        let png = composite(1, 1, &[(0, 0, &half)]).encode_png().unwrap();
        assert!(!png.is_empty());
    }
}
```

- [ ] **Step 2〜4**: 失敗を確かめ、実装し、通す(`cargo test -p s2v-render draw`)。

- [ ] **Step 5: 全体の検証とコミット** — `feat: 吹き出し・ト書きを縦書きで描き、画面の大きさの画像に合成する`

---

### Task 6: `bubbles.json` の型と検証

**Files:**
- Create: `crates/s2v-render/src/bubbles.rs`
- Modify: `crates/s2v-render/src/lib.rs`

**Interfaces:**
- Consumes: Task 5 の `ItemKind`
- Produces:

```rust
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BubblesFile { pub version: u32, pub canvas: Canvas, pub items: std::collections::BTreeMap<String, Placed> }
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Canvas { pub w: u32, pub h: u32 }
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Placed {
    pub kind: ItemKind,
    pub x: i32, pub y: i32, pub w: u32, pub h: u32,
    #[serde(default)] pub tail: Option<Tail>,
    #[serde(default)] pub start_offset_ms: i64,
    #[serde(default)] pub end_offset_ms: i64,
}
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tail { pub dx: i32, pub dy: i32 }

pub const MAX_ITEMS: usize = 10_000;
pub const MAX_OFFSET_MS: i64 = 60_000;
/// 読み込みと検証(§5.2)。Err は理由の一覧(エラー 2)
pub fn parse_bubbles(json: &str) -> Result<BubblesFile, Vec<String>>;
/// 画面の大きさ (w, h) に合わせる。縦横比が違えば Err(エラー 2)
pub fn scale_to(file: &BubblesFile, w: u32, h: u32) -> Result<BubblesFile, String>;
/// 表示時刻(元の時刻 + offset を [0, duration] に収める)。開始 ≥ 終了なら Err
pub fn effective_times(start_ms: i64, end_ms: i64, p: Option<&Placed>, duration_ms: i64) -> Result<(i64, i64), String>;
/// 本文が変わって収まらない手動の大きさを広げる: 幅と高さを max(保存値, min) にし、画面の内側に寄せる
pub fn expand_to_min(p: &Placed, min: (u32, u32), canvas: Canvas) -> Placed;
```

検証の項目(すべて集めて返す): JSON の誤り・未知のキー(serde のエラーをそのまま1件)、`version != 1`、canvas が §4.7 の範囲外(w: 16〜7680 の偶数、h: 16〜4320 の偶数)、items が 10000 件超、ID の形式(`^S\d+-L\d+$` は bubble、`^S\d+-N\d+$` は note。kind と違えば誤り)、note に `tail`、`w`・`h` が 0、枠が画面の外(`x < 0`、`y < 0`、`x + w > canvas.w`、`y + h > canvas.h`。i64 で計算)、尻尾の先端(枠の中心 + (dx, dy))が画面の外、`|offset| > 60000`。

- [ ] **Step 1: 失敗するテストを書く**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const OK: &str = r#"{"version":1,"canvas":{"w":1920,"h":1080},"items":{
        "S01-L003":{"kind":"bubble","x":1200,"y":80,"w":220,"h":420,"tail":{"dx":0,"dy":290},"start_offset_ms":0,"end_offset_ms":0},
        "S01-N01":{"kind":"note","x":1700,"y":40,"w":160,"h":380}}}"#;

    #[test]
    fn valid_file_parses_with_defaults() {
        let f = parse_bubbles(OK).unwrap();
        assert_eq!(f.items.len(), 2);
        assert_eq!(f.items["S01-N01"].start_offset_ms, 0);
        assert_eq!(f.items["S01-N01"].tail, None);
    }

    #[test]
    fn unknown_key_and_broken_json_are_errors() {
        // Review Focus 4
        let e = parse_bubbles(&OK.replace("\"w\":220", "\"w\":220,\"color\":\"red\"")).unwrap_err();
        assert!(e[0].contains("color"), "{e:?}");
        assert!(!parse_bubbles("{").unwrap_err().is_empty());
        assert!(!parse_bubbles(&OK.replace("\"version\":1", "\"version\":2")).unwrap_err().is_empty());
    }

    #[test]
    fn each_limit_is_checked() {
        let cases = [
            OK.replace("\"x\":1200", "\"x\":1800"),                  // 枠が右にはみ出す
            OK.replace("\"y\":80", "\"y\":-1"),
            OK.replace("\"dy\":290", "\"dy\":900"),                  // 尻尾の先端が下にはみ出す
            OK.replace("\"start_offset_ms\":0", "\"start_offset_ms\":60001"),
            OK.replace("\"kind\":\"note\"", "\"kind\":\"bubble\""), // N の ID に bubble
            OK.replace("\"S01-N01\":{\"kind\":\"note\",", "\"S01-N01\":{\"kind\":\"note\",\"tail\":{\"dx\":0,\"dy\":1},"),
            OK.replace("\"w\":220", "\"w\":0"),
            OK.replace("\"w\":1920", "\"w\":1921"),
            OK.replace("\"S01-L003\"", "\"X01\""),
        ];
        for c in cases {
            assert!(parse_bubbles(&c).is_err(), "{c}");
        }
    }

    #[test]
    fn too_many_items_is_an_error() {
        let items: Vec<String> = (1..=10_001).map(|i| format!("\"S01-L{i:05}\":{{\"kind\":\"bubble\",\"x\":0,\"y\":0,\"w\":10,\"h\":10}}")).collect();
        let json = format!("{{\"version\":1,\"canvas\":{{\"w\":1920,\"h\":1080}},\"items\":{{{}}}}}", items.join(","));
        let e = parse_bubbles(&json).unwrap_err();
        assert!(e.iter().any(|m| m.contains("10000")), "{e:?}");
    }

    #[test]
    fn scaling_keeps_aspect_and_rejects_other_ratios() {
        let f = parse_bubbles(OK).unwrap();
        let s = scale_to(&f, 1280, 720).unwrap();
        let b = &s.items["S01-L003"];
        assert_eq!((b.x, b.y, b.w, b.h), (800, 53, 147, 280)); // 2/3 倍で四捨五入
        assert_eq!(b.tail, Some(Tail { dx: 0, dy: 193 }));
        assert_eq!(s.canvas, Canvas { w: 1280, h: 720 });
        assert!(scale_to(&f, 1080, 1080).is_err());
    }

    #[test]
    fn effective_times_clamp_and_reject_inversion() {
        let p = parse_bubbles(OK).unwrap().items["S01-L003"].clone();
        assert_eq!(effective_times(1000, 2000, Some(&Placed { start_offset_ms: -2000, end_offset_ms: 500, ..p.clone() }), 2300), Ok((0, 2300)));
        assert!(effective_times(1000, 2000, Some(&Placed { start_offset_ms: 1500, ..p.clone() }), 5000).is_err());
        assert_eq!(effective_times(1000, 2000, None, 5000), Ok((1000, 2000)));
    }

    #[test]
    fn expand_to_min_grows_and_moves_inside() {
        let p = Placed { kind: ItemKind::Bubble, x: 1800, y: 900, w: 100, h: 100, tail: None, start_offset_ms: 0, end_offset_ms: 0 };
        let q = expand_to_min(&p, (200, 300), Canvas { w: 1920, h: 1080 });
        assert_eq!((q.w, q.h), (200, 300));
        assert_eq!((q.x, q.y), (1720, 780)); // 右下へ広げ、はみ出す分を内側へ寄せる
        let r = expand_to_min(&p, (50, 50), Canvas { w: 1920, h: 1080 });
        assert_eq!((r.w, r.h, r.x, r.y), (100, 100, 1800, 900));
    }
}
```

- [ ] **Step 2〜4**: 失敗を確かめ、実装し、通す(`cargo test -p s2v-render bubbles`)。拡大縮小の丸めは `((v as i64 × new + old/2) / old)` の四捨五入(負の値は符号を分けて同様)。

- [ ] **Step 5: 全体の検証とコミット** — `feat: bubbles.json の型・検証・拡大縮小・表示時刻・最小への拡大を加える`

---

### Task 7: 自動配置

**Files:**
- Create: `crates/s2v-render/src/autolayout.rs`
- Modify: `crates/s2v-render/src/lib.rs`

**Interfaces:**
- Consumes: Task 5 の `ItemKind`、Task 6 の `Placed`・`Tail`・`Canvas`
- Produces:

```rust
#[derive(Debug, Clone)]
pub struct LayoutItem {
    pub id: String,
    pub kind: ItemKind,
    pub start_ms: i64,
    pub end_ms: i64,
    /// 台本の並び
    pub order: usize,
    /// min_size の結果
    pub size: (u32, u32),
}
pub struct AutoParams { pub canvas: Canvas, pub margin: u32, pub bubble_gap: u32, pub tail_length: u32 }
/// manual に無い項目の配置を決める(設計書 §4.4)。戻り値: (id → 配置, 警告)
pub fn auto_layout(items: &[LayoutItem], manual: &std::collections::BTreeMap<String, Placed>, p: &AutoParams)
    -> (std::collections::BTreeMap<String, Placed>, Vec<String>);
```

規則(設計書 §4.4):
- 大きさは `size`。画面より大きければ画面の大きさから余白を引いた大きさに縮め、警告「<ID> の文字が多く、画面に収まりません」(Review Focus 1)。
- ト書き: `x = canvas.w − margin − w`、`y = margin`、尻尾なし。
- 吹き出し: `(start_ms, order)` の順に置く。`y = margin`。x の候補は `canvas.w − margin − w` から `margin` まで `bubble_gap` ずつ減らした値(最後に `margin` も候補にする)。時間が重なる(`a.start < b.end && b.start < a.end`)配置済みの項目(ト書き・先に置いた吹き出し・手動の項目)の枠と、`bubble_gap` だけ広げた枠どうしが重ならない最初の x を採る。無ければ最初の候補に置いて警告「<ID> を重ならずに置けませんでした」。
- 尻尾: `(0, h/2 + tail_length)`。先端が画面の下端を超えるなら `dy = canvas.h − 1 − (y + h/2)`(0 未満なら尻尾なし)。
- 手動の項目は、ト書き・吹き出しの配置のときに障害物として扱う(手動の項目自体は戻り値に含めない)。

- [ ] **Step 1: 失敗するテストを書く**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn item(id: &str, kind: ItemKind, s: i64, e: i64, order: usize) -> LayoutItem {
        LayoutItem { id: id.into(), kind, start_ms: s, end_ms: e, order, size: (200, 300) }
    }
    fn params() -> AutoParams {
        AutoParams { canvas: Canvas { w: 1920, h: 1080 }, margin: 40, bubble_gap: 24, tail_length: 80 }
    }

    #[test]
    fn note_goes_top_right_and_bubbles_go_left_of_it() {
        let items = vec![
            item("S01-N01", ItemKind::Note, 0, 5000, 0),
            item("S01-L001", ItemKind::Bubble, 0, 2000, 1),
            item("S01-L002", ItemKind::Bubble, 1000, 3000, 2),
        ];
        let (out, w) = auto_layout(&items, &BTreeMap::new(), &params());
        assert!(w.is_empty(), "{w:?}");
        let n = &out["S01-N01"];
        assert_eq!((n.x, n.y, n.tail), (1920 - 40 - 200, 40, None));
        let b1 = &out["S01-L001"];
        let b2 = &out["S01-L002"];
        assert!(b1.x + 200 + 24 <= n.x, "{b1:?} {n:?}");
        assert!(b2.x + 200 + 24 <= b1.x, "{b2:?} {b1:?}");
        assert_eq!(b1.tail, Some(Tail { dx: 0, dy: 150 + 80 }));
    }

    #[test]
    fn non_overlapping_times_reuse_the_same_slot() {
        let items = vec![item("S01-L001", ItemKind::Bubble, 0, 1000, 0), item("S01-L002", ItemKind::Bubble, 1000, 2000, 1)];
        let (out, _) = auto_layout(&items, &BTreeMap::new(), &params());
        assert_eq!(out["S01-L001"].x, out["S01-L002"].x);
    }

    #[test]
    fn manual_items_are_obstacles_and_not_returned() {
        let mut manual = BTreeMap::new();
        manual.insert("S01-L001".to_string(), Placed { kind: ItemKind::Bubble, x: 1680, y: 40, w: 200, h: 300, tail: None, start_offset_ms: 0, end_offset_ms: 0 });
        let items = vec![item("S01-L001", ItemKind::Bubble, 0, 2000, 0), item("S01-L002", ItemKind::Bubble, 0, 2000, 1)];
        let (out, _) = auto_layout(&items, &manual, &params());
        assert!(!out.contains_key("S01-L001"));
        assert!(out["S01-L002"].x + 200 + 24 <= 1680);
    }

    #[test]
    fn no_room_falls_back_to_the_right_with_warning() {
        let items: Vec<LayoutItem> = (0..10).map(|i| item(&format!("S01-L{:03}", i + 1), ItemKind::Bubble, 0, 1000, i)).collect();
        let (out, w) = auto_layout(&items, &BTreeMap::new(), &params());
        assert_eq!(out.len(), 10);
        assert!(!w.is_empty());
        assert_eq!(out["S01-L010"].x, 1920 - 40 - 200);
    }

    #[test]
    fn oversized_item_is_clamped_with_warning() {
        // Review Focus 1
        let mut big = item("S01-L001", ItemKind::Bubble, 0, 1000, 0);
        big.size = (3000, 2000);
        let (out, w) = auto_layout(&[big], &BTreeMap::new(), &params());
        let b = &out["S01-L001"];
        assert!(b.x >= 0 && b.y >= 0 && b.x as u32 + b.w <= 1920 && b.y as u32 + b.h <= 1080, "{b:?}");
        assert!(w.iter().any(|m| m.contains("S01-L001")), "{w:?}");
    }

    #[test]
    fn tail_is_shortened_at_the_bottom() {
        let mut tall = item("S01-L001", ItemKind::Bubble, 0, 1000, 0);
        tall.size = (200, 1000);
        let (out, _) = auto_layout(&[tall], &BTreeMap::new(), &params());
        let b = &out["S01-L001"];
        let tip_y = b.y + b.h as i32 / 2 + b.tail.map_or(0, |t| t.dy);
        assert!(tip_y <= 1079);
    }
}
```

- [ ] **Step 2〜4**: 失敗を確かめ、実装し、通す(`cargo test -p s2v-render autolayout`)。

- [ ] **Step 5: 全体の検証とコミット** — `feat: 吹き出しとト書きの自動配置を加える`

---

### Task 8: 素材の照合

**Files:**
- Create: `crates/s2v-render/src/assets.rs`
- Modify: `crates/s2v-render/src/lib.rs`

**Interfaces:**
- Produces:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetKind { Png, Jpeg, Video }
#[derive(Debug, Clone, PartialEq)]
pub struct Asset { pub cut_id: String, pub path: std::path::PathBuf, pub kind: AssetKind }
#[derive(Debug, Default)]
pub struct AssetMatch {
    pub found: std::collections::BTreeMap<String, Asset>,
    pub missing: Vec<String>,
    /// エラー 2 の理由(重複・種類違い・リンク・外側)
    pub errors: Vec<String>,
}
/// S01-C01 → "S01"、S01-C02 → "S01_C02"(桁は ID のまま)
pub fn asset_stem(cut_id: &str) -> String;
/// cuts: (カット ID, #visual で明示された種類)。tracks_dir が無ければ全部 missing
pub fn match_assets(tracks_dir: &std::path::Path, cuts: &[(String, Option<s2v_timeline::VisualKind>)]) -> AssetMatch;
```

規則(設計書 §4.3): `tracks_dir` を1回読み、各カットについて、語幹(拡張子を除いた名前)が `asset_stem` と大文字小文字を区別せずに等しく、拡張子が許可されたもの(画像 `png`・`jpg`・`jpeg`、動画 `mp4`・`mov`・`webm`)を集める。0 件 → missing。2 件以上 → エラー(ファイル名を全部並べる)。`symlink_metadata` がリンク → エラー。正規化した実体が `tracks_dir` の正規化した実体の中に無い → エラー。明示された種類(image/video)と違う → エラー「<カット ID>: #visual は video ですが、素材 <名前> は画像です」。`dialogue.wav` などの関係ないファイルは無視する。

- [ ] **Step 1: 失敗するテストを書く**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use s2v_timeline::VisualKind;

    fn dir(files: &[&str]) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        for f in files {
            std::fs::write(d.path().join(f), b"x").unwrap();
        }
        d
    }
    fn cuts(ids: &[(&str, Option<VisualKind>)]) -> Vec<(String, Option<VisualKind>)> {
        ids.iter().map(|(i, k)| (i.to_string(), *k)).collect()
    }

    #[test]
    fn stems() {
        assert_eq!(asset_stem("S01-C01"), "S01");
        assert_eq!(asset_stem("S01-C02"), "S01_C02");
        assert_eq!(asset_stem("S12-C0003"), "S12_C0003");
    }

    #[test]
    fn finds_images_and_videos_and_reports_missing() {
        let d = dir(&["S01.png", "S01_C02.mp4", "dialogue.wav", "S02.webp"]);
        let m = match_assets(d.path(), &cuts(&[("S01-C01", None), ("S01-C02", Some(VisualKind::Video)), ("S02-C01", None)]));
        assert_eq!(m.found["S01-C01"].kind, AssetKind::Png);
        assert_eq!(m.found["S01-C02"].kind, AssetKind::Video);
        assert_eq!(m.missing, vec!["S02-C01".to_string()]); // WebP は受け付けない
        assert!(m.errors.is_empty(), "{:?}", m.errors);
    }

    #[test]
    fn stem_and_extension_match_case_insensitively() {
        // Review Focus 2
        let d = dir(&["s01.PNG", "S01_c02.JPEG"]);
        let m = match_assets(d.path(), &cuts(&[("S01-C01", None), ("S01-C02", None)]));
        assert_eq!(m.found["S01-C01"].kind, AssetKind::Png);
        assert_eq!(m.found["S01-C02"].kind, AssetKind::Jpeg);
    }

    #[test]
    fn two_assets_for_one_cut_is_an_error_naming_both() {
        // Review Focus 3
        let d = dir(&["S01.png", "S01.jpg"]);
        let m = match_assets(d.path(), &cuts(&[("S01-C01", None)]));
        assert_eq!(m.errors.len(), 1);
        assert!(m.errors[0].contains("S01.png") && m.errors[0].contains("S01.jpg"), "{:?}", m.errors);
        assert!(!m.found.contains_key("S01-C01"));
    }

    #[test]
    fn explicit_kind_mismatch_is_an_error() {
        let d = dir(&["S01.png"]);
        let m = match_assets(d.path(), &cuts(&[("S01-C01", Some(VisualKind::Video))]));
        assert_eq!(m.errors.len(), 1, "{:?}", m.errors);
    }

    #[test]
    fn missing_tracks_dir_means_all_missing() {
        let d = tempfile::tempdir().unwrap();
        let m = match_assets(&d.path().join("無い"), &cuts(&[("S01-C01", None)]));
        assert_eq!(m.missing, vec!["S01-C01".to_string()]);
    }

    #[test]
    fn symlinked_asset_is_rejected_when_links_can_be_made() {
        let d = dir(&[]);
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("x.png");
        std::fs::write(&target, b"x").unwrap();
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_file(&target, d.path().join("S01.png")).is_ok();
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(&target, d.path().join("S01.png")).is_ok();
        if !made {
            return; // リンクを作る権限が無い環境
        }
        let m = match_assets(d.path(), &cuts(&[("S01-C01", None)]));
        assert_eq!(m.errors.len(), 1, "{:?}", m.errors);
    }
}
```

- [ ] **Step 2〜4**: 失敗を確かめ、実装し、通す(`cargo test -p s2v-render assets`)。

- [ ] **Step 5: 全体の検証とコミット** — `feat: tracks/ の素材をカット ID で照合する`

---

### Task 9: `s2v-video` クレート、コマ・区間・重ね画像の一覧

**Files:**
- Modify: `Cargo.toml`(members に `"crates/s2v-video"`)
- Create: `crates/s2v-video/Cargo.toml`、`src/lib.rs`、`src/frames.rs`

**Interfaces:**
- Produces:

```rust
pub fn frame(t_ms: i64, fps: u32) -> i64;          // floor((t*fps + 500) / 1000)(i128)
pub fn total_frames(duration_ms: i64, fps: u32) -> i64; // ceil(duration_ms*fps/1000)
#[derive(Debug, Clone, PartialEq)]
pub struct ShownItem { pub id: String, pub is_note: bool, pub start_ms: i64, pub end_ms: i64, pub order: usize }
#[derive(Debug, Clone, PartialEq)]
pub struct Interval {
    /// カットの先頭からの相対コマ番号 r_k
    pub rel_start: i64,
    /// 表示中の項目の ID(重ねる順: ト書き → 吹き出し(表示開始の順、同時なら order))
    pub items: Vec<String>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct CutPlan { pub cut_id: String, pub start_frame: i64, pub frames: i64, pub intervals: Vec<Interval> }
/// cuts: (カット ID, start_ms, end_ms)(台本の並び、隙間なし)。コマ数 0 のカットは含めない
pub fn plan_cuts(cuts: &[(String, i64, i64)], items: &[ShownItem], duration_ms: i64, fps: u32) -> Vec<CutPlan>;
/// 重ね画像の一覧(ffconcat)。intervals: (r_k, PNG のファイル名)、n: カットのコマ数
pub fn overlay_list(intervals: &[(i64, String)], n: i64, fps: u32) -> String;
```

規則(設計書 §4.2、§4.5): カット i のコマは `[frame(start), frame(end))`、最後のカットの終わりは `total_frames`。項目のコマは `[frame(start), frame(end))`。区間の境目は、カットの始めと、カットの内側(始め < f < 終わり)にある項目の始めと終わりのコマ。`overlay_list` は `ffconcat version 1.0` の行に続いて、区間ごとに `file '<名前>'`・`option framerate <2fps>`・`duration <秒>`、最後に最後の名前をもう一度 `file`・`option framerate` で書く(LF で区切り、末尾も LF)。`u` はマイクロ秒の整数で、`u_0 = 0`、`u_k = round_half_up((2 r_k − 1) × 1_000_000 / (2 fps))`、`u_m = round_half_up(n × 1_000_000 / fps)`。`duration = u_{k+1} − u_k` を `"{秒}.{マイクロ秒:06}"` で書く。

- [ ] **Step 1: 失敗するテストを書く**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_rounding() {
        assert_eq!(frame(0, 30), 0);
        assert_eq!(frame(16, 30), 0);   // 0.48 → 0
        assert_eq!(frame(17, 30), 1);   // 0.51 → 1
        assert_eq!(frame(50, 30), 2);   // 1.5 → 2(0.5 は切り上げ)
        assert_eq!(total_frames(1000, 30), 30);
        assert_eq!(total_frames(1001, 30), 31);
        assert_eq!(frame(86_400_000, 120), 10_368_000);
    }

    #[test]
    fn cuts_cover_all_frames_for_random_inputs() {
        // 境目の丸めが積み重ならないこと
        let mut seed = 12345u64;
        for _ in 0..200 {
            let mut cuts = Vec::new();
            let mut t = 0;
            let n = 1 + (seed % 30) as usize;
            for i in 0..n {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                let len = (seed >> 33) % 5000;
                cuts.push((format!("S01-C{:02}", i + 1), t, t + len as i64));
                t += len as i64;
            }
            let dur = t;
            let plans = plan_cuts(&cuts, &[], dur, 30);
            let total: i64 = plans.iter().map(|p| p.frames).sum();
            assert_eq!(total, total_frames(dur, 30));
            for w in plans.windows(2) {
                assert_eq!(w[0].start_frame + w[0].frames, w[1].start_frame);
            }
        }
    }

    #[test]
    fn intervals_split_at_item_boundaries_inside_the_cut() {
        let cuts = vec![("S01-C01".to_string(), 0, 2000), ("S01-C02".to_string(), 2000, 4000)];
        let items = vec![
            ShownItem { id: "S01-N01".into(), is_note: true, start_ms: 0, end_ms: 4000, order: 0 },
            ShownItem { id: "S01-L001".into(), is_note: false, start_ms: 500, end_ms: 2500, order: 1 },
        ];
        let p = plan_cuts(&cuts, &items, 4000, 30);
        assert_eq!(p[0].start_frame, 0);
        assert_eq!(p[0].frames, 60);
        assert_eq!(p[0].intervals, vec![
            Interval { rel_start: 0, items: vec!["S01-N01".into()] },
            Interval { rel_start: 15, items: vec!["S01-N01".into(), "S01-L001".into()] },
        ]);
        assert_eq!(p[1].intervals, vec![
            Interval { rel_start: 0, items: vec!["S01-N01".into(), "S01-L001".into()] },
            Interval { rel_start: 15, items: vec!["S01-N01".into()] },
        ]);
    }

    #[test]
    fn zero_frame_cuts_are_skipped() {
        let cuts = vec![("S01-C01".to_string(), 0, 10), ("S01-C02".to_string(), 10, 1000)];
        let p = plan_cuts(&cuts, &[], 1000, 30);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].cut_id, "S01-C02");
    }

    #[test]
    fn overlay_list_switches_half_a_frame_early_and_repeats_the_last() {
        let s = overlay_list(&[(0, "ov_0001.png".into()), (15, "ov_empty.png".into())], 60, 30);
        assert_eq!(s, "ffconcat version 1.0\n\
file 'ov_0001.png'\noption framerate 60\nduration 0.483333\n\
file 'ov_empty.png'\noption framerate 60\nduration 1.516667\n\
file 'ov_empty.png'\noption framerate 60\n");
        // 1つだけの区間
        let s = overlay_list(&[(0, "ov_empty.png".into())], 30, 30);
        assert!(s.contains("duration 1.000000\n"), "{s}");
    }

    #[test]
    fn overlay_list_durations_sum_to_cut_length() {
        let iv: Vec<(i64, String)> = (0..50).map(|k| (k * 7, format!("ov_{k:04}.png"))).collect();
        let s = overlay_list(&iv, 400, 30);
        let total: i64 = s.lines().filter_map(|l| l.strip_prefix("duration ")).map(|d| {
            let (a, b) = d.split_once('.').unwrap();
            a.parse::<i64>().unwrap() * 1_000_000 + b.parse::<i64>().unwrap()
        }).sum();
        assert_eq!(total, 13_333_333); // round(400/30 秒)
    }
}
```

(`overlay_list` の 1 つ目の期待値: u_1 = (2×15−1)/(2×30) 秒 = 29/60 = 0.483333…、u_2 = 60/30 = 2.000000、差 1.516667。)

- [ ] **Step 2〜4**: 失敗を確かめ、実装し、通す(`cargo test -p s2v-video frames`)。

`crates/s2v-video/Cargo.toml`:

```toml
[package]
name = "s2v-video"
version = "0.1.0"
edition = "2021"

[dependencies]
serde.workspace = true
serde_json = "1"
realfft = "3"
anyhow.workspace = true

[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 5: 全体の検証とコミット** — `feat: s2v-video クレートにコマ・区間・重ね画像の一覧の計算を加える`

---

### Task 10: ffmpeg の版の判定と引数の組み立て

**Files:**
- Create: `crates/s2v-video/src/ffmpeg.rs`
- Modify: `crates/s2v-video/src/lib.rs`

**Interfaces:**
- Produces:

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum VersionCheck { Ok, TooOld(String), Unknown(String) }
/// `ffmpeg -version` の1行目を判定する。"ffmpeg version 6.1.1 …"・"n7.1"・git の日付版 "2026-03-26-git-…"(2023 年以降)を受ける
pub fn check_version(first_line: &str) -> VersionCheck;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fit { Contain, Cover }
#[derive(Debug, Clone)]
pub struct EncodeSettings { pub width: u32, pub height: u32, pub fps: u32, pub crf: u32, pub preset: String, pub threads: u32, pub fit: Fit, pub background: [u8; 3] }
#[derive(Debug, Clone)]
pub enum Background {
    Png { path: String },
    Jpeg { path: String },
    Video { path: String, looped: bool, matrix: String, range: String },
}
/// カットファイルを作る ffmpeg の引数(先頭に "-nostdin","-hide_banner","-n")
pub fn cut_args(bg: &Background, overlay_list: &str, n: i64, out: &str, s: &EncodeSettings) -> Vec<String>;
pub fn cut_filter(bg: &Background, n: i64, s: &EncodeSettings) -> String;
pub fn concat_list(files: &[String]) -> String;
pub fn concat_args(list: &str, out: &str) -> Vec<String>;
pub fn mux_args(video: &str, wav: &str, out: &str, audio_bitrate: &str) -> Vec<String>;
```

フィルタ(設計書 §4.5。`W`・`H`・`fps`・`N` は値、`<bg>` は `0xRRGGBB`):

- 収め方の scale: `contain` は `scale=W:H:force_original_aspect_ratio=decrease<IN>:out_color_matrix=bt709:out_range=tv,pad=W:H:(ow-iw)/2:(oh-ih)/2:color=<bg>`、`cover` は `scale=W:H:force_original_aspect_ratio=increase<IN>:out_color_matrix=bt709:out_range=tv,crop=W:H`。`<IN>` は PNG なら空、JPEG なら `:in_color_matrix=bt601:in_range=pc`、動画なら `:in_color_matrix=<matrix>:in_range=<range>`。
- 全体: `[0:v]setpts=PTS-STARTPTS,fps=<fps>,<収め方>,setsar=1,format=yuv420p<TPAD>,trim=end_frame=<N>,setpts=PTS-STARTPTS[bg];[1:v]setpts=PTS-STARTPTS,format=rgba,scale=out_color_matrix=bt709:out_range=tv,format=yuva420p[o];[bg][o]overlay=x=0:y=0:format=yuv420:alpha=straight:eof_action=repeat:repeatlast=1:shortest=0,format=yuv420p[v]`。`<TPAD>` は動画で `looped` が偽のときだけ `,tpad=stop_mode=clone:stop=-1`。
- 入力: PNG・JPEG は `-loop 1 -framerate <fps> -i <path>`、動画は `[-stream_loop -1] -i <path>`。重ねは `-f concat -safe 0 -i <overlay_list>`。
- 出力: `-filter_complex <filter> -map [v] -an -frames:v <N> -fps_mode cfr -threads <threads> -c:v libx264 -preset <preset> -crf <crf> -profile:v high -pix_fmt yuv420p -colorspace bt709 -color_primaries bt709 -color_trc bt709 -color_range tv -video_track_timescale <fps×1000> <out>`。
- 連結: `-nostdin -hide_banner -n -f concat -safe 0 -i <list> -c:v copy <out>`。一覧は各行 `file '<名前>'`(LF、末尾も LF)。
- 多重化: `-nostdin -hide_banner -n -i <video> -i <wav> -map 0:v -map 1:a -c:v copy -c:a aac -profile:a aac_low -b:a <bitrate> -ar 48000 -ac 2 -movflags +faststart <out>`。

- [ ] **Step 1: 失敗するテストを書く**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn s() -> EncodeSettings {
        EncodeSettings { width: 1920, height: 1080, fps: 30, crf: 20, preset: "medium".into(), threads: 4, fit: Fit::Contain, background: [0, 0, 0] }
    }

    #[test]
    fn version_parsing() {
        // Review Focus 5
        assert_eq!(check_version("ffmpeg version 6.1.1-full_build-www.gyan.dev Copyright"), VersionCheck::Ok);
        assert_eq!(check_version("ffmpeg version n7.1 Copyright"), VersionCheck::Ok);
        assert_eq!(check_version("ffmpeg version 2026-03-26-git-fd9f1e9c52-full_build-www.gyan.dev Copyright"), VersionCheck::Ok);
        assert!(matches!(check_version("ffmpeg version 5.1.2 Copyright"), VersionCheck::TooOld(_)));
        assert!(matches!(check_version("ffmpeg version 2021-01-01-git-x"), VersionCheck::TooOld(_)));
        assert!(matches!(check_version("something else"), VersionCheck::Unknown(_)));
    }

    #[test]
    fn png_contain_filter_is_exact() {
        assert_eq!(
            cut_filter(&Background::Png { path: "a.png".into() }, 60, &s()),
            "[0:v]setpts=PTS-STARTPTS,fps=30,scale=1920:1080:force_original_aspect_ratio=decrease:out_color_matrix=bt709:out_range=tv,pad=1920:1080:(ow-iw)/2:(oh-ih)/2:color=0x000000,setsar=1,format=yuv420p,trim=end_frame=60,setpts=PTS-STARTPTS[bg];[1:v]setpts=PTS-STARTPTS,format=rgba,scale=out_color_matrix=bt709:out_range=tv,format=yuva420p[o];[bg][o]overlay=x=0:y=0:format=yuv420:alpha=straight:eof_action=repeat:repeatlast=1:shortest=0,format=yuv420p[v]"
        );
    }

    #[test]
    fn video_cover_with_tpad_and_input_matrix() {
        let mut st = s();
        st.fit = Fit::Cover;
        let f = cut_filter(&Background::Video { path: "v.mp4".into(), looped: false, matrix: "bt601".into(), range: "pc".into() }, 90, &st);
        assert!(f.contains("scale=1920:1080:force_original_aspect_ratio=increase:in_color_matrix=bt601:in_range=pc:out_color_matrix=bt709:out_range=tv,crop=1920:1080"), "{f}");
        assert!(f.contains("format=yuv420p,tpad=stop_mode=clone:stop=-1,trim=end_frame=90"), "{f}");
        let looped = cut_filter(&Background::Video { path: "v.mp4".into(), looped: true, matrix: "bt709".into(), range: "tv".into() }, 90, &st);
        assert!(!looped.contains("tpad"), "{looped}");
    }

    #[test]
    fn jpeg_declares_bt601_full_range() {
        let f = cut_filter(&Background::Jpeg { path: "a.jpg".into() }, 30, &s());
        assert!(f.contains("decrease:in_color_matrix=bt601:in_range=pc:out_color_matrix=bt709"), "{f}");
    }

    #[test]
    fn cut_args_are_exact() {
        let a = cut_args(&Background::Video { path: "D:/素材/波.mp4".into(), looped: true, matrix: "bt709".into(), range: "tv".into() }, "ovl_0001.txt", 60, "cut_0001.mp4", &s());
        let head: Vec<&str> = a.iter().take(10).map(|x| x.as_str()).collect();
        assert_eq!(head, vec!["-nostdin", "-hide_banner", "-n", "-stream_loop", "-1", "-i", "D:/素材/波.mp4", "-f", "concat", "-safe"]);
        let tail: Vec<&str> = a.iter().rev().take(4).map(|x| x.as_str()).collect();
        assert_eq!(tail, vec!["cut_0001.mp4", "30000", "-video_track_timescale", "tv"]);
        for must in ["-frames:v", "60", "-fps_mode", "cfr", "-threads", "4", "-profile:v", "high", "-an", "-map", "[v]"] {
            assert!(a.iter().any(|x| x == must), "{must}: {a:?}");
        }
        let img = cut_args(&Background::Png { path: "a.png".into() }, "ovl_0001.txt", 60, "cut_0001.mp4", &s());
        assert_eq!(&img[3..9], &["-loop", "1", "-framerate", "30", "-i", "a.png"]);
    }

    #[test]
    fn concat_and_mux_args() {
        assert_eq!(concat_list(&["cut_0001.mp4".into(), "cut_0002.mp4".into()]), "file 'cut_0001.mp4'\nfile 'cut_0002.mp4'\n");
        assert_eq!(concat_args("concat.txt", "video.mp4"), vec!["-nostdin", "-hide_banner", "-n", "-f", "concat", "-safe", "0", "-i", "concat.txt", "-c:v", "copy", "video.mp4"]);
        let m = mux_args("video.mp4", "D:/台本/full_dialogue.wav", "output.tmp.mp4", "192k");
        assert_eq!(m.join(" "), "-nostdin -hide_banner -n -i video.mp4 -i D:/台本/full_dialogue.wav -map 0:v -map 1:a -c:v copy -c:a aac -profile:a aac_low -b:a 192k -ar 48000 -ac 2 -movflags +faststart output.tmp.mp4");
    }
}
```

- [ ] **Step 2〜4**: 失敗を確かめ、実装し、通す(`cargo test -p s2v-video ffmpeg`)。

- [ ] **Step 5: 全体の検証とコミット** — `feat: ffmpeg の版の判定と、カット・連結・多重化の引数の組み立てを加える`

---

### Task 11: ffprobe の結果の検査と音声の検証

**Files:**
- Create: `crates/s2v-video/src/probe.rs`、`crates/s2v-video/src/audio_check.rs`
- Modify: `crates/s2v-video/src/lib.rs`

**Interfaces:**
- Produces(probe.rs):

```rust
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
pub struct Stream {
    pub codec_type: Option<String>, pub codec_name: Option<String>, pub codec_tag_string: Option<String>,
    pub profile: Option<String>, pub level: Option<i64>, pub pix_fmt: Option<String>, pub field_order: Option<String>,
    pub width: Option<u32>, pub height: Option<u32>, pub sample_aspect_ratio: Option<String>,
    pub color_range: Option<String>, pub color_space: Option<String>, pub color_transfer: Option<String>, pub color_primaries: Option<String>,
    pub time_base: Option<String>, pub r_frame_rate: Option<String>, pub start_pts: Option<i64>, pub duration_ts: Option<i64>,
    pub nb_read_frames: Option<String>, pub extradata_hash: Option<String>,
    pub sample_rate: Option<String>, pub channels: Option<u32>,
}
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
pub struct Probe { #[serde(default)] pub streams: Vec<Stream> }
pub fn parse_probe(json: &str) -> Result<Probe, String>;
/// ffprobe の引数(カット・出力の検査用): -v error -count_frames -show_streams -show_data_hash sha256 -of json <path>
pub fn probe_args(path: &str) -> Vec<String>;
/// 素材の検査用(数えない): -v error -show_streams -of json <path>
pub fn asset_probe_args(path: &str) -> Vec<String>;
/// 動画の素材の色。Ok((scale の入力の行列, レンジ))、受け付けない値は Err(エラー 2 の理由)
pub fn video_color_input(s: &Stream) -> Result<(String, String), String>;
/// 1つのカットファイルの検査(映像1本だけ、time base 1/(fps*1000)、start_pts 0、duration_ts N*1000、nb_read_frames N)
pub fn check_cut(p: &Probe, n: i64, fps: u32) -> Vec<String>;
/// 全カットの一致(codec・tag・profile・level・pix_fmt・field order・幅・高さ・SAR・色4項目・time base・フレームレート・extradata)
pub fn check_cuts_consistent(probes: &[(String, Probe)]) -> Vec<String>;
/// 出力の検査(映像1+音声1、H.264・幅・高さ・fps/1・コマ数、AAC LC・48000・2ch)
pub fn check_output(p: &Probe, width: u32, height: u32, fps: u32, total_frames: i64) -> Vec<String>;
```

- Produces(audio_check.rs):

```rust
/// 出力の音声を PCM(f32le、48k、モノラル)に復号する ffmpeg の引数
pub fn decode_args(path: &str) -> Vec<String>; // -nostdin -hide_banner -v error -i <path> -map 0:a -ac 1 -ar 48000 -f f32le -
/// f32le のバイト列を読む
pub fn f32le(bytes: &[u8]) -> Vec<f32>;
/// 相互相関で最も一致するずれ(サンプル、正なら decoded が遅れている)。search は ±search サンプル
pub fn best_lag(reference: &[f32], decoded: &[f32], search: usize) -> i64;
/// 設計書 §4.6 の音声の検査。reference は full_dialogue.wav の左右の平均
pub fn check_audio(reference: &[f32], decoded: &[f32]) -> Vec<String>;
```

`check_audio` の規則: (1) `|decoded.len() − reference.len()| ≤ 1024`。(2) 参照の最初の 10 秒(短ければ全体)の RMS が -60 dBFS 未満なら、最初に |x| ≥ 10^(-60/20) となる位置から 10 秒を窓にする。全体が無音なら検査を飛ばす(空の Vec を返し、呼び出し側が Process.log に書く。区別のため、戻り値ではなく `pub fn is_silent(reference: &[f32]) -> bool` も用意する)。(3) 窓で `best_lag(search = 2048)` が ±2 以内。`best_lag` は realfft で、窓の長さ + 2×search 以上の 2 のべき乗の長さで相互相関を求める。

- [ ] **Step 1: 失敗するテストを書く(ffprobe の出力の JSON を手で書いた固定値で)**

```rust
// probe.rs の tests
#[cfg(test)]
mod tests {
    use super::*;

    fn cut_json(frames: &str, tb: &str, dts: i64, hash: &str) -> String {
        format!(r#"{{"streams":[{{"codec_type":"video","codec_name":"h264","codec_tag_string":"avc1","profile":"High","level":40,"pix_fmt":"yuv420p","field_order":"progressive","width":1920,"height":1080,"sample_aspect_ratio":"1:1","color_range":"tv","color_space":"bt709","color_transfer":"bt709","color_primaries":"bt709","time_base":"{tb}","r_frame_rate":"30/1","start_pts":0,"duration_ts":{dts},"nb_read_frames":"{frames}","extradata_hash":"{hash}"}}]}}"#)
    }

    #[test]
    fn good_cut_passes() {
        let p = parse_probe(&cut_json("60", "1/30000", 60_000, "SHA256:aa")).unwrap();
        assert!(check_cut(&p, 60, 30).is_empty());
    }

    #[test]
    fn each_cut_mismatch_is_reported() {
        assert!(!check_cut(&parse_probe(&cut_json("59", "1/30000", 60_000, "x")).unwrap(), 60, 30).is_empty());
        assert!(!check_cut(&parse_probe(&cut_json("60", "1/15360", 60_000, "x")).unwrap(), 60, 30).is_empty());
        assert!(!check_cut(&parse_probe(&cut_json("60", "1/30000", 59_000, "x")).unwrap(), 60, 30).is_empty());
        let two = cut_json("60", "1/30000", 60_000, "x").replace("}]}", "},{\"codec_type\":\"audio\"}]}");
        assert!(!check_cut(&parse_probe(&two).unwrap(), 60, 30).is_empty());
    }

    #[test]
    fn cuts_must_agree() {
        let a = parse_probe(&cut_json("60", "1/30000", 60_000, "SHA256:aa")).unwrap();
        let b = parse_probe(&cut_json("30", "1/30000", 30_000, "SHA256:bb")).unwrap();
        let errs = check_cuts_consistent(&[("cut_0001.mp4".into(), a.clone()), ("cut_0002.mp4".into(), b)]);
        assert!(errs.iter().any(|e| e.contains("extradata") && e.contains("cut_0002.mp4")), "{errs:?}");
        assert!(check_cuts_consistent(&[("1".into(), a.clone()), ("2".into(), a)]).is_empty());
    }

    #[test]
    fn video_color_rules() {
        let mut s = Stream { codec_type: Some("video".into()), ..Default::default() };
        assert_eq!(video_color_input(&s), Ok(("bt709".into(), "tv".into())));
        s.color_space = Some("smpte170m".into());
        s.color_range = Some("pc".into());
        assert_eq!(video_color_input(&s), Ok(("bt601".into(), "pc".into())));
        s.color_space = Some("bt470bg".into());
        assert_eq!(video_color_input(&s).unwrap().0, "bt601");
        s.color_transfer = Some("smpte2084".into());
        assert!(video_color_input(&s).is_err()); // HDR
        s.color_transfer = Some("unknown".into());
        s.color_primaries = Some("bt2020".into());
        assert!(video_color_input(&s).is_err());
        s.color_primaries = None;
        s.color_space = Some("bt2020nc".into());
        assert!(video_color_input(&s).is_err());
    }

    #[test]
    fn output_checks() {
        let ok = r#"{"streams":[
            {"codec_type":"video","codec_name":"h264","width":1920,"height":1080,"r_frame_rate":"30/1","nb_read_frames":"90"},
            {"codec_type":"audio","codec_name":"aac","profile":"LC","sample_rate":"48000","channels":2}]}"#;
        assert!(check_output(&parse_probe(ok).unwrap(), 1920, 1080, 30, 90).is_empty());
        assert!(!check_output(&parse_probe(&ok.replace("\"90\"", "\"89\"")).unwrap(), 1920, 1080, 30, 90).is_empty());
        assert!(!check_output(&parse_probe(&ok.replace("\"LC\"", "\"HE-AAC\"")).unwrap(), 1920, 1080, 30, 90).is_empty());
        assert!(!check_output(&parse_probe(&ok.replace("\"channels\":2", "\"channels\":1")).unwrap(), 1920, 1080, 30, 90).is_empty());
    }

    #[test]
    fn args() {
        assert_eq!(probe_args("cut_0001.mp4"), vec!["-v", "error", "-count_frames", "-show_streams", "-show_data_hash", "sha256", "-of", "json", "cut_0001.mp4"]);
    }
}

// audio_check.rs の tests
#[cfg(test)]
mod tests {
    use super::*;

    fn noise(n: usize, seed: u64) -> Vec<f32> {
        let mut s = seed;
        (0..n).map(|_| { s = s.wrapping_mul(6364136223846793005).wrapping_add(1); ((s >> 40) as f32 / (1u64 << 24) as f32) - 0.5 }).collect()
    }

    #[test]
    fn lag_is_found() {
        let r = noise(48_000, 7);
        let mut d = vec![0.0; 100];
        d.extend_from_slice(&r);
        assert_eq!(best_lag(&r, &d, 2048), 100);
        assert_eq!(best_lag(&r, &r, 2048), 0);
    }

    #[test]
    fn audio_check_accepts_aligned_and_rejects_shifted() {
        let r = noise(48_000 * 3, 9);
        assert!(check_audio(&r, &r).is_empty());
        let mut shifted = vec![0.0; 100];
        shifted.extend_from_slice(&r[..r.len() - 100]);
        assert!(!check_audio(&r, &shifted).is_empty()); // 100 サンプルのずれ
        assert!(!check_audio(&r, &r[..r.len() - 2000]).is_empty()); // 長さの差 > 1024
    }

    #[test]
    fn leading_silence_moves_the_window_and_all_silence_is_detected() {
        let mut r = vec![0.0; 48_000 * 12];
        r.extend(noise(48_000 * 3, 3));
        assert!(check_audio(&r, &r).is_empty());
        assert!(is_silent(&vec![0.0; 48_000]));
        assert!(!is_silent(&r));
    }

    #[test]
    fn f32le_reads_little_endian() {
        let b: Vec<u8> = [1.0f32, -0.5].iter().flat_map(|x| x.to_le_bytes()).collect();
        assert_eq!(f32le(&b), vec![1.0, -0.5]);
    }
}
```

- [ ] **Step 2〜4**: 失敗を確かめ、実装し、通す(`cargo test -p s2v-video`)。比較の項目の名前(`extradata` など)をエラーメッセージに含める。

- [ ] **Step 5: 全体の検証とコミット** — `feat: ffprobe の結果の検査と、出力の音声の検証を加える`

---

### Task 12: `render` コマンドと ffmpeg の結合テスト

**Files:**
- Create: `crates/s2v-cli/src/render.rs`、`crates/s2v-cli/tests/render_ffmpeg.rs`
- Modify: `crates/s2v-cli/src/lib.rs`、`src/main.rs`、`crates/s2v-cli/Cargo.toml`(`s2v-render`・`s2v-video`・`tiny-skia` は不要なら s2v-render 経由で)、`crates/s2v-cli/src/layout.rs`(`WORK_SUBDIRS` に `"render"` を足す、または `check_contained(root, "work/render")` を呼ぶ)

**Interfaces:**
- Consumes: 計画 2a の `freshness::check_fresh`、`lock::ScriptLock`、`proclog::ProcLog::append`、`layout::{check_reserved_name, check_not_symlink, check_contained, script_dir}`、`fsutil::sha256_file`、`s2v_export::wavio::read_stereo_48k`。Task 1〜11 のすべて。
- Produces: `pub struct RenderArgs { pub script: PathBuf, pub config: Option<PathBuf> }`、`pub fn run_render(args: &RenderArgs) -> Outcome`。CLI: `script2video render <台本> [--config <path>]`。

処理の順(設計書 §4.1。失敗はすべて `output.mp4` を変えない):
1. `check_reserved_name`、`check_not_symlink`、台本が無ければ「台本を読めません」で 2。設定を読む(`--config` か同梱)・`validate`。`ScriptLock::acquire(script_dir)`。
2. `ProcLog::append(Process.log, "render")`。`check_fresh(script, true)`(警告は log、Err は log に書いて 2)。
3. フォント(`font_path` が相対なら台本フォルダから。`FontFace::load`)→ 失敗は 2。ffmpeg・ffprobe(設定のパス、無ければ "ffmpeg"・"ffprobe")を `-version` で起動 → 起動できなければ「ffmpeg が見つかりません: <名前>(PATH か config.toml の render.ffmpeg を確かめてください)」で 2、`check_version` が TooOld なら 2、Unknown なら警告。
4. `bubbles.json` があれば `parse_bubbles` → `scale_to(width, height)`。どちらの失敗も 2。
5. 項目を作る: 台詞 → 吹き出し(本文は `lines[].text`)、ト書き → ト書き。表示時刻は `effective_times`(Err は 2)。`timeline` に無い ID の `bubbles.json` の項目は警告。フォントに無い文字は項目ごとに警告。
6. 大きさ: 手動の項目で `fits` が偽なら `expand_to_min` して警告。自動の項目は `min_size`(`max_inner_h = height × ratio`)。`auto_layout` で残りを決める(警告は log)。
7. 素材: コマ数 1 以上のカットについて `match_assets(tracks/, …)`。エラーは 2。不足は `default_image`(相対なら台本フォルダから、拡張子で Png/Jpeg)があれば警告して代わりに使い、無ければ一覧を出して 2。動画の素材は ffprobe(`asset_probe_args`)で映像のストリームを確かめ、`video_color_input` で色を決める(Err は 2)。画像の素材も ffprobe で映像のストリームがあることを確かめる。
8. `work/render/` を `check_contained` のあと作り直し(`remove_dir_all` してから `create_dir_all("work/render/seg")`)。
9. `plan_cuts`。区間ごとに項目の組で重ね画像を合成(同じ組は1枚にまとめる。空の組は `ov_empty.png`)して `encode_png` で書く。カットごとに `overlay_list` を `ovl_XXXX.txt` に書く。
10. カットファイルを `jobs` 個まで並列に作る(`std::thread::scope`、`threads = max(1, available_parallelism / jobs)`)。ffmpeg の失敗は標準エラーの末尾を log に書いて 2。カットごとの所要時間を log。
11. 各カットを ffprobe(`probe_args`)→ `check_cut`・`check_cuts_consistent`。失敗は「内部の誤り」として 2。`concat.txt` → 連結(`video.mp4`)→ 多重化(`output.tmp.mp4`。作業フォルダの中)。
12. `output.tmp.mp4` を ffprobe → `check_output`。音声: `decode_args` で復号、`read_stereo_48k(full_dialogue.wav)` の左右の平均と `check_audio`(`is_silent` なら飛ばして log)。失敗は 2。
13. `std::fs::rename(work/render/seg/output.tmp.mp4, output.mp4)`。失敗は 2(元の `output.mp4` は残る)。成功したら「output.mp4 を書き出しました(<コマ数> コマ、<秒> 秒)」を log。
14. 終了コード: 警告があれば 1、無ければ 0。

- [ ] **Step 1: 失敗する結合テストを書く(`tests/render_ffmpeg.rs`)**

テストの補助(ファイルの先頭に置く):
- `skip()`: `std::env::var("S2V_SKIP_FFMPEG_TESTS").as_deref() == Ok("1")` なら true。各テストの最初で `if skip() { return; }`。ffmpeg が無いときは、テストの中の ffmpeg の起動が失敗して panic する(=テストは失敗)。
- `project(script_body: &str, assets: &[(&str, Asset)])`: 一時フォルダ(名前に日本語と空白を含む `台本 テスト`)に台本を書き、`build` を通して `full_dialogue.wav` と timeline を作る(既存の `build_golden.rs` の `write_voice` と同じ 440Hz の録音を `voices/` に置く)。設定ファイル `config.toml` を書き、`[render]` に `width = 320`、`height = 180`、`fps = 30`、`font_path = <crates/s2v-render/tests/fonts/test-vrt2.ttf の絶対パス>`、`font_size = 16.0`、`bubble_padding = 6`、`note_padding = 4`、`margin = 8`、`bubble_gap = 6`、`tail_length = 12`、`preset = "ultrafast"` を書く(他は既定)。
- 素材の作り方: 単色の PNG は tiny-skia で作って `encode_png`。動画は、コマ番号 k のコマを「左半分が k×8 の灰色」の画像にした PNG を 60 枚作り、`ffmpeg -nostdin -n -framerate 30 -i f_%03d.png -c:v libx264 -qp 0 -pix_fmt yuv420p clip.mp4` で作る(lossless に近い)。
- `frame_rgb(mp4, n) -> Vec<u8>`: `ffmpeg -nostdin -v error -i <mp4> -vf select=eq(n\,<n>) -frames:v 1 -f rawvideo -pix_fmt rgb24 -` の標準出力。

テスト(それぞれ `run_render` の戻り値と、出力を確かめる):

```rust
#[test]
fn renders_image_cut_with_bubble_and_passes_output_checks() {
    // 台本: 1シーン、#visual (image)、台詞1つ(1.11 秒)、#note。素材 S01.png は赤
    // 期待: Outcome::Ok か Warnings。output.mp4 があり、ffprobe で 320x180・30fps・AAC。
    // 台詞の表示中のコマ(例 n=10)に吹き出しの白(255,255,255 近く)の画素がある。
    // 背景の赤(220 以上, 40 未満, 40 未満)の画素もある。
}

#[test]
fn overlay_switches_exactly_at_the_boundary_frame() {
    // 台詞の表示開始の絶対時刻から frame(start) を求め、そのコマには吹き出しの白があり、1つ前のコマには無い(背景の色だけ)ことを確かめる。fps 30 と 60(設定を変えて2回)。
}

#[test]
fn overlay_list_pts_are_on_the_half_frame_grid() {
    // render 後の work/render/seg/ovl_0001.txt を ffprobe -v error -show_packets -select_streams v -of json -f concat -safe 0 -i ovl_0001.txt で読み、
    // pts の列が 0, 2r_1-1, …, 2N(time base 1/(2fps))であること。fps 30 と 60。
}

#[test]
fn video_asset_continues_across_intervals_and_freezes_or_loops() {
    // 素材 S01.mp4(60 コマ、コマ k は左半分が k*8 の灰色)。カットの長さ 3 秒(90 コマ)、途中で吹き出しが出入りして区間が分かれる。
    // loop なし: 出力のコマ k(<60)の左半分の灰色 ≒ k*8(±6)で、区間の境目で飛ばない・重ならない。k ≥ 60 では 59*8 のまま(最後のコマで止まる)。
    // #visual (video loop): コマ 60+j の灰色 ≒ j*8。
}

#[test]
fn colors_survive_within_tolerance() {
    // 既知の色の見本: sRGB の PNG(RGB (200,100,50))、bubbles の白、BT.709 の動画、BT.601(smpte170m)の動画、full レンジの動画(ffmpeg で -colorspace/-color_range を付けて作る)。
    // 出力のコマを rgb24 に戻した値が元の値と ±3 以内。出力のタグが bt709・tv(ffprobe)。
}

#[test]
fn hdr_or_bt2020_asset_is_rejected() {
    // ffmpeg で -color_primaries bt2020 -color_trc smpte2084 を付けた動画を素材にすると Outcome::Failed、output.mp4 は作られない。
}

#[test]
fn audio_matches_full_dialogue() {
    // output.mp4 の音声を decode_args で復号し、full_dialogue.wav との best_lag が ±2、長さの差が 1024 以内(render の検査と同じ関数で、独立に確かめる)。
}

#[test]
fn failed_render_keeps_previous_output() {
    // 先に output.mp4(内容 b"old")を置き、素材を置かずに render → Failed。output.mp4 は b"old" のまま。
}

#[test]
fn broken_bubbles_json_keeps_previous_output() {
    // Review Focus 4: bubbles.json に "{" を書いて render → Failed、Process.log に bubbles.json の理由、output.mp4 は変わらない。
}

#[test]
fn missing_ffmpeg_is_failed_without_writing() {
    // Review Focus 5: config の render.ffmpeg = "存在しない-ffmpeg.exe" → Failed、work/render/ が作られない、output.mp4 も無い。(この1本は S2V_SKIP_FFMPEG_TESTS でも実行してよい)
}

#[test]
fn default_image_is_used_with_warning() {
    // 素材なし、config に default_image = 単色 PNG の絶対パス → Outcome::Warnings、output.mp4 ができる。
}

#[test]
fn stale_build_is_rejected() {
    // build の後で full_dialogue.wav を書き換える → Failed(check_fresh)。
}
```

各テストの本文は、上のコメントの手順と期待を、補助関数を使って具体的に書く(数値の許容は書いたとおり)。

`render.rs` の単体テスト(ffmpeg を使わない):
- `items_are_built_from_timeline`: 台詞・ト書きから項目と表示時刻が作られ、`bubbles.json` の offset が反映される。
- `distinct_overlay_sets_are_written_once`: 同じ項目の組の区間が複数あっても PNG は1枚(関数 `assign_overlay_files(plans) -> (BTreeMap<Vec<String>, String>, Vec<Vec<(i64, String)>>)` を切り出してテストする。空の組は `ov_empty.png`)。
- `threads_split_cores_by_jobs`: `threads_for(cores: usize, jobs: u32) -> u32`(`max(1, cores / jobs)`)。

- [ ] **Step 2: 失敗を確かめる** — Run: `cargo test -p s2v-cli --test render_ffmpeg` / Expected: `run_render` が無くコンパイルエラー。

- [ ] **Step 3: 実装する** — 上の順で `render.rs` を書く。外部プロセスの起動は `std::process::Command::new(ffmpeg).args(..).current_dir(seg_dir)`。標準エラーは `Stdio::piped()` で受け、失敗時の末尾 20 行を log に書く。`main.rs` に `Render { script, #[arg(long)] config }` を加える。

- [ ] **Step 4: テストが通ることを確かめる** — Run: `cargo test -p s2v-cli`(ffmpeg のある環境で、`S2V_SKIP_FFMPEG_TESTS` を付けずに)

- [ ] **Step 5: 全体の検証とコミット** — `feat: render コマンドで素材・吹き出し・音声を合成して output.mp4 を作る`

---

### Task 13: 性能試験と README

**Files:**
- Create: `crates/s2v-cli/tests/render_perf.rs`
- Modify: `README.md`

- [ ] **Step 1: 性能試験を書く(`#[ignore]`)**

```rust
//! 性能試験(手動で実行: cargo test -p s2v-cli --release --test render_perf -- --ignored --nocapture)
//! 設計書 §8: (a) 200 カット・1000 吹き出し・30 分(素材は画像)、(b) 30 分の動画の素材1つのカットに 1000 吹き出し。
//! どちらも 30 分以内、プロセス全体のメモリ 2 GiB 以内、work/render/ 2 GiB 以内。
```

台本は機械的に生成する(台詞は 1.8 秒の録音を 1000 個。録音は同じ wav を複製)。所要時間を `Instant` で測り、`work/render/` の大きさを合計して表示し、上限を `assert!` する(メモリは測れる範囲で表示だけ。Windows ではタスクマネージャで目視する旨をコメントに書く)。素材は 1920×1080 の単色 PNG と、ffmpeg の `testsrc2` で作る 30 分の 1920×1080・30fps の動画(`-preset ultrafast`)。

- [ ] **Step 2: README を更新する**

`README.md` に `render` の節を加える: 使い方(`script2video render 台本.txt [--config config.toml]`)、前提(ffmpeg・ffprobe 6.0 以上が PATH にあるか `[render] ffmpeg`/`ffprobe` で指定)、素材の置き方(`tracks/S01.png`・`tracks/S01_C02.mp4`、拡張子)、吹き出し・ト書きは縦書きで自動配置(`bubbles.json` があればその位置)、出力(`output.mp4`、中間は `work/render/`)、終了コード、主な失敗の理由(素材の不足・重複・種類違い、HDR の動画、フォントが読めない、build のやり直しが必要)、`[render]` の主な設定(解像度・fps・fit・default_image・font_path・jobs)。

- [ ] **Step 3: 全体の検証とコミット** — `docs: render の使い方を README に書き、性能試験を加える`

- [ ] **Step 4: 手動の確認(Human)**

sample の台本に素材(`tracks/S01.png` など)を置き、`script2video build` → `script2video render` で `output.mp4` を作り、縦書きの吹き出しが台詞に合わせて出入りし、ト書きの枠が次の区切りまで出て、動画の素材が最後のコマで止まる(`loop` ではループする)ことを目で確認してもらう。インストールは `cargo install --path crates/s2v-cli --locked`。
