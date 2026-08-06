use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, State};

/// Manages all running terminal sessions (one per tab).
pub struct TerminalManager {
    inner: Mutex<Inner>,
}

struct Inner {
    sessions: HashMap<u32, Session>,
    next_id: u32,
}

struct Session {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
}

impl Default for TerminalManager {
    fn default() -> Self {
        Self {
            inner: Mutex::new(Inner {
                sessions: HashMap::new(),
                next_id: 1,
            }),
        }
    }
}

#[derive(Clone, serde::Serialize)]
struct TerminalOutput {
    id: u32,
    data: String,
}

#[derive(Clone, serde::Serialize)]
struct TerminalExit {
    id: u32,
}

fn default_shell() -> String {
    #[cfg(target_os = "windows")]
    {
        "powershell.exe".to_string()
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::env::var("SHELL").unwrap_or_else(|_| "sh".to_string())
    }
}

/// 解析终端默认工作目录:使用设置值,未设置时回退到用户主目录。
fn default_cwd(settings: &crate::shortcuts::SettingsState) -> std::path::PathBuf {
    let configured = settings
        .0
        .lock()
        .unwrap()
        .default_path
        .clone()
        .filter(|p| !p.trim().is_empty());

    if let Some(path) = configured {
        let pb = std::path::PathBuf::from(path);
        if pb.exists() && pb.is_dir() {
            return pb;
        }
    }

    if let Some(home) = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(std::path::PathBuf::from)
    {
        return home;
    }
    std::env::current_dir().unwrap_or_default()
}

/// 解析终端启动目录:优先使用传入的 cwd(须存在且为目录),
/// 否则回退到设置值 / 用户主目录。
fn resolve_cwd(settings: &crate::shortcuts::SettingsState, cwd: Option<String>) -> std::path::PathBuf {
    if let Some(path) = cwd {
        let trimmed = path.trim();
        let pb = std::path::PathBuf::from(trimmed);
        if !trimmed.is_empty() && pb.is_dir() {
            return pb;
        }
    }
    default_cwd(settings)
}

