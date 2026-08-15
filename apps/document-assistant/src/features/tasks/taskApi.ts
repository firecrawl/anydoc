import { invokeCommand } from '../../lib/desktop/api';

export type TaskStage =
  | 'queued'
  | 'parsing'
  | 'rendering'
  | 'vision_analysis'
  | 'text_synthesis'
  | 'paused'
  | 'completed'
  | 'failed'
  | 'cancelled';

export interface DocumentTask {
  id: string;
  documentId: string;
  stage: TaskStage;
  completed: number;
  total: number;
  message: string;
  error: string | null;
  failedPages: number;
  createdAt: number;
  updatedAt: number;
}

export interface TaskApi {
  getTask(taskId: string): Promise<DocumentTask>;
  pauseTask(taskId: string): Promise<DocumentTask>;
  resumeTask(taskId: string): Promise<DocumentTask>;
  cancelTask(taskId: string): Promise<DocumentTask>;
  retryFailedPages(taskId: string): Promise<DocumentTask>;
}

export const taskApi: TaskApi = {
  getTask: (taskId) => invokeCommand('get_task', { taskId }),
  pauseTask: (taskId) => invokeCommand('pause_task', { taskId }),
  resumeTask: (taskId) => invokeCommand('resume_task', { taskId }),
  cancelTask: (taskId) => invokeCommand('cancel_task', { taskId }),
  retryFailedPages: (taskId) => invokeCommand('retry_failed_pages', { taskId }),
};

export function startDocumentTask(documentId: string): Promise<DocumentTask> {
  return invokeCommand('start_document_task', { documentId });
}

export function listRecoverableTasks(): Promise<DocumentTask[]> {
  return invokeCommand('list_recoverable_tasks');
}
