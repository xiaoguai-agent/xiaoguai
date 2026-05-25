import { useTranslation } from 'react-i18next';

export function ProvidersPane() {
  const { t } = useTranslation();
  return (
    <>
      <h1>{t('pane.providers.title')}</h1>
      <div className="empty">{t('pane.providers.placeholder')}</div>
    </>
  );
}
