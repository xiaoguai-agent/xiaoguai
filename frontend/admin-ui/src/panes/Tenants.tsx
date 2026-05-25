import { useTranslation } from 'react-i18next';

export function TenantsPane() {
  const { t } = useTranslation();
  return (
    <>
      <h1>{t('pane.tenants.title')}</h1>
      <div className="empty">{t('pane.tenants.placeholder')}</div>
    </>
  );
}
