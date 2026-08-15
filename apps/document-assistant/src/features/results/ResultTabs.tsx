export type ResultView = 'insights' | 'markdown' | 'source' | 'chat' | 'json';

interface ResultTabsProps {
  active: ResultView;
  onChange: (view: ResultView) => void;
}

const RESULT_VIEWS: ReadonlyArray<{ id: ResultView; label: string }> = [
  { id: 'insights', label: '智能解读' },
  { id: 'markdown', label: 'Markdown' },
  { id: 'source', label: '原文预览' },
  { id: 'chat', label: '文档问答' },
  { id: 'json', label: '结构数据' },
];

export function ResultTabs({ active, onChange }: ResultTabsProps) {
  return (
    <div className="result-tabs" role="tablist" aria-label="文档结果视图">
      {RESULT_VIEWS.map((view) => (
        <button
          key={view.id}
          id={`tab-${view.id}`}
          className="result-tab"
          type="button"
          role="tab"
          aria-selected={active === view.id}
          aria-controls={`panel-${view.id}`}
          tabIndex={active === view.id ? 0 : -1}
          onClick={() => onChange(view.id)}
        >
          {view.label}
        </button>
      ))}
    </div>
  );
}
