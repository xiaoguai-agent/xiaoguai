/**
 * XiaoguaiError hierarchy.
 *
 * All errors thrown by XiaoguaiClient extend XiaoguaiError, so callers
 * can use a single `catch (e) { if (e instanceof XiaoguaiError) ... }` guard.
 */

export class XiaoguaiError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "XiaoguaiError";
    // Maintain proper prototype chain in compiled JS (TypeScript caveat).
    Object.setPrototypeOf(this, new.target.prototype);
  }
}

/** Any non-2xx HTTP response. Subclasses narrow by status range. */
export class HttpError extends XiaoguaiError {
  readonly status: number;
  readonly body: unknown;

  constructor(status: number, body: unknown) {
    const message = `HTTP ${status}: ${_extractMessage(body)}`;
    super(message);
    this.name = "HttpError";
    this.status = status;
    this.body = body;
    Object.setPrototypeOf(this, new.target.prototype);
  }
}

/** 401 Unauthorized — missing or invalid bearer token. */
export class AuthError extends HttpError {
  constructor(body: unknown) {
    super(401, body);
    this.name = "AuthError";
    Object.setPrototypeOf(this, new.target.prototype);
  }
}

/** 403 Forbidden — valid token but insufficient permissions. */
export class ForbiddenError extends HttpError {
  constructor(body: unknown) {
    super(403, body);
    this.name = "ForbiddenError";
    Object.setPrototypeOf(this, new.target.prototype);
  }
}

/** 404 Not Found. */
export class NotFoundError extends HttpError {
  constructor(body: unknown) {
    super(404, body);
    this.name = "NotFoundError";
    Object.setPrototypeOf(this, new.target.prototype);
  }
}

/** 409 Conflict — e.g. pack already installed. */
export class ConflictError extends HttpError {
  constructor(body: unknown) {
    super(409, body);
    this.name = "ConflictError";
    Object.setPrototypeOf(this, new.target.prototype);
  }
}

/** 422 Unprocessable Entity / 400 Bad Request — validation errors. */
export class ValidationError extends HttpError {
  constructor(status: number, body: unknown) {
    super(status, body);
    this.name = "ValidationError";
    Object.setPrototypeOf(this, new.target.prototype);
  }
}

/** 429 Too Many Requests. */
export class RateLimitError extends HttpError {
  /** Value of `Retry-After` header in seconds, if provided. */
  readonly retryAfter?: number;

  constructor(body: unknown, retryAfter?: number) {
    super(429, body);
    this.name = "RateLimitError";
    this.retryAfter = retryAfter;
    Object.setPrototypeOf(this, new.target.prototype);
  }
}

/** 5xx Server Error. */
export class ServerError extends HttpError {
  constructor(status: number, body: unknown) {
    super(status, body);
    this.name = "ServerError";
    Object.setPrototypeOf(this, new.target.prototype);
  }
}

/**
 * Feature not yet implemented server-side.
 * Thrown by SDK methods that are defined in the interface but have no
 * corresponding server endpoint yet (mirrors Python SDK's NotImplementedError).
 */
export class NotImplementedError extends XiaoguaiError {
  constructor(message: string) {
    super(message);
    this.name = "NotImplementedError";
    Object.setPrototypeOf(this, new.target.prototype);
  }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

function _extractMessage(body: unknown): string {
  if (typeof body === "string") return body;
  if (body && typeof body === "object") {
    const b = body as Record<string, unknown>;
    if (typeof b["error"] === "string") return b["error"];
    if (typeof b["message"] === "string") return b["message"];
    try {
      return JSON.stringify(body);
    } catch {
      return String(body);
    }
  }
  return String(body ?? "unknown error");
}

/**
 * Map an HTTP status code + body to the appropriate error subclass.
 * Does nothing if status is in the 2xx range.
 */
export function throwForStatus(
  status: number,
  body: unknown,
  headers?: Record<string, string>,
): void {
  if (status >= 200 && status < 300) return;

  switch (status) {
    case 400:
    case 422:
      throw new ValidationError(status, body);
    case 401:
      throw new AuthError(body);
    case 403:
      throw new ForbiddenError(body);
    case 404:
      throw new NotFoundError(body);
    case 409:
      throw new ConflictError(body);
    case 429: {
      const ra = headers?.["retry-after"];
      throw new RateLimitError(body, ra ? Number(ra) : undefined);
    }
    default:
      if (status >= 500) throw new ServerError(status, body);
      throw new HttpError(status, body);
  }
}
