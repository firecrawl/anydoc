import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { StructuredJsonView } from './StructuredJsonView';

describe('StructuredJsonView', () => {
  it('renders and copies versioned structured analysis JSON', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.assign(navigator, { clipboard: { writeText } });

    render(<StructuredJsonView value={{ schemaVersion: 1, theme: '测试文档' }} />);

    expect(screen.getByText(/"schemaVersion": 1/)).toBeVisible();
    await userEvent.click(screen.getByRole('button', { name: '复制结构数据' }));
    expect(writeText).toHaveBeenCalledWith(expect.stringContaining('"schemaVersion": 1'));
  });

  it('labels failed raw output as diagnostic data', () => {
    render(<StructuredJsonView value={'not-json'} diagnostic />);
    expect(screen.getByRole('status')).toHaveTextContent('诊断数据');
  });
});
