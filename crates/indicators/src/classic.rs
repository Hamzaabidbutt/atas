//! Classical price indicators.
//!
//! These are here for context rather than as the platform's selling point: an
//! order-flow read is stronger when you can see it happening at a level a
//! moving average or an ATR band already made interesting.
//!
//! Every one of them tracks warm-up honestly. A 20-period average that has
//! seen three bars is not a 20-period average, and platforms that plot it
//! anyway produce a curve that is wrong precisely where a trader is most
//! likely to be reading history.

use atas_core::Price;
use atas_engine::Bar;

use crate::traits::BarIndicator;

/// Simple moving average of closing prices.
#[derive(Debug, Clone)]
pub struct Sma {
    period: usize,
    window: std::collections::VecDeque<i64>,
    sum: i128,
    name: String,
}

impl Sma {
    /// An average over `period` bars.
    pub fn new(period: usize) -> Self {
        assert!(period > 0, "period must be positive");
        Self {
            period,
            window: std::collections::VecDeque::with_capacity(period),
            sum: 0,
            name: format!("SMA({period})"),
        }
    }

    fn current(&self) -> Option<Price> {
        (self.window.len() == self.period)
            .then(|| Price::from_minor((self.sum / self.period as i128) as i64))
    }
}

impl BarIndicator for Sma {
    type Value = Option<Price>;

    fn name(&self) -> &str {
        &self.name
    }

    fn update(&mut self, bar: &Bar) -> Option<Price> {
        self.window.push_back(bar.close.minor());
        self.sum += bar.close.minor() as i128;
        if self.window.len() > self.period {
            self.sum -= self.window.pop_front().expect("just checked length") as i128;
        }
        self.current()
    }

    fn value(&self) -> Option<Option<Price>> {
        Some(self.current())
    }

    fn reset(&mut self) {
        self.window.clear();
        self.sum = 0;
    }

    fn is_warm(&self) -> bool {
        self.window.len() == self.period
    }
}

/// Exponential moving average of closing prices.
#[derive(Debug, Clone)]
pub struct Ema {
    period: usize,
    alpha: f64,
    value: Option<f64>,
    seen: usize,
    name: String,
}

impl Ema {
    /// An EMA with the standard `2 / (period + 1)` smoothing.
    pub fn new(period: usize) -> Self {
        assert!(period > 0, "period must be positive");
        Self {
            period,
            alpha: 2.0 / (period as f64 + 1.0),
            value: None,
            seen: 0,
            name: format!("EMA({period})"),
        }
    }
}

impl BarIndicator for Ema {
    type Value = Option<Price>;

    fn name(&self) -> &str {
        &self.name
    }

    fn update(&mut self, bar: &Bar) -> Option<Price> {
        let close = bar.close.minor() as f64;
        self.value = Some(match self.value {
            Some(previous) => previous + self.alpha * (close - previous),
            None => close,
        });
        self.seen += 1;
        self.value.map(|v| Price::from_minor(v as i64))
    }

    fn value(&self) -> Option<Option<Price>> {
        Some(self.value.map(|v| Price::from_minor(v as i64)))
    }

    fn reset(&mut self) {
        self.value = None;
        self.seen = 0;
    }

    fn is_warm(&self) -> bool {
        self.seen >= self.period
    }
}

/// Average true range, smoothed with Wilder's method.
#[derive(Debug, Clone)]
pub struct Atr {
    period: usize,
    previous_close: Option<Price>,
    /// True ranges collected during warm-up, before the first average.
    warmup: Vec<i64>,
    value: Option<f64>,
    name: String,
}

impl Atr {
    /// An ATR over `period` bars.
    pub fn new(period: usize) -> Self {
        assert!(period > 0, "period must be positive");
        Self {
            period,
            previous_close: None,
            warmup: Vec::with_capacity(period),
            value: None,
            name: format!("ATR({period})"),
        }
    }

