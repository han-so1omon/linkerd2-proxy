pub use super::permit::PermitConfiguredDsts;
use http_body::Body as HttpBody;
use linkerd2_app_core::{
    exp_backoff::{ExponentialBackoff, ExponentialBackoffStream},
    proxy::{
        api_resolve as api,
        resolve::{self, recover},
    },
    Error, Recover,
};
use tonic::{
    body::{Body, BoxBody},
    client::GrpcService,
    Code, Status,
};

pub type Resolve<S> =
    recover::Resolve<BackoffUnlessInvalidArgument, resolve::make_unpin::Resolve<api::Resolve<S>>>;

pub fn new<S>(service: S, token: &str, backoff: ExponentialBackoff) -> Resolve<S>
where
    S: GrpcService<BoxBody> + Clone + Send + 'static,
    S::Error: Into<Error> + Send,
    S::ResponseBody: Send,
    <S::ResponseBody as Body>::Data: Send,
    <S::ResponseBody as HttpBody>::Error: Into<Error> + Send,
    S::Future: Send,
{
    recover::Resolve::new(
        backoff.into(),
        resolve::make_unpin(api::Resolve::new(service).with_context_token(token)),
    )
}

#[derive(Clone, Debug, Default)]
pub struct BackoffUnlessInvalidArgument(ExponentialBackoff);

// === impl BackoffUnlessInvalidArgument ===

impl From<ExponentialBackoff> for BackoffUnlessInvalidArgument {
    fn from(eb: ExponentialBackoff) -> Self {
        BackoffUnlessInvalidArgument(eb)
    }
}

impl Recover<Error> for BackoffUnlessInvalidArgument {
    type Backoff = ExponentialBackoffStream;

    fn recover(&self, error: Error) -> Result<Self::Backoff, Error> {
        if let Some(status) = error.downcast_ref::<Status>() {
            if status.code() == Code::InvalidArgument {
                tracing::debug!(%status, "Cannot recover");
                return Err(error);
            }
        }

        tracing::trace!(%error, "Recovering");
        Ok(self.0.stream())
    }
}
