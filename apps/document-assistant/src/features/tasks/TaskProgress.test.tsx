import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { TaskProgress } from './TaskProgress';
import type { DocumentTask } from './taskApi';

const task: DocumentTask = {
  id: 'task-1',
  documentId: 'doc-1',
  stage: 'rendering',
  completed: 2,
  total: 5,
  message: '正在生成第 3 页',
  error: null,
  failedPages: 1,
  createdAt: 1,
  updatedAt: 2,
};

describe('TaskProgress', () => {
  it('shows persisted progress and invokes pause control', () => {
    const onPause = vi.fn();
    render(
      <TaskProgress
        task={task}
        onPause={onPause}
        onResume={vi.fn()}
        onCancel={vi.fn()}
        onRetry={vi.fn()}
      />,
    );

    expect(screen.getByText('逐页渲染')).toBeInTheDocument();
    expect(screen.getByText('2 / 5 页')).toBeInTheDocument();
    expect(screen.getByText('失败页：1')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: '暂停' }));
    expect(onPause).toHaveBeenCalledTimes(1);
  });

  it('offers retry only for failed tasks', () => {
    render(
      <TaskProgress
        task={{ ...task, stage: 'failed', error: '模型超时' }}
        onPause={vi.fn()}
        onResume={vi.fn()}
        onCancel={vi.fn()}
        onRetry={vi.fn()}
      />,
    );

    expect(screen.getByRole('button', { name: '重试失败页' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: '暂停' })).not.toBeInTheDocument();
  });
});
