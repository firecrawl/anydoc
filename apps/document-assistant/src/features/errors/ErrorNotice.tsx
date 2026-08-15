interface ErrorNoticeProps {
  error: string;
  onOpenSettings?: () => void;
  onContinueTextOnly?: () => void;
}

function guidance(error: string) {
  const normalized = error.toLowerCase();
  if (normalized.includes('401') || normalized.includes('403') || normalized.includes('authorization')) {
    return { title: '模型授权失败', message: '请检查模型配置、API Key 和服务权限。', action: 'settings' as const };
  }
  if (normalized.includes('429') || normalized.includes('rate limit')) {
    return { title: '请求过于频繁', message: '模型服务已限流，请稍后重试。', action: null };
  }
  if (normalized.includes('encrypt') || normalized.includes('密码')) {
    return { title: '文档受密码保护', message: '请先解除文件密码保护，再重新导入。', action: null };
  }
  if (normalized.includes('renderer') || normalized.includes('office') || normalized.includes('libreoffice')) {
    return { title: '无法生成页面图像', message: '可以安装 Office/LibreOffice，或继续纯文本分析。', action: 'text' as const };
  }
  if (normalized.includes('json') || normalized.includes('schema')) {
    return { title: '模型返回结构无效', message: '原始响应已保存为诊断数据，可重试失败页。', action: null };
  }
  return { title: '处理遇到问题', message: error, action: null };
}

export function ErrorNotice({ error, onOpenSettings, onContinueTextOnly }: ErrorNoticeProps) {
  const detail = guidance(error);
  return (
    <section className="error-notice" role="alert">
      <strong>{detail.title}</strong>
      <p>{detail.message}</p>
      {detail.action === 'settings' && onOpenSettings ? <button type="button" onClick={onOpenSettings}>检查模型配置</button> : null}
      {detail.action === 'text' && onContinueTextOnly ? <button type="button" onClick={onContinueTextOnly}>继续纯文本分析</button> : null}
    </section>
  );
}
