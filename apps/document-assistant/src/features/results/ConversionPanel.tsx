import type { ConversionState } from '../import/useDocumentConversion';
import type { ResultView } from './ResultTabs';
import type { DocumentSummary } from '../../lib/analysis/types';
import { InsightsView } from './InsightsView';
import { StructuredJsonView } from './StructuredJsonView';
import { DocumentChat } from '../chat/DocumentChat';
import { SourceViewer, type SourcePage } from '../source/SourceViewer';

interface ConversionPanelProps {
  state: ConversionState;
  activeView: ResultView;
  summary?: DocumentSummary | null;
  analysisStatus?: 'idle' | 'running' | 'failed';
  analysisError?: string | null;
  canAnalyze?: boolean;
  onRequestAnalysis?: () => void;
  onNavigateToPage?: (pageNumber: number) => void;
  documentId?: string | null;
  sourcePages?: SourcePage[];
  selectedPage?: number;
  onSelectPage?: (pageNumber: number) => void;
}

const VIEW_HINTS: Record<ResultView, string> = {
  insights: '视觉模型与文本模型将在这里协作整理文档内容。',
  markdown: '导入文档后，这里会显示 AnyDoc 转换出的 Markdown。',
  source: '原始页面渲染与定位引用将在这里展示。',
  chat: '完成解析后，可以围绕当前文档连续提问。',
  json: '文档结构、页面和资源数据将在这里展示。',
};

function markdownFileName(fileName: string) {
  const baseName = fileName.replace(/\.[^.]+$/, '') || 'document';
  return `${baseName}.md`;
}

export function ConversionPanel({
  state,
  activeView,
  summary,
  analysisStatus = 'idle',
  analysisError,
  canAnalyze = false,
  onRequestAnalysis,
  onNavigateToPage = () => undefined,
  documentId,
  sourcePages = [],
  selectedPage = 1,
  onSelectPage = () => undefined,
}: ConversionPanelProps) {
  if (state.status === 'reading' || state.status === 'converting') {
    return (
      <div className="conversion-progress" role="status">
        <span className="spinner" aria-hidden="true" />
        <div>
          <strong>{state.status === 'reading' ? '正在读取文件' : 'AnyDoc 正在解析'}</strong>
          <span>{state.fileName}</span>
        </div>
      </div>
    );
  }

  if (state.status === 'failed') {
    return (
      <div className="conversion-error" role="alert">
        <span className="error-code">{state.error.code ?? 'conversion_error'}</span>
        <strong>文档转换失败</strong>
        <p>{state.error.message}</p>
      </div>
    );
  }

  if (state.status === 'idle') {
    return (
      <div className="empty-result">
        <div>
          <strong>等待导入文档</strong>
          {VIEW_HINTS[activeView]}
        </div>
      </div>
    );
  }

  const { result } = state;
  const markdownUrl = `data:text/markdown;charset=utf-8,${encodeURIComponent(result.markdown)}`;

  return (
    <div className="conversion-result">
      <div className="result-summary">
        <div>
          <strong>{result.fileName}</strong>
          <div className="result-metadata" aria-label="转换信息">
            <span>{result.format.toUpperCase()}</span>
            <span>{result.characterCount} 字符</span>
            <span>{result.elapsedMs} ms</span>
          </div>
        </div>
        <div className="result-actions">
          <button
            type="button"
            onClick={() => navigator.clipboard.writeText(result.markdown)}
          >
            复制 Markdown
          </button>
          <a href={markdownUrl} download={markdownFileName(result.fileName)}>
            下载 Markdown
          </a>
        </div>
      </div>

      {activeView === 'markdown' ? (
        <pre className="markdown-output">{result.markdown}</pre>
      ) : activeView === 'insights' && summary ? (
        <InsightsView summary={summary} onNavigateToPage={onNavigateToPage} />
      ) : activeView === 'insights' ? (
        <div className="analysis-callout">
          <strong>{analysisStatus === 'running' ? '正在进行双模型分析' : '生成智能解读'}</strong>
          <p>{analysisError ?? (canAnalyze ? '视觉模型理解逐页画面，文本模型再整理全文逻辑。' : '请先用 Windows 文件选择器导入，并等待逐页渲染完成。')}</p>
          <button type="button" disabled={!canAnalyze || analysisStatus === 'running'} onClick={onRequestAnalysis}>
            {analysisStatus === 'running' ? '分析中…' : '开始智能分析'}
          </button>
        </div>
      ) : activeView === 'json' ? (
        <StructuredJsonView value={summary ?? { schemaVersion: 1, anyDocDocument: result.document }} />
      ) : activeView === 'chat' && documentId && summary ? (
        <DocumentChat documentId={documentId} onNavigateToPage={onNavigateToPage} />
      ) : activeView === 'source' ? (
        <SourceViewer
          pages={sourcePages}
          selectedPage={selectedPage}
          onSelectPage={onSelectPage}
          onRetryPage={() => undefined}
        />
      ) : (
        <div className="empty-result compact">
          <div>{VIEW_HINTS[activeView]}</div>
        </div>
      )}
    </div>
  );
}
