import { useCallback, useRef, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { TitleBar } from './components/TitleBar'
import { SettingsModal } from './components/SettingsModal'
import { TabBar, type Tab } from './components/TabBar'
import { TerminalView } from './components/TerminalView'
import './App.css'

function App() {
  const [settingsOpen, setSettingsOpen] = useState(false)

  const keySeq = useRef(1)
  const [tabs, setTabs] = useState<Tab[]>([{ key: 1, sessionId: null }])
  const [activeKey, setActiveKey] = useState<number>(1)

  const addTab = useCallback(() => {
    keySeq.current += 1
    const key = keySeq.current
    setTabs((t) => [...t, { key, sessionId: null }])
    setActiveKey(key)
  }, [])

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
      <TitleBar onOpenSettings={() => setSettingsOpen(true)} />
      <TabBar
        tabs={tabs}
        activeKey={activeKey}
        onSelect={setActiveKey}
        onClose={closeTab}
        onAdd={addTab}
      />
      <div className="terminal-area">
        {tabs.map((tab) => (
          <div
            key={tab.key}
            className={`terminal-pane ${tab.key === activeKey ? '' : 'hidden'}`}
          >
            <TerminalView active={tab.key === activeKey} onSessionId={handleSessionId(tab.key)} />
          </div>
        ))}
        {tabs.length === 0 && <div className="terminal-empty">点击 + 新建终端</div>}
      </div>
      <SettingsModal open={settingsOpen} onClose={() => setSettingsOpen(false)} />
    </div>
  )
}

export default App
