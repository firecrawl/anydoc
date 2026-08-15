import { useState } from 'react';
import { AppHeader } from './components/AppHeader';
import { DocumentDropZone } from './features/import/DocumentDropZone';
import { useDocumentConversion } from './features/import/useDocumentConversion';
import { ConversionPanel } from './features/results/ConversionPanel';
import { ResultTabs, type ResultView } from './features/results/ResultTabs';
import { ModelSettings } from './features/settings/ModelSettings';
import {
  saveModelProfile,
  setModelApiKey,
  type ModelProfile,
  type ModelRole,
} from './features/settings/modelProfile';
import type { AnyDocClient } from './lib/anydoc/types';
import './styles/tokens.css';
import './styles/app.css';

interface AppProps {
  anyDocClient?: AnyDocClient;
}

const DEFAULT_PROFILES: Record<ModelRole, ModelProfile> = {
  vision: {
    id: 'vision-primary',
    role: 'vision',
    baseUrl: '',
    model: '',
    supportsVision: true,
    timeoutSecs: 120,
    maxConcurrency: 2,
  },
  text: {
    id: 'text-primary',
    role: 'text',
    baseUrl: '',
    model: '',
    supportsVision: false,
    timeoutSecs: 120,
    maxConcurrency: 2,
  },
};

export function App({ anyDocClient }: AppProps = {}) {
  const [activeView, setActiveView] = useState<ResultView>('markdown');
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [settingsRole, setSettingsRole] = useState<ModelRole>('vision');
  const { state, convert } = useDocumentConversion(anyDocClient);
  const activeProfile = DEFAULT_PROFILES[settingsRole];

  return (
    <div id="top" className="app-shell">
      <AppHeader onOpenSettings={() => setSettingsOpen(true)} />
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

      {settingsOpen ? (
        <div className="dialog-backdrop">
          <section
            className="settings-dialog"
            role="dialog"
            aria-modal="true"
            aria-labelledby="settings-title"
          >
            <div className="dialog-heading">
              <div>
                <p className="eyebrow">Dual-model routing</p>
                <h2 id="settings-title">模型设置</h2>
              </div>
              <button type="button" onClick={() => setSettingsOpen(false)} aria-label="关闭模型设置">
                ×
              </button>
            </div>
            <div className="profile-switch" role="tablist" aria-label="配置角色">
              <button
                type="button"
                role="tab"
                aria-selected={settingsRole === 'vision'}
                onClick={() => setSettingsRole('vision')}
              >
                视觉模型配置
              </button>
              <button
                type="button"
                role="tab"
                aria-selected={settingsRole === 'text'}
                onClick={() => setSettingsRole('text')}
              >
                文本模型配置
              </button>
            </div>
            <ModelSettings
              key={activeProfile.id}
              profile={activeProfile}
              hasApiKey={false}
              onSave={saveModelProfile}
              onSetApiKey={(apiKey) => setModelApiKey(activeProfile.id, apiKey)}
            />
          </section>
        </div>
      ) : null}
    </div>
  );
}
