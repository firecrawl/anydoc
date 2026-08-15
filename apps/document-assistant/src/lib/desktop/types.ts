export interface AppInfo {
  version: string;
  appDataDir: string;
}

export type TaskProgressStage =
  | 'queued'
  | 'parsing'
  | 'rendering'
  | 'vision_analysis'
  | 'text_synthesis'
  | 'completed'
  | 'failed';

export interface TaskProgressEvent {
  documentId: string;
  stage: TaskProgressStage;
  completed: number;
  total: number;
  message: string;
}

export const TASK_PROGRESS_EVENT = 'task-progress';
