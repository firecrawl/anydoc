import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import type { ConversionState } from '../import/useDocumentConversion';
import { ConversionPanel } from './ConversionPanel';

const completed: ConversionState = {
  status: 'completed',
  result: {
    fileName: 'notes.rtf',
    format: 'rtf',
    markdown: '# Notes',
    characterCount: 7,
    elapsedMs: 12,
    document: { blocks: [] },
  },
};

describe('ConversionPanel', () => {
  it('shows conversion metadata and exports the markdown', async () => {
    const user = userEvent.setup();
    render(<ConversionPanel state={completed} activeView="markdown" />);

    expect(screen.getByText('notes.rtf')).toBeVisible();
    expect(screen.getByText('RTF')).toBeVisible();
    expect(screen.getByText('7 字符')).toBeVisible();
    expect(screen.getByText('12 ms')).toBeVisible();
    expect(screen.getByText('# Notes')).toBeVisible();

    await user.click(screen.getByRole('button', { name: '复制 Markdown' }));
    await expect(navigator.clipboard.readText()).resolves.toBe('# Notes');

    expect(screen.getByRole('link', { name: '下载 Markdown' })).toHaveAttribute(
      'download',
      'notes.md',
    );
  });

  it('shows the AnyDoc code and message when conversion fails', () => {
    render(
      <ConversionPanel
        activeView="markdown"
        state={{
          status: 'failed',
          error: { code: 'encrypted', message: 'document is encrypted' },
        }}
      />,
    );

    expect(screen.getByText('encrypted')).toBeVisible();
    expect(screen.getByText('document is encrypted')).toBeVisible();
  });
});
