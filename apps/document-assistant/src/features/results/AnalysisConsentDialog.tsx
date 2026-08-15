interface AnalysisConsentDialogProps {
  sendsImages: boolean;
  visionModel: string | null;
  textModel: string;
  onConfirm: () => void;
  onCancel: () => void;
}

export function AnalysisConsentDialog({ sendsImages, visionModel, textModel, onConfirm, onCancel }: AnalysisConsentDialogProps) {
  return (
    <div className="dialog-backdrop">
      <section className="consent-dialog" role="dialog" aria-modal="true" aria-labelledby="consent-title">
        <p className="eyebrow">Remote model consent</p>
        <h2 id="consent-title">确认发送文档内容</h2>
        <p>本地解析已经完成。继续后，以下内容会发送到你配置的模型服务：</p>
        <ul>
          <li>提取文本 → {textModel || '文本模型'}</li>
          {sendsImages ? <li>逐页 PNG 图像 → {visionModel || '视觉模型'}</li> : null}
        </ul>
        <p className="consent-note">只记录本次同意的时间和模型配置 ID；API Key 不会写入文档数据库。</p>
        <div className="consent-actions">
          <button type="button" onClick={onCancel}>取消</button>
          <button type="button" onClick={onConfirm}>同意并开始分析</button>
        </div>
      </section>
    </div>
  );
}
