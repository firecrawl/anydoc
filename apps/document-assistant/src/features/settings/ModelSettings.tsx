import { useState, type FormEvent } from 'react';
import type { ModelProfile } from './modelProfile';

interface ModelSettingsProps {
  profile: ModelProfile;
  hasApiKey: boolean;
  onSave: (profile: ModelProfile) => Promise<void>;
  onSetApiKey: (apiKey: string) => Promise<void>;
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
}: ModelSettingsProps) {
  const [draft, setDraft] = useState(profile);
  const [apiKey, setApiKey] = useState('');
  const [reuseTextProfile, setReuseTextProfile] = useState(false);
  const [error, setError] = useState<string>();

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
      <div className="settings-role" role="group" aria-label="模型角色">
        <button
          type="button"
          aria-pressed={draft.role === 'vision'}
          onClick={() => setDraft({ ...draft, role: 'vision', supportsVision: true })}
        >
          视觉模型
        </button>
        <button
          type="button"
          aria-pressed={draft.role === 'text'}
          onClick={() => setDraft({ ...draft, role: 'text' })}
        >
          文本模型
        </button>
      </div>

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

      <label className="toggle-row">
        <input
          type="checkbox"
          checked={reuseTextProfile}
          onChange={(event) => setReuseTextProfile(event.target.checked)}
        />
        复用文本模型配置
      </label>

      {error ? <p role="alert">{error}</p> : null}
      <button type="submit">保存模型配置</button>
    </form>
  );
}
