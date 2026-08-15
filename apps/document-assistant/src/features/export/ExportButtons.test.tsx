import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { ExportButtons } from './ExportButtons';

describe('ExportButtons', () => {
  it('exports enhanced Markdown and shows destination and size', async () => {
    const exportDocument = vi.fn().mockResolvedValue({ destination: 'D:\\报告.md', bytesWritten: 2048 });
    render(<ExportButtons documentId="doc-1" exportDocument={exportDocument} />);
    await userEvent.click(screen.getByRole('button', { name: '导出增强 Markdown' }));
    expect(exportDocument).toHaveBeenCalledWith('doc-1', 'enhanced_markdown');
    expect(await screen.findByRole('status')).toHaveTextContent('D:\\报告.md');
    expect(screen.getByRole('status')).toHaveTextContent('2.0 KB');
  });
});
