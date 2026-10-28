use std::collections::HashMap;

use rand::Rng;

use crate::{
    models::{
        channel::{self, Channel, ChannelHealth, PreparedChannel, ProtocolConfig},
        now_ms,
    },
    protocol::Protocol,
};

#[derive(Debug, Clone)]
pub struct Candidate {
    pub channel: Channel,
    pub upstream_model: String,
    pub upstream_kind: String,
    pub upstream_config: ProtocolConfig,
}

/// Pick candidate channels for one request. Only channels that match the
/// requested model and the API key's channel allowlist are cloned; the cached
/// channel list itself is shared.
pub fn candidates(
    channels: &[PreparedChannel],
    health: &HashMap<i64, ChannelHealth>,
    model: &str,
    inbound: Protocol,
    allowed_channel_ids: &[i64],
) -> Vec<Candidate> {
    let now = now_ms();
    let inbound_kind = channel::kind_for_protocol(inbound.as_str());
    let mut grouped: HashMap<i64, Vec<(Candidate, i64)>> = HashMap::new();

    for prepared in channels {
        let channel = &prepared.channel;
        if channel.enabled == 0 {
            continue;
        }
        if !allowed_channel_ids.is_empty() && !allowed_channel_ids.contains(&channel.id) {
            continue;
        }
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

        let Some(route) = prepared.resolve_route(inbound_kind) else {
            continue;
        };
        if !route
            .config
            .models
            .iter()
            .any(|item| item == "*" || item == model)
        {
            continue;
        }

        let upstream_model = route
            .config
            .model_mapping
            .get(model)
            .cloned()
            .unwrap_or_else(|| model.to_string());

        let weight = channel.weight.max(1);
        grouped.entry(channel.priority).or_default().push((
            Candidate {
                channel: channel.clone(),
                upstream_model,
                upstream_kind: route.upstream_kind,
                upstream_config: route.config,
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

#[cfg(test)]
mod tests {
    use super::weighted_permutation;

    #[test]
    fn weighted_permutation_returns_every_candidate_once() {
        let mut rng = rand::thread_rng();
        let order = weighted_permutation(&[1, 5, 2, 1, 9], &mut rng);
        let mut sorted = order.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn weighted_permutation_respects_weights() {
        let mut rng = rand::thread_rng();
        let rounds = 20_000;
        let mut first_wins = 0;
        for _ in 0..rounds {
            if weighted_permutation(&[3, 1], &mut rng)[0] == 0 {
                first_wins += 1;
            }
        }
        let ratio = f64::from(first_wins) / f64::from(rounds);
        assert!((ratio - 0.75).abs() < 0.03, "unexpected ratio {ratio}");
    }
}
