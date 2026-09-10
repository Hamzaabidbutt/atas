//! Paper trading tests. PnL arithmetic and fill realism are checked against
//! hand-computed values, because both are wrong in ways that look plausible.

use atas_core::{Instrument, InstrumentKind, OrderBook, Price, Qty, Side, Trade, Ts, Venue};
use atas_trading::*;

fn px(s: &str) -> Price {
    Price::parse(s).unwrap()
}
fn qty(s: &str) -> Qty {
    Qty::parse(s).unwrap()
}
fn instrument() -> Instrument {
    Instrument::spot("TEST", Venue::Sim, px("0.25"), qty("1"))
}

/// Book with 10 at each of three levels per side, touch 99.75 / 100.00.
fn book() -> OrderBook {
    let mut b = OrderBook::new();
    b.apply_snapshot(
        Ts::from_secs(1),
        [
            (px("99.75"), qty("10")),
            (px("99.50"), qty("20")),
            (px("99.25"), qty("30")),
        ],
        [
            (px("100.00"), qty("10")),
            (px("100.25"), qty("20")),
            (px("100.50"), qty("30")),
        ],
        1,
    );
    b
}

fn engine() -> PaperEngine {
    PaperEngine::new(instrument(), TradingConfig::default())
}

fn no_fee_engine() -> PaperEngine {
    PaperEngine::new(
        instrument(),
        TradingConfig {
            commission_rate: 0.0,
            ..Default::default()
        },
    )
}

fn trade(price: &str, q: &str, side: Side, ms: i64) -> Trade {
    Trade::new(Ts::from_millis(ms), px(price), qty(q), side, 0)
}

// --- Market orders --------------------------------------------------------

#[test]
fn a_market_order_inside_the_touch_fills_at_the_touch() {
    let mut e = no_fee_engine();
    let out = e
        .submit(OrderRequest::market(Side::Buy, qty("5")), &book(), Ts::from_secs(1))
        .unwrap();

    assert_eq!(out.fills.len(), 1);
    assert_eq!(out.fills[0].price, px("100.00"));
    assert_eq!(out.fills[0].qty, qty("5"));
    assert_eq!(out.order.status, OrderStatus::Filled);
    assert_eq!(e.position().qty, qty("5"));
    assert_eq!(e.position().avg_price, px("100.00"));
}

#[test]
fn a_market_order_walks_the_book_and_pays_slippage() {
    // 25 lots against 10 @ 100.00, 20 @ 100.25: fills 10 + 15.
    let mut e = no_fee_engine();
    let out = e
        .submit(OrderRequest::market(Side::Buy, qty("25")), &book(), Ts::from_secs(1))
        .unwrap();

    assert_eq!(out.fills.len(), 2, "must consume two levels");
    assert_eq!((out.fills[0].price, out.fills[0].qty), (px("100.00"), qty("10")));
    assert_eq!((out.fills[1].price, out.fills[1].qty), (px("100.25"), qty("15")));

    // VWAP = (10*100.00 + 15*100.25) / 25 = 100.15
    assert_eq!(out.order.avg_fill_price, Some(px("100.15")));
    assert_eq!(e.position().avg_price, px("100.15"));
    assert!(
        e.position().avg_price > px("100.00"),
        "filling size at the touch price would be a lie"
    );
}

#[test]
fn a_market_sell_walks_the_bid_side() {
    let mut e = no_fee_engine();
    let out = e
        .submit(OrderRequest::market(Side::Sell, qty("25")), &book(), Ts::from_secs(1))
        .unwrap();

    assert_eq!((out.fills[0].price, out.fills[0].qty), (px("99.75"), qty("10")));
    assert_eq!((out.fills[1].price, out.fills[1].qty), (px("99.50"), qty("15")));
    assert!(e.position().is_short());
}

#[test]
fn a_market_order_with_no_book_is_rejected() {
    let mut e = engine();
    let empty = OrderBook::new();
    let err = e
        .submit(OrderRequest::market(Side::Buy, qty("1")), &empty, Ts::from_secs(1))
        .unwrap_err();
    assert_eq!(err, RejectReason::NoMarket);
    assert!(e.position().is_flat());
}

