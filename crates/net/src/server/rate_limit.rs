//! Token-bucket datagram rate limiting (per address, per match and host-wide).
//!
//! The limiter runs on the source address alone, straight after `recv_from` and before the
//! header is decoded (see `UdpTransport::recv_admitted`), so every datagram is charged,
//! including junk that would fail to decode.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use crate::host::HostBudget;

use super::NetServer;

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
    host: Option<Arc<HostBudget>>,
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
            host: None,
        }
    }

    /// Admits one datagram from `addr`, charging every enabled budget.
    ///
    /// The per-address bucket is checked first, so one flooding address is stopped by its
    /// own bucket without draining the per-match or host-wide budgets that its neighbours
    /// share. A datagram is charged only when every budget admits it.
    pub(super) fn allow_at(&mut self, addr: SocketAddr, now: Instant) -> bool {
        let bucket = if self.rate > 0.0 {
            let index = self.bucket_index(addr, now);
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
            Some(index)
        } else {
            None
        };
        if self.total_rate > 0.0 {
            let elapsed = now.saturating_duration_since(self.total_last).as_secs_f32();
            self.total_tokens =
                (self.total_tokens + elapsed * self.total_rate).min(self.total_burst);
            self.total_last = now;
            if self.total_tokens < 1.0 {
                return false;
            }
        }
        if let Some(host) = self.host.as_deref()
            && !host.try_take(now)
        {
            return false;
        }
        if self.total_rate > 0.0 {
            self.total_tokens -= 1.0;
        }
        if let Some(index) = bucket {
            self.buckets[index].tokens -= 1.0;
        }
        true
    }

    /// Index of `addr`'s bucket, creating it (evicting the least recently seen bucket when
    /// full) on first sight.
    fn bucket_index(&mut self, addr: SocketAddr, now: Instant) -> usize {
        if let Some(index) = self.buckets.iter().position(|bucket| bucket.addr == addr) {
            return index;
        }
        let fresh = RateBucket {
            addr,
            tokens: self.burst,
            last_refill: now,
            last_seen: now,
        };
        if self.buckets.len() >= self.max_buckets {
            let oldest = self
                .buckets
                .iter()
                .enumerate()
                .min_by_key(|(_, bucket)| bucket.last_seen)
                .map_or(0, |(index, _)| index);
            self.buckets[oldest] = fresh;
            oldest
        } else {
            self.buckets.push(fresh);
            self.buckets.len() - 1
        }
    }
}

impl NetServer {
    /// Shares a host-wide datagram budget with other matches (see [`HostBudget`]).
    ///
    /// The budget is charged after this match's per-address and per-match budgets admit a
    /// datagram and before it is decoded. `None` removes it. [`crate::MatchHost`] installs
    /// one on every match when [`crate::MatchHost::set_host_packet_budget`] is used.
    pub fn set_host_budget(&mut self, budget: Option<Arc<HostBudget>>) {
        self.limiter.host = budget;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn addr(port: u16) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], port))
    }

    #[test]
    fn one_flooding_address_does_not_drain_the_match_budget() {
        // 10 pps per address, 20 pps per match.
        let mut limiter = RateLimiter::new(10, 20, 8);
        let now = Instant::now();
        let flooded = (0..1_000)
            .filter(|_| limiter.allow_at(addr(1), now))
            .count();
        assert_eq!(flooded, 10, "the flooder only gets its own burst");
        let neighbour = (0..10).filter(|_| limiter.allow_at(addr(2), now)).count();
        assert_eq!(neighbour, 10, "a neighbour still gets its full burst");
        // The match budget (20) is now spent.
        assert!(!limiter.allow_at(addr(3), now));
        // It refills over time.
        assert!(limiter.allow_at(addr(3), now + Duration::from_millis(100)));
    }

    #[test]
    fn host_budget_is_shared_and_charged_last() {
        let host = Arc::new(HostBudget::new(5));
        let mut first = RateLimiter::new(3, 0, 8);
        let mut second = RateLimiter::new(3, 0, 8);
        first.host = Some(Arc::clone(&host));
        second.host = Some(Arc::clone(&host));
        let now = Instant::now();
        // Address-limited datagrams never reach the host budget.
        let a = (0..100).filter(|_| first.allow_at(addr(1), now)).count();
        assert_eq!(a, 3);
        let b = (0..100).filter(|_| second.allow_at(addr(2), now)).count();
        assert_eq!(b, 2, "only two host tokens were left");
        assert_eq!(host.rejected(), 98);
        // A host rejection does not charge the address bucket.
        assert!(second.allow_at(addr(2), now + Duration::from_millis(250)));
    }

    #[test]
    fn zero_rates_disable_each_budget_independently() {
        let mut limiter = RateLimiter::new(0, 4, 8);
        let now = Instant::now();
        assert_eq!(
            (0..10).filter(|_| limiter.allow_at(addr(1), now)).count(),
            4
        );
        let mut open = RateLimiter::new(0, 0, 8);
        assert!((0..10_000).all(|_| open.allow_at(addr(1), now)));
    }
}