/// 读取指定进程的当前工作目录。
///
/// Windows 下通过 PEB 的 ProcessParameters.CurrentDirectory 读取,与
/// Windows Terminal 的做法一致:不向终端注入命令、不解析输出。进程已退出、
/// 权限不足或非 Windows 平台时返回 None,上层回退到默认目录。
#[cfg(target_os = "windows")]
mod win_cwd {
    use std::ffi::c_void;
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ,
    };

    const PROCESS_BASIC_INFORMATION: u32 = 0;

    #[link(name = "ntdll")]
    extern "system" {
        fn NtQueryInformationProcess(
            process_handle: HANDLE,
            process_information_class: u32,
            process_information: *mut c_void,
            process_information_length: u32,
            return_length: *mut u32,
        ) -> i32;
    }

    /// x64 PEB 中 ProcessParameters 指针的偏移。
    const PEB_PROCESS_PARAMETERS_OFFSET: usize = 0x20;

    #[repr(C)]
    struct ProcessBasicInformation {
        reserved1: *mut c_void,
        peb_base_address: *mut c_void,
        reserved2: [*mut c_void; 2],
        unique_process_id: *mut c_void,
        reserved3: *mut c_void,
    }

    #[derive(Copy, Clone)]
    #[repr(C)]
    struct UnicodeString {
        length: u16,
        maximum_length: u16,
        buffer: *mut u16,
    }

    #[derive(Copy, Clone)]
    #[repr(C)]
    struct CurDir {
        dos_path: UnicodeString,
        handle: *mut c_void,
    }

    /// 只读前 0x58 字节即可取到 CurrentDirectory.DosPath(x64 布局)。
    #[derive(Copy, Clone)]
    #[repr(C)]
    struct RtlUserProcessParameters {
        maximum_length: u32,
        length: u32,
        flags: u32,
        debug_flags: u32,
        console_handle: *mut c_void,
        console_flags: u32,
        standard_input: *mut c_void,
        standard_output: *mut c_void,
        standard_error: *mut c_void,
        current_directory: CurDir,
        dll_path: UnicodeString,
    }

    struct HandleGuard(HANDLE);

    impl Drop for HandleGuard {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    unsafe fn read_memory<T: Copy>(handle: HANDLE, address: *const c_void) -> Option<T> {
        let mut value: T = std::mem::zeroed();
        let size = std::mem::size_of::<T>();
        let mut read = 0usize;
        ReadProcessMemory(
            handle,
            address,
            (&mut value as *mut T).cast(),
            size,
            Some(&mut read),
        )
        .ok()?;
        (read == size).then_some(value)
    }

    /// 查询指定进程的当前工作目录(带盘符的绝对路径)。
    pub fn process_cwd(pid: u32) -> Option<String> {
        unsafe {
            let handle = OpenProcess(
                PROCESS_QUERY_INFORMATION | PROCESS_VM_READ,
                false,
                pid,
            )
            .ok()?;
            let _guard = HandleGuard(handle);

            let mut pbi: ProcessBasicInformation = std::mem::zeroed();
            let status = NtQueryInformationProcess(
                handle,
                PROCESS_BASIC_INFORMATION,
                (&mut pbi as *mut ProcessBasicInformation).cast(),
                std::mem::size_of::<ProcessBasicInformation>() as u32,
                std::ptr::null_mut(),
            );
            if status != 0 || pbi.peb_base_address.is_null() {
                return None;
            }

            let params_ptr: *mut c_void =
                read_memory(handle, (pbi.peb_base_address as usize + PEB_PROCESS_PARAMETERS_OFFSET) as *const c_void)?;
            if params_ptr.is_null() {
                return None;
            }

            let params: RtlUserProcessParameters = read_memory(handle, params_ptr)?;
            let dos_path = params.current_directory.dos_path;
            let len = dos_path.length as usize;
            if dos_path.buffer.is_null() || len == 0 || len % 2 != 0 {
                return None;
            }

            let mut buf = vec![0u16; len / 2];
            let mut read = 0usize;
            ReadProcessMemory(
                handle,
                dos_path.buffer.cast(),
                buf.as_mut_ptr().cast(),
                len,
                Some(&mut read),
            )
            .ok()?;
            if read != len {
                return None;
            }

            let cwd = String::from_utf16(&buf).ok()?;
            if cwd.is_empty() {
                None
            } else {
                Some(cwd)
            }
        }
    }
}

#[cfg(not(target_os = "windows"))]
mod win_cwd {
    pub fn process_cwd(_pid: u32) -> Option<String> {
        None
    }
}

#[tauri::command]
pub fn create_terminal(
    app: AppHandle,
    state: State<'_, TerminalManager>,
    settings: State<'_, crate::shortcuts::SettingsState>,
    cols: u16,
    rows: u16,
    cwd: Option<String>,
) -> Result<u32, String> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("打开 PTY 失败: {e}"))?;

    let mut cmd = CommandBuilder::new(default_shell());
    cmd.cwd(resolve_cwd(&settings, cwd));
    let child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| format!("启动 shell 失败: {e}"))?;
    drop(pair.slave);

    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| format!("获取读取端失败: {e}"))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|e| format!("获取写入端失败: {e}"))?;

    let id = {
        let mut inner = state.inner.lock().unwrap();
        let id = inner.next_id;
        inner.next_id += 1;
        inner.sessions.insert(
            id,
            Session {
                master: pair.master,
                writer,
                child,
            },
        );
        id
    };

    // 读取线程:持续把 shell 输出转发给前端,进程退出后清理会话
    let app_handle = app.clone();
    let exit_app = app.clone();
    std::thread::spawn(move || {
        let mut pending: Vec<u8> = Vec::new();
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break, // EOF: 进程已退出
                Ok(n) => {
                    pending.extend_from_slice(&buf[..n]);
                    // 只发送完整的 UTF-8 序列,避免跨包截断产生乱码
                    match std::str::from_utf8(&pending) {
                        Ok(s) => {
                            let _ = app_handle.emit(
                                "terminal-output",
                                TerminalOutput {
                                    id,
                                    data: s.to_string(),
                                },
                            );
                            pending.clear();
                        }
                        Err(e) => {
                            let valid = e.valid_up_to();
                            if valid > 0 {
                                let s = std::str::from_utf8(&pending[..valid]).unwrap();
                                let _ = app_handle.emit(
                                    "terminal-output",
                                    TerminalOutput {
                                        id,
                                        data: s.to_string(),
                                    },
                                );
                                pending.drain(..valid);
                            }
                            // error_len() == None 表示不完整的多字节序列,等下一包
                        }
                    }
                }
                Err(_) => break,
            }
        }
        let _ = exit_app.emit("terminal-exit", TerminalExit { id });
        if let Some(mgr) = exit_app.try_state::<TerminalManager>() {
            mgr.inner.lock().unwrap().sessions.remove(&id);
        }
    });

    Ok(id)
}