#[test]
fn book_walking_can_be_disabled() {
    let mut e = PaperEngine::new(
        instrument(),
        TradingConfig {
            allow_book_walk: false,
            commission_rate: 0.0,
            ..Default::default()
        },
    );
    // 5 fits at the touch.
    assert!(e
        .submit(OrderRequest::market(Side::Buy, qty("5")), &book(), Ts::from_secs(1))
        .is_ok());
    // 25 does not, and must be refused rather than silently sweeping.
    let err = e
        .submit(OrderRequest::market(Side::Buy, qty("25")), &book(), Ts::from_secs(1))
        .unwrap_err();
    assert_eq!(err, RejectReason::NoMarket);
}

// --- Limit orders ---------------------------------------------------------

#[test]
fn a_marketable_limit_takes_liquidity_immediately() {
    let mut e = no_fee_engine();
    // Buy limit at 100.25 crosses into asks at 100.00 and 100.25.
    let out = e
        .submit(
            OrderRequest::limit(Side::Buy, qty("15"), px("100.25")),
            &book(),
            Ts::from_secs(1),
        )
        .unwrap();

    assert_eq!(out.fills.len(), 2);
    assert_eq!(out.order.status, OrderStatus::Filled);
    // It pays the book's prices, not its own limit, where the book is better.
    assert_eq!(out.fills[0].price, px("100.00"));
    assert_eq!(out.fills[1].price, px("100.25"));
}

#[test]
fn a_resting_limit_needs_the_market_to_trade_through_it() {
    let mut e = no_fee_engine();
    let out = e
        .submit(
            OrderRequest::limit(Side::Buy, qty("5"), px("99.00")),
            &book(),
            Ts::from_secs(1),
        )
        .unwrap();
    assert!(out.fills.is_empty(), "not marketable, so it rests");
    assert_eq!(out.order.status, OrderStatus::Working);

    // A trade *at* the limit does not fill it: the trader is behind the queue.
    let fills = e.on_trade(&trade("99.00", "50", Side::Sell, 2), &book());
    assert!(
        fills.is_empty(),
        "a print at the limit must not fill a resting order"
    );

    // A trade *through* it does.
    let fills = e.on_trade(&trade("98.75", "50", Side::Sell, 3), &book());
    assert_eq!(fills.len(), 1);
    assert_eq!(fills[0].price, px("99.00"), "fills at the limit, not better");
    assert_eq!(e.position().qty, qty("5"));
}

#[test]
fn a_resting_sell_limit_fills_on_a_trade_above_it() {
    let mut e = no_fee_engine();
    e.submit(
        OrderRequest::limit(Side::Sell, qty("5"), px("101.00")),
        &book(),
        Ts::from_secs(1),
    )
    .unwrap();

    assert!(e.on_trade(&trade("101.00", "50", Side::Buy, 2), &book()).is_empty());
    let fills = e.on_trade(&trade("101.25", "50", Side::Buy, 3), &book());
    assert_eq!(fills.len(), 1);
    assert!(e.position().is_short());
}

#[test]
fn a_resting_limit_fills_partially_against_a_small_print() {
    let mut e = no_fee_engine();
    e.submit(
        OrderRequest::limit(Side::Buy, qty("10"), px("99.00")),
        &book(),
        Ts::from_secs(1),
    )
    .unwrap();

    let fills = e.on_trade(&trade("98.75", "4", Side::Sell, 2), &book());
    assert_eq!(fills[0].qty, qty("4"), "cannot fill more than printed");
    let order = e.working_orders().next().unwrap();
    assert_eq!(order.status, OrderStatus::PartiallyFilled);
    assert_eq!(order.remaining(), qty("6"));
}

// --- Time in force --------------------------------------------------------

#[test]
fn fill_or_kill_is_atomic() {
    let mut e = no_fee_engine();
    // 100 lots against a book holding 60 on the ask side.
    let out = e
        .submit(
            OrderRequest::market(Side::Buy, qty("100")).with_tif(TimeInForce::Fok),
            &book(),
            Ts::from_secs(1),
        )
        .unwrap();

    assert!(out.fills.is_empty(), "FOK must not fill partially");
    assert_eq!(out.order.status, OrderStatus::Cancelled);
    assert_eq!(out.order.filled, Qty::ZERO);
    assert!(e.position().is_flat(), "nothing may reach the position");
    assert!(e.fills().is_empty());
}

