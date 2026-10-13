# 流用元の記録

- 流用元: `D:\UDS\宇摩コードワークス\Script2Voice-Rust版`
- コミット: `d28ec03`(chore(beads): s2v-18b.15 のクローズ記録を追加する)
- コピーしたもの: `crates/s2v-core`、`crates/s2v-audio`、`crates/s2v-export`、`Cargo.lock`
- コピーしなかったもの: `s2v-engines`(音声合成エンジン。Script2Video は持たない)、`s2v-gui`、`s2v-video`(④最終合成で改めて検討)、ルートの `src/`
- 以後は独立して進化させる。流用元へ変更を戻さない。
- `Cargo.lock` は流用元の作業ツリーからコピーした(流用元では未追跡・git管理外のため d28ec03 には含まれない)。その後 cargo が s2v-cli のエントリを追加して更新した。流用元とのハッシュ一致は期待しない。