/// 查询指定终端会话当前的工作目录。
/// 用于"双击标签以相同路径新建终端"。读取失败时返回 None,
/// 前端回退到默认目录。
#[tauri::command]
pub fn get_terminal_cwd(state: State<'_, TerminalManager>, id: u32) -> Option<String> {
    let inner = state.inner.lock().unwrap();
    let session = inner.sessions.get(&id)?;
    let pid = session.child.process_id()?;
    win_cwd::process_cwd(pid)
}

#[tauri::command]
pub fn write_terminal(
    state: State<'_, TerminalManager>,
    id: u32,
    data: String,
) -> Result<(), String> {
    let mut inner = state.inner.lock().unwrap();
    let session = inner
        .sessions
        .get_mut(&id)
        .ok_or_else(|| "终端不存在".to_string())?;
    session
        .writer
        .write_all(data.as_bytes())
        .map_err(|e| format!("写入失败: {e}"))?;
    session
        .writer
        .flush()
        .map_err(|e| format!("写入失败: {e}"))
}

#[tauri::command]
pub fn resize_terminal(
    state: State<'_, TerminalManager>,
    id: u32,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let inner = state.inner.lock().unwrap();
    let session = inner
        .sessions
        .get(&id)
        .ok_or_else(|| "终端不存在".to_string())?;
    session
        .master
        .resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("调整尺寸失败: {e}"))
}

#[tauri::command]
pub fn kill_terminal(state: State<'_, TerminalManager>, id: u32) {
    let mut inner = state.inner.lock().unwrap();
    if let Some(mut session) = inner.sessions.remove(&id) {
        let _ = session.child.kill();
    }
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;

    /// 真实环境验证:能通过 PEB 读取其他进程的当前工作目录。
    /// 这是"双击标签以相同路径新建终端"的核心依赖。
    #[test]
    fn reads_other_process_cwd() {
        let dir = std::env::temp_dir().join(format!("oops-term-cwd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // 以指定目录启动一个短暂存活的 PowerShell 进程
        let mut child = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-Command", "Start-Sleep -Seconds 10"])
            .current_dir(&dir)
            .spawn()
            .expect("spawn powershell");

        // 等待进程完成初始化,确保 PEB 的进程参数已就绪
        std::thread::sleep(std::time::Duration::from_millis(500));

        let cwd = win_cwd::process_cwd(child.id());
        let _ = child.kill();
        let _ = child.wait();

        let _ = std::fs::remove_dir_all(&dir);

        let expected = std::fs::canonicalize(&dir).unwrap_or(dir);
        let expected_str = expected.to_string_lossy().to_string();
        match cwd {
            Some(actual) => {
                let actual_trimmed = actual.trim_end_matches('\\');
                assert!(
                    actual_trimmed.eq_ignore_ascii_case(&expected_str),
                    "cwd mismatch: actual={actual} expected={expected_str}"
                );
            }
            None => panic!("process_cwd 未能读取进程工作目录"),
        }
    }
}
