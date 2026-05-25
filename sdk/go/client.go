package xiaoguai

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"math"
	"net/http"
	"strings"
	"time"
)

const (
	defaultTimeout    = 30 * time.Second
	maxRetries        = 3
	retryBaseDelay    = 100 * time.Millisecond
	retryMaxDelay     = 2 * time.Second
)

// Logger is an optional interface for verbose request/response logging.
// Pass an implementation via WithLogger.
type Logger interface {
	Printf(format string, args ...interface{})
}

// Client is the Xiaoguai REST API client.
// All methods are safe for concurrent use.
type Client struct {
	baseURL    string
	token      string
	httpClient *http.Client
	log        Logger
}

// Option is a functional option for NewClient.
type Option func(*Client)

// WithToken sets the Bearer token sent in the Authorization header.
func WithToken(token string) Option {
	return func(c *Client) { c.token = token }
}

// WithHTTPClient replaces the default http.Client (e.g. for transport injection in tests).
func WithHTTPClient(hc *http.Client) Option {
	return func(c *Client) { c.httpClient = hc }
}

// WithLogger attaches a logger for verbose request/response tracing.
func WithLogger(l Logger) Option {
	return func(c *Client) { c.log = l }
}

// WithTimeout overrides the default 30 s per-request timeout.
func WithTimeout(d time.Duration) Option {
	return func(c *Client) { c.httpClient.Timeout = d }
}

// NewClient constructs a Client targeting baseURL.
// baseURL must NOT include a trailing /v1 segment (e.g. "http://localhost:8080").
func NewClient(baseURL string, opts ...Option) (*Client, error) {
	if baseURL == "" {
		return nil, fmt.Errorf("xiaoguai: baseURL must not be empty")
	}
	c := &Client{
		baseURL: strings.TrimRight(baseURL, "/"),
		httpClient: &http.Client{
			Timeout: defaultTimeout,
		},
	}
	for _, o := range opts {
		o(c)
	}
	return c, nil
}

// ---------------------------------------------------------------------------
// Internal HTTP helpers
// ---------------------------------------------------------------------------

func (c *Client) get(ctx context.Context, path string, queryParams map[string]string) ([]byte, error) {
	url := c.baseURL + path
	if len(queryParams) > 0 {
		url += "?"
		parts := make([]string, 0, len(queryParams))
		for k, v := range queryParams {
			parts = append(parts, k+"="+v)
		}
		url += strings.Join(parts, "&")
	}
	return c.doWithRetry(ctx, http.MethodGet, url, nil)
}

func (c *Client) post(ctx context.Context, path string, body interface{}) ([]byte, error) {
	data, err := json.Marshal(body)
	if err != nil {
		return nil, fmt.Errorf("xiaoguai: marshal request: %w", err)
	}
	return c.doWithRetry(ctx, http.MethodPost, c.baseURL+path, data)
}

func (c *Client) delete(ctx context.Context, path string) ([]byte, error) {
	return c.doWithRetry(ctx, http.MethodDelete, c.baseURL+path, nil)
}

func (c *Client) doWithRetry(ctx context.Context, method, url string, body []byte) ([]byte, error) {
	var lastErr error
	for attempt := 0; attempt < maxRetries; attempt++ {
		if attempt > 0 {
			delay := retryDelay(attempt)
			select {
			case <-ctx.Done():
				return nil, ctx.Err()
			case <-time.After(delay):
			}
		}

		respBody, statusCode, err := c.do(ctx, method, url, body)
		if err != nil {
			// Network / context error — don't retry context cancellations.
			if ctx.Err() != nil {
				return nil, ctx.Err()
			}
			lastErr = err
			continue
		}

		// Retry on 5xx only.
		if statusCode >= 500 {
			if c.log != nil {
				c.log.Printf("xiaoguai: %s %s → %d (attempt %d/%d)", method, url, statusCode, attempt+1, maxRetries)
			}
			lastErr = newHTTPError(statusCode, respBody, extractMessage(respBody))
			continue
		}

		// Non-5xx: classify and return immediately (don't retry 4xx).
		if statusCode < 200 || statusCode >= 300 {
			return nil, newHTTPError(statusCode, respBody, extractMessage(respBody))
		}
		return respBody, nil
	}
	return nil, lastErr
}

func (c *Client) do(ctx context.Context, method, url string, body []byte) ([]byte, int, error) {
	var bodyReader io.Reader
	if body != nil {
		bodyReader = bytes.NewReader(body)
	}

	req, err := http.NewRequestWithContext(ctx, method, url, bodyReader)
	if err != nil {
		return nil, 0, fmt.Errorf("xiaoguai: build request: %w", err)
	}
	req.Header.Set("Accept", "application/json")
	if body != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	if c.token != "" {
		req.Header.Set("Authorization", "Bearer "+c.token)
	}

	resp, err := c.httpClient.Do(req)
	if err != nil {
		return nil, 0, fmt.Errorf("xiaoguai: execute request: %w", err)
	}
	defer resp.Body.Close() //nolint:errcheck

	respBody, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, resp.StatusCode, fmt.Errorf("xiaoguai: read response body: %w", err)
	}
	return respBody, resp.StatusCode, nil
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

func retryDelay(attempt int) time.Duration {
	d := float64(retryBaseDelay) * math.Pow(2, float64(attempt-1))
	if d > float64(retryMaxDelay) {
		d = float64(retryMaxDelay)
	}
	return time.Duration(d)
}

func extractMessage(body []byte) string {
	var m map[string]interface{}
	if err := json.Unmarshal(body, &m); err == nil {
		if msg, ok := m["error"].(string); ok {
			return msg
		}
		if msg, ok := m["message"].(string); ok {
			return msg
		}
	}
	return ""
}

func decode[T any](data []byte) (T, error) {
	var v T
	if err := json.Unmarshal(data, &v); err != nil {
		return v, fmt.Errorf("xiaoguai: decode response: %w", err)
	}
	return v, nil
}
