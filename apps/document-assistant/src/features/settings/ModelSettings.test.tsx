import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { ModelSettings } from './ModelSettings';
import type { ModelProfile } from './modelProfile';

const savedProfile: ModelProfile = {
  id: 'vision-primary',
  role: 'vision',
  baseUrl: 'https://api.example.com/v1',
  model: 'vision-model',
  supportsVision: true,
  timeoutSecs: 120,
  maxConcurrency: 2,
};

describe('ModelSettings', () => {
  it('never renders a saved key in plaintext', () => {
    render(
      <ModelSettings
        profile={savedProfile}
        hasApiKey
        onSave={async () => undefined}
        onSetApiKey={async () => undefined}
      />,
    );

    expect(screen.getByLabelText('API Key')).toHaveValue('');
    expect(screen.queryByDisplayValue('sk-test')).not.toBeInTheDocument();
    expect(screen.getByText('密钥已安全保存')).toBeVisible();
  });

  it('rejects insecure remote model endpoints', async () => {
    const onSave = vi.fn().mockResolvedValue(undefined);
    render(
      <ModelSettings
        profile={{ ...savedProfile, baseUrl: 'http://api.example.com/v1' }}
        hasApiKey={false}
        onSave={onSave}
        onSetApiKey={async () => undefined}
      />,
    );

    await userEvent.click(screen.getByRole('button', { name: '保存模型配置' }));

    expect(screen.getByRole('alert')).toHaveTextContent('远程地址必须使用 HTTPS');
    expect(onSave).not.toHaveBeenCalled();
  });
});
