use std::time::Duration;

const PROCESS_CHANGES_THRESHOLD: usize = 1000;
const PROCESSING_WARN_THRESHOLD: Duration = Duration::from_secs(5);
const PROCESS_BUFFER_INTERVAL: Duration = Duration::from_millis(600);
const PURGE_CHANGES_INTERVAL: Duration = Duration::from_secs(300);

/// Configuration used by a [`Matcher`]'s event loop
#[derive(Clone)]
pub struct MatcherLoopConfig {
    /// Maximum number of changes that will be buffered before being sent to
    /// the subscriber
    pub changes_threshold: usize,
    /// The interval at which buffered changes will be processed, if the number
    /// of buffered changes does not surpass the [`Self::changes_threshold`]
    pub process_buffer_interval: Duration,
    /// If processing changes takes
    pub processing_warn_threshold: Duration,
    /// Interval at which old changes are purged from the database
    pub purge_changes_interval: Duration,
}

impl MatcherLoopConfig {
    /// A config suitable for testing
    #[inline]
    pub fn testing() -> Self {
        Self {
            changes_threshold: 0,
            ..Default::default()
        }
    }
}

impl Default for MatcherLoopConfig {
    fn default() -> Self {
        Self {
            changes_threshold: PROCESS_CHANGES_THRESHOLD,
            process_buffer_interval: PROCESS_BUFFER_INTERVAL,
            processing_warn_threshold: PROCESSING_WARN_THRESHOLD,
            purge_changes_interval: PURGE_CHANGES_INTERVAL,
        }
    }
}
