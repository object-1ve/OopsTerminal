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

#[tauri::command]
pub fn create_terminal(
    app: AppHandle,
    state: State<'_, TerminalManager>,
) -> Result<u32, String> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 30,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("打开 PTY 失败: {e}"))?;

    let cmd = CommandBuilder::new(default_shell());
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
