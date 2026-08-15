import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { RecoveryDialog } from './RecoveryDialog';
import type { DocumentTask } from './taskApi';

const task: DocumentTask = {
  id: 'task-1', documentId: 'doc-1', stage: 'paused', completed: 2, total: 5,
  message: '用户已暂停', error: null, failedPages: 0, createdAt: 1, updatedAt: 2,
};

describe('RecoveryDialog', () => {
  it('offers explicit resume, later, and cancel choices', async () => {
    const onResume = vi.fn();
    render(<RecoveryDialog tasks={[task]} onResume={onResume} onLater={() => undefined} onCancel={() => undefined} />);
    expect(screen.getByText('已完成 2 / 5 页')).toBeVisible();
    await userEvent.click(screen.getByRole('button', { name: '继续' }));
    expect(onResume).toHaveBeenCalledWith('task-1');
    expect(screen.getByRole('button', { name: '稍后' })).toBeVisible();
    expect(screen.getByRole('button', { name: '取消任务' })).toBeVisible();
  });
});
