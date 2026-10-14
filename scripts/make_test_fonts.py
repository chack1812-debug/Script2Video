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
    # 縦書きの寸法テーブルが片方だけ無いフォント(test-vrt2.ttf から作る)
    for table, name in (("vhea", "test-novhea.ttf"), ("vmtx", "test-novmtx.ttf")):
        font = TTFont(OUT / "test-vrt2.ttf")
        del font[table]
        font.save(OUT / name)

if __name__ == "__main__":
    main()
