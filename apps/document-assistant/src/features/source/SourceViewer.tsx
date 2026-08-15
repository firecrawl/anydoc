import { useState } from 'react';

export interface SourcePage {
  pageNumber: number;
  imageUrl: string | null;
  text: string | null;
  status: string;
  analysis: unknown | null;
  error: string | null;
}

interface SourceViewerProps {
  pages: SourcePage[];
  selectedPage: number;
  onSelectPage: (pageNumber: number) => void;
  onRetryPage: (pageNumber: number) => void;
}

export function SourceViewer({ pages, selectedPage, onSelectPage, onRetryPage }: SourceViewerProps) {
  const [showText, setShowText] = useState(false);
  const [showJson, setShowJson] = useState(false);
  const [zoom, setZoom] = useState(100);
  const index = Math.max(0, pages.findIndex((page) => page.pageNumber === selectedPage));
  const page = pages[index];

  if (!page) return <div className="empty-result">尚无可核对的页面。</div>;

  return (
    <section className="source-viewer" aria-label="原页核对">
      <aside className="thumbnail-rail" aria-label="页面缩略图">
        {pages.map((item) => (
          <button key={item.pageNumber} type="button" aria-current={item.pageNumber === page.pageNumber ? 'page' : undefined} onClick={() => onSelectPage(item.pageNumber)}>
            {item.imageUrl ? <img src={item.imageUrl} alt="" /> : <span>纯文本</span>}
            第 {item.pageNumber} 页
          </button>
        ))}
      </aside>
      <div className="source-stage">
        <div className="source-toolbar">
          <button type="button" disabled={index === 0} onClick={() => onSelectPage(pages[index - 1].pageNumber)}>上一页</button>
          <span>第 {page.pageNumber} / {pages.length} 页</span>
          <button type="button" disabled={index === pages.length - 1} onClick={() => onSelectPage(pages[index + 1].pageNumber)}>下一页</button>
          <button type="button" onClick={() => setZoom((value) => Math.max(50, value - 25))}>缩小</button>
          <span>{zoom}%</span>
          <button type="button" onClick={() => setZoom((value) => Math.min(200, value + 25))}>放大</button>
          <button type="button" aria-pressed={showText} onClick={() => setShowText((value) => !value)}>页面文本</button>
          <button type="button" aria-pressed={showJson} onClick={() => setShowJson((value) => !value)}>视觉 JSON</button>
        </div>
        {page.imageUrl ? (
          <div className="page-canvas"><img src={page.imageUrl} alt={`第 ${page.pageNumber} 页`} style={{ width: `${zoom}%` }} /></div>
        ) : (
          <div className="text-only-page">此页没有渲染图像，仍可查看提取文本并进行纯文本分析。</div>
        )}
        {page.error ? (
          <div className="source-error" role="alert">
            <span>视觉分析失败：{page.error}</span>
            <button type="button" onClick={() => onRetryPage(page.pageNumber)}>重试本页</button>
          </div>
        ) : null}
        {showText ? <pre className="source-detail">{page.text || '此页没有单独提取的文本。'}</pre> : null}
        {showJson ? <pre className="source-detail">{JSON.stringify(page.analysis, null, 2)}</pre> : null}
      </div>
    </section>
  );
}
