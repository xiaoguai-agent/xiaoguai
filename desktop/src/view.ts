// DOM rendering for the message stream. Pure view helpers — no Tauri / network
// here. The controller (main.ts) owns state and calls these to mutate the DOM.

export type Role = 'user' | 'assistant' | 'tool' | 'system';

/** Resolve a required element by id, throwing a clear error if the HTML drifts. */
export function el<T extends HTMLElement>(id: string): T {
  const node = document.getElementById(id);
  if (!node) throw new Error(`missing #${id} in index.html`);
  return node as T;
}

/** Append a new bubble and return it so callers can stream text into it. */
export function addBubble(container: HTMLElement, role: Role, text = ''): HTMLDivElement {
  const bubble = document.createElement('div');
  bubble.className = `bubble ${role}`;
  bubble.textContent = text;
  container.appendChild(bubble);
  scrollToBottom(container);
  return bubble;
}

/** Append text to an existing bubble (streaming deltas). */
export function appendText(bubble: HTMLDivElement, delta: string): void {
  bubble.textContent = (bubble.textContent ?? '') + delta;
  if (bubble.parentElement) scrollToBottom(bubble.parentElement);
}

/** Render a one-line tool-call indicator bubble. */
export function addToolLine(container: HTMLElement, text: string, isError: boolean): void {
  const bubble = addBubble(container, 'tool', text);
  if (isError) bubble.classList.add('tool-error');
}

/** Keep the latest message in view. */
export function scrollToBottom(container: HTMLElement): void {
  container.scrollTop = container.scrollHeight;
}

/** Set the small status text in the title bar. */
export function setStatus(node: HTMLElement, text: string): void {
  node.textContent = text;
}

/** Grow a single-line textarea up to a few rows as the user types. */
export function autoGrow(textarea: HTMLTextAreaElement, maxPx = 120): void {
  textarea.style.height = 'auto';
  textarea.style.height = `${Math.min(textarea.scrollHeight, maxPx)}px`;
}