#[test]
fn fill_or_kill_completes_when_the_book_is_deep_enough() {
    let mut e = no_fee_engine();
    let out = e
        .submit(
            OrderRequest::market(Side::Buy, qty("30")).with_tif(TimeInForce::Fok),
            &book(),
            Ts::from_secs(1),
        )
        .unwrap();
    assert_eq!(out.order.status, OrderStatus::Filled);
    assert_eq!(e.position().qty, qty("30"));
}

#[test]
fn immediate_or_cancel_keeps_what_filled() {
    let mut e = no_fee_engine();
    let out = e
        .submit(
            OrderRequest::limit(Side::Buy, qty("100"), px("100.00")).with_tif(TimeInForce::Ioc),
            &book(),
            Ts::from_secs(1),
        )
        .unwrap();

    // Only 10 is available at or below 100.00.
    assert_eq!(out.fills.len(), 1);
    assert_eq!(e.position().qty, qty("10"));
    assert_eq!(out.order.status, OrderStatus::Cancelled);
    assert_eq!(e.working_orders().count(), 0, "the remainder must not rest");
}

// --- Stops ----------------------------------------------------------------

#[test]
fn a_buy_stop_triggers_on_a_trade_at_or_through_it() {
    let mut e = no_fee_engine();
    let out = e
        .submit(
            OrderRequest::stop(Side::Buy, qty("5"), px("101.00")),
            &book(),
            Ts::from_secs(1),
        )
        .unwrap();
    assert_eq!(out.order.status, OrderStatus::Pending);
    assert!(out.fills.is_empty());

    // Below the trigger: nothing.
    assert!(e.on_trade(&trade("100.50", "1", Side::Buy, 2), &book()).is_empty());
    // At the trigger: fires, and becomes a market order.
    let fills = e.on_trade(&trade("101.00", "1", Side::Buy, 3), &book());
    assert_eq!(fills.len(), 1);
    assert_eq!(fills[0].price, px("100.00"), "market order takes the book");
    assert_eq!(e.position().qty, qty("5"));
}

#[test]
fn a_sell_stop_triggers_downward() {
    let mut e = no_fee_engine();
    e.submit(
        OrderRequest::stop(Side::Sell, qty("5"), px("99.00")),
        &book(),
        Ts::from_secs(1),
    )
    .unwrap();

    assert!(e.on_trade(&trade("99.50", "1", Side::Sell, 2), &book()).is_empty());
    let fills = e.on_trade(&trade("99.00", "1", Side::Sell, 3), &book());
    assert_eq!(fills.len(), 1);
    assert!(e.position().is_short());
}

// --- OCO ------------------------------------------------------------------

#[test]
fn filling_one_leg_of_an_oco_cancels_the_other() {
    let mut e = no_fee_engine();
    // Long first, then bracket it.
    e.submit(OrderRequest::market(Side::Buy, qty("5")), &book(), Ts::from_secs(1))
        .unwrap();

    let take_profit = e
        .submit(
            OrderRequest::limit(Side::Sell, qty("5"), px("102.00")).with_oco(7),
            &book(),
            Ts::from_secs(1),
        )
        .unwrap()
        .order
        .id;
    let stop = e
        .submit(
            OrderRequest::stop(Side::Sell, qty("5"), px("98.00")).with_oco(7),
            &book(),
            Ts::from_secs(1),
        )
        .unwrap()
        .order
        .id;
    assert_eq!(e.working_orders().count(), 2);

    // Price runs up through the target.
    let fills = e.on_trade(&trade("102.25", "10", Side::Buy, 2), &book());
    assert_eq!(fills.len(), 1);
    assert_eq!(e.order(take_profit).unwrap().status, OrderStatus::Filled);
    assert_eq!(
        e.order(stop).unwrap().status,
        OrderStatus::Cancelled,
        "the losing leg must be pulled"
    );
    assert!(e.position().is_flat());
}

// --- Position and PnL -----------------------------------------------------

#[test]
fn adding_to_a_position_re_averages_the_entry() {
    let mut position = Position::new(&instrument());
    position.apply_fill(Side::Buy, px("100.00"), qty("10"), Qty::ZERO);
    position.apply_fill(Side::Buy, px("110.00"), qty("10"), Qty::ZERO);

    assert_eq!(position.qty, qty("20"));
    assert_eq!(position.avg_price, px("105.00"));
    assert_eq!(position.realised, Qty::ZERO, "adding realises nothing");
}

