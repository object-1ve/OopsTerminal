import "./TabBar.css";

export type Tab = {
  /** 前端标签页 key */
  key: number;
  /** 后端终端会话 id,创建成功后回填 */
  sessionId: number | null;
};

export const TabBar = ({
  tabs,
  activeKey,
  onSelect,
  onClose,
  onAdd,
}: {
  tabs: Tab[];
  activeKey: number | null;
  onSelect: (key: number) => void;
  onClose: (key: number) => void;
  onAdd: () => void;
}) => {
  return (
    <div className="tab-bar">
      {tabs.map((tab, i) => (
        <div
          key={tab.key}
          className={`tab ${tab.key === activeKey ? "active" : ""}`}
          onClick={() => onSelect(tab.key)}
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
