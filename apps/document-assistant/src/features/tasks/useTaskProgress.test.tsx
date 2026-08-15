import { act, renderHook, waitFor } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import type { DocumentTask, TaskApi } from './taskApi';
import { useTaskProgress, type ListenTaskProgress } from './useTaskProgress';

const renderingTask: DocumentTask = {
  id: 'task-1',
  documentId: 'doc-1',
  stage: 'rendering',
  completed: 1,
  total: 3,
  message: '渲染中',
  error: null,
  failedPages: 0,
  createdAt: 1,
  updatedAt: 1,
};

describe('useTaskProgress', () => {
  it('refreshes from backend and then applies matching persisted events', async () => {
    let listener: ((task: DocumentTask) => void) | undefined;
    const listenProgress: ListenTaskProgress = async (handler) => {
      listener = handler;
      return () => undefined;
    };
    const api: TaskApi = {
      getTask: vi.fn().mockResolvedValue(renderingTask),
      pauseTask: vi.fn(),
      resumeTask: vi.fn(),
      cancelTask: vi.fn(),
      retryFailedPages: vi.fn(),
    };
    const { result } = renderHook(() =>
      useTaskProgress('task-1', { api, listenProgress }),
    );

    await waitFor(() => expect(result.current.task?.completed).toBe(1));
    act(() =>
      listener?.({
        ...renderingTask,
        stage: 'completed',
        completed: 3,
        updatedAt: 2,
      }),
    );

    expect(result.current.task?.stage).toBe('completed');
    expect(result.current.task?.completed).toBe(3);
  });
});
