package xiaoguai

import "fmt"

// HTTPError is the base error type for all non-2xx API responses.
// Use errors.As to check for specific sub-types.
type HTTPError struct {
	StatusCode int
	Body       []byte
	Message    string
}

func (e *HTTPError) Error() string {
	if e.Message != "" {
		return fmt.Sprintf("HTTP %d: %s", e.StatusCode, e.Message)
	}
	return fmt.Sprintf("HTTP %d: %s", e.StatusCode, string(e.Body))
}

// AuthError is returned for 401 Unauthorized responses.
type AuthError struct{ *HTTPError }

func (e *AuthError) Error() string { return "auth error: " + e.HTTPError.Error() }
func (e *AuthError) Unwrap() error { return e.HTTPError }

// NotFoundError is returned for 404 Not Found responses.
type NotFoundError struct{ *HTTPError }

func (e *NotFoundError) Error() string { return "not found: " + e.HTTPError.Error() }
func (e *NotFoundError) Unwrap() error { return e.HTTPError }

// ValidationError is returned for 400 / 422 responses (bad request body or params).
type ValidationError struct{ *HTTPError }

func (e *ValidationError) Error() string { return "validation error: " + e.HTTPError.Error() }
func (e *ValidationError) Unwrap() error { return e.HTTPError }

// ConflictError is returned for 409 responses (e.g. pack already installed).
type ConflictError struct{ *HTTPError }

func (e *ConflictError) Error() string { return "conflict: " + e.HTTPError.Error() }
func (e *ConflictError) Unwrap() error { return e.HTTPError }

// RateLimitError is returned for 429 Too Many Requests responses.
type RateLimitError struct{ *HTTPError }

func (e *RateLimitError) Error() string { return "rate limit: " + e.HTTPError.Error() }
func (e *RateLimitError) Unwrap() error { return e.HTTPError }

// ServerError is returned for 5xx responses that survive all retry attempts.
type ServerError struct{ *HTTPError }

func (e *ServerError) Error() string { return "server error: " + e.HTTPError.Error() }
func (e *ServerError) Unwrap() error { return e.HTTPError }

// newHTTPError constructs the narrowest error type for the given status code.
func newHTTPError(statusCode int, body []byte, message string) error {
	base := &HTTPError{StatusCode: statusCode, Body: body, Message: message}
	switch {
	case statusCode == 401:
		return &AuthError{base}
	case statusCode == 404:
		return &NotFoundError{base}
	case statusCode == 400 || statusCode == 422:
		return &ValidationError{base}
	case statusCode == 409:
		return &ConflictError{base}
	case statusCode == 429:
		return &RateLimitError{base}
	case statusCode >= 500:
		return &ServerError{base}
	default:
		return base
	}
}
