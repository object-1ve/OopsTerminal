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
    /// 当前工作目录,随 PowerShell 提示符渲染实时更新。
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
    let app_handle = app.clone();
    let exit_app = app.clone();
    std::thread::spawn(move || {
        let mut pending: Vec<u8> = Vec::new();
        let mut buf = [0u8; 8192];
        // 最近一段已解码输出,用于识别 PowerShell 提示符并同步目录
        let mut tail = String::new();
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break, // EOF: 进程已退出
                Ok(n) => {
                    pending.extend_from_slice(&buf[..n]);
                    // 只发送完整的 UTF-8 序列,避免跨包截断产生乱码
                    match std::str::from_utf8(&pending) {
                        Ok(s) => {
                            update_cwd_from_output(&exit_app, id, s, &mut tail);
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
                                update_cwd_from_output(&exit_app, id, s, &mut tail);
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

/// 把一段已解码的终端输出并入 tail,从中识别 PowerShell 提示符里的目录。
/// 返回最近一次识别到的目录;未识别到返回 None。
///
/// 单独拆出便于单元测试;真正的状态更新由调用方完成。
fn parse_tracked_cwd(decoded: &str, tail: &mut String) -> Option<String> {
    const MAX_TAIL: usize = 4096;
    tail.push_str(decoded);
    if tail.len() > MAX_TAIL {
        tail.drain(..tail.len() - MAX_TAIL);
    }
    if !tail.contains("PS ") {
        return None;
    }
    parse_prompt_cwd(tail)
}

/// 把输出并入 tail 并同步会话的 cwd。识别失败不影响输出转发。
fn update_cwd_from_output(app: &AppHandle, id: u32, decoded: &str, tail: &mut String) {
    let Some(cwd) = parse_tracked_cwd(decoded, tail) else {
        return;
    };
    if let Some(mgr) = app.try_state::<TerminalManager>() {
        if let Some(session) = mgr.inner.lock().unwrap().sessions.get_mut(&id) {
            session.cwd = std::path::PathBuf::from(&cwd);
        }
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
/// 每行一个 JSON 对象:`{"time": <RFC3339 本地时间>, "id": <会话id>, "content": <输入>}`。
/// 写失败时仅记录告警,不影响终端本身。
fn append_input_log(path: &std::path::Path, id: u32, content: &str) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    let line = serde_json::json!({
        "time": chrono::Local::now().to_rfc3339(),
        "id": id,
        "content": content,
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

    // 记录用户输入到本地日志(写入 PTY 成功后追加,失败不影响终端)
    if let Some(path) = state.log_path.lock().unwrap().clone() {
        if let Err(e) = append_input_log(&path, id, &data) {
            log::warn!("写入输入日志失败 ({}): {e}", path.display());
        }
    }

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

        append_input_log(&path, 7, "cd C:\\proj\r").expect("append 1");
        append_input_log(&path, 7, "ls -la\r").expect("append 2");

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

        append_input_log(&path, 1, "first\r").expect("append 1");
        append_input_log(&path, 1, "second\r").expect("append 2");
        // 混入一行坏数据,读取时应跳过
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"not-json\n")
            .unwrap();
        append_input_log(&path, 2, "third\r").expect("append 3");

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

    /// 提示符文本被 ANSI/网络分块拆开时,tail 应能跨块拼出完整提示符。
    #[test]
    fn parse_tracked_cwd_across_chunks() {
        let mut tail = String::new();
        // 初始:启动输出与第一个提示符的前半段
        assert_eq!(parse_tracked_cwd("Windows PowerShell\r\nPS ", &mut tail), None);
        // 路径前半
        assert_eq!(parse_tracked_cwd("C:\\Users\\me\\pr", &mut tail), None);
        // 路径后半 + 箭头(注意路径在箭头前,直接拼接)
        assert_eq!(
            parse_tracked_cwd("oj> ", &mut tail),
            Some("C:\\Users\\me\\proj".into())
        );
        // 同一目录重复出现,结果一致
        assert_eq!(
            parse_tracked_cwd("\r\nPS C:\\Users\\me\\proj> ", &mut tail),
            Some("C:\\Users\\me\\proj".into())
        );
        // 之后 cd 到新目录,解析出新路径
        assert_eq!(
            parse_tracked_cwd("\r\nPS C:\\Users\\me\\other> ", &mut tail),
            Some("C:\\Users\\me\\other".into())
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
