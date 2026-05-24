use thiserror::Error;
use xiaoguai_agent::AgentError;

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("agent: {0}")]
    Agent(#[from] AgentError),
    /// A [`crate::sink::RuntimeSink`] impl returned an error while
    /// the runtime was feeding it events. The runtime keeps running
    /// the agent loop (sink errors are informational) but surfaces
    /// the first one once the loop terminates.
    #[error("sink: {0}")]
    Sink(String),
    /// The agent task itself panicked or was aborted before completing.
    #[error("agent task: {0}")]
    Join(String),
}
