import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { AppHeader } from './AppHeader';

describe('AppHeader', () => {
  it('opens model settings from the header action', async () => {
    const onOpenSettings = vi.fn();
    render(<AppHeader onOpenSettings={onOpenSettings} />);

    await userEvent.click(screen.getByRole('button', { name: '模型设置' }));

    expect(onOpenSettings).toHaveBeenCalledOnce();
  });
});
