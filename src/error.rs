use std::{fmt::Debug, sync::Arc};

use futures::future::BoxFuture;
use teloxide::{RequestError, error_handlers::ErrorHandler};

pub struct UpdateErrorHandler {
    text: String,
}

impl UpdateErrorHandler {
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            text: "Update listener".to_owned(),
        })
    }
}

default impl<E> ErrorHandler<E> for UpdateErrorHandler
where
    E: Debug,
{
    fn handle_error(self: Arc<Self>, error: E) -> BoxFuture<'static, ()> {
        log::error!("{text}: {:?}", error, text = self.text);
        Box::pin(async {})
    }
}

impl ErrorHandler<RequestError> for UpdateErrorHandler {
    fn handle_error(self: Arc<Self>, error: RequestError) -> BoxFuture<'static, ()> {
        if let RequestError::Network(ref e) = error {
            if e.is_timeout() {
                // ignore
                return Box::pin(async {});
            }
        }
        log::error!("{text}: {:?}", error, text = self.text);
        Box::pin(async {})
    }
}
