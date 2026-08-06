import "./TabBar.css";

export type Tab = {
  /** 前端标签页 key */
  key: number;
  /** 后端终端会话 id,创建成功后回填 */
  sessionId: number | null;
  /** 创建该标签时指定的启动目录(双击复制终端时使用) */
  startCwd?: string;
};

export const TabBar = ({
  tabs,
  activeKey,
  onSelect,
  onClose,
  onAdd,
  onDuplicate,
}: {
  tabs: Tab[];
  activeKey: number | null;
  onSelect: (key: number) => void;
  onClose: (key: number) => void;
  onAdd: () => void;
  onDuplicate: (key: number) => void;
}) => {
  return (
    <div className="tab-bar">
      {tabs.map((tab, i) => (
        <div
          key={tab.key}
          className={`tab ${tab.key === activeKey ? "active" : ""}`}
          onClick={() => onSelect(tab.key)}
          onDoubleClick={(e) => {
            // 双击关闭按钮不触发复制
            if ((e.target as HTMLElement).closest(".tab-close")) return;
            onDuplicate(tab.key);
          }}
          title="双击以相同路径新建终端"
        >
          <span className="tab-title">PowerShell {i + 1}</span>
          <button
            type="button"
            className="tab-close"
            title="关闭标签页"
            onClick={(e) => {
              e.stopPropagation();
              onClose(tab.key);
            }}
          >
            ×
          </button>
        </div>
      ))}
      <button type="button" className="tab-add" title="新建终端" onClick={onAdd}>
        +
      </button>
      {tabs.length === 0 && <span className="tab-empty-hint">点击 + 新建终端</span>}
    </div>
  );
};
