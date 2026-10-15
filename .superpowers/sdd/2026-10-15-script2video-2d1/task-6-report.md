# Task 6 報告(media.rs)
- RED: media.rs にテストだけ置いて `cargo test -p s2v-bubbles media` -> E0425/E0433(open_locked・Slots 等が未定義)で 18 エラー、コンパイル失敗。
- GREEN: 実装後 7 テスト全て通過(Windows 専用 2 件を含む)。
- 検証: cargo fmt --check / clippy --workspace --all-targets -D warnings / cargo test --workspace 全て通過。
- Windows 専用テストは上書き・削除・改名が Err、別ハンドル読み取り Ok、read_at は元の内容、drop 後に削除 Ok、書き込みで開かれたファイルは open_locked が Err を実際に assert。
- 補足: Slots::try_acquire は load + compare_exchange の繰り返し。clippy の len_without_is_empty 対応で is_empty を追加。空ファイル + Range なしは 200 / Content-Length 0 / body None。
