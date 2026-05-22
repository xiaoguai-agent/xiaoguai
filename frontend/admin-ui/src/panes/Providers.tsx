export function ProvidersPane() {
  return (
    <>
      <h1>LLM Providers</h1>
      <div className="empty">
        Provider listing endpoint (`/v1/admin/providers`) is on the v0.6.1
        backlog. Until then use the CLI: <code>xiaoguai provider list</code>.
      </div>
    </>
  );
}
