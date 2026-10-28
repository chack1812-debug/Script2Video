use std::future::Future;
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;

use tracing::{info, warn};

use crate::job::{resume_suspended_process, EngineJob};
use crate::lock;

/// 起動待機のポーリング間隔。Python 版の `await sleep(1)` に合わせて 1 秒固定。
const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// 自動起動の既定待機時間。AivisSpeech の初回モデルロードが 30 秒で足りない事例を受け 60 秒。
pub const DEFAULT_STARTUP_TIMEOUT: Duration = Duration::from_secs(60);

/// 使用中のエンジンへの参照を保持する。
///
/// `job` は名前付き Job Object のハンドル。自分がエンジンを spawn したかどうかに関わらず、
/// エンジンを使っている間ずっと保持する。Drop するとハンドルが閉じ、自分が最後の保持者なら
/// `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` により OS がエンジンツリーを終了させる。
///
/// `child` は自分が spawn した場合のみ `Some`。既に起動していたエンジンを使う場合は `None`。
///
/// どちらのフィールドも「読む」ためではなく Drop させるために保持しているので、
/// 通常のビルドでは never read になる（`dead_code` はそれを承知で抑止している）。
#[allow(dead_code)]
pub(crate) struct EngineProcess {
    child: Option<Child>,
    job: EngineJob,
    /// `child` を Job に割り当てられたか。割当に失敗した子は `KILL_ON_JOB_CLOSE` の対象外なので、
    /// 解放時に明示的に終了させる（`Child` は drop しても子を終了させない）。
    assigned: bool,
}

/// 接続確認を行い、失敗時は `exe_path` が設定されていればプロセスを起動して
/// 起動完了まで待機する。既に起動済みなら何もしない。
pub(crate) async fn ensure_running<F, Fut>(
    name: &str,
    key: &str,
    exe_path: Option<&str>,
    args: &[String],
    timeout: Duration,
    process: &Mutex<Option<EngineProcess>>,
    is_alive: F,
) -> anyhow::Result<()>
where
    F: Fn() -> Fut,
    Fut: Future<Output = bool>,
{
    // 「起動確認 → spawn → 起動完了待ち」を跨いで排他する。ここを覆わないと、
    // P1 が spawn した直後（まだ /version が応答しない間）に P2 が来て二重に spawn してしまう。
    // ロックガードはこの関数を抜けるまで保持する。
    let startup_lock = lock::acquire(key, timeout).await;
    if startup_lock.is_none() {
        warn!("[{name}] 起動ロックを取得できませんでした。排他せずに続行します。");
    }

    let job = EngineJob::open_or_create(&format!("Local\\Script2Voice_Engine_{key}"))
        .map_err(|e| anyhow::anyhow!("{name}: Job Object の作成に失敗しました: {e}"))?;

    if is_alive().await {
        info!("[{name}] 既に起動しています。");
        // 自分が起動していなくても Job ハンドルは保持する。
        // これを保持しないと、エンジンを起動したプロセスが先に終了した時点で
        // 使用中のエンジンが終了してしまう。
        *process.lock().unwrap() = Some(EngineProcess {
            child: None,
            job,
            assigned: true,
        });
        return Ok(());
    }

    let path = exe_path.ok_or_else(|| {
        anyhow::anyhow!("{name}: サーバーに接続できず、exe_path も未設定のため起動できません")
    })?;

    info!(
        "[{name}] 起動を確認できません。プロセスを起動します: {path} {}",
        args.join(" ")
    );
    let (child, assigned) = spawn_into_job(name, path, args, &job, || {})?;

    *process.lock().unwrap() = Some(EngineProcess {
        child: Some(child),
        job,
        assigned,
    });

    let retries = timeout.as_secs().max(1);
    for _ in 0..retries {
        tokio::time::sleep(POLL_INTERVAL).await;
        if is_alive().await {
            info!("[{name}] エンジンの起動を確認しました。");
            return Ok(());
        }
    }
    anyhow::bail!("{name}: 起動待機が {} 秒でタイムアウトしました", retries)
}

