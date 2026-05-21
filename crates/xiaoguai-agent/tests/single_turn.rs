//! v0.1 agent loop: one LLM call, no tools, return the assistant text.

use xiaoguai_agent::Agent;
use xiaoguai_llm::MockBackend;

#[tokio::test]
async fn agent_returns_canned_response() {
    let backend = MockBackend::with_response("Hi user, this is Xiaoguai.");
    let agent = Agent::new(Box::new(backend), "mock");
    let answer = agent.run_once("hello").await.expect("answer");
    assert_eq!(answer, "Hi user, this is Xiaoguai.");
}