    /// True range: the largest of the bar's range and its gaps from the
    /// previous close. The gap terms are what distinguish it from range.
    fn true_range(&self, bar: &Bar) -> i64 {
        let range = (bar.high - bar.low).minor();
        match self.previous_close {
            None => range,
            Some(previous) => range
                .max((bar.high - previous).abs().minor())
                .max((bar.low - previous).abs().minor()),
        }
    }
}

impl BarIndicator for Atr {
    type Value = Option<Price>;

    fn name(&self) -> &str {
        &self.name
    }

    fn update(&mut self, bar: &Bar) -> Option<Price> {
        let tr = self.true_range(bar) as f64;
        self.previous_close = Some(bar.close);

        match self.value {
            // Wilder smoothing once seeded.
            Some(previous) => {
                self.value = Some((previous * (self.period as f64 - 1.0) + tr) / self.period as f64);
            }
            None => {
                self.warmup.push(tr as i64);
                if self.warmup.len() == self.period {
                    let mean =
                        self.warmup.iter().sum::<i64>() as f64 / self.period as f64;
                    self.value = Some(mean);
                }
            }
        }
        self.value.map(|v| Price::from_minor(v as i64))
    }

    fn value(&self) -> Option<Option<Price>> {
        Some(self.value.map(|v| Price::from_minor(v as i64)))
    }

    fn reset(&mut self) {
        self.previous_close = None;
        self.warmup.clear();
        self.value = None;
    }

    fn is_warm(&self) -> bool {
        self.value.is_some()
    }
}

/// Relative strength index, smoothed with Wilder's method.
#[derive(Debug, Clone)]
pub struct Rsi {
    period: usize,
    previous_close: Option<Price>,
    gains: Vec<f64>,
    losses: Vec<f64>,
    avg_gain: Option<f64>,
    avg_loss: Option<f64>,
    name: String,
}

impl Rsi {
    /// An RSI over `period` bars.
    pub fn new(period: usize) -> Self {
        assert!(period > 0, "period must be positive");
        Self {
            period,
            previous_close: None,
            gains: Vec::with_capacity(period),
            losses: Vec::with_capacity(period),
            avg_gain: None,
            avg_loss: None,
            name: format!("RSI({period})"),
        }
    }

    fn current(&self) -> Option<f64> {
        let (gain, loss) = (self.avg_gain?, self.avg_loss?);
        // An unbroken run of gains has no downside to divide by; RSI is 100 by
        // definition rather than a division by zero.
        if loss == 0.0 {
            return Some(100.0);
        }
        let rs = gain / loss;
        Some(100.0 - (100.0 / (1.0 + rs)))
    }
}

impl BarIndicator for Rsi {
    type Value = Option<f64>;

    fn name(&self) -> &str {
        &self.name
    }

    fn update(&mut self, bar: &Bar) -> Option<f64> {
        let Some(previous) = self.previous_close else {
            self.previous_close = Some(bar.close);
            return None;
        };
        self.previous_close = Some(bar.close);

        let change = (bar.close - previous).minor() as f64;
        let gain = change.max(0.0);
        let loss = (-change).max(0.0);

        match (self.avg_gain, self.avg_loss) {
            (Some(ag), Some(al)) => {
                let n = self.period as f64;
                self.avg_gain = Some((ag * (n - 1.0) + gain) / n);
                self.avg_loss = Some((al * (n - 1.0) + loss) / n);
            }
            _ => {
                self.gains.push(gain);
                self.losses.push(loss);
                if self.gains.len() == self.period {
                    let n = self.period as f64;
                    self.avg_gain = Some(self.gains.iter().sum::<f64>() / n);
                    self.avg_loss = Some(self.losses.iter().sum::<f64>() / n);
                }
            }
        }
        self.current()
    }

    fn value(&self) -> Option<Option<f64>> {
        Some(self.current())
    }

    fn reset(&mut self) {
        self.previous_close = None;
        self.gains.clear();
        self.losses.clear();
        self.avg_gain = None;
        self.avg_loss = None;
    }

    fn is_warm(&self) -> bool {
        self.avg_gain.is_some()
    }
}
