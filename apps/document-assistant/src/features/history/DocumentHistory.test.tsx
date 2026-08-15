import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { DocumentHistory, type HistoryDocument } from './DocumentHistory';

const documents: HistoryDocument[] = [
  {
    id: 'doc-1',
    fileName: '季度报告.docx',
    format: 'docx',
    status: 'completed',
    updatedAt: 1_723_700_001,
    cacheSize: 2048,
  },
];

describe('DocumentHistory', () => {
  it('warns that deleting history preserves the source file', async () => {
    const onDelete = vi.fn().mockResolvedValue(undefined);
    render(
      <DocumentHistory
        documents={documents}
        onDelete={onDelete}
        onClearAll={async () => undefined}
      />,
    );

    await userEvent.click(screen.getByRole('button', { name: '删除缓存' }));

    expect(screen.getByText('只删除分析缓存，不删除原文件')).toBeVisible();
    await userEvent.click(screen.getByRole('button', { name: '确认删除缓存' }));
    expect(onDelete).toHaveBeenCalledWith('doc-1');
  });

  it('shows total size and count before clearing all caches', async () => {
    render(
      <DocumentHistory
        documents={documents}
        onDelete={async () => undefined}
        onClearAll={async () => undefined}
      />,
    );

    await userEvent.click(screen.getByRole('button', { name: '清理全部缓存' }));

    const confirmation = screen.getByRole('alertdialog', {
      name: '确认清理全部缓存',
    });
    expect(within(confirmation).getByText(/1 个文档/)).toBeVisible();
    expect(within(confirmation).getByText(/2 KB/)).toBeVisible();
  });
});
