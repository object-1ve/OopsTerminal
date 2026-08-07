import { useCallback, useRef, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { TitleBar } from './components/TitleBar'
import { SettingsModal } from './components/SettingsModal'
import { LogModal } from './components/LogModal'
import { TabBar, type Tab } from './components/TabBar'
import { TerminalView } from './components/TerminalView'
import './App.css'

function App() {
  const [settingsOpen, setSettingsOpen] = useState(false)
  const [logOpen, setLogOpen] = useState(false)

  const keySeq = useRef(1)
  const [tabs, setTabs] = useState<Tab[]>([{ key: 1, sessionId: null }])
  const [activeKey, setActiveKey] = useState<number>(1)

  const addTab = useCallback((cwd?: string) => {
    keySeq.current += 1
    const key = keySeq.current
    // 防御: 事件对象等非字符串值绝不存入 tab, 避免污染 create_terminal 参数
    const cleanCwd = typeof cwd === "string" ? cwd : undefined
    setTabs((t) => [...t, { key, sessionId: null, startCwd: cleanCwd }])
    setActiveKey(key)
  }, [])

  // 双击标签:读取源终端的当前目录,以相同路径新建一个终端
  const duplicateTab = useCallback(
    (key: number) => {
      const tab = tabs.find((x) => x.key === key)
      if (tab?.sessionId == null) {
        addTab()
        return
      }
      invoke<string | null>('get_terminal_cwd', { id: tab.sessionId })
        .then((cwd) => addTab(cwd ?? undefined))
        .catch(() => addTab())
    },
    [tabs, addTab],
  )

  const closeTab = useCallback(
    (key: number) => {
      const tab = tabs.find((x) => x.key === key)
      if (tab?.sessionId != null) {
        invoke('kill_terminal', { id: tab.sessionId }).catch(() => {})
      }
      const remaining = tabs.filter((x) => x.key !== key)
      setTabs(remaining)
      if (activeKey === key) {
        setActiveKey(remaining.length ? remaining[remaining.length - 1].key : 0)
      }
    },
    [tabs, activeKey],
  )

  const handleSessionId = useCallback((key: number) => {
    return (sessionId: number) => {
      setTabs((t) => t.map((x) => (x.key === key ? { ...x, sessionId } : x)))
    }
  }, [])

  return (
    <div className="app-shell">
      <TitleBar onOpenSettings={() => setSettingsOpen(true)} onOpenLog={() => setLogOpen(true)} />
      <TabBar
        tabs={tabs}
        activeKey={activeKey}
        onSelect={setActiveKey}
        onClose={closeTab}
        onAdd={() => addTab()}
        onDuplicate={duplicateTab}
      />
      <div className="terminal-area">
        {tabs.map((tab) => (
          <div
            key={tab.key}
            className={`terminal-pane ${tab.key === activeKey ? '' : 'hidden'}`}
          >
            <TerminalView
              active={tab.key === activeKey}
              onSessionId={handleSessionId(tab.key)}
              startCwd={tab.startCwd}
            />
          </div>
        ))}
        {tabs.length === 0 && <div className="terminal-empty">点击 + 新建终端</div>}
      </div>
      <SettingsModal open={settingsOpen} onClose={() => setSettingsOpen(false)} />
      <LogModal open={logOpen} onClose={() => setLogOpen(false)} />
    </div>
  )
}

export default App
