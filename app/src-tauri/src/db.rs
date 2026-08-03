use rusqlite::{params, Connection, Result};
use std::path::PathBuf;

#[derive(Clone, serde::Serialize)]
pub struct Settings {
    pub toggle_window_shortcut: Option<String>,
    pub quit_shortcut: Option<String>,
    pub default_path: Option<String>,
    pub show_tray_icon: bool,
    pub show_taskbar_icon: bool,
    /// 终端字体文件路径 (ttf/otf/woff/woff2),None 表示使用默认字体
    pub terminal_font_path: Option<String>,
}

pub fn init_db(app_dir: PathBuf) -> Result<Connection> {
    let db_path = app_dir.join("oops_terminal.db");
    let conn = Connection::open(db_path)?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        )",
        [],
    )?;

    Ok(conn)
}

pub fn load_settings(conn: &Connection) -> Result<Settings> {
    Ok(Settings {
        toggle_window_shortcut: load_setting(conn, "toggle_window_shortcut")?,
        quit_shortcut: load_setting(conn, "quit_shortcut")?,
        default_path: load_setting(conn, "default_path")?,
        show_tray_icon: load_bool(conn, "show_tray_icon", true)?,
        show_taskbar_icon: load_bool(conn, "show_taskbar_icon", false)?,
        terminal_font_path: load_setting(conn, "terminal_font_path")?,
    })
}

fn load_bool(conn: &Connection, key: &str, default: bool) -> Result<bool> {
    match load_setting(conn, key)? {
        Some(v) => Ok(v == "true"),
        None => Ok(default),
    }
}

fn load_setting(conn: &Connection, key: &str) -> Result<Option<String>> {
    let mut stmt = conn.prepare("SELECT value FROM settings WHERE key = ?1")?;
    let mut rows = stmt.query(params![key])?;

    if let Some(row) = rows.next()? {
        Ok(Some(row.get(0)?))
    } else {
        Ok(None)
    }
}

pub fn save_setting(conn: &Connection, key: &str, value: Option<&str>) -> Result<()> {
    match value {
        Some(v) => {
            conn.execute(
                "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
                params![key, v],
            )?;
        }
        None => {
            conn.execute("DELETE FROM settings WHERE key = ?1", params![key])?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_roundtrip_and_clear() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
            [],
        )
        .unwrap();

        save_setting(&conn, "toggle_window_shortcut", Some("Ctrl+Shift+K")).unwrap();
        save_setting(&conn, "quit_shortcut", None).unwrap();

        let loaded = load_settings(&conn).unwrap();
        assert_eq!(
            loaded.toggle_window_shortcut.as_deref(),
            Some("Ctrl+Shift+K")
        );
        assert_eq!(loaded.quit_shortcut, None);

        // Clearing an existing key removes the row.
        save_setting(&conn, "toggle_window_shortcut", None).unwrap();
        let loaded = load_settings(&conn).unwrap();
        assert_eq!(loaded.toggle_window_shortcut, None);
    }
}

