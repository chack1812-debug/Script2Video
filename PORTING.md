# 流用元の記録

- 流用元: `D:\UDS\宇摩コードワークス\Script2Voice-Rust版`
- コミット: `d28ec03`(chore(beads): s2v-18b.15 のクローズ記録を追加する)
- コピーしたもの: `crates/s2v-core`、`crates/s2v-audio`、`crates/s2v-export`、`Cargo.lock`
- コピーしなかったもの: `s2v-engines`(音声合成エンジン。Script2Video は持たない)、`s2v-gui`、`s2v-video`(④最終合成で改めて検討)、ルートの `src/`
- 以後は独立して進化させる。流用元へ変更を戻さない。
- `Cargo.lock` は流用元の作業ツリーからコピーした(流用元では未追跡・git管理外のため d28ec03 には含まれない)。その後 cargo が s2v-cli のエントリを追加して更新した。流用元とのハッシュ一致は期待しない。

## s2v-engines の追加(合成音声の取り込み)

- 流用元: `D:\UDS\宇摩コードワークス\Script2Voice-Rust版` の `crates/s2v-engines`(コミット `55cffe4`)を `crates/s2v-engines` へコピーした。上の「コピーしなかったもの」に挙げた `s2v-engines` は、合成音声の取り込みに伴いここで持つことにした。
- 変更点: `s2v_core::Cast` への依存を `SynthRequest`(`speaker_name` / `style` / `language` / `params`)に置き換えた。`Engine::is_cast_valid` は `is_request_valid` に改名、`EngineManager::synthesize` はエンジン名を引数で受ける。`style` と `language` は `params` からではなく `SynthRequest` の専用フィールドから読む。`s2v-core` への path 依存は残している(設定型は後続タスクで追加する)。
- `tokio` は流用元のワークスペース依存だったため、このクレートの `Cargo.toml` に直接書いた。
