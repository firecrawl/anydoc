import type { DocumentTask } from './taskApi';

interface RecoveryDialogProps {
  tasks: DocumentTask[];
  onResume: (taskId: string) => void;
  onLater: () => void;
  onCancel: (taskId: string) => void;
}

export function RecoveryDialog({ tasks, onResume, onLater, onCancel }: RecoveryDialogProps) {
  if (tasks.length === 0) return null;
  return (
    <div className="dialog-backdrop">
      <section className="recovery-dialog" role="dialog" aria-modal="true" aria-labelledby="recovery-title">
        <p className="eyebrow">Task recovery</p>
        <h2 id="recovery-title">发现未完成的文档任务</h2>
        <p>应用不会自动恢复模型请求，请选择如何处理。</p>
        <div className="recovery-list">
          {tasks.map((task) => (
            <article key={task.id}>
              <div><strong>文档任务</strong><span>{task.message}</span></div>
              <p>已完成 {task.completed} / {task.total} 页</p>
              <div className="recovery-actions">
                <button type="button" onClick={() => onResume(task.id)}>继续</button>
                <button type="button" onClick={() => onCancel(task.id)}>取消任务</button>
              </div>
            </article>
          ))}
        </div>
        <button type="button" onClick={onLater}>稍后</button>
      </section>
    </div>
  );
}