/// エンジンを起動して `job` に割り当てる。戻り値の `bool` は割当に成功したかどうか。
///
/// `CREATE_SUSPENDED` で起動し、Job に割り当ててからスレッドを再開する。VOICEVOX / AivisSpeech の
/// `run.exe` のように起動直後に本体を孫プロセスとして起動するランチャーでは、通常どおり起動すると
/// spawn から割当までの間に孫が生まれて Job の外に出てしまい、`KILL_ON_JOB_CLOSE` が効かない。
///
/// `before_assign` は spawn 直後・割当直前に呼ばれる（テストで割当を遅らせ、競合を再現するため）。
fn spawn_into_job(
    name: &str,
    path: &str,
    args: &[String],
    job: &EngineJob,
    before_assign: impl FnOnce(),
) -> anyhow::Result<(Child, bool)> {
    // `path` は絶対パス（実行ファイル）と PATH 上のコマンド名（例: "python"）の両方を許容する。
    // 事前の存在チェックはせず、OS の解決結果を spawn() のエラーでそのまま扱う。
    const CREATE_SUSPENDED: u32 = windows_sys::Win32::System::Threading::CREATE_SUSPENDED;
    let mut child = Command::new(path)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_SUSPENDED)
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                anyhow::anyhow!("{name}: 実行ファイルが見つかりません: {path}")
            } else {
                anyhow::anyhow!("{name}: プロセスの起動に失敗しました: {e}")
            }
        })?;

    before_assign();

    let assigned = match job.assign(&child) {
        Ok(()) => true,
        Err(e) => {
            warn!(
                "[{name}] プロセスを Job Object に割り当てられませんでした(孫プロセスは終了対象外になる可能性があります): {e}"
            );
            false
        }
    };

    // 割当に失敗しても起動自体は続行する（解放時に child を kill する）。
    if let Err(e) = resume_suspended_process(child.id()) {
        let _ = child.kill();
        let _ = child.wait();
        anyhow::bail!("{name}: 起動したプロセスを再開できませんでした: {e}");
    }
    Ok((child, assigned))
}

