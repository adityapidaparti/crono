//! Property-based tests (proptest) for the pure core.
//!
//! Properties, not examples: parsers are total (never panic), display/parse
//! round-trips are identities, and day resolution is consistent.

use chrono::{Datelike, NaiveDate, NaiveTime, Timelike};
use crono::client::{find_measure, resolve_amount, Food, Measure};
use crono::types::{cron_day, cron_time, parse_cron_day, Amount, DaySpec, Meal};
use proptest::prelude::*;

fn arb_date() -> impl Strategy<Value = NaiveDate> {
    (0i64..1_000_000).prop_map(|n| NaiveDate::from_num_days_from_ce_opt(700_000 + n as i32).unwrap())
}

fn food_with_measures(measures: Vec<Measure>) -> Food {
    serde_json::from_value(serde_json::json!({
        "id": 1, "name": "test food",
        "defaultMeasureId": measures.first().map(|m| m.id).unwrap_or(0),
        "measures": measures.iter().map(|m| serde_json::json!({
            "id": m.id, "name": m.name, "value": m.value,
            "amount": m.amount, "type": m.kind,
        })).collect::<Vec<_>>(),
        "nutrients": [{"id": 208, "amount": 100.0}]
    }))
    .unwrap()
}

fn gram_measure(id: i64, name: &str, grams: f64) -> Measure {
    serde_json::from_value(serde_json::json!({
        "id": id, "name": name, "value": grams, "amount": 1.0, "type": "Weight"
    }))
    .unwrap()
}

