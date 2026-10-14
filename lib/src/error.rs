//! Context is attached at subsystem boundaries; native causes remain inspectable.
use std::error::Error;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{operation}: {source}")]
    Operation {
        operation: &'static str,
        #[source]
        source: Box<dyn Error + Send + Sync>,
    },
    #[error("{0}")]
    Message(String),
}

impl AppError {
    pub fn message(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }

    pub fn context(operation: &'static str, source: impl Error + Send + Sync + 'static) -> Self {
        Self::Operation {
            operation,
            source: Box::new(source),
        }
    }
}

pub trait ResultContext<T> {
    fn context(self, operation: &'static str) -> Result<T, AppError>;
}

impl<T, E: Error + Send + Sync + 'static> ResultContext<T> for Result<T, E> {
    fn context(self, operation: &'static str) -> Result<T, AppError> {
        self.map_err(|source| AppError::context(operation, source))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_preserves_the_native_cause() {
        let result: std::io::Result<()> = Err(std::io::Error::from_raw_os_error(2));
        let error = result.context("Open sender metadata").unwrap_err();
        let cause = error
            .source()
            .unwrap()
            .downcast_ref::<std::io::Error>()
            .unwrap();
        assert_eq!(cause.raw_os_error(), Some(2));
        assert!(error.to_string().starts_with("Open sender metadata: "));
    }
}
