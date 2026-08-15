import type { DocumentTask, TaskStage } from './taskApi';

interface TaskProgressProps {
  task: DocumentTask;
  onPause: () => void;
  onResume: () => void;
  onCancel: () => void;
  onRetry: () => void;
}

const STAGE_LABELS: Record<TaskStage, string> = {
  queued: '等待处理',
  parsing: '本地解析',
  rendering: '逐页渲染',
  vision_analysis: '视觉理解',
  text_synthesis: '内容整理',
  paused: '已暂停',
  completed: '已完成',
  failed: '处理失败',
  cancelled: '已取消',
};

export function TaskProgress({
  task,
  onPause,
  onResume,
  onCancel,
  onRetry,
}: TaskProgressProps) {
  const percentage = task.total > 0 ? (task.completed / task.total) * 100 : 0;
  const active = [
    'queued',
    'parsing',
    'rendering',
    'vision_analysis',
    'text_synthesis',
  ].includes(task.stage);

  return (
    <section className="task-progress" aria-label="文档处理进度">
      <div className="task-progress__heading">
        <div>
          <span className="status-dot" data-stage={task.stage} />
          <strong>{STAGE_LABELS[task.stage]}</strong>
        </div>
        <span>{task.total > 0 ? `${task.completed} / ${task.total} 页` : '准备中'}</span>
      </div>
      <div
        className="task-progress__track"
        role="progressbar"
        aria-valuemin={0}
        aria-valuemax={task.total}
        aria-valuenow={task.completed}
      >
        <span style={{ width: `${Math.min(100, percentage)}%` }} />
      </div>
      <p>{task.error ?? task.message}</p>
      {task.failedPages > 0 ? (
        <p className="task-progress__failed">失败页：{task.failedPages}</p>
      ) : null}
      <div className="task-progress__actions">
        {active ? <button onClick={onPause}>暂停</button> : null}
        {task.stage === 'paused' ? <button onClick={onResume}>继续</button> : null}
        {active || task.stage === 'paused' ? <button onClick={onCancel}>取消</button> : null}
        {task.stage === 'failed' ? <button onClick={onRetry}>重试失败页</button> : null}
      </div>
    </section>
  );
}