proptest! {
    // ---- totality: no input can panic a parser ----

    #[test]
    fn amount_parser_is_total(s in ".*") {
        let _ = s.parse::<Amount>();
    }

    #[test]
    fn dayspec_parser_is_total(s in ".*") {
        let _ = s.parse::<DaySpec>();
    }

    #[test]
    fn meal_parser_is_total(s in ".*") {
        let _ = s.parse::<Meal>();
    }

    // ---- round-trips ----

    /// format(parse(x)) == format(x) for well-formed amounts.
    #[test]
    fn amount_display_parse_roundtrip(
        value in 0.01f64..100_000.0,
        unit in proptest::option::of("[a-z]{1,12}( [a-z]{1,12})?"),
    ) {
        // Quantize to what a CLI user would actually type.
        let value = (value * 100.0).round() / 100.0;
        prop_assume!(value > 0.0);
        let original = Amount::new(value, unit.as_deref());
        let reparsed: Amount = original.to_string().parse().unwrap();
        prop_assert_eq!(&original, &reparsed);
    }

    #[test]
    fn meal_display_parse_roundtrip(idx in 0usize..Meal::ALL.len()) {
        let meal = Meal::ALL[idx];
        let reparsed: Meal = meal.as_str().parse().unwrap();
        prop_assert_eq!(meal, reparsed);
    }

    #[test]
    fn dayspec_iso_roundtrip(days in 0i64..1_000_000) {
        let date = NaiveDate::from_num_days_from_ce_opt(700_000 + days as i32).unwrap();
        let spec: DaySpec = date.format("%Y-%m-%d").to_string().parse().unwrap();
        prop_assert_eq!(spec, DaySpec::Date(date));
    }

    // ---- semantics ----

    /// A parsed amount is always positive and finite.
    #[test]
    fn parsed_amounts_are_positive(s in "[0-9]{1,6}(\\.[0-9]{1,3})?( ?[a-z]{0,10})?") {
        if let Ok(a) = s.parse::<Amount>() {
            prop_assert!(a.value.is_finite());
            prop_assert!(a.value > 0.0);
        }
    }

    /// Units are normalized to lowercase with no surrounding whitespace
    /// (zero amounts are rejected by design, so only Ok cases are checked).
    #[test]
    fn parsed_units_are_normalized(s in "[0-9]{1,4} ?[a-zA-Z]{1,10}") {
        if let Ok(a) = s.parse::<Amount>() {
            let unit = a.unit.expect("input had a unit");
            prop_assert_eq!(&unit, &unit.trim().to_ascii_lowercase());
        }
    }

    /// resolve() of Offset(n) is exactly n days from the reference date.
    #[test]
    fn offset_resolution_is_linear(n in -10_000i64..10_000, base in 0i64..1_000_000) {
        let today = NaiveDate::from_num_days_from_ce_opt(700_000 + base as i32).unwrap();
        let resolved = DaySpec::Offset(n).resolve(today);
        prop_assert_eq!(resolved - today, chrono::Duration::days(n));
    }

    /// "today"/"yesterday"/"tomorrow" resolve to the expected calendar days.
    #[test]
    fn named_days_resolve_correctly(base in 0i64..1_000_000) {
        let today = NaiveDate::from_num_days_from_ce_opt(700_000 + base as i32).unwrap();
        prop_assert_eq!("today".parse::<DaySpec>().unwrap().resolve(today), today);
        prop_assert_eq!(
            "yesterday".parse::<DaySpec>().unwrap().resolve(today),
            today.pred_opt().unwrap()
        );
        prop_assert_eq!(
            "tomorrow".parse::<DaySpec>().unwrap().resolve(today),
            today.succ_opt().unwrap()
        );
    }

    // ---- Cronometer wire format ----

    /// API dates are non-zero-padded and round-trip through the parser.
    #[test]
    fn cron_day_roundtrip_and_never_padded(date in arb_date()) {
        let s = cron_day(date);
        prop_assert_eq!(parse_cron_day(&s), Some(date));
        for part in s.split('-').skip(1) {
            prop_assert!(!part.starts_with('0') || part == "0" || part.len() == 1,
                "padded component in {}", s);
        }
    }

    /// The parser also accepts zero-padded ISO (responses use it).
    #[test]
    fn cron_day_parses_padded_iso(date in arb_date()) {
        let iso = date.format("%Y-%m-%d").to_string();
        prop_assert_eq!(parse_cron_day(&iso), Some(date));
    }

    #[test]
    fn parse_cron_day_is_total(s in ".*") {
        let _ = parse_cron_day(&s);
    }

    /// cron_time is H:M:S non-padded and each field is in range.
    #[test]
    fn cron_time_components_valid(secs in 0u32..86_400) {
        let t = NaiveTime::from_num_seconds_from_midnight_opt(secs, 0).unwrap();
        let s = cron_time(t);
        let parts: Vec<u32> = s.split(':').map(|p| p.parse().unwrap()).collect();
        prop_assert_eq!(parts.clone(), vec![t.hour(), t.minute(), t.second()]);
        prop_assert!(parts[0] < 24 && parts[1] < 60 && parts[2] < 60);
    }

    /// order = (group << 16) | 1 packs and unpacks losslessly for every meal.
    #[test]
    fn meal_order_pack_unpack_roundtrip(idx in 0usize..Meal::ALL.len()) {
        let meal = Meal::ALL[idx];
        let order = meal.pack_order();
        prop_assert_eq!(Meal::from_order(order), meal);
        prop_assert_eq!(order & 0xFFFF, 1);
    }

    /// Unpacking any i64 order never panics and yields a valid meal.
    #[test]
    fn meal_from_order_is_total(order in any::<i64>()) {
        let meal = Meal::from_order(order);
        prop_assert!(Meal::ALL.contains(&meal));
    }

    /// Auto-meal covers every hour and matches the app's ranges.
    #[test]
    fn auto_meal_by_hour_matches_app(hour in 0u32..24) {
        let expected = match hour {
            4..=9 => Meal::Breakfast,
            10..=13 => Meal::Lunch,
            14..=20 => Meal::Dinner,
            _ => Meal::Snacks,
        };
        prop_assert_eq!(Meal::from_hour(hour), expected);
    }

    // ---- amount resolution against measures ----

    /// "<X>g" always submits exactly X grams regardless of the food's measures.
    #[test]
    fn gram_amounts_are_literal(
        value in 1.0f64..5_000.0,
        measure_grams in 1.0f64..500.0,
    ) {
        let value = (value * 10.0).round() / 10.0;
        let food = food_with_measures(vec![
            gram_measure(10, "cup", measure_grams),
            gram_measure(11, "g", 1.0),
        ]);
        let amt = Amount::new(value, Some("g"));
        let r = resolve_amount(&food, &amt).unwrap();
        prop_assert!((r.grams - value).abs() < 1e-9);
    }

    /// Named-measure amounts scale linearly: N × measure = N × (1 × measure).
    #[test]
    fn measure_amounts_scale_linearly(
        count in 1.0f64..50.0,
        measure_grams in 1.0f64..500.0,
    ) {
        let count = (count * 4.0).round() / 4.0;
        let food = food_with_measures(vec![gram_measure(10, "cup", measure_grams)]);
        let one = resolve_amount(&food, &Amount::new(1.0, Some("cup"))).unwrap();
        let many = resolve_amount(&food, &Amount::new(count, Some("cup"))).unwrap();
        prop_assert!((many.grams - count * one.grams).abs() < 1e-6);
        prop_assert_eq!(one.measure_id, many.measure_id);
    }

    /// Measure lookup prefers exact name matches over prefix/substring hits.
    #[test]
    fn measure_lookup_prefers_exact(extra in "[a-z]{1,8}") {
        prop_assume!(extra != "cup");
        let food = food_with_measures(vec![
            gram_measure(1, &format!("cup {extra}"), 100.0),
            gram_measure(2, "cup", 200.0),
        ]);
        let m = find_measure(&food, "cup").unwrap();
        prop_assert_eq!(m.id, 2);
    }

    /// Resolution is total for arbitrary unit strings: Ok or Err, no panic.
    #[test]
    fn resolve_amount_is_total(unit in ".{0,20}", value in 0.1f64..100.0) {
        let food = food_with_measures(vec![gram_measure(1, "piece", 39.0)]);
        let _ = resolve_amount(&food, &Amount::new(value, Some(&unit)));
    }
}
