# 流用元の記録

- 流用元: `D:\UDS\宇摩コードワークス\Script2Voice-Rust版`
- コミット: `d28ec03`(chore(beads): s2v-18b.15 のクローズ記録を追加する)
- コピーしたもの: `crates/s2v-core`、`crates/s2v-audio`、`crates/s2v-export`、`Cargo.lock`、`config.toml`(s2v-core のテストがルートの config.toml を読むため)
- コピーしなかったもの: `s2v-engines`(音声合成エンジン。Script2Video は持たない)、`s2v-gui`、`s2v-video`(④最終合成で改めて検討)、ルートの `src/`
- 以後は独立して進化させる。流用元へ変更を戻さない。
