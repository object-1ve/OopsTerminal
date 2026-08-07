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
    cwd: std::path::PathBuf,
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

/// 去掉 ANSI 转义序列,得到纯文本输出。
///
/// 只处理 CSI(`ESC [ ... final byte`)与 OSC(`ESC ] ... BEL|ESC \`),
/// 其他 ESC 前缀(如 `ESC 7` / `ESC 8`)直接丢弃 ESC 本身。
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('[') => {
                chars.next();
                // 消费到 CSI 的最终字节(@ ~ ~ 范围)
                for c2 in chars.by_ref() {
                    if ('\x40'..='\x7e').contains(&c2) {
                        break;
                    }
                }
            }
            Some(']') => {
                chars.next();
                // 消费到 BEL(0x07)或 ESC \(ST)
                loop {
                    match chars.next() {
                        Some('\x07') => break,
                        Some('\x1b') => {
                            if chars.next() == Some('\\') {
                                break;
                            }
                        }
                        Some(_) => continue,
                        None => break,
                    }
                }
            }
            _ => { /* 其他 ESC 序列,丢弃 ESC 继续 */ }
        }
    }
    out
}

/// 判断一段文本是否形如文件系统路径(盘符路径或 UNC 路径)。
fn looks_like_path(text: &str) -> bool {
    let b = text.as_bytes();
    (b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/'))
        || (b.len() >= 2 && b[0] == b'\\' && b[1] == b'\\')
}

/// 从一行提示符文本解析出 PowerShell 当前目录。
///
/// 默认提示符形如 `PS C:\path> `(嵌套时 `>> `)。只识别文件系统路径;
/// 位于其他 provider(如 `PS Env:> `)或用户自定义提示符时返回 None。
fn parse_prompt_line(line: &str) -> Option<String> {
    let body = line
        .strip_prefix("PS ")
        .or_else(|| line.strip_prefix("PS\t"))?;
    let trimmed = body.trim_end_matches(' ');
    let t = trimmed.as_bytes();
    // 找到结尾的箭头串(> 或 >>),箭头之前是路径
    let mut j = t.len();
    while j > 0 && t[j - 1] == b'>' {
        j -= 1;
    }
    if j == t.len() {
        return None; // 没有箭头,不是提示符
    }
    let path = &trimmed[..j];
    let path = path.trim_end_matches(' ').trim_end_matches('\r');
    if path.is_empty() || !looks_like_path(path) {
        return None;
    }
    Some(path.to_string())
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

    let start_cwd = resolve_cwd(&settings, cwd);

    let mut cmd = CommandBuilder::new(default_shell());
    cmd.cwd(&start_cwd);
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
                cwd: start_cwd,
            },
        );
        id
    };

    // 读取线程:持续把 shell 输出转发给前端,同时从回显中同步当前目录。
    let app_handle = app.clone();
    let exit_app = app.clone();
    std::thread::spawn(move || {
        let mut pending: Vec<u8> = Vec::new();
        let mut buf = [0u8; 8192];
        let mut tracker = OutputTracker::new();
        // 处理一段已解码输出:同步 cwd 并转发给前端。
        let handle_output = |decoded: &str, tracker: &mut OutputTracker| {
            if let Some(cwd) = tracker.push(decoded) {
                if let Some(mgr) = exit_app.try_state::<TerminalManager>() {
                    if let Some(session) = mgr.inner.lock().unwrap().sessions.get_mut(&id) {
                        session.cwd = std::path::PathBuf::from(&cwd);
                    }
                }
            }
            let _ = app_handle.emit(
                "terminal-output",
                TerminalOutput {
                    id,
                    data: decoded.to_string(),
                },
            );
        };
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break, // EOF: 进程已退出
                Ok(n) => {
                    pending.extend_from_slice(&buf[..n]);
                    // 只发送完整的 UTF-8 序列,避免跨包截断产生乱码
                    match std::str::from_utf8(&pending) {
                        Ok(s) => {
                            handle_output(s, &mut tracker);
                            pending.clear();
                        }
                        Err(e) => {
                            let valid = e.valid_up_to();
                            if valid > 0 {
                                let s = std::str::from_utf8(&pending[..valid]).unwrap();
                                handle_output(s, &mut tracker);
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

/// 输出跟踪器:按行消费 shell 输出,识别提示符目录。
///
/// 只用于同步当前工作目录,不记录或识别用户输入。
struct OutputTracker {
    /// 已解码输出累积,跨块拼出完整行。
    tail: String,
    /// tail 中已消费到的字节位置(逐行推进,避免重复处理)。
    scanned: usize,
    /// 最近一次提示符解析出的目录(含空命令的新提示符)。
    cwd: Option<String>,
}

fn floor_char_boundary(text: &str, mut index: usize) -> usize {
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

impl OutputTracker {
    fn new() -> Self {
        OutputTracker {
            tail: String::new(),
            scanned: 0,
            cwd: None,
        }
    }

    /// 并入一段输出,返回当前提示符目录。
    fn push(&mut self, decoded: &str) -> Option<String> {
        const MAX_TAIL: usize = 8192;
        self.tail.push_str(decoded);
        loop {
            let rest = &self.tail[self.scanned..];
            let Some(i) = rest.find('\n') else { break };
            let line = &rest[..i];
            self.scanned += i + 1;
            let stripped = strip_ansi(line).trim_end_matches('\r').to_string();
            // 任何 `PS <路径>> ` 提示符行(含空命令)都刷新目录。
            if let Some(path) = parse_prompt_line(&stripped) {
                self.cwd = Some(path);
            }
        }
        if self.tail.len() > MAX_TAIL {
            // 裁掉旧字节时不能落在多字节 UTF-8 字符中间。
            let cut = floor_char_boundary(&self.tail, self.tail.len() - MAX_TAIL);
            self.tail.drain(..cut);
            self.scanned = self.scanned.saturating_sub(cut);
        }
        self.cwd.clone()
    }
}

/// 查询指定终端会话当前的工作目录。
/// 用于"双击标签以相同路径新建终端"。读取失败时返回 None,
/// 前端回退到默认目录。
#[tauri::command]
pub fn get_terminal_cwd(state: State<'_, TerminalManager>, id: u32) -> Option<String> {
    let inner = state.inner.lock().unwrap();
    let session = inner.sessions.get(&id)?;
    Some(session.cwd.to_string_lossy().to_string())
}

#[tauri::command]
pub fn write_terminal(
    state: State<'_, TerminalManager>,
    id: u32,
    data: String,
) -> Result<(), String> {
    // 写入 PTY 只是转发用户输入,不做任何记录。
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
        .map_err(|e| format!("写入失败: {e}"))?;
    Ok(())
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

    fn make_settings(default_path: Option<String>) -> crate::shortcuts::SettingsState {
        crate::shortcuts::SettingsState(std::sync::Mutex::new(crate::db::Settings {
            default_path,
            show_tray_icon: true,
            show_taskbar_icon: false,
            terminal_font_path: None,
        }))
    }

    fn unique_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("oops-term-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 规范化路径:canonicalize 展开短名 / 8.3 名,再去掉结尾反斜杠,便于比较。
    fn normalized(p: &std::path::Path) -> String {
        std::fs::canonicalize(p)
            .unwrap_or_else(|_| p.to_path_buf())
            .to_string_lossy()
            .trim_end_matches('\\')
            .to_string()
    }

    // ---------- OutputTracker 单元测试 ----------

    #[test]
    fn parses_default_prompt() {
        let mut t = OutputTracker::new();
        assert_eq!(
            t.push("PS C:\\Users\\me\\proj> \r\n"),
            Some("C:\\Users\\me\\proj".into())
        );
    }

    #[test]
    fn parses_prompt_with_ansi_colors() {
        // PSReadLine 可能给路径上色:PS <ESC>[36mC:\path<ESC>[0m>
        let mut t = OutputTracker::new();
        let s = "\x1b[4;1H\x1b[?25hPS \x1b[36mC:\\Users\\me\\proj\x1b[0m> \r\n";
        assert_eq!(t.push(s), Some("C:\\Users\\me\\proj".into()));
    }

    #[test]
    fn parses_nested_prompt() {
        let mut t = OutputTracker::new();
        assert_eq!(t.push("PS C:\\a\\b>> \r\n"), Some("C:\\a\\b".into()));
    }

    #[test]
    fn parses_unc_prompt() {
        let mut t = OutputTracker::new();
        assert_eq!(
            t.push("PS \\\\server\\share\\dir> \r\n"),
            Some("\\\\server\\share\\dir".into())
        );
    }

    #[test]
    fn parses_path_with_spaces() {
        let mut t = OutputTracker::new();
        assert_eq!(
            t.push("PS C:\\Program Files\\Some App> \r\n"),
            Some("C:\\Program Files\\Some App".into())
        );
    }

    #[test]
    fn ignores_non_filesystem_provider_prompt() {
        let mut t = OutputTracker::new();
        assert_eq!(t.push("PS Env:> \r\n"), None);
        assert_eq!(t.push("PS HKLM:\\Software> \r\n"), None);
    }

    #[test]
    fn ignores_custom_prompt() {
        let mut t = OutputTracker::new();
        assert_eq!(t.push("PS> \r\n"), None);
        assert_eq!(t.push("user@host:~$ \r\n"), None);
    }

    #[test]
    fn picks_latest_prompt() {
        let mut t = OutputTracker::new();
        assert_eq!(
            t.push("PS C:\\old> \r\nPS C:\\new> \r\n"),
            Some("C:\\new".into())
        );
    }

    #[test]
    fn strips_osc_title_sequence() {
        let mut t = OutputTracker::new();
        assert_eq!(
            t.push("\x1b]0;powershell.exe\x07PS C:\\x> \r\n"),
            Some("C:\\x".into())
        );
    }

    #[test]
    fn parses_root_prompt() {
        let mut t = OutputTracker::new();
        assert_eq!(t.push("PS C:\\> \r\n"), Some("C:\\".into()));
    }

    #[test]
    fn tracks_cwd_across_chunks() {
        let mut t = OutputTracker::new();
        assert_eq!(t.push("PS C:\\proj> ech"), None);
        assert_eq!(
            t.push("o hi\r\nPS C:\\new> \r\n"),
            Some("C:\\new".into())
        );
    }

    #[test]
    fn tracks_cwd_but_not_input() {
        let mut t = OutputTracker::new();
        assert_eq!(t.push("PS C:\\proj> echo hi\r\nhi\r\n"), None);
        assert_eq!(t.push("PS C:\\proj> \r\n"), Some("C:\\proj".into()));
    }

    #[test]
    fn keeps_last_cwd_without_new_prompt() {
        let mut t = OutputTracker::new();
        assert_eq!(t.push("PS C:\\proj> \r\n"), Some("C:\\proj".into()));
        assert_eq!(t.push("echo hi\r\nhi\r\n"), Some("C:\\proj".into()));
        assert_eq!(t.push("PS C:\\new> \r\n"), Some("C:\\new".into()));
    }

    #[test]
    fn trims_large_tail_without_splitting_utf8_characters() {
        let mut t = OutputTracker::new();
        // 让 8192 字节裁剪点落在一个中文字符中间,确保不会触发 drain 断言。
        let long = format!("{}你好{}", "a".repeat(8190), "a".repeat(8190));
        t.push(&long);
        assert!(t.tail.len() <= 8194);
    }

    /// resolve_cwd:显式 cwd 优先;无效 / 空白 / 缺失时回退默认路径。
    #[test]
    fn resolve_cwd_override_then_fallback() {
        let over = unique_dir("ovr");
        let def = unique_dir("def");
        let settings = make_settings(Some(def.to_string_lossy().to_string()));
        let over_s = over.to_string_lossy().to_string();

        // 显式目录有效时优先
        assert_eq!(
            normalized(&resolve_cwd(&settings, Some(over_s.clone()))),
            normalized(&over)
        );
        // 无效目录回退默认
        assert_eq!(
            normalized(&resolve_cwd(&settings, Some(r"Z:\oops\missing\dir".into()))),
            normalized(&def)
        );
        // 空白字符串回退默认
        assert_eq!(
            normalized(&resolve_cwd(&settings, Some("   ".into()))),
            normalized(&def)
        );
        // 未传回退默认
        assert_eq!(normalized(&resolve_cwd(&settings, None)), normalized(&def));

        let _ = std::fs::remove_dir_all(&over);
        let _ = std::fs::remove_dir_all(&def);
    }
}