#[test]
fn reducing_a_position_banks_pnl_and_keeps_the_average() {
    let mut position = Position::new(&instrument());
    position.apply_fill(Side::Buy, px("100.00"), qty("10"), Qty::ZERO);
    let pnl = position.apply_fill(Side::Sell, px("110.00"), qty("4"), Qty::ZERO);

    assert_eq!(pnl, qty("40"), "4 lots × 10 points");
    assert_eq!(position.realised, qty("40"));
    assert_eq!(position.qty, qty("6"));
    assert_eq!(position.avg_price, px("100.00"), "the average is unchanged");
}

#[test]
fn closing_a_position_flattens_it() {
    let mut position = Position::new(&instrument());
    position.apply_fill(Side::Buy, px("100.00"), qty("10"), Qty::ZERO);
    position.apply_fill(Side::Sell, px("105.00"), qty("10"), Qty::ZERO);

    assert!(position.is_flat());
    assert_eq!(position.realised, qty("50"));
    assert_eq!(position.avg_price, Price::ZERO);
    assert_eq!(position.unrealised(px("200.00")), Qty::ZERO);
}

#[test]
fn flipping_a_position_banks_the_old_one_and_re_anchors() {
    // The case a naive implementation gets wrong by carrying the old average.
    let mut position = Position::new(&instrument());
    position.apply_fill(Side::Buy, px("100.00"), qty("10"), Qty::ZERO);
    let pnl = position.apply_fill(Side::Sell, px("110.00"), qty("25"), Qty::ZERO);

    assert_eq!(pnl, qty("100"), "the 10 long lots close for +10 each");
    assert_eq!(position.realised, qty("100"));
    assert_eq!(position.qty, qty("-15"), "and 15 short remain");
    assert_eq!(
        position.avg_price,
        px("110.00"),
        "the new short is anchored at the flip price, not the old average"
    );
}

#[test]
fn short_pnl_is_signed_correctly() {
    let mut position = Position::new(&instrument());
    position.apply_fill(Side::Sell, px("100.00"), qty("10"), Qty::ZERO);

    // Price falls: a short profits.
    assert_eq!(position.unrealised(px("90.00")), qty("100"));
    // Price rises: a short loses.
    assert_eq!(position.unrealised(px("110.00")), qty("-100"));

    let pnl = position.apply_fill(Side::Buy, px("95.00"), qty("10"), Qty::ZERO);
    assert_eq!(pnl, qty("50"));
}

#[test]
fn inverse_contracts_use_a_non_linear_payoff() {
    // A linear formula applied to an inverse contract misreports every
    // position, so the two must not share a branch.
    let mut inverse = Instrument::spot("BTCUSD", Venue::Sim, px("0.5"), qty("1"));
    inverse.kind = InstrumentKind::InversePerp;

    let mut position = Position::new(&inverse);
    position.apply_fill(Side::Buy, px("10000.00"), qty("10000"), Qty::ZERO);

    // qty × (1/entry − 1/exit) = 10000 × (1/10000 − 1/20000) = 0.5 base units.
    let pnl = position.unrealised(px("20000.00"));
    assert!(
        (pnl.to_f64() - 0.5).abs() < 1e-6,
        "expected 0.5 base units, got {pnl}"
    );

    // The linear formula would have said 10000 × 10000 = 100,000,000.
    let mut linear = Position::new(&instrument());
    linear.apply_fill(Side::Buy, px("10000.00"), qty("10000"), Qty::ZERO);
    assert_ne!(linear.unrealised(px("20000.00")), pnl);
}

#[test]
fn commission_accumulates_and_reduces_total_pnl() {
    let mut e = engine(); // default 4 bps
    e.submit(OrderRequest::market(Side::Buy, qty("10")), &book(), Ts::from_secs(1))
        .unwrap();

    let commission = e.position().commission;
    assert!(commission.is_positive(), "a fee should have been charged");
    // 10 × 100.00 × 0.0004 = 0.4
    assert!((commission.to_f64() - 0.4).abs() < 1e-6, "got {commission}");

    // Flat PnL, so total is exactly the negative of commission.
    assert_eq!(e.total_pnl(), -commission);
}

// --- Risk and lifecycle ---------------------------------------------------

