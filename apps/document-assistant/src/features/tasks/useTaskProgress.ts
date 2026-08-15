import { listen } from '@tauri-apps/api/event';
import { useCallback, useEffect, useState } from 'react';
import { taskApi, type DocumentTask, type TaskApi } from './taskApi';

export type ListenTaskProgress = (
  handler: (task: DocumentTask) => void,
) => Promise<() => void>;

const listenToTaskProgress: ListenTaskProgress = async (handler) => {
  const unlisten = await listen<DocumentTask>('task-progress', (event) => {
    handler(event.payload);
  });
  return unlisten;
};

interface TaskProgressDependencies {
  api?: TaskApi;
  listenProgress?: ListenTaskProgress;
}

export function useTaskProgress(
  taskId: string | null,
  dependencies: TaskProgressDependencies = {},
) {
  const api = dependencies.api ?? taskApi;
  const listenProgress = dependencies.listenProgress ?? listenToTaskProgress;
  const [task, setTask] = useState<DocumentTask | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    if (!taskId) {
      setTask(null);
      return null;
    }
    try {
      const latest = await api.getTask(taskId);
      setTask(latest);
      setError(null);
      return latest;
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
      return null;
    }
  }, [api, taskId]);

  useEffect(() => {
    if (!taskId) return undefined;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void refresh();
    void listenProgress((nextTask) => {
      if (!disposed && nextTask.id === taskId) {
        setTask(nextTask);
      }
    }).then((cleanup) => {
      if (disposed) cleanup();
      else unlisten = cleanup;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [listenProgress, refresh, taskId]);

  const runAction = useCallback(
    async (action: (id: string) => Promise<DocumentTask>) => {
      if (!taskId) return null;
      try {
        const nextTask = await action(taskId);
        setTask(nextTask);
        setError(null);
        return await refresh();
      } catch (reason) {
        setError(reason instanceof Error ? reason.message : String(reason));
        return null;
      }
    },
    [refresh, taskId],
  );

  return {
    task,
    error,
    refresh,
    pause: () => runAction(api.pauseTask),
    resume: () => runAction(api.resumeTask),
    cancel: () => runAction(api.cancelTask),
    retry: () => runAction(api.retryFailedPages),
  };
}
