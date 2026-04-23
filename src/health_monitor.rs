use soroban_sdk::{contracterror, contracttype, symbol_short, Env, Symbol, Vec};

#[derive(Clone)]
#[contracttype]
pub enum HealthKey {
    Metric(Symbol),
    Registry,
}

#[contracterror]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum HealthError {
    MetricBurstExceeded = 1,
    EmptyMetricBatch = 2,
}

#[derive(Clone, Debug, PartialEq)]
#[contracttype]
pub struct HealthMetricInput {
    pub metric: Symbol,
    pub value: u64,
}

#[derive(Clone, Debug, PartialEq)]
#[contracttype]
pub struct HealthMetricSummary {
    pub metric: Symbol,
    pub samples: u64,
    pub total: u64,
    pub minimum: u64,
    pub maximum: u64,
    pub last_value: u64,
    pub last_updated: u64,
}

#[derive(Clone, Debug, PartialEq)]
#[contracttype]
pub struct HealthSummary {
    pub metric_count: u32,
    pub total_samples: u64,
    pub total_value: u64,
    pub last_metric: Symbol,
    pub last_value: u64,
    pub metrics: Vec<HealthMetricSummary>,
}

#[derive(Clone, Debug, PartialEq)]
#[contracttype]
struct HealthAggregate {
    samples: u64,
    total: u64,
    minimum: u64,
    maximum: u64,
    last_value: u64,
    last_updated: u64,
}

fn default_aggregate(value: u64, timestamp: u64) -> HealthAggregate {
    HealthAggregate {
        samples: 1,
        total: value,
        minimum: value,
        maximum: value,
        last_value: value,
        last_updated: timestamp,
    }
}

fn load_registry(env: &Env) -> Vec<Symbol> {
    env.storage()
        .persistent()
        .get(&HealthKey::Registry)
        .unwrap_or_else(|| Vec::new(env))
}

fn store_registry(env: &Env, registry: &Vec<Symbol>) {
    env.storage().persistent().set(&HealthKey::Registry, registry);
}

fn record_metric(env: &Env, metric: Symbol, value: u64) -> HealthMetricSummary {
    let mut registry = load_registry(env);
    let mut seen = false;

    let mut i = 0u32;
    while i < registry.len() {
        if let Some(existing) = registry.get(i) {
            if existing == metric {
                seen = true;
                break;
            }
        }
        i += 1;
    }

    if !seen {
        registry.push_back(metric.clone());
        store_registry(env, &registry);
    }

    let exists = env
        .storage()
        .persistent()
        .has(&HealthKey::Metric(metric.clone()))
        ;

    let mut aggregate = if exists {
        env.storage()
            .persistent()
            .get(&HealthKey::Metric(metric.clone()))
            .unwrap_or(default_aggregate(value, env.ledger().timestamp()))
    } else {
        default_aggregate(value, env.ledger().timestamp())
    };

    if exists {
        aggregate.samples = aggregate.samples.saturating_add(1);
        aggregate.total = aggregate.total.saturating_add(value);
        aggregate.minimum = aggregate.minimum.min(value);
        aggregate.maximum = aggregate.maximum.max(value);
        aggregate.last_value = value;
        aggregate.last_updated = env.ledger().timestamp();
    }

    env.storage()
        .persistent()
        .set(&HealthKey::Metric(metric.clone()), &aggregate);

    let summary = HealthMetricSummary {
        metric: metric.clone(),
        samples: aggregate.samples,
        total: aggregate.total,
        minimum: aggregate.minimum,
        maximum: aggregate.maximum,
        last_value: aggregate.last_value,
        last_updated: aggregate.last_updated,
    };

    env.events().publish(
        (symbol_short!("health"), symbol_short!("record")),
        (metric, value, summary.samples, summary.total),
    );

    summary
}

pub fn record_contract_health(env: &Env, metric: Symbol, value: u64) -> HealthMetricSummary {
    record_metric(env, metric, value)
}

pub fn record_contract_health_batch(
    env: &Env,
    metrics: Vec<HealthMetricInput>,
) -> Result<Vec<HealthMetricSummary>, HealthError> {
    if metrics.len() == 0 {
        return Err(HealthError::EmptyMetricBatch);
    }
    if metrics.len() > 50 {
        return Err(HealthError::MetricBurstExceeded);
    }

    let mut summaries = Vec::new(env);
    let mut i = 0u32;
    while i < metrics.len() {
        if let Some(input) = metrics.get(i) {
            summaries.push_back(record_metric(env, input.metric, input.value));
        }
        i += 1;
    }

    Ok(summaries)
}

pub fn get_health_summary(env: &Env) -> HealthSummary {
    let registry = load_registry(env);
    let mut metrics = Vec::new(env);
    let mut total_samples = 0u64;
    let mut total_value = 0u64;
    let mut last_metric = symbol_short!("none");
    let mut last_value = 0u64;

    let mut i = 0u32;
    while i < registry.len() {
        if let Some(metric) = registry.get(i) {
            if let Some(aggregate) = env
                .storage()
                .persistent()
                .get::<HealthKey, HealthAggregate>(&HealthKey::Metric(metric.clone()))
            {
                total_samples = total_samples.saturating_add(aggregate.samples);
                total_value = total_value.saturating_add(aggregate.total);
                last_metric = metric.clone();
                last_value = aggregate.last_value;
                metrics.push_back(HealthMetricSummary {
                    metric,
                    samples: aggregate.samples,
                    total: aggregate.total,
                    minimum: aggregate.minimum,
                    maximum: aggregate.maximum,
                    last_value: aggregate.last_value,
                    last_updated: aggregate.last_updated,
                });
            }
        }
        i += 1;
    }

    HealthSummary {
        metric_count: metrics.len(),
        total_samples,
        total_value,
        last_metric,
        last_value,
        metrics,
    }
}
