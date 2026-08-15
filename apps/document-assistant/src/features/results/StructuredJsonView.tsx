interface StructuredJsonViewProps {
  value: unknown;
  diagnostic?: boolean;
}

export function StructuredJsonView({ value, diagnostic = false }: StructuredJsonViewProps) {
  const formatted = typeof value === 'string' ? value : JSON.stringify(value, null, 2);
  return (
    <section className="structured-json">
      <div className="structured-json__heading">
        <div>
          <h3>{diagnostic ? '原始模型响应' : '结构化分析数据'}</h3>
          {diagnostic ? <p role="status">诊断数据：该响应未通过结构校验，不作为有效分析使用。</p> : null}
        </div>
        <button type="button" onClick={() => void navigator.clipboard.writeText(formatted)}>
          复制结构数据
        </button>
      </div>
      <pre className="markdown-output">{formatted}</pre>
    </section>
  );
}
