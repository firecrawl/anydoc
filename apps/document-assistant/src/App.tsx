import { useEffect, useState } from 'react';
import { AppHeader } from './components/AppHeader';
import { DocumentDropZone } from './features/import/DocumentDropZone';
import { useDocumentConversion } from './features/import/useDocumentConversion';
import { ConversionPanel } from './features/results/ConversionPanel';
import { AnalysisConsentDialog } from './features/results/AnalysisConsentDialog';
import { ResultTabs, type ResultView } from './features/results/ResultTabs';
import { ModelSettings } from './features/settings/ModelSettings';
import {
  saveModelProfile,
  setModelApiKey,
  type ModelProfile,
  type ModelRole,
  listModelProfiles,
  type ModelProfileStatus,
} from './features/settings/modelProfile';
import { testModelProfile } from './features/settings/modelTestApi';
import { pickAndRegisterDocument, saveDocumentMarkdown } from './features/import/desktopImportApi';
import { startDocumentTask } from './features/tasks/taskApi';
import { useTaskProgress } from './features/tasks/useTaskProgress';
import { TaskProgress } from './features/tasks/TaskProgress';
import { analyzeDocument } from './lib/analysis/analysisApi';
import type { DocumentSummary } from './lib/analysis/types';
import { getDocumentPages } from './features/source/sourceApi';
import type { SourcePage } from './features/source/SourceViewer';
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
  const [profiles, setProfiles] = useState<Record<ModelRole, ModelProfileStatus>>({
    vision: { ...DEFAULT_PROFILES.vision, hasApiKey: false },
    text: { ...DEFAULT_PROFILES.text, hasApiKey: false },
  });
  const [documentId, setDocumentId] = useState<string | null>(null);
  const [taskId, setTaskId] = useState<string | null>(null);
  const [summary, setSummary] = useState<DocumentSummary | null>(null);
  const [consentOpen, setConsentOpen] = useState(false);
  const [analysisStatus, setAnalysisStatus] = useState<'idle' | 'running' | 'failed'>('idle');
  const [analysisError, setAnalysisError] = useState<string | null>(null);
  const [sourcePages, setSourcePages] = useState<SourcePage[]>([]);
  const [selectedPage, setSelectedPage] = useState(1);
  const { state, convert } = useDocumentConversion(anyDocClient);
  const progress = useTaskProgress(taskId);
  const activeProfile = profiles[settingsRole];
  const canAnalyze = Boolean(
    documentId &&
    profiles.text.baseUrl && profiles.text.model && profiles.text.hasApiKey &&
    (!taskId || progress.task?.stage === 'completed'),
  );

  useEffect(() => {
    if (anyDocClient) return;
    void listModelProfiles().then((saved) => {
      setProfiles((current) => {
        const next = { ...current };
        for (const profile of saved) next[profile.role] = profile;
        return next;
      });
    }).catch(() => undefined);
  }, [anyDocClient]);

  useEffect(() => {
    if (!documentId || (taskId && progress.task?.stage !== 'completed')) return;
    void getDocumentPages(documentId).then((pages) => {
      setSourcePages(pages);
      if (pages.length > 0) {
        setSelectedPage((current) => pages.some((page) => page.pageNumber === current) ? current : pages[0].pageNumber);
      }
    }).catch(() => undefined);
  }, [documentId, progress.task?.stage, summary, taskId]);

  const browseWindowsDocument = async () => {
    try {
      const selected = await pickAndRegisterDocument();
      if (!selected) return;
      setDocumentId(selected.document.id);
      setSourcePages([]);
      setSelectedPage(1);
      setSummary(null);
      setAnalysisStatus('idle');
      const file = new File([new Uint8Array(selected.bytes)], selected.document.fileName);
      const result = await convert(file);
      if (!result) return;
      await saveDocumentMarkdown(selected.document.id, result.markdown);
      const task = await startDocumentTask(selected.document.id);
      setTaskId(task.id);
    } catch (cause) {
      setAnalysisError(cause instanceof Error ? cause.message : String(cause));
    }
  };

  const saveProfile = async (profile: ModelProfile) => {
    await saveModelProfile(profile);
    setProfiles((current) => ({
      ...current,
      [profile.role]: { ...profile, hasApiKey: current[profile.role].hasApiKey },
    }));
  };

  const setProfileKey = async (apiKey: string) => {
    await setModelApiKey(activeProfile.id, apiKey);
    setProfiles((current) => ({
      ...current,
      [settingsRole]: { ...current[settingsRole], hasApiKey: true },
    }));
  };

  const runAnalysis = async () => {
    if (!documentId) return;
    setConsentOpen(false);
    setAnalysisStatus('running');
    setAnalysisError(null);
    setActiveView('insights');
    try {
      const nextSummary = await analyzeDocument({
        documentId,
        visionProfileId: profiles.vision.baseUrl && profiles.vision.hasApiKey ? profiles.vision.id : null,
        textProfileId: profiles.text.id,
        confirmRemoteProcessing: true,
      });
      setSummary(nextSummary);
      setAnalysisStatus('idle');
    } catch (cause) {
      setAnalysisStatus('failed');
      setAnalysisError(cause instanceof Error ? cause.message : String(cause));
    }
  };

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

        <DocumentDropZone
          onFile={(file) => void convert(file)}
          onBrowse={anyDocClient ? undefined : () => void browseWindowsDocument()}
        />

        {progress.task ? (
          <TaskProgress
            task={progress.task}
            onPause={() => void progress.pause()}
            onResume={() => void progress.resume()}
            onCancel={() => void progress.cancel()}
            onRetry={() => void progress.retry()}
          />
        ) : null}

        <section className="workspace" aria-label="文档工作区">
          <ResultTabs active={activeView} onChange={setActiveView} />
          <div
            id={`panel-${activeView}`}
            className="result-panel"
            role="tabpanel"
            aria-labelledby={`tab-${activeView}`}
          >
            <ConversionPanel
              state={state}
              activeView={activeView}
              summary={summary}
              analysisStatus={analysisStatus}
              analysisError={analysisError}
              canAnalyze={canAnalyze}
              onRequestAnalysis={() => setConsentOpen(true)}
              onNavigateToPage={(pageNumber) => {
                setSelectedPage(pageNumber);
                setActiveView('source');
              }}
              documentId={documentId}
              sourcePages={sourcePages}
              selectedPage={selectedPage}
              onSelectPage={setSelectedPage}
            />
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
              onSave={saveProfile}
              onSetApiKey={setProfileKey}
              onTest={async (profile, apiKey) => {
                await saveProfile(profile);
                if (apiKey) await setProfileKey(apiKey);
                await testModelProfile(profile.id);
              }}
            />
          </section>
        </div>
      ) : null}

      {consentOpen ? (
        <AnalysisConsentDialog
          sendsImages={Boolean(profiles.vision.baseUrl && profiles.vision.hasApiKey)}
          visionModel={profiles.vision.model || null}
          textModel={profiles.text.model}
          onCancel={() => setConsentOpen(false)}
          onConfirm={() => void runAnalysis()}
        />
      ) : null}
    </div>
  );
}
