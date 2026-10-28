//! Windows Job Object の RAII ラッパー。
//!
//! `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` を設定した名前付き Job にエンジンプロセスを割り当て、
//! そのエンジンを使う全プロセスが同名 Job のハンドルを保持する。
//! ハンドルが1つでも残っている間はエンジンが生き続け、最後の1つが閉じられた瞬間に
//! OS がエンジンツリー全体を終了させる。正常終了・クラッシュ・強制終了のいずれでも動く。

use std::io;
use std::os::windows::io::AsRawHandle;
use std::process::Child;

use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};

pub(crate) struct EngineJob {
    handle: HANDLE,
}

// SAFETY: HANDLE はカーネルオブジェクトへの不透明なポインタであり、
// 対応する Win32 API（CreateJobObjectW/AssignProcessToJobObject 等）は
// どのスレッドから呼んでもよい。
unsafe impl Send for EngineJob {}
unsafe impl Sync for EngineJob {}

impl EngineJob {
    /// 名前付き Job Object を開く（存在しなければ作成する）。
    ///
    /// 同じ名前で開いたハンドルはプロセスを跨いで同じ Job を指す。エンジンを共有する
    /// 全プロセスがハンドルを保持することで、「そのエンジンを使っている最後のプロセスが
    /// 終了した瞬間に、OS が `KILL_ON_JOB_CLOSE` でエンジンツリーを終了させる」という意味になる。
    pub(crate) fn open_or_create(name: &str) -> io::Result<Self> {
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), wide.as_ptr()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }

        // 既存の Job を開いた場合は作成者が設定済みなので触らない。
        // 新規作成時だけ KILL_ON_JOB_CLOSE を設定する。
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            return Ok(Self { handle });
        }

        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;

        let ok = unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if ok == 0 {
            let err = io::Error::last_os_error();
            unsafe { CloseHandle(handle) };
            return Err(err);
        }

        Ok(Self { handle })
    }

    /// 指定したプロセスをこの Job に割り当てる。
    /// 以後そのプロセスが起動する子プロセス（孫プロセス）も同じ Job に属する。
    pub(crate) fn assign(&self, child: &Child) -> io::Result<()> {
        let process_handle = child.as_raw_handle() as HANDLE;
        let ok = unsafe { AssignProcessToJobObject(self.handle, process_handle) };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

/// `CREATE_SUSPENDED` で起動したプロセスのスレッドを再開させる。
///
/// std の `Child` はメインスレッドのハンドルを公開しないため、Toolhelp のスレッド一覧から
/// 対象プロセスに属するスレッドを探して `ResumeThread` する。サスペンド起動直後のプロセスは
/// メインスレッドしか持たない。1つも再開できなければエラーを返す。
pub(crate) fn resume_suspended_process(pid: u32) -> io::Result<()> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }

    let mut entry = THREADENTRY32 {
        dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };
    let mut resumed = 0usize;
    let mut last_err: Option<io::Error> = None;
    let mut ok = unsafe { Thread32First(snapshot, &mut entry) };
    while ok != 0 {
        if entry.th32OwnerProcessID == pid {
            let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
            if thread.is_null() {
                last_err = Some(io::Error::last_os_error());
            } else {
                if unsafe { ResumeThread(thread) } == u32::MAX {
                    last_err = Some(io::Error::last_os_error());
                } else {
                    resumed += 1;
                }
                unsafe { CloseHandle(thread) };
            }
        }
        ok = unsafe { Thread32Next(snapshot, &mut entry) };
    }
    unsafe { CloseHandle(snapshot) };

    if resumed == 0 {
        return Err(last_err.unwrap_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("プロセス {pid} のスレッドが見つかりません"),
            )
        }));
    }
    Ok(())
}

impl Drop for EngineJob {
    fn drop(&mut self) {
        // ハンドルを閉じる。自分が最後の保持者なら、KILL_ON_JOB_CLOSE により
        // OS が Job 配下のプロセス(ランチャー＋孫プロセス)を自動的に終了する。
        // 他の Script2Voice プロセスがまだ同じ Job を保持していればエンジンは生き残る。
        unsafe { CloseHandle(self.handle) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn spawn_long_running() -> Child {
        std::process::Command::new("cmd")
            .args(["/c", "ping", "-n", "60", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap()
    }

    /// Rust のテストは同一プロセス内のスレッドで並列実行されるため、
    /// Job 名が固定だと別テスト同士が同じ Job を共有してしまう。テストごとに一意にする。
    fn unique_job_name(tag: &str) -> String {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        format!(
            "Local\\s2v_test_{}_{}_{}",
            tag,
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
        )
    }

    /// 今回のバグ(先に終了したプロセスが共有エンジンを殺す)に対応する回帰テスト。
    /// 同名 Job のハンドルを2つ開くことで、2プロセスが共有している状況を同一プロセス内で再現する。
    #[test]
    fn assigned_process_survives_until_last_handle_is_dropped() {
        let name = unique_job_name("shared");
        let mut child = spawn_long_running();

        let first = EngineJob::open_or_create(&name).unwrap();
        first.assign(&child).unwrap();
        let second = EngineJob::open_or_create(&name).unwrap();

        drop(first);
        std::thread::sleep(Duration::from_millis(500));
        assert!(
            child.try_wait().unwrap().is_none(),
            "他プロセス相当のハンドルが残っている間はエンジンが生き続けること"
        );

        drop(second);
        std::thread::sleep(Duration::from_millis(500));
        assert!(
            child.try_wait().unwrap().is_some(),
            "最後のハンドルが閉じた時点でエンジンが終了すること"
        );
    }

    #[test]
    fn dropping_job_terminates_assigned_process_via_kill_on_close() {
        let mut child = spawn_long_running();
        assert!(
            child.try_wait().unwrap().is_none(),
            "プロセスが起動していること"
        );

        {
            let job = EngineJob::open_or_create(&unique_job_name("drop")).unwrap();
            job.assign(&child).unwrap();
            // 明示的な後始末をせずに job をドロップする(クラッシュ相当の状況を模す)
        }

        std::thread::sleep(Duration::from_millis(300));
        assert!(
            child.try_wait().unwrap().is_some(),
            "Jobハンドルのクローズで自動終了していること(KILL_ON_JOB_CLOSE)"
        );
    }
}
