/**
 * XiaoguaiLogo — the brand mark shown at the top of the chat-ui sidebar.
 *
 * A compact line-art monster (horns, two big eyes, a one-tooth smile) echoing
 * the project logo, drawn with `currentColor` so it inherits the sidebar's
 * text colour and adapts to light/dark themes. Self-contained (inline styles)
 * so it needs no stylesheet changes.
 *
 * The wordmark shows the owner's white-label assistant name when set
 * (`GET /v1/branding`), falling back to the locale's default ("Xiaoguai" /
 * "小怪").
 */
import { useI18n } from './i18n/I18nProvider';
import { useBrandName } from './branding';

export function XiaoguaiLogo({ size = 26 }: { size?: number }) {
  const { t } = useI18n();
  const name = useBrandName() || t.ui.assistant_name;
  return (
    <div
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: 8,
        padding: '2px 0 8px',
        fontWeight: 700,
        fontSize: 18,
        lineHeight: 1,
      }}
    >
      <svg
        width={size}
        height={size}
        viewBox="0 0 48 48"
        fill="none"
        stroke="currentColor"
        strokeWidth={2.2}
        strokeLinecap="round"
        strokeLinejoin="round"
        aria-hidden="true"
        focusable="false"
      >
        {/* horns */}
        <path d="M16 9c-1 3-3 4.5-5 4.5 2 1.2 4.2 0.2 5-2.2" />
        <path d="M32 9c1 3 3 4.5 5 4.5-2 1.2-4.2 0.2-5-2.2" />
        {/* head */}
        <path d="M11 23a13 13 0 0 1 26 0v8a8 8 0 0 1-8 8H19a8 8 0 0 1-8-8z" />
        {/* eyes */}
        <circle cx="19.5" cy="24" r="2.3" fill="currentColor" stroke="none" />
        <circle cx="28.5" cy="24" r="2.3" fill="currentColor" stroke="none" />
        {/* smile + one tooth */}
        <path d="M18.5 31c2 2.4 9 2.4 11 0" />
        <path d="M23.5 32v3" />
      </svg>
      <span>{name}</span>
    </div>
  );
}
