import { useState } from 'react';
import { AppHeader } from './components/AppHeader';
import { DocumentDropZone } from './features/import/DocumentDropZone';
import { ResultTabs, type ResultView } from './features/results/ResultTabs';
import './styles/tokens.css';
import './styles/app.css';

const VIEW_HINTS: Record<ResultView, string> = {
  insights: '视觉模型与文本模型将在这里协作整理文档内容。',
  markdown: '导入文档后，这里会显示 AnyDoc 转换出的 Markdown。',
  source: '原始页面渲染与定位引用将在这里展示。',
  chat: '完成解析后，可以围绕当前文档连续提问。',
  json: '文档结构、页面和资源数据将在这里展示。',
};

export function App() {
  const [activeView, setActiveView] = useState<ResultView>('markdown');
  const [selectedFile, setSelectedFile] = useState<File>();

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

        <DocumentDropZone onFile={setSelectedFile} />

        <section className="workspace" aria-label="文档工作区">
          <ResultTabs active={activeView} onChange={setActiveView} />
          <div
            id={`panel-${activeView}`}
            className="result-panel"
            role="tabpanel"
            aria-labelledby={`tab-${activeView}`}
          >
            {selectedFile ? (
              <div className="selected-file">已选择 · {selectedFile.name}</div>
            ) : null}
            <div className="empty-result">
              <div>
                <strong>等待导入文档</strong>
                {VIEW_HINTS[activeView]}
              </div>
            </div>
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
