use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, State};

/// Manages all running terminal sessions (one per tab).
pub struct TerminalManager {
    inner: Mutex<Inner>,
    /// 用户输入日志文件路径(JSONL)。None 表示尚未初始化。
    log_path: Mutex<Option<PathBuf>>,
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
            log_path: Mutex::new(None),
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

/// 从终端输出文本中解析出最近一次 PowerShell 提示符里的当前目录。
///
/// 先去掉 ANSI 序列(PSReadLine 可能给路径上色),再按行反向匹配
/// `PS <路径>> `。找不到(自定义提示符等)返回 None,调用方回退默认目录。
///
/// 运行时已被 `OutputTracker` 逐行解析取代,保留用于单元测试覆盖提示符解析。
#[cfg_attr(not(test), allow(dead_code))]
fn parse_prompt_cwd(text: &str) -> Option<String> {
    let stripped = strip_ansi(text);
    for line in stripped.lines().rev() {
        if let Some(path) = parse_prompt_line(line) {
            return Some(path);
        }
    }
    None
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

    // 初始化用户输入日志文件路径(与应用数据目录同处,只初始化一次)
    {
        let mut log_guard = state.log_path.lock().unwrap();
        if log_guard.is_none() {
            if let Ok(dir) = app.path().app_data_dir() {
                *log_guard = Some(dir.join("input_log.jsonl"));
            }
        }
    }

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

    // 读取线程:持续把 shell 输出转发给前端,进程退出后清理会话
    // 输入记录在此实现:解析输出回显中的 `PS <路径>> <命令>` 行,
    // 识别用户已提交的命令并写入日志(完全不监听键盘输入)。
    let app_handle = app.clone();
    let exit_app = app.clone();
    let log_path = state.log_path.lock().unwrap().clone();
    std::thread::spawn(move || {
        let mut pending: Vec<u8> = Vec::new();
        let mut buf = [0u8; 8192];
        let mut tracker = OutputTracker::new();
        // 处理一段已解码输出:识别命令写日志、同步 cwd、转发给前端
        let handle_output = |decoded: &str, tracker: &mut OutputTracker| {
            for (cmd_cwd, cmd) in tracker.push(decoded) {
                if let Some(lp) = &log_path {
                    if let Err(e) = append_input_log(lp, id, &cmd, &cmd_cwd) {
                        log::warn!("写入输入日志失败 ({}): {e}", lp.display());
                    }
                }
            }
            if let Some(cwd) = tracker.cwd.clone() {
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

/// 从一行提示符文本提取用户已提交的命令。
///
/// 匹配 PSReadLine 的回显格式 `PS <路径>> <命令>`。
/// 命令为空或非提示符行返回 None。返回 (命令执行时的目录, 命令)。
fn parse_prompt_command(line: &str) -> Option<(String, String)> {
    let body = line.strip_prefix("PS ")?.trim_end();
    // 第一个 `>` 是提示符箭头;命令里后续的 `>`(如重定向)不受影响
    let arrow = body.find('>')?;
    let path = body[..arrow].trim_end();
    let cmd = body[arrow + 1..].trim();
    if !looks_like_path(path) || cmd.is_empty() {
        return None;
    }
    Some((path.to_string(), cmd.to_string()))
}

/// 输出跟踪器:按行消费 shell 输出,识别提示符目录与用户提交的命令。
///
/// 输入记录完全在输出侧实现,不监听键盘:PowerShell(PSReadLine)在用户
/// 按下回车后会输出 `PS <路径>> <命令>` 回显行,解析该行即得到已提交的
/// 最终命令文本 —— 退格、方向键、IME 组合等编辑过程天然不可见。
struct OutputTracker {
    /// 已解码输出累积,跨块拼出完整行。
    tail: String,
    /// tail 中已消费到的字节位置(逐行推进,避免重复处理)。
    scanned: usize,
    /// 最近一次提示符解析出的目录(含空命令的新提示符)。
    cwd: Option<String>,
    /// 当前正在累积的命令(多行输入由 `>> ` 续行扩展)。
    pending: Option<PendingCmd>,
}

/// 一条正在累积的待提交命令,附带其执行时的目录。
struct PendingCmd {
    path: String,
    cmd: String,
}

impl OutputTracker {
    fn new() -> Self {
        OutputTracker {
            tail: String::new(),
            scanned: 0,
            cwd: None,
            pending: None,
        }
    }

    /// 并入一段输出,返回新识别出的 (目录, 命令) 列表。
    fn push(&mut self, decoded: &str) -> Vec<(String, String)> {
        const MAX_TAIL: usize = 8192;
        self.tail.push_str(decoded);
        let mut recorded = Vec::new();
        loop {
            let rest = &self.tail[self.scanned..];
            let Some(i) = rest.find('\n') else { break };
            let line = &rest[..i];
            self.scanned += i + 1;
            let stripped = strip_ansi(line).trim_end_matches('\r').to_string();
            // 目录:任何 `PS <路径>> ` 提示符行(含空命令)都刷新目录
            if let Some(path) = parse_prompt_line(&stripped) {
                self.cwd = Some(path);
            }
            // 命令:回显行开始新命令,`>> ` 续行扩展,其他行结束当前命令
            if let Some((path, cmd)) = parse_prompt_command(&stripped) {
                self.pending = Some(PendingCmd { path, cmd });
            } else if stripped.starts_with(">> ") {
                if let Some(p) = self.pending.as_mut() {
                    p.cmd.push('\n');
                    p.cmd.push_str(&stripped[3..]);
                }
            } else if let Some(p) = self.pending.take() {
                recorded.push((p.path, p.cmd));
            }
        }
        if self.tail.len() > MAX_TAIL {
            let cut = self.tail.len() - MAX_TAIL;
            self.tail.drain(..cut);
            self.scanned = self.scanned.saturating_sub(cut);
        }
        recorded
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

/// 把一次用户输入追加到 JSONL 日志文件。
///
fn append_input_log(path: &std::path::Path, id: u32, content: &str, cwd: &str) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    let line = serde_json::json!({
        "time": chrono::Local::now().to_rfc3339(),
        "id": id,
        "content": content,
        "cwd": cwd,
    });
    writeln!(file, "{}", line)?;
    file.flush()
}

/// 输入日志的读取结果:文件路径 + 解析后的记录(最新在前)。
#[derive(serde::Serialize)]
pub struct InputLogData {
    /// 日志文件绝对路径。
    path: String,
    /// 解析后的记录,按时间倒序(最新在前)。
    entries: Vec<InputLogEntry>,
}

#[derive(serde::Serialize)]
pub struct InputLogEntry {
    time: String,
    id: u32,
    content: String,
    /// 终端当前工作目录,用于日志展示。
    cwd: Option<String>,
}

/// 读取输入日志文件,返回解析后的记录(最新在前)。
///
/// 损坏的行会被跳过;只返回最近 MAX 条,避免日志过大时拖慢界面。
#[tauri::command]
pub fn read_input_log(
    app: AppHandle,
    state: State<'_, TerminalManager>,
) -> Result<InputLogData, String> {
    const MAX_ENTRIES: usize = 2000;

    let path = state
        .log_path
        .lock()
        .unwrap()
        .clone()
        .or_else(|| app.path().app_data_dir().ok().map(|d| d.join("input_log.jsonl")))
        .ok_or_else(|| "无法确定日志文件位置".to_string())?;

    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(InputLogData {
            path: path.to_string_lossy().to_string(),
            entries: Vec::new(),
        }),
        Err(e) => return Err(format!("读取日志失败: {e}")),
    };

    let mut entries: Vec<InputLogEntry> = text
        .lines()
        .filter_map(|line| {
            let v: serde_json::Value = serde_json::from_str(line).ok()?;
            Some(InputLogEntry {
                time: v.get("time")?.as_str()?.to_string(),
                id: v.get("id")?.as_u64().map(|x| x as u32)?,
                content: v.get("content")?.as_str()?.to_string(),
                cwd: v.get("cwd").and_then(|v| v.as_str()).map(String::from),
            })
        })
        .rev()
        .take(MAX_ENTRIES)
        .collect();

    // 确保至少返回文件存在与否的信息;空文件返回空列表
    entries.shrink_to_fit();

    Ok(InputLogData {
        path: path.to_string_lossy().to_string(),
        entries,
    })
}

#[tauri::command]
pub fn write_terminal(
    state: State<'_, TerminalManager>,
    id: u32,
    data: String,
) -> Result<(), String> {
    // 输入记录不在此处实现:写入 PTY 只是转发用户输入,
    // 日志改由读取线程从 shell 输出回显中识别已提交的命令(见 OutputTracker)。
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
            toggle_window_shortcut: None,
            quit_shortcut: None,
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

    // ---------- parse_prompt_cwd 单元测试 ----------

    #[test]
    fn parses_default_prompt() {
        assert_eq!(
            parse_prompt_cwd("PS C:\\Users\\me\\proj> "),
            Some("C:\\Users\\me\\proj".into())
        );
    }

    #[test]
    fn parses_prompt_with_ansi_colors() {
        // PSReadLine 可能给路径上色:PS <ESC>[36mC:\path<ESC>[0m>
        let s = "\x1b[4;1H\x1b[?25hPS \x1b[36mC:\\Users\\me\\proj\x1b[0m> ";
        assert_eq!(parse_prompt_cwd(s), Some("C:\\Users\\me\\proj".into()));
    }

    #[test]
    fn parses_nested_prompt() {
        assert_eq!(parse_prompt_cwd("PS C:\\a\\b>> "), Some("C:\\a\\b".into()));
    }

    #[test]
    fn parses_unc_prompt() {
        assert_eq!(
            parse_prompt_cwd("PS \\\\server\\share\\dir> "),
            Some("\\\\server\\share\\dir".into())
        );
    }

    #[test]
    fn parses_path_with_spaces() {
        assert_eq!(
            parse_prompt_cwd("PS C:\\Program Files\\Some App> "),
            Some("C:\\Program Files\\Some App".into())
        );
    }

    #[test]
    fn ignores_non_filesystem_provider_prompt() {
        assert_eq!(parse_prompt_cwd("PS Env:> "), None);
        assert_eq!(parse_prompt_cwd("PS HKLM:\\Software> "), None);
    }

    #[test]
    fn ignores_custom_prompt() {
        assert_eq!(parse_prompt_cwd("PS> "), None);
        assert_eq!(parse_prompt_cwd("user@host:~$ "), None);
    }

    #[test]
    fn picks_latest_prompt() {
        let s = "PS C:\\old> \r\nPS C:\\new> ";
        assert_eq!(parse_prompt_cwd(s), Some("C:\\new".into()));
    }

    #[test]
    fn strips_osc_title_sequence() {
        let s = "\x1b]0;powershell.exe\x07PS C:\\x> ";
        assert_eq!(parse_prompt_cwd(s), Some("C:\\x".into()));
    }

    #[test]
    fn parses_root_prompt() {
        assert_eq!(parse_prompt_cwd("PS C:\\> "), Some("C:\\".into()));
    }

    /// 输入日志:每行都是合法 JSON,包含本地时间与输入内容。
    #[test]
    fn input_log_writes_valid_jsonl() {
        let dir = unique_dir("log");
        let path = dir.join("input_log.jsonl");

        append_input_log(&path, 7, "cd C:\\proj\r", "C:\\").expect("append 1");
        append_input_log(&path, 7, "ls -la\r", "C:\\").expect("append 2");

        let text = std::fs::read_to_string(&path).expect("read log");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "每行一条记录");

        let v0: serde_json::Value = serde_json::from_str(lines[0]).expect("line 0 json");
        assert!(v0["time"].is_string(), "time 字段存在");
        assert_eq!(v0["id"], 7);
        assert_eq!(v0["content"], "cd C:\\proj\r");

        let v1: serde_json::Value = serde_json::from_str(lines[1]).expect("line 1 json");
        assert_eq!(v1["content"], "ls -la\r");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 读取日志:条目按最新在前返回,坏行被跳过。
    #[test]
    fn input_log_reads_back_latest_first() {
        let dir = unique_dir("logread");
        let path = dir.join("input_log.jsonl");

        append_input_log(&path, 1, "first\r", "/tmp").expect("append 1");
        append_input_log(&path, 1, "second\r", "/tmp").expect("append 2");
        // 混入一行坏数据,读取时应跳过
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"not-json\n")
            .unwrap();
        append_input_log(&path, 2, "third\r", "/tmp").expect("append 3");

        let text = std::fs::read_to_string(&path).expect("read log");
        let entries: Vec<serde_json::Value> = text
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .rev()
            .collect();
        // 坏行被过滤后,应剩 3 条,最新在前
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0]["content"], "third\r");
        assert_eq!(entries[1]["content"], "second\r");
        assert_eq!(entries[2]["content"], "first\r");

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---------- 输出侧命令识别(OutputTracker)测试 ----------

    /// 回显行 `PS <路径>> <命令>` 识别出已提交的命令;输出行/新提示符结束命令。
    #[test]
    fn tracker_records_submitted_command() {
        let mut t = OutputTracker::new();
        // 启动 + 初始空提示符:不产生命令
        assert!(t.push("Windows PowerShell\r\nPS C:\\proj> \r\n").is_empty());
        // 输入 echo hi 并回车:回显行 + 输出 + 新提示符
        let rec = t.push("PS C:\\proj> echo hi\r\nhi\r\nPS C:\\proj> \r\n");
        assert_eq!(rec, vec![("C:\\proj".to_string(), "echo hi".to_string())]);
    }

    /// 回显行被网络/分块拆开时,跨块仍能拼出完整命令。
    #[test]
    fn tracker_across_chunks() {
        let mut t = OutputTracker::new();
        assert!(t.push("PS C:\\proj> ech").is_empty());
        let rec = t.push("o hi\r\nPS C:\\proj> \r\n");
        assert_eq!(rec, vec![("C:\\proj".to_string(), "echo hi".to_string())]);
    }

    /// 多行命令:续行 `>> ` 合并进同一条命令。
    #[test]
    fn tracker_merges_continuation_lines() {
        let mut t = OutputTracker::new();
        let rec = t.push(
            "PS C:\\proj> foreach ($i in 1..3) {\r\n>> $i\r\n>> }\r\n1\r\n2\r\n3\r\nPS C:\\proj> \r\n",
        );
        assert_eq!(
            rec,
            vec![(
                "C:\\proj".to_string(),
                "foreach ($i in 1..3) {\n$i\n}".to_string()
            )]
        );
    }

    /// 命令里的重定向 `>` 不是提示符箭头,保留在命令内。
    #[test]
    fn tracker_keeps_redirect_in_command() {
        let mut t = OutputTracker::new();
        let rec = t.push("PS C:\\proj> dir > out.txt\r\nPS C:\\proj> \r\n");
        assert_eq!(rec, vec![("C:\\proj".to_string(), "dir > out.txt".to_string())]);
    }

    /// 直接回车(空命令)不记录;ANSI 着色回显被剥离。
    #[test]
    fn tracker_skips_empty_and_strips_ansi() {
        let mut t = OutputTracker::new();
        let rec = t.push(
            "PS C:\\proj> \r\n\x1b[36mPS C:\\proj\x1b[0m> git status\r\nPS C:\\proj> \r\n",
        );
        assert_eq!(rec, vec![("C:\\proj".to_string(), "git status".to_string())]);
    }

    /// 已消费的行不会重复处理:无新增输出时不再产生命令。
    #[test]
    fn tracker_no_duplicate_without_new_output() {
        let mut t = OutputTracker::new();
        let chunk = "PS C:\\proj> echo hi\r\nPS C:\\proj> \r\n";
        assert_eq!(
            t.push(chunk),
            vec![("C:\\proj".to_string(), "echo hi".to_string())]
        );
        assert!(t.push("").is_empty());
    }

    /// cd 命令:记录执行时(旧)目录,新提示符只刷新会话 cwd。
    #[test]
    fn tracker_uses_command_time_cwd() {
        let mut t = OutputTracker::new();
        let rec = t.push("PS C:\\old> cd ..\r\nPS C:\\new> \r\n");
        assert_eq!(rec, vec![("C:\\old".to_string(), "cd ..".to_string())]);
        assert_eq!(t.cwd.as_deref(), Some("C:\\new"));
    }

    #[test]
    fn parse_prompt_command_basics() {
        assert_eq!(
            parse_prompt_command("PS C:\\x> echo hi"),
            Some(("C:\\x".to_string(), "echo hi".to_string()))
        );
        // 空命令 / 无箭头 / 非提示符行 → None
        assert_eq!(parse_prompt_command("PS C:\\x> "), None);
        assert_eq!(parse_prompt_command("PS C:\\x>"), None);
        assert_eq!(parse_prompt_command("hi"), None);
        assert_eq!(parse_prompt_command("PS Env:> "), None);
        // 路径含空格
        assert_eq!(
            parse_prompt_command("PS C:\\Program Files\\App> ls"),
            Some(("C:\\Program Files\\App".to_string(), "ls".to_string()))
        );
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
