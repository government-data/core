//! The payment provider, chosen by the environment (fake for local
//! development, Stripe in production), so no domain or application logic
//! recompiles when it changes.

use std::sync::Arc;

use application::{PaymentsService, WebhookService};
use infrastructure::{
    FakePaymentProvider, PgPaymentRepository, PgWebhookEventStore, ReqwestHttpClient,
    StripeProvider, StripeWebhookConfig, StripeWebhookVerifier,
};

use crate::config::{env_value, PaymentProviderConfig};

/// The payment webhooks (Stripe, once its signing secret is set) and the
/// admin transactions surface, which is mounted only when a provider is
/// configured — a refund needs one. Both go through the same Postgres payment
/// repository. Building them here validates the configuration at startup.
pub(crate) fn payments(
    pool: &infrastructure::PgPool,
) -> Result<(Option<WebhookService>, Option<PaymentsService>), Box<dyn std::error::Error>> {
    let config = PaymentProviderConfig::from_env()?;
    tracing::info!(provider = config.label(), "payments provider selected");
    let (provider, webhooks): (Option<Arc<dyn payments::PaymentProvider>>, _) = match config {
        PaymentProviderConfig::Disabled => (None, None),
        PaymentProviderConfig::Fake => (Some(Arc::new(FakePaymentProvider::new())), None),
        PaymentProviderConfig::Stripe(cfg) => {
            let provider = Arc::new(StripeProvider::new(cfg, Arc::new(ReqwestHttpClient::new())));
            let webhooks = env_value("STRIPE_WEBHOOK_SECRET").map(|secret| {
                tracing::info!("stripe webhooks enabled");
                WebhookService::new(
                    Arc::new(StripeWebhookVerifier::new(StripeWebhookConfig::new(secret))),
                    Arc::new(PgWebhookEventStore::new(pool.clone())),
                    Arc::new(PgPaymentRepository::new(pool.clone())),
                )
            });
            (Some(provider), webhooks)
        }
    };
    let transactions = provider.map(|provider| {
        PaymentsService::new(Arc::new(PgPaymentRepository::new(pool.clone())), provider)
    });
    Ok((webhooks, transactions))
}
