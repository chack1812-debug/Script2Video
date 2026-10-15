# テスト用フォント

`s2v-render` のテストで使う、Noto Sans JP(SIL Open Font License 1.1、`OFL.txt` 参照)の部分集合。

- 元のフォント: Noto Sans JP の可変フォント `NotoSansJP-VF.ttf`(Windows 11 の `C:\Windows\Fonts\NotoSansJP-VF.ttf`)。wght=400 に固定してから部分集合にしている。
- 部分集合の文字: `scripts/make_test_fonts.py` の `TEXT`(ひらがな・小書き文字、カタカナ、漢数字と一部の漢字「港朝昼夜海波岸壁翌人黙早今日晴雨台詞吹出書枠縦横字」、句読点・括弧、数字、ASCII の英数字と記号)。`ゑ`・`龘`・絵文字は含めない(`font.rs` のテストがこれに頼る)。
- 再生成: リポジトリのルートで次を実行する(fonttools が必要)。

```
python -I scripts/make_test_fonts.py "C:\Windows\Fonts\NotoSansJP-VF.ttf"
```

## 生成物(fonttools で確認済み)

| ファイル | GSUB の機能 | vhea / vmtx |
| --- | --- | --- |
| `test-vrt2.ttf` | `vert`、`vrt2` | あり |
| `test-vert.ttf` | `vert` のみ | あり |
| `test-novert.ttf` | なし(縦書きの機能が無いので拒否される) | あり |
| `test-novhea.ttf` | `vert`、`vrt2`(`test-vrt2.ttf` から `vhea` だけを消した) | vmtx のみ(拒否される) |
| `test-novmtx.ttf` | `vert`、`vrt2`(`test-vrt2.ttf` から `vmtx` だけを消した) | vhea のみ(拒否される) |