#[test]
fn orders_are_validated_before_they_reach_the_book() {
    let mut e = engine();
    let b = book();

    let bad_qty = OrderRequest::market(Side::Buy, Qty::ZERO);
    assert_eq!(
        e.submit(bad_qty, &b, Ts::from_secs(1)).unwrap_err(),
        RejectReason::NonPositiveQty
    );

    let bad_price = OrderRequest::limit(Side::Buy, qty("1"), Price::ZERO);
    assert_eq!(
        e.submit(bad_price, &b, Ts::from_secs(1)).unwrap_err(),
        RejectReason::NonPositivePrice
    );
    assert!(e.position().is_flat());
}

#[test]
fn the_position_limit_blocks_additions_but_never_exits() {
    let mut e = PaperEngine::new(
        instrument(),
        TradingConfig {
            max_position: Some(qty("10")),
            commission_rate: 0.0,
            ..Default::default()
        },
    );
    let b = book();

    e.submit(OrderRequest::market(Side::Buy, qty("10")), &b, Ts::from_secs(1))
        .unwrap();

    // Adding beyond the limit is refused.
    let err = e
        .submit(OrderRequest::market(Side::Buy, qty("5")), &b, Ts::from_secs(1))
        .unwrap_err();
    assert_eq!(err, RejectReason::PositionLimit { limit: qty("10") });

    // Reducing is always allowed — otherwise a trader cannot get out of a
    // position that is already too large.
    assert!(e
        .submit(OrderRequest::market(Side::Sell, qty("5")), &b, Ts::from_secs(1))
        .is_ok());
    assert_eq!(e.position().qty, qty("5"));
}

#[test]
fn cancel_removes_a_working_order() {
    let mut e = engine();
    let id = e
        .submit(
            OrderRequest::limit(Side::Buy, qty("5"), px("90.00")),
            &book(),
            Ts::from_secs(1),
        )
        .unwrap()
        .order
        .id;
    assert_eq!(e.working_orders().count(), 1);

    assert!(e.cancel(id, Ts::from_secs(2)));
    assert_eq!(e.order(id).unwrap().status, OrderStatus::Cancelled);
    assert_eq!(e.working_orders().count(), 0);
    assert!(!e.cancel(id, Ts::from_secs(3)), "cancelling twice is a no-op");
}

#[test]
fn flatten_cancels_everything_and_closes_the_position() {
    let mut e = no_fee_engine();
    let b = book();

    e.submit(OrderRequest::market(Side::Buy, qty("10")), &b, Ts::from_secs(1))
        .unwrap();
    e.submit(
        OrderRequest::limit(Side::Buy, qty("5"), px("90.00")),
        &b,
        Ts::from_secs(1),
    )
    .unwrap();
    assert_eq!(e.working_orders().count(), 1);

    let fills = e.flatten(&b, Ts::from_secs(2)).unwrap();
    assert!(!fills.is_empty());
    assert!(e.position().is_flat());
    assert_eq!(e.working_orders().count(), 0, "resting orders must be pulled");

    // Flattening when already flat is a no-op, not an error.
    assert!(e.flatten(&b, Ts::from_secs(3)).unwrap().is_empty());
}

#[test]
fn a_full_round_trip_reports_the_right_pnl() {
    let mut e = no_fee_engine();
    let b = book();

    // Long 10 at the touch, 100.00.
    e.submit(OrderRequest::market(Side::Buy, qty("10")), &b, Ts::from_secs(1))
        .unwrap();
    assert_eq!(e.position().avg_price, px("100.00"));

    // Market rallies; mark-to-market moves with it.
    e.on_trade(&trade("105.00", "1", Side::Buy, 2), &b);
    assert_eq!(e.unrealised(), qty("50"));
    assert_eq!(e.position().realised, Qty::ZERO);

    // Sell out into the bid at 99.75, a small loss against the 100.00 entry.
    e.submit(OrderRequest::market(Side::Sell, qty("10")), &b, Ts::from_secs(3))
        .unwrap();
    assert!(e.position().is_flat());
    assert_eq!(e.position().realised, qty("-2.5"), "10 × -0.25");
    assert_eq!(e.unrealised(), Qty::ZERO);
    assert_eq!(e.total_pnl(), qty("-2.5"));
    assert_eq!(e.fills().len(), 2);
}
