export function TenantsPane() {
  return (
    <>
      <h1>Tenants</h1>
      <div className="empty">
        Tenant management endpoint (`/v1/admin/tenants`) is on the v0.6.1
        backlog — once that ships, this pane will list / create / archive
        tenants.
      </div>
    </>
  );
}
