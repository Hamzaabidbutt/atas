//! Pumping a feed into a session.
//!
//! This is the loop the desktop shell runs. It lives here rather than in the
//! Tauri binary for the same reason the session does: a loop that can only be
//! exercised by launching a window is a loop nobody tests, and its failure
//! modes — starving the UI, silently swallowing feed errors, losing events on
//! a full channel — are exactly the ones that need testing.

use atas_core::MarketEvent;
use atas_feed::Feed;

use crate::session::{AppEvent, Session};

/// Where driver output goes.
///
/// The Tauri shell implements this by emitting to the webview; tests
/// implement it by pushing to a vector. Nothing else about the loop changes.
pub trait EventSink {
    /// Publish one event.
    fn emit(&mut self, event: &AppEvent);
}

impl<F: FnMut(&AppEvent)> EventSink for F {
    fn emit(&mut self, event: &AppEvent) {
        self(event);
    }
}

/// Collects events into a vector. For tests and for replaying offline.
#[derive(Debug, Default)]
pub struct CollectingSink {
    /// Everything emitted so far.
    pub events: Vec<AppEvent>,
}

impl EventSink for CollectingSink {
    fn emit(&mut self, event: &AppEvent) {
        self.events.push(event.clone());
    }
}

/// What a pump pass did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PumpReport {
    /// Market events taken from the feed.
    pub consumed: usize,
    /// Application events emitted to the sink.
    pub emitted: usize,
    /// Whether the feed had nothing left when the pass ended.
    pub drained: bool,
    /// Whether the pass stopped because it hit its budget.
    pub budget_exhausted: bool,
}

/// Running totals across the driver's life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DriverStats {
    /// Market events processed.
    pub consumed: u64,
    /// Application events emitted.
    pub emitted: u64,
    /// Feed errors survived.
    pub errors: u64,
}

/// Drives one session from one feed.
#[derive(Debug)]
pub struct SessionDriver<F, S> {
    session: Session,
    feed: F,
    sink: S,
    stats: DriverStats,
    last_error: Option<String>,
}

impl<F: Feed, S: EventSink> SessionDriver<F, S> {
    /// Build a driver.
    pub fn new(session: Session, feed: F, sink: S) -> Self {
        Self {
            session,
            feed,
            sink,
            stats: DriverStats::default(),
            last_error: None,
        }
    }

    /// The session being driven.
    pub fn session(&self) -> &Session {
        &self.session
    }

    /// Mutable access, for order commands arriving from the UI.
    pub fn session_mut(&mut self) -> &mut Session {
        &mut self.session
    }

    /// The sink, for reading collected output.
    pub fn sink(&self) -> &S {
        &self.sink
    }

    /// Running totals.
    pub fn stats(&self) -> DriverStats {
        self.stats
    }

    /// The most recent feed error, if any survived.
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// Process up to `budget` market events.
    ///
    /// The budget is not a tuning knob to ignore: a live instrument can
    /// produce events faster than they can be rendered, and an unbounded loop
    /// would never return to the caller — the UI would freeze while the
    /// backlog grew. Bounding each pass keeps the shell responsive and lets
    /// backpressure build in the feed's own queue, where it belongs.
    pub fn pump(&mut self, budget: usize) -> PumpReport {
        let mut report = PumpReport::default();

        for _ in 0..budget {
            match self.feed.next_event() {
                Ok(Some(event)) => {
                    report.consumed += 1;
                    self.stats.consumed += 1;
                    report.emitted += self.dispatch(&event);
                }
                Ok(None) => {
                    report.drained = true;
                    return report;
                }
                Err(e) => {
                    // A malformed message must not take the session down: the
                    // next one is very likely fine, and a feed that kills the
                    // app on one bad frame is worse than one that skips it.
                    self.stats.errors += 1;
                    self.last_error = Some(e.to_string());
                    report.consumed += 1;
                }
            }
        }

        report.budget_exhausted = true;
        report
    }

    /// Drain the feed completely. Only safe on a finite feed such as a replay.
    pub fn run_to_completion(&mut self) -> PumpReport {
        let mut total = PumpReport::default();
        loop {
            let pass = self.pump(4_096);
            total.consumed += pass.consumed;
            total.emitted += pass.emitted;
            if pass.drained {
                total.drained = true;
                return total;
            }
        }
    }

    fn dispatch(&mut self, event: &MarketEvent) -> usize {
        let produced = self.session.on_market_event(event);
        // Copied out before emitting: `on_market_event` borrows the session,
        // and the sink may want to call back into it.
        let batch: Vec<AppEvent> = produced.to_vec();
        for app_event in &batch {
            self.sink.emit(app_event);
        }
        self.stats.emitted += batch.len() as u64;
        batch.len()
    }
}

/// A feed that always fails, for exercising error handling.
#[cfg(test)]
#[derive(Debug, Default)]
pub struct FailingFeed {
    /// How many errors to produce before going quiet.
    pub failures: usize,
}

#[cfg(test)]
impl Feed for FailingFeed {
    fn next_event(&mut self) -> Result<Option<MarketEvent>, atas_feed::FeedError> {
        if self.failures == 0 {
            return Ok(None);
        }
        self.failures -= 1;
        Err(atas_feed::FeedError::Unhandled("synthetic failure".to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atas_core::{Instrument, Price, Qty, Venue};
    use atas_engine::BarSpec;

    use crate::session::SessionConfig;

    fn session() -> Session {
        let instrument = Instrument::spot(
            "TEST",
            Venue::Sim,
            Price::parse("0.25").unwrap(),
            Qty::parse("1").unwrap(),
        );
        Session::new(
            instrument,
            SessionConfig {
                bar_spec: BarSpec::Tick { count: 5 },
                ..Default::default()
            },
        )
        .unwrap()
    }

    #[test]
    fn a_feed_error_is_survived_not_fatal() {
        // A single malformed frame must not take the session down: the next
        // one is very likely fine, and an app that dies on one bad message is
        // worse than one that skips it.
        let feed = FailingFeed { failures: 3 };
        let mut driver = SessionDriver::new(session(), feed, CollectingSink::default());

        let report = driver.pump(10);
        assert_eq!(driver.stats().errors, 3);
        assert!(report.drained, "the feed goes quiet after its failures");
        assert!(driver.last_error().is_some());
        assert!(driver.sink().events.is_empty(), "nothing valid arrived");
    }

    #[test]
    fn errors_do_not_consume_the_budget_silently() {
        // Each failed read still counts as work, or a permanently failing feed
        // would spin inside one pump call for ever.
        let feed = FailingFeed { failures: 1_000 };
        let mut driver = SessionDriver::new(session(), feed, CollectingSink::default());

        let report = driver.pump(16);
        assert_eq!(report.consumed, 16);
        assert!(report.budget_exhausted);
        assert!(!report.drained);
        assert_eq!(driver.stats().errors, 16);
    }
}
