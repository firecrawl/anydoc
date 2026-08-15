import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { ErrorNotice } from './ErrorNotice';

describe('ErrorNotice', () => {
  it('maps authorization and encrypted errors to actionable Chinese guidance', () => {
    const { rerender } = render(<ErrorNotice error="model returned HTTP 401" />);
    expect(screen.getByText(/检查模型配置/)).toBeVisible();
    rerender(<ErrorNotice error="document is encrypted" />);
    expect(screen.getByText(/解除文件密码保护/)).toBeVisible();
  });
});
