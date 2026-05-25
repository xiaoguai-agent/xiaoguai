// Package xiaoguai provides a Go client for the Xiaoguai agent-platform REST API.
//
// # Wave-3 Endpoints
//
// The client covers three API surface areas:
//
//   - HotL (Human-on-the-Loop) boundary policy management
//   - Outcomes ROI telemetry
//   - Skills pack marketplace
//
// # Quick Start
//
//	client, err := xiaoguai.NewClient("http://localhost:8080",
//	    xiaoguai.WithToken("my-bearer-token"),
//	)
//	if err != nil {
//	    log.Fatal(err)
//	}
//
//	ctx := context.Background()
//	policies, err := client.ListHotlPolicies(ctx, "my-tenant-id")
//
// # Error Handling
//
// All methods return typed errors that wrap [HTTPError]. Use [errors.As] to
// match specific sub-types:
//
//	_, err := client.GetHotlPolicy(ctx, "non-existent-id")
//	var notFound *xiaoguai.NotFoundError
//	if errors.As(err, &notFound) {
//	    fmt.Println("policy not found:", notFound.StatusCode)
//	}
//
// # Retries
//
// The client automatically retries 5xx responses up to 3 times with
// exponential back-off (100 ms base, capped at 2 s). Pass a custom
// [http.Client] via [WithHTTPClient] to override transport behaviour.
package xiaoguai
