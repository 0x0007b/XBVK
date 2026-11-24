//! Unified logging bridge forwarding XCL log macros to `tracing`.

#[macro_export]
macro_rules! xlog_error {
    ($($a:tt)*) => {
        tracing::error!(target: "xcl_core", "{}", format_args!($($a)*))
    };
}

#[macro_export]
macro_rules! xlog_warn {
    ($($a:tt)*) => {
        tracing::warn!(target: "xcl_core", "{}", format_args!($($a)*))
    };
}

#[macro_export]
macro_rules! xlog_info {
    ($($a:tt)*) => {
        tracing::info!(target: "xcl_core", "{}", format_args!($($a)*))
    };
}

#[macro_export]
macro_rules! xlog_debug {
    ($($a:tt)*) => {
        tracing::debug!(target: "xcl_core", "{}", format_args!($($a)*))
    };
}

#[macro_export]
macro_rules! xlog_trace {
    ($($a:tt)*) => {
        tracing::trace!(target: "xcl_core", "{}", format_args!($($a)*))
    };
}
