/**
 * AiDisclosureBanner — unit tests
 *
 * Coverage:
 *  1. Renders by default (config: enabled=true, dismissible=true)
 *  2. Hides after dismiss within the same session (sessionStorage gate)
 *  3. Re-renders in a fresh session (sessionStorage cleared between tests)
 *  4. Respects dismissible=false — no dismiss button, banner always visible
 *  5. Hides when enabled=false
 *  6. Renders text_override instead of default translated text
 *  7. Renders "Learn more" link when link_to_disclosure is provided
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { AiDisclosureBanner } from './AiDisclosureBanner';
import type { AiDisclosureConfig } from '@xiaoguai/shared';

// Default config mirrors getAiDisclosureConfig fallback defaults.
const defaultConfig: AiDisclosureConfig = {
  enabled: true,
  dismissible: true,
};

// Each test gets a clean sessionStorage so dismiss state never bleeds.
beforeEach(() => {
  sessionStorage.clear();
  vi.restoreAllMocks();
});

afterEach(() => {
  sessionStorage.clear();
});

describe('AiDisclosureBanner', () => {
  it('renders the disclosure text by default', () => {
    render(<AiDisclosureBanner config={defaultConfig} />);
    expect(
      screen.getByText(/You are interacting with an AI assistant/i),
    ).toBeInTheDocument();
  });

  it('hides after the dismiss button is clicked within the same session', () => {
    render(<AiDisclosureBanner config={defaultConfig} />);
    const btn = screen.getByRole('button', { name: /dismiss/i });
    expect(btn).toBeInTheDocument();
    fireEvent.click(btn);
    expect(
      screen.queryByText(/You are interacting with an AI assistant/i),
    ).not.toBeInTheDocument();
    // sessionStorage flag should be set.
    expect(sessionStorage.getItem('xiaoguai.ai_disclosure.dismissed')).toBe('1');
  });

  it('re-renders in a new session (sessionStorage cleared)', () => {
    // First session: dismiss.
    const { unmount } = render(
      <AiDisclosureBanner config={defaultConfig} />,
    );
    fireEvent.click(screen.getByRole('button', { name: /dismiss/i }));
    expect(
      screen.queryByText(/You are interacting with an AI assistant/i),
    ).not.toBeInTheDocument();
    unmount();

    // Simulate a new browser session by clearing sessionStorage.
    sessionStorage.clear();

    render(<AiDisclosureBanner config={defaultConfig} />);
    expect(
      screen.getByText(/You are interacting with an AI assistant/i),
    ).toBeInTheDocument();
  });

  it('does not render a dismiss button when dismissible=false', () => {
    const cfg: AiDisclosureConfig = { enabled: true, dismissible: false };
    render(<AiDisclosureBanner config={cfg} />);
    expect(
      screen.getByText(/You are interacting with an AI assistant/i),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole('button', { name: /dismiss/i }),
    ).not.toBeInTheDocument();
  });

  it('remains visible after attempt to dismiss when dismissible=false', () => {
    const cfg: AiDisclosureConfig = { enabled: true, dismissible: false };
    render(<AiDisclosureBanner config={cfg} />);
    // No button — banner must still be present.
    expect(
      screen.getByRole('note'),
    ).toBeInTheDocument();
  });

  it('renders nothing when enabled=false', () => {
    const cfg: AiDisclosureConfig = { enabled: false, dismissible: true };
    render(<AiDisclosureBanner config={cfg} />);
    expect(screen.queryByRole('note')).not.toBeInTheDocument();
  });

  it('uses text_override instead of default translated text', () => {
    const cfg: AiDisclosureConfig = {
      enabled: true,
      dismissible: true,
      text_override: 'Custom operator disclosure text.',
    };
    render(<AiDisclosureBanner config={cfg} />);
    expect(screen.getByText(/Custom operator disclosure text\./i)).toBeInTheDocument();
    expect(
      screen.queryByText(/You are interacting with an AI assistant/i),
    ).not.toBeInTheDocument();
  });

  it('renders a "Learn more" link when link_to_disclosure is provided', () => {
    const cfg: AiDisclosureConfig = {
      enabled: true,
      dismissible: true,
      link_to_disclosure: 'https://example.com/ai-transparency',
    };
    render(<AiDisclosureBanner config={cfg} />);
    const link = screen.getByRole('link', { name: /learn more/i });
    expect(link).toBeInTheDocument();
    expect(link).toHaveAttribute('href', 'https://example.com/ai-transparency');
    expect(link).toHaveAttribute('target', '_blank');
  });
});
