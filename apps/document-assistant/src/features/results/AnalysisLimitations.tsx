interface AnalysisLimitationsProps {
  limitations: string[];
}

export function AnalysisLimitations({ limitations }: AnalysisLimitationsProps) {
  if (limitations.length === 0) return null;

  return (
    <section className="limitations" aria-labelledby="limitations-title">
      <h3 id="limitations-title">分析范围与限制</h3>
      <ul>
        {limitations.map((limitation) => <li key={limitation}>{limitation}</li>)}
      </ul>
    </section>
  );
}
