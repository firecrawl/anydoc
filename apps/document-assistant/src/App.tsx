import { useState } from 'react';
import { AppHeader } from './components/AppHeader';
import { DocumentDropZone } from './features/import/DocumentDropZone';
import { useDocumentConversion } from './features/import/useDocumentConversion';
import { ConversionPanel } from './features/results/ConversionPanel';
import { ResultTabs, type ResultView } from './features/results/ResultTabs';
import './styles/tokens.css';
import './styles/app.css';

export function App() {
  const [activeView, setActiveView] = useState<ResultView>('markdown');
  const { state, convert } = useDocumentConversion();

  return (
    <div id="top" className="app-shell">
      <AppHeader onOpenSettings={() => undefined} />
      <main>
        <section className="hero" aria-labelledby="hero-title">
          <p className="eyebrow">Local-first document intelligence</p>
          <h1 id="hero-title">
            Any document in.
            <br />
            <span>Clear answers out.</span>
          </h1>
          <p className="hero-description">
            在本机解析 Word、PowerPoint、PDF 和表格，再用视觉与文本模型理顺结构、提炼重点并连续问答。
          </p>
        </section>

        <DocumentDropZone onFile={(file) => void convert(file)} />

        <section className="workspace" aria-label="文档工作区">
          <ResultTabs active={activeView} onChange={setActiveView} />
          <div
            id={`panel-${activeView}`}
            className="result-panel"
            role="tabpanel"
            aria-labelledby={`tab-${activeView}`}
          >
            <ConversionPanel state={state} activeView={activeView} />
          </div>
        </section>
      </main>

      <footer className="app-footer">
        <span>文件解析在本机完成，模型请求仅在你配置后启用。</span>
        <span>
          解析能力基于{' '}
          <a href="https://github.com/firecrawl/anydoc" target="_blank" rel="noreferrer">
            AnyDoc（MIT）
          </a>
        </span>
      </footer>
    </div>
  );
}
