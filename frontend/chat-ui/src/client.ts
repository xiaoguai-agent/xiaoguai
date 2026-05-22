import { XiaoguaiClient } from '@xiaoguai/shared';

const baseUrl =
  (import.meta.env.VITE_API_URL as string | undefined) ??
  (typeof window !== 'undefined' ? window.location.origin : 'http://localhost:8080');

export const client = new XiaoguaiClient({ baseUrl });
