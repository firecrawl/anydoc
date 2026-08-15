import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { ModelSettings } from './ModelSettings';
import type { ModelProfile } from './modelProfile';

const profile: ModelProfile = {
  id: 'vision-primary',
  role: 'vision',
  baseUrl: 'https://api.example.com/v1',
  model: 'vision-model',
  supportsVision: true,
  timeoutSecs: 120,
  maxConcurrency: 2,
};

describe('ModelSettings integration', () => {
  it('tests the draft profile without replacing saved settings on failure', async () => {
    const onSave = vi.fn().mockResolvedValue(undefined);
    const onTest = vi.fn().mockRejectedValue(new Error('模型未返回预期内容'));
    render(
      <ModelSettings
        profile={profile}
        hasApiKey
        onSave={onSave}
        onSetApiKey={async () => undefined}
        onTest={onTest}
      />,
    );

    await userEvent.click(screen.getByRole('button', { name: '测试视觉能力' }));

    expect(onTest).toHaveBeenCalledWith(profile, '');
    expect(await screen.findByRole('alert')).toHaveTextContent('模型未返回预期内容');
    expect(onSave).not.toHaveBeenCalled();
  });
});
