import { useState, type FormEvent } from 'react';
import type { ModelProfile } from './modelProfile';

interface ModelSettingsProps {
  profile: ModelProfile;
  hasApiKey: boolean;
  onSave: (profile: ModelProfile) => Promise<void>;
  onSetApiKey: (apiKey: string) => Promise<void>;
  onTest?: (profile: ModelProfile, apiKey: string) => Promise<void>;
}

function endpointError(baseUrl: string) {
  try {
    const url = new URL(baseUrl);
    const local = url.hostname === 'localhost' || url.hostname === '127.0.0.1';
    if (url.protocol !== 'https:' && !local) return '远程地址必须使用 HTTPS';
    if (!['http:', 'https:'].includes(url.protocol)) return '模型地址必须是 HTTP 或 HTTPS';
    return undefined;
  } catch {
    return '请输入有效的模型地址';
  }
}

export function ModelSettings({
  profile,
  hasApiKey,
  onSave,
  onSetApiKey,
  onTest,
}: ModelSettingsProps) {
  const [draft, setDraft] = useState(profile);
  const [apiKey, setApiKey] = useState('');
  const [reuseTextProfile, setReuseTextProfile] = useState(false);
  const [error, setError] = useState<string>();
  const [testStatus, setTestStatus] = useState<'idle' | 'testing' | 'passed'>('idle');

  const testConnection = async () => {
    const validationError = endpointError(draft.baseUrl);
    if (validationError) {
      setError(validationError);
      return;
    }
    if (!onTest) return;
    setError(undefined);
    setTestStatus('testing');
    try {
      await onTest(draft, apiKey.trim());
      setTestStatus('passed');
    } catch (cause) {
      setTestStatus('idle');
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  };

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    const validationError = endpointError(draft.baseUrl);
    if (validationError) {
      setError(validationError);
      return;
    }
    setError(undefined);
    await onSave(draft);
    if (apiKey.trim()) {
      await onSetApiKey(apiKey.trim());
      setApiKey('');
    }
  };

  return (
    <form className="model-settings" onSubmit={(event) => void submit(event)}>
      <h3>{draft.role === 'vision' ? '视觉模型' : '文本模型'}</h3>

      <label>
        API Base URL
        <input
          value={draft.baseUrl}
          onChange={(event) => setDraft({ ...draft, baseUrl: event.target.value })}
        />
      </label>
      <label>
        模型名称
        <input
          value={draft.model}
          onChange={(event) => setDraft({ ...draft, model: event.target.value })}
        />
      </label>
      <label>
        API Key
        <input
          type="password"
          value={apiKey}
          autoComplete="new-password"
          placeholder={hasApiKey ? '留空则保留已保存密钥' : '输入 API Key'}
          onChange={(event) => setApiKey(event.target.value)}
        />
      </label>
      {hasApiKey ? <span className="credential-status">密钥已安全保存</span> : null}

      <div className="model-settings__numbers">
        <label>
          超时（秒）
          <input
            type="number"
            min={1}
            max={600}
            value={draft.timeoutSecs}
            onChange={(event) => setDraft({ ...draft, timeoutSecs: Number(event.target.value) })}
          />
        </label>
        <label>
          最大并发
          <input
            type="number"
            min={1}
            max={8}
            value={draft.maxConcurrency}
            onChange={(event) => setDraft({ ...draft, maxConcurrency: Number(event.target.value) })}
          />
        </label>
      </div>

      {draft.role === 'vision' ? (
        <label className="toggle-row">
          <input
            type="checkbox"
            checked={draft.supportsVision}
            onChange={(event) => setDraft({ ...draft, supportsVision: event.target.checked })}
          />
          此配置支持图片输入
        </label>
      ) : null}

      {draft.role === 'vision' ? (
        <label className="toggle-row">
          <input
            type="checkbox"
            checked={reuseTextProfile}
            onChange={(event) => setReuseTextProfile(event.target.checked)}
          />
          该服务同时用于文本模型（可在文本模型页填入相同配置）
        </label>
      ) : null}

      {error ? <p role="alert">{error}</p> : null}
      <div className="settings-actions">
        {onTest ? (
          <button type="button" disabled={testStatus === 'testing'} onClick={() => void testConnection()}>
            {testStatus === 'testing'
              ? '正在测试…'
              : draft.role === 'vision'
                ? '测试视觉能力'
                : '测试文本能力'}
          </button>
        ) : null}
        <button type="submit">保存模型配置</button>
      </div>
      {testStatus === 'passed' ? <p className="test-success" role="status">连接及能力测试通过</p> : null}
    </form>
  );
}
