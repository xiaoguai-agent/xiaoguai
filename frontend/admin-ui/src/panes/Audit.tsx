export function AuditPane() {
  return (
    <>
      <h1>Audit Log</h1>
      <div className="empty">
        Audit log endpoint (`/v1/admin/audit`) is on the v0.6.1 backlog.
        The HMAC-chained log itself already runs server-side; this pane
        will surface a paginated, signature-verified view once the
        endpoint lands.
      </div>
    </>
  );
}
