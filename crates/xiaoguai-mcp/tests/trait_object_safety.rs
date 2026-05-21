//! Compile-time check that `McpClient` stays object-safe so the supervisor
//! can store `Arc<dyn McpClient>`.

use std::sync::Arc;
use xiaoguai_mcp::McpClient;

#[test]
fn client_is_object_safe() {
    fn assert_object_safe(_: &dyn McpClient) {}
    let _: Box<dyn Fn(Arc<dyn McpClient>)> = Box::new(|c| assert_object_safe(&*c));
}
