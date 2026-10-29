use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use rand::Rng;

use crate::{
    models::{
        channel::{self, Channel, ChannelHealth, PreparedChannel, ProtocolConfig},
        now_ms,
    },
    protocol::Protocol,
};

/// A routable channel. It holds the shared prepared channel and an index into
/// its protocol configs, so selecting a candidate does not clone the channel
/// row or its parsed configuration.
#[derive(Debug, Clone)]
pub struct Candidate {
    prepared: Arc<PreparedChannel>,
    pub upstream_model: String,
    pub upstream_kind: String,
    config_index: usize,
}

impl Candidate {
    pub fn channel(&self) -> &Channel {
        &self.prepared.channel
    }

    pub fn upstream_config(&self) -> &ProtocolConfig {
        self.prepared.config_at(self.config_index)
    }
}

/// Resolve the route a channel would use for one inbound protocol and model.
/// Shared by candidate construction and the "all channels cooling" check so
/// both apply exactly the same enable/allowlist/route/model filters.
fn matching_route(
    prepared: &PreparedChannel,
    model: &str,
    inbound_kind: &str,
    allowed_channels: Option<&HashSet<i64>>,
) -> Option<(String, usize, String)> {
    let channel = &prepared.channel;
    if channel.enabled == 0 {
        return None;
    }
    if allowed_channels.is_some_and(|allowed| !allowed.contains(&channel.id)) {
        return None;
    }
    let (upstream_kind, config_index) = prepared.resolve_route_ref(inbound_kind)?;
    let config = prepared.config_at(config_index);
    if !config
        .models
        .iter()
        .any(|item| item == "*" || item == model)
    {
        return None;
    }
    let upstream_model = config
        .model_mapping
        .get(model)
        .cloned()
        .unwrap_or_else(|| model.to_string());
    Some((upstream_kind.to_string(), config_index, upstream_model))
}

/// Whether any enabled, allowlisted channel could serve this model once its
/// cooldown expires. Used to distinguish "model does not exist" (404) from
/// "every channel for this model is temporarily cooling down" (503).
pub fn has_matching_channel(
    channels: &[Arc<PreparedChannel>],
    model: &str,
    inbound: Protocol,
    allowed_channel_ids: &[i64],
) -> bool {
    let inbound_kind = channel::kind_for_protocol(inbound.as_str());
    let allowed_channels: Option<HashSet<i64>> =
        (!allowed_channel_ids.is_empty()).then(|| allowed_channel_ids.iter().copied().collect());
    channels.iter().any(|prepared| {
        matching_route(prepared, model, inbound_kind, allowed_channels.as_ref()).is_some()
    })
}

/// Pick candidate channels for one request. Only channels that match the
/// requested model and the API key's channel allowlist are considered; the
/// cached channel list itself is shared.
pub fn candidates(
    channels: &[Arc<PreparedChannel>],
    health: &HashMap<i64, ChannelHealth>,
    model: &str,
    inbound: Protocol,
    allowed_channel_ids: &[i64],
) -> Vec<Candidate> {
    let now = now_ms();
    let inbound_kind = channel::kind_for_protocol(inbound.as_str());
    let allowed_channels: Option<HashSet<i64>> =
        (!allowed_channel_ids.is_empty()).then(|| allowed_channel_ids.iter().copied().collect());
    let mut grouped: HashMap<i64, Vec<(Candidate, i64)>> = HashMap::new();

    for prepared in channels {
        let channel = &prepared.channel;
        let Some((upstream_kind, config_index, upstream_model)) =
            matching_route(prepared, model, inbound_kind, allowed_channels.as_ref())
        else {
            continue;
        };
        // The in-process overlay is newer than the cached row after a failure.
        let disabled_until = health
            .get(&channel.id)
            .map(|health| health.disabled_until)
            .unwrap_or(channel.disabled_until);
        if disabled_until
            .map(|disabled_until| disabled_until > now)
            .unwrap_or(false)
        {
            continue;
        }

        let weight = channel.weight.max(1);
        grouped.entry(channel.priority).or_default().push((
            Candidate {
                prepared: prepared.clone(),
                upstream_model,
                upstream_kind,
                config_index,
            },
            weight,
        ));
    }

    let mut priorities: Vec<i64> = grouped.keys().copied().collect();
    priorities.sort_unstable_by(|left, right| right.cmp(left));

    let mut result = Vec::new();
    let mut rng = rand::thread_rng();
    for priority in priorities {
        let Some(group) = grouped.remove(&priority) else {
            continue;
        };
        let weights: Vec<i64> = group.iter().map(|(_, weight)| *weight).collect();
        let mut taken: Vec<Option<Candidate>> = group
            .into_iter()
            .map(|(candidate, _)| Some(candidate))
            .collect();
        for index in weighted_permutation(&weights, &mut rng) {
            if let Some(candidate) = taken[index].take() {
                result.push(candidate);
            }
        }
    }

    result
}

/// Weighted sampling without replacement in one pass.
///
/// Efraimidis-Spirakis assigns each item the key `ln(u) / weight` for a uniform
/// `u` in (0, 1); sorting keys descending produces the same distribution as
/// repeatedly drawing by weight. This replaces the previous O(n²) loop that
/// re-summed the group and removed from the middle of a `Vec` for every pick.
fn weighted_permutation(weights: &[i64], rng: &mut impl Rng) -> Vec<usize> {
    let mut keyed: Vec<(f64, usize)> = weights
        .iter()
        .enumerate()
        .map(|(index, weight)| {
            let uniform = rng.gen::<f64>().max(f64::MIN_POSITIVE);
            (uniform.ln() / (*weight).max(1) as f64, index)
        })
        .collect();
    keyed.sort_unstable_by(|left, right| {
        right
            .0
            .partial_cmp(&left.0)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    keyed.into_iter().map(|(_, index)| index).collect()
}
