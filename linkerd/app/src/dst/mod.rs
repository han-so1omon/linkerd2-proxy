mod default_resolve;
mod permit;
mod resolve;

use self::default_resolve::RecoverDefaultResolve;
use indexmap::IndexSet;
use linkerd2_app_core::{
    control, dns, profiles, proxy::identity, request_filter::RequestFilter, svc, transport::tls,
    Addr, ControlHttpMetrics, Error,
};
use permit::PermitConfiguredDsts;
use std::time::Duration;
use tonic::body::BoxBody;

#[derive(Clone, Debug)]
pub struct Config {
    pub control: control::Config,
    pub context: String,
    pub get_suffixes: IndexSet<dns::Suffix>,
    pub get_networks: IndexSet<ipnet::IpNet>,
    pub profile_suffixes: IndexSet<dns::Suffix>,
    pub profile_networks: IndexSet<ipnet::IpNet>,
    pub initial_profile_timeout: Duration,
}

#[derive(Clone, Debug)]
pub struct Rejected(());

/// Handles to destination service clients.
///
/// The addr is preserved for logging.
pub struct Dst {
    pub addr: control::ControlAddr,
    pub profiles: RequestFilter<
        PermitConfiguredDsts<profiles::InvalidProfileAddr>,
        profiles::Client<control::Client<BoxBody>, resolve::BackoffUnlessInvalidArgument>,
    >,
    pub resolve: RecoverDefaultResolve<
        RequestFilter<PermitConfiguredDsts, resolve::Resolve<control::Client<BoxBody>>>,
    >,
}

impl Config {
    pub fn build(
        self,
        dns: dns::Resolver,
        metrics: ControlHttpMetrics,
        identity: tls::Conditional<identity::Local>,
    ) -> Result<Dst, Error> {
        let addr = self.control.addr.clone();
        let backoff = self.control.connect.backoff.clone();
        let svc = self.control.build(dns, metrics, identity);
        let resolve = svc::stack(resolve::new(svc.clone(), &self.context, backoff))
            .push(RequestFilter::layer(PermitConfiguredDsts::new(
                self.get_suffixes,
                self.get_networks,
            )))
            .push(default_resolve::layer())
            .into_inner();

        let profiles = svc::stack(profiles::Client::new(
            svc,
            resolve::BackoffUnlessInvalidArgument::from(backoff),
            self.initial_profile_timeout,
            self.context,
        ))
        .push(RequestFilter::layer(
            PermitConfiguredDsts::new(self.profile_suffixes, self.profile_networks)
                .with_error::<profiles::InvalidProfileAddr>(),
        ))
        .into_inner();

        Ok(Dst {
            addr,
            resolve,
            profiles,
        })
    }
}

impl std::fmt::Display for Rejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "rejected discovery")
    }
}

impl std::error::Error for Rejected {}

impl From<Addr> for Rejected {
    fn from(_: Addr) -> Self {
        Rejected(())
    }
}

impl Rejected {
    fn matches(err: &(dyn std::error::Error + 'static)) -> bool {
        if err.is::<Self>() {
            return true;
        }

        if let Some(status) = err.downcast_ref::<tonic::Status>() {
            return status.code() == tonic::Code::InvalidArgument;
        }

        err.source().map(Self::matches).unwrap_or(false)
    }
}