/// エンジンへの参照（名前付き Job のハンドル）を解放する。
///
/// 明示的にエンジンを終了させることはしない。自分が最後の保持者であれば、ハンドルが閉じた
/// 時点で OS が `KILL_ON_JOB_CLOSE` によりエンジンツリーを終了させる。他の Script2Voice
/// プロセスがまだ同じエンジンを使っている場合はエンジンが生き残る。
pub(crate) fn terminate_process(name: &str, process: &Mutex<Option<EngineProcess>>) {
    let mut guard = process.lock().unwrap();
    if let Some(mut entry) = guard.take() {
        info!("[{name}] エンジンへの参照を解放します（最後の利用者ならエンジンも終了します）。");
        if !entry.assigned {
            // Job 外の子は KILL_ON_JOB_CLOSE で終了しないので、自分で終了させる。
            // （その子が既に起動した孫プロセスまでは終了させられない）
            if let Some(child) = entry.child.as_mut() {
                warn!("[{name}] Job Object 未割当のため、起動したプロセスを直接終了します。");
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        drop(entry);
    }
}

/// エンジンごとに一意な資源名（Job Object 名・ロックファイル名）の材料を作る。
///
/// 同じエンジン種別でもポートが違えば別インスタンスなので、`url` のポート番号まで含める。
/// ポートを取り出せない `url` の場合は、`url` 全体の英数字以外を `_` に潰したものを使う
/// （Job 名・ファイル名として使えるようにするため）。
pub(crate) fn engine_resource_key(name: &str, url: &str) -> String {
    let port = url.rsplit_once(':').and_then(|(_, rest)| {
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        (!digits.is_empty()).then_some(digits)
    });
    let suffix = port.unwrap_or_else(|| sanitize_for_resource_name(url));
    format!("{}_{}", sanitize_for_resource_name(name), suffix)
}

fn sanitize_for_resource_name(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;

    /// Job 名・ロックキーはテストごとに一意にする（テストは同一プロセス内で並列実行されるため）。
    fn unique_key(tag: &str) -> String {
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        format!(
            "test_{}_{}_{}",
            tag,
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
        )
    }

    /// cmd.exe の `%~dp0` でバッチファイル自身のディレクトリを解決させることで、
    /// 日本語ユーザー名を含む一時ディレクトリでもパスのエンコード崩れを避ける。
    fn write_marker_script(dir: &std::path::Path) -> std::path::PathBuf {
        let script = dir.join("fake_engine.cmd");
        std::fs::write(&script, "@echo off\r\necho ready > \"%~dp0marker.txt\"\r\n").unwrap();
        script
    }

    /// ランチャーが孫プロセスを spawn し続けるダミースクリプト一式を書き出す。
    /// ランチャー自身は起動直後に `launcher_marker.txt` を作成し（is_alive 用の合図）、
    /// 孫プロセスは `grandchild_log.txt` に "alive" を1行ずつ追記し続ける。
    fn write_launcher_with_grandchild(dir: &std::path::Path) -> std::path::PathBuf {
        let grandchild = dir.join("grandchild.cmd");
        std::fs::write(
            &grandchild,
            "@echo off\r\n:loop\r\necho alive >> \"%~dp0grandchild_log.txt\"\r\nping -n 2 127.0.0.1 > nul\r\ngoto loop\r\n",
        )
        .unwrap();

        let launcher = dir.join("launcher.cmd");
        std::fs::write(
            &launcher,
            "@echo off\r\necho ready > \"%~dp0launcher_marker.txt\"\r\nstart \"\" /min cmd /c \"%~dp0grandchild.cmd\"\r\n:loop\r\nping -n 2 127.0.0.1 > nul\r\ngoto loop\r\n",
        )
        .unwrap();

        launcher
    }

    /// 最後の参照が解放されたとき、ランチャーだけでなく孫プロセスまで終了することを確認する。
    /// VOICEVOX / AivisSpeech の run.exe はエンジン本体を孫プロセスとして起動するため、
    /// Job Object を使わないと孫プロセスが残ってしまう。
    #[tokio::test]
    async fn releasing_last_reference_kills_grandchild_processes_via_job_object() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = write_launcher_with_grandchild(dir.path());
        let marker = dir.path().join("launcher_marker.txt");
        let log = dir.path().join("grandchild_log.txt");

        let process: Mutex<Option<EngineProcess>> = Mutex::new(None);
        let marker_for_check = marker.clone();
        ensure_running(
            "test",
            &unique_key("grandchild"),
            launcher.to_str(),
            &[],
            Duration::from_secs(30),
            &process,
            move || {
                let marker = marker_for_check.clone();
                async move { marker.exists() }
            },
        )
        .await
        .unwrap();

        // 孫プロセスがログに書き込み始めるまで待つ(最大15秒)
        let mut lines_before = 0usize;
        for _ in 0..30 {
            if let Ok(content) = std::fs::read_to_string(&log) {
                lines_before = content.lines().count();
                if lines_before > 0 {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        assert!(
            lines_before > 0,
            "孫プロセスが起動してログに書き込み始めていること"
        );

        terminate_process("test", &process);

        // 孫プロセスが書き込みを止めた(=終了した)ことを確認する
        tokio::time::sleep(Duration::from_secs(3)).await;
        let lines_after = std::fs::read_to_string(&log).unwrap().lines().count();
        assert_eq!(
            lines_after, lines_before,
            "Job Object経由でランチャーだけでなく孫プロセスも終了していること"
        );
    }

    /// 孫プロセスのログ行数が増え続けているか（=孫が生きているか）を、2回の観測で判定する。
    async fn grandchild_log_is_growing(log: &std::path::Path) -> bool {
        let before = std::fs::read_to_string(log)
            .map(|c| c.lines().count())
            .unwrap_or(0);
        tokio::time::sleep(Duration::from_secs(3)).await;
        let after = std::fs::read_to_string(log)
            .map(|c| c.lines().count())
            .unwrap_or(0);
        after > before
    }

    /// s2v-18b.1: spawn から Job 割当までの間に孫プロセスが生まれても、孫が Job の外へ逃げないこと。
    /// 割当前に意図的に待つことで競合を決定的に再現する（CREATE_SUSPENDED で起動していれば、
    /// 割当・再開までランチャーは1命令も実行しないので孫は生まれない）。
    #[test]
    fn grandchild_spawned_before_job_assignment_does_not_escape_the_job() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = write_launcher_with_grandchild(dir.path());
        let log = dir.path().join("grandchild_log.txt");
        let job = EngineJob::open_or_create(&format!(
            "Local\\Script2Voice_Engine_{}",
            unique_key("suspended")
        ))
        .unwrap();

        let (child, assigned) =
            spawn_into_job("test", launcher.to_str().unwrap(), &[], &job, || {
                std::thread::sleep(Duration::from_secs(3))
            })
            .unwrap();
        assert!(assigned, "Job に割り当てられていること");

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            // 孫プロセスがログを書き始めるまで待つ(最大15秒)
            for _ in 0..30 {
                if log.exists() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            assert!(log.exists(), "孫プロセスが起動していること");

            let process: Mutex<Option<EngineProcess>> = Mutex::new(Some(EngineProcess {
                child: Some(child),
                job,
                assigned,
            }));
            terminate_process("test", &process);
            tokio::time::sleep(Duration::from_secs(1)).await;
            assert!(
                !grandchild_log_is_growing(&log).await,
                "割当前に生まれうる孫プロセスも Job 経由で終了していること"
            );
        });
    }

    /// 長時間ログを書き続けるだけのスクリプト（子プロセス自身がループする）。
    fn write_ticker_script(dir: &std::path::Path) -> std::path::PathBuf {
        let script = dir.join("ticker.cmd");
        std::fs::write(
            &script,
            "@echo off\r\n:loop\r\necho alive >> \"%~dp0grandchild_log.txt\"\r\nping -n 2 127.0.0.1 > nul\r\ngoto loop\r\n",
        )
        .unwrap();
        script
    }

    /// s2v-18b.1: Job 割当に失敗した子プロセスは、解放時に明示的に終了させること
    /// （std::process::Child は drop しても子を終了させないため、放置すると孤児化する）。
    #[tokio::test]
    async fn release_kills_child_when_job_assignment_failed() {
        let dir = tempfile::tempdir().unwrap();
        let script = write_ticker_script(dir.path());
        let log = dir.path().join("grandchild_log.txt");
        let child = std::process::Command::new(&script)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let job = EngineJob::open_or_create(&format!(
            "Local\\Script2Voice_Engine_{}",
            unique_key("unassigned")
        ))
        .unwrap();

        for _ in 0..30 {
            if log.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        assert!(log.exists(), "子プロセスが起動していること");

        let process: Mutex<Option<EngineProcess>> = Mutex::new(Some(EngineProcess {
            child: Some(child),
            job,
            assigned: false,
        }));
        terminate_process("test", &process);
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert!(
            !grandchild_log_is_growing(&log).await,
            "Job 未割当の子プロセスは解放時に kill されること"
        );
    }

    #[tokio::test]
    async fn ensure_running_does_not_spawn_when_already_alive() {
        let process: Mutex<Option<EngineProcess>> = Mutex::new(None);
        let calls = Arc::new(AtomicUsize::new(0));
        let calls2 = Arc::clone(&calls);

        ensure_running(
            "test",
            &unique_key("alive"),
            None,
            &[],
            Duration::from_secs(30),
            &process,
            move || {
                calls2.fetch_add(1, Ordering::SeqCst);
                async { true }
            },
        )
        .await
        .unwrap();

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let guard = process.lock().unwrap();
        let entry = guard
            .as_ref()
            .expect("既存エンジンを使う場合も Job ハンドルを保持すること");
        assert!(entry.child.is_none(), "自分では spawn していないこと");
    }

    #[tokio::test]
    async fn ensure_running_errors_when_not_alive_and_no_exe_path() {
        let process: Mutex<Option<EngineProcess>> = Mutex::new(None);

        let result = ensure_running(
            "test",
            &unique_key("noexe"),
            None,
            &[],
            Duration::from_secs(30),
            &process,
            || async { false },
        )
        .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn ensure_running_errors_when_exe_path_does_not_exist() {
        let process: Mutex<Option<EngineProcess>> = Mutex::new(None);

        let result = ensure_running(
            "test",
            &unique_key("missing"),
            Some("C:/no/such/engine.exe"),
            &[],
            Duration::from_secs(30),
            &process,
            || async { false },
        )
        .await;

        assert!(result.is_err());
        assert!(process.lock().unwrap().is_none());
    }

    /// XTTS の同梱設定は `exe_path = "python"` のように PATH 上のコマンド名だけを指定し、
    /// 実引数は `args` で渡す想定。"python" は cwd 相対の実在ファイルではないため、
    /// 単純な `Path::new(path).exists()` 判定では常に「見つからない」扱いになってしまう
    /// （これが実際の同梱 config.toml の起動失敗の原因だった）。
    #[tokio::test]
    async fn ensure_running_spawns_path_resolved_command_with_args() {
        let dir = tempfile::tempdir().unwrap();
        let script = write_marker_script(dir.path());
        let marker = dir.path().join("marker.txt");
        assert!(!marker.exists());

        let process: Mutex<Option<EngineProcess>> = Mutex::new(None);
        let marker_for_check = marker.clone();
        let args = vec!["/c".to_string(), script.to_str().unwrap().to_string()];

        ensure_running(
            "test",
            &unique_key("pathcmd"),
            Some("cmd"),
            &args,
            Duration::from_secs(30),
            &process,
            move || {
                let marker = marker_for_check.clone();
                async move { marker.exists() }
            },
        )
        .await
        .unwrap();

        assert!(
            marker.exists(),
            "PATH解決コマンド('cmd')にargsを渡して起動できること"
        );
        terminate_process("test", &process);
    }

    #[tokio::test]
    async fn ensure_running_spawns_process_and_waits_until_alive() {
        let dir = tempfile::tempdir().unwrap();
        let script = write_marker_script(dir.path());
        let marker = dir.path().join("marker.txt");
        assert!(!marker.exists());

        let process: Mutex<Option<EngineProcess>> = Mutex::new(None);
        let marker_for_check = marker.clone();

        ensure_running(
            "test",
            &unique_key("spawnwait"),
            script.to_str(),
            &[],
            Duration::from_secs(30),
            &process,
            move || {
                let marker = marker_for_check.clone();
                async move { marker.exists() }
            },
        )
        .await
        .unwrap();

        assert!(
            marker.exists(),
            "起動したプロセスがマーカーファイルを作成していること"
        );
        assert!(
            process.lock().unwrap().is_some(),
            "起動したプロセスが保持されていること"
        );

        terminate_process("test", &process);
    }

    #[tokio::test]
    async fn terminate_process_kills_running_process_and_clears_handle() {
        let child = std::process::Command::new("cmd")
            .args(["/c", "ping", "-n", "60", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();

        let job = EngineJob::open_or_create(&format!(
            "Local\\Script2Voice_Engine_{}",
            unique_key("clear")
        ))
        .unwrap();
        job.assign(&child).unwrap();

        let process: Mutex<Option<EngineProcess>> = Mutex::new(Some(EngineProcess {
            child: Some(child),
            job,
            assigned: true,
        }));
        terminate_process("test", &process);

        assert!(
            process.lock().unwrap().is_none(),
            "ハンドルが解放されていること"
        );
    }

    #[test]
    fn terminate_process_is_noop_when_nothing_was_spawned() {
        let process: Mutex<Option<EngineProcess>> = Mutex::new(None);
        // パニックしないことを確認する
        terminate_process("test", &process);
        let _ = AtomicBool::new(false);
    }

    /// 起動されるたびに `count.txt` へ1行追記するダミーエンジン。
    /// 二重 spawn が起きれば行数が2以上になる。
    fn write_counting_script(dir: &std::path::Path) -> std::path::PathBuf {
        let script = dir.join("counting_engine.cmd");
        std::fs::write(
            &script,
            "@echo off\r\necho spawned >> \"%~dp0count.txt\"\r\n",
        )
        .unwrap();
        script
    }

    /// 2プロセスがほぼ同時に起動を試みても、排他により engine の spawn は1回だけであること。
    #[tokio::test]
    async fn concurrent_ensure_running_spawns_engine_only_once() {
        let dir = tempfile::tempdir().unwrap();
        let script = write_counting_script(dir.path());
        let count = dir.path().join("count.txt");
        let key = unique_key("dup");

        let p1: Mutex<Option<EngineProcess>> = Mutex::new(None);
        let p2: Mutex<Option<EngineProcess>> = Mutex::new(None);
        let c1 = count.clone();
        let c2 = count.clone();

        let a = ensure_running(
            "t1",
            &key,
            script.to_str(),
            &[],
            Duration::from_secs(30),
            &p1,
            move || {
                let c = c1.clone();
                async move { c.exists() }
            },
        );
        let b = ensure_running(
            "t2",
            &key,
            script.to_str(),
            &[],
            Duration::from_secs(30),
            &p2,
            move || {
                let c = c2.clone();
                async move { c.exists() }
            },
        );

        let (ra, rb) = tokio::join!(a, b);
        ra.unwrap();
        rb.unwrap();

        let lines = std::fs::read_to_string(&count).unwrap().lines().count();
        assert_eq!(lines, 1, "排他により spawn は1回だけであること");

        terminate_process("t1", &p1);
        terminate_process("t2", &p2);
    }

    /// ロックを取れないまま timeout した場合は、警告のうえ従来どおりの経路に進むこと
    /// （待ち続けてハングしない）。
    #[tokio::test]
    async fn ensure_running_falls_through_when_lock_is_unavailable() {
        let key = unique_key("busy");
        let _held = crate::lock::try_acquire(&key).unwrap().unwrap();

        let process: Mutex<Option<EngineProcess>> = Mutex::new(None);
        let start = std::time::Instant::now();

        ensure_running(
            "test",
            &key,
            None,
            &[],
            Duration::from_secs(1),
            &process,
            || async { true },
        )
        .await
        .unwrap();

        assert!(
            start.elapsed() >= Duration::from_secs(1),
            "ロック待ちを経てからフォールスルーすること"
        );
    }

    #[test]
    fn engine_resource_key_uses_port_from_url() {
        assert_eq!(
            engine_resource_key("voicevox", "http://127.0.0.1:50021"),
            "voicevox_50021"
        );
        assert_eq!(
            engine_resource_key("aivis", "http://127.0.0.1:10101/"),
            "aivis_10101"
        );
        assert_eq!(
            engine_resource_key("xtts", "http://127.0.0.1:8020/api"),
            "xtts_8020"
        );
    }

    #[test]
    fn engine_resource_key_differs_per_port_for_same_engine() {
        assert_ne!(
            engine_resource_key("voicevox", "http://127.0.0.1:50021"),
            engine_resource_key("voicevox", "http://127.0.0.1:50022"),
        );
    }

    #[test]
    fn engine_resource_key_falls_back_to_sanitized_url_without_port() {
        // ポートを取り出せない URL は、英数字以外を '_' に潰した URL 全体を使う。
        // Job 名・ファイル名に使うため、'/' や ':' が残っていてはいけない。
        let key = engine_resource_key("voicevox", "http://localhost");
        assert_eq!(key, "voicevox_http___localhost");
        assert!(key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
    }
}
