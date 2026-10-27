//! Token-bucket datagram rate limiting (per address and server-wide).

use std::net::SocketAddr;
use std::time::Instant;

#[derive(Debug)]
struct RateBucket {
    addr: SocketAddr,
    tokens: f32,
    last_refill: Instant,
    last_seen: Instant,
}

#[derive(Debug)]
pub(super) struct RateLimiter {
    rate: f32,
    burst: f32,
    total_rate: f32,
    total_burst: f32,
    total_tokens: f32,
    total_last: Instant,
    max_buckets: usize,
    buckets: Vec<RateBucket>,
}

impl RateLimiter {
    pub(super) fn new(
        packets_per_second: u32,
        total_packets_per_second: u32,
        max_buckets: usize,
    ) -> Self {
        let rate = packets_per_second as f32;
        let total_rate = total_packets_per_second as f32;
        Self {
            rate,
            burst: rate.max(1.0),
            total_rate,
            total_burst: total_rate.max(1.0),
            total_tokens: total_rate.max(1.0),
            total_last: Instant::now(),
            max_buckets,
            buckets: Vec::new(),
        }
    }

    pub(super) fn allow_at(&mut self, addr: SocketAddr, now: Instant) -> bool {
        if self.rate <= 0.0 {
            return true;
        }
        if self.total_rate > 0.0 {
            let elapsed = now.saturating_duration_since(self.total_last).as_secs_f32();
            self.total_tokens =
                (self.total_tokens + elapsed * self.total_rate).min(self.total_burst);
            self.total_last = now;
            if self.total_tokens < 1.0 {
                return false;
            }
            self.total_tokens -= 1.0;
        }
        let position = self.buckets.iter().position(|bucket| bucket.addr == addr);
        let index = if let Some(index) = position {
            index
        } else if self.buckets.len() >= self.max_buckets {
            let oldest = self
                .buckets
                .iter()
                .enumerate()
                .min_by_key(|(_, bucket)| bucket.last_seen)
                .map_or(0, |(index, _)| index);
            self.buckets[oldest] = RateBucket {
                addr,
                tokens: self.burst,
                last_refill: now,
                last_seen: now,
            };
            oldest
        } else {
            self.buckets.push(RateBucket {
                addr,
                tokens: self.burst,
                last_refill: now,
                last_seen: now,
            });
            self.buckets.len() - 1
        };
        let bucket = &mut self.buckets[index];
        let elapsed = now
            .saturating_duration_since(bucket.last_refill)
            .as_secs_f32();
        bucket.tokens = (bucket.tokens + elapsed * self.rate).min(self.burst);
        bucket.last_refill = now;
        bucket.last_seen = now;
        if bucket.tokens < 1.0 {
            return false;
        }
        bucket.tokens -= 1.0;
        true
    }
}
